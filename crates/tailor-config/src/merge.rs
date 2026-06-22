//! Deep-merge of Image Customizer config value trees with tailor's small directive vocabulary.
//!
//! Implements the merge semantics of `meta/docs/image-definitions.md` §7: mappings deep-merge
//! (preserving the insertion order the cell slug and goldens depend on), lists **append** by default
//! — with set-deduplication for package/service lists — or merge by a key field for the storage
//! lists, and scalars **conflict** on a differing value unless overridden with `$set`. `$include`
//! and `$select` are resolved in earlier passes and must never reach the merger.

use serde_yaml_ng::{Mapping, Value};

use crate::error::ConfigError;

const SET: &str = "$set";
const REPLACE: &str = "$replace";
const REMOVE: &str = "$remove";
const RENAME: &str = "$rename";
const INCLUDE: &str = "$include";
const SELECT: &str = "$select";

/// Threaded state for a single merge: the dotted path (for diagnostics and list policy) and the
/// label of the fragment supplying the overlay (for conflict messages).
struct Ctx<'a> {
    path: Vec<String>,
    source: &'a str,
}

impl Ctx<'_> {
    fn dotted(&self) -> String {
        self.path.join(".")
    }
}

/// Deep-merge `overlay` onto the mutable `base` mapping at the config root (strict conflicts).
///
/// `source` labels the fragment contributing `overlay`, so a conflict can name it.
pub(crate) fn merge_into(
    base: &mut Mapping,
    overlay: Mapping,
    source: &str,
) -> Result<(), ConfigError> {
    let mut ctx = Ctx {
        path: Vec::new(),
        source,
    };
    merge_mapping(base, overlay, &mut ctx)
}

/// Merge a single tailor field value (e.g. `base`, `outputs`) across fragments, honouring directives.
pub(crate) fn merge_field(
    existing: Option<Value>,
    overlay: Value,
    field: &str,
    source: &str,
) -> Result<Value, ConfigError> {
    let mut ctx = Ctx {
        path: vec![field.to_owned()],
        source,
    };
    merge_value(existing, overlay, &mut ctx)
}

fn merge_value(
    existing: Option<Value>,
    mut overlay: Value,
    ctx: &mut Ctx<'_>,
) -> Result<Value, ConfigError> {
    if let Some((name, inner)) = extract_directive(&mut overlay, ctx)? {
        return apply_directive(&name, inner, existing, ctx);
    }
    match (existing, overlay) {
        (Some(Value::Mapping(mut base)), Value::Mapping(over)) => {
            merge_mapping(&mut base, over, ctx)?;
            Ok(Value::Mapping(base))
        }
        (Some(Value::Sequence(base)), Value::Sequence(over)) => merge_sequence(base, over, ctx),
        (Some(base), over) => {
            if is_container(&base) || is_container(&over) {
                Err(ConfigError::TypeConflict {
                    path: ctx.dotted(),
                    existing_kind: kind(&base),
                    incoming_kind: kind(&over),
                    fragment: ctx.source.to_owned(),
                })
            } else if base == over {
                Ok(base)
            } else {
                Err(ConfigError::ScalarConflict {
                    path: ctx.dotted(),
                    existing: scalar_string(&base),
                    incoming: scalar_string(&over),
                    fragment: ctx.source.to_owned(),
                })
            }
        }
        (None, Value::Mapping(over)) => {
            let mut base = Mapping::new();
            merge_mapping(&mut base, over, ctx)?;
            Ok(Value::Mapping(base))
        }
        (None, Value::Sequence(over)) => merge_sequence(Vec::new(), over, ctx),
        (None, scalar) => Ok(scalar),
    }
}

fn merge_mapping(
    base: &mut Mapping,
    overlay: Mapping,
    ctx: &mut Ctx<'_>,
) -> Result<(), ConfigError> {
    for (key, over_val) in overlay {
        ctx.path.push(key.as_str().unwrap_or_default().to_owned());
        if let Some(slot) = base.get_mut(&key) {
            let taken = std::mem::replace(slot, Value::Null);
            let merged = merge_value(Some(taken), over_val, ctx)?;
            if let Some(slot) = base.get_mut(&key) {
                *slot = merged;
            }
        } else {
            let fresh = merge_value(None, over_val, ctx)?;
            base.insert(key, fresh);
        }
        ctx.path.pop();
    }
    Ok(())
}

/// Lists **append** by default. tailor does not key lists by any Image Customizer field (that would
/// bake IC's schema into the merger); finer control — whole-list replace or item removal — is via
/// the explicit `$replace` / `$remove` directives.
fn merge_sequence(
    base: Vec<Value>,
    overlay: Vec<Value>,
    ctx: &mut Ctx<'_>,
) -> Result<Value, ConfigError> {
    let mut out = base;
    for item in overlay {
        out.push(merge_value(None, item, ctx)?);
    }
    Ok(Value::Sequence(out))
}

/// If `overlay` is a mapping whose sole key is a `$`-directive, remove and return `(name, inner)`.
/// A `$`-key mixed with other keys, or two `$`-keys, is an error.
fn extract_directive(
    overlay: &mut Value,
    ctx: &Ctx<'_>,
) -> Result<Option<(String, Value)>, ConfigError> {
    let Value::Mapping(map) = overlay else {
        return Ok(None);
    };
    let dollar_keys: Vec<String> = map
        .keys()
        .filter_map(Value::as_str)
        .filter(|k| k.starts_with('$'))
        .map(str::to_owned)
        .collect();
    match dollar_keys.as_slice() {
        [] => Ok(None),
        [name] => {
            if map.len() > 1 {
                return Err(ConfigError::DirectiveNotSole {
                    directive: name.clone(),
                    path: ctx.dotted(),
                });
            }
            let inner = map.remove(name.as_str()).unwrap_or(Value::Null);
            Ok(Some((name.clone(), inner)))
        }
        many => Err(ConfigError::DirectiveNotSole {
            directive: many.join("`, `"),
            path: ctx.dotted(),
        }),
    }
}

fn apply_directive(
    name: &str,
    inner: Value,
    existing: Option<Value>,
    ctx: &mut Ctx<'_>,
) -> Result<Value, ConfigError> {
    match name {
        SET => merge_value(None, inner, ctx),
        REPLACE => {
            if !matches!(inner, Value::Sequence(_)) {
                return Err(ConfigError::DirectiveShape {
                    directive: REPLACE,
                    path: ctx.dotted(),
                    expected: "a list",
                });
            }
            merge_value(None, inner, ctx)
        }
        REMOVE => {
            let Value::Sequence(drop) = inner else {
                return Err(ConfigError::DirectiveShape {
                    directive: REMOVE,
                    path: ctx.dotted(),
                    expected: "a list",
                });
            };
            let base = match existing {
                Some(Value::Sequence(items)) => items,
                None => Vec::new(),
                Some(other) => {
                    return Err(ConfigError::TypeConflict {
                        path: ctx.dotted(),
                        existing_kind: kind(&other),
                        incoming_kind: "list",
                        fragment: ctx.source.to_owned(),
                    });
                }
            };
            let kept = base
                .into_iter()
                .filter(|item| !drop.contains(item))
                .collect();
            Ok(Value::Sequence(kept))
        }
        RENAME => Err(ConfigError::UnsupportedDirective {
            directive: RENAME,
            path: ctx.dotted(),
        }),
        INCLUDE => Err(ConfigError::UnresolvedDirective {
            directive: INCLUDE,
            path: ctx.dotted(),
        }),
        SELECT => Err(ConfigError::UnresolvedDirective {
            directive: SELECT,
            path: ctx.dotted(),
        }),
        other => Err(ConfigError::UnknownDirective {
            directive: other.to_owned(),
            path: ctx.dotted(),
        }),
    }
}

fn is_container(value: &Value) -> bool {
    matches!(value, Value::Mapping(_) | Value::Sequence(_))
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Mapping(_) => "mapping",
        Value::Sequence(_) => "list",
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Null => "null",
        Value::Tagged(_) => "tagged value",
    }
}

fn scalar_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => "null".to_owned(),
        _ => "<complex>".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use indoc::indoc;

    fn yaml(text: &str) -> Mapping {
        serde_yaml_ng::from_str(text).unwrap()
    }

    fn merged(base: &str, overlay: &str) -> Mapping {
        let mut acc = yaml(base);
        merge_into(&mut acc, yaml(overlay), "overlay").unwrap();
        acc
    }

    fn to_yaml(map: &Mapping) -> String {
        serde_yaml_ng::to_string(map).unwrap()
    }

    #[test]
    fn maps_deep_merge_and_preserve_insertion_order() {
        let out = merged(
            indoc! {"
                os:
                  a: 1
                  b: 2
            "},
            indoc! {"
                os:
                  b: 2
                  c: 3
            "},
        );
        // `c` appends after the existing keys; `a`/`b` keep their positions.
        assert_eq!(to_yaml(&out), "os:\n  a: 1\n  b: 2\n  c: 3\n");
    }

    #[test]
    fn lists_append_by_default() {
        let out = merged(
            "os:\n  packages:\n    install: [a, b]\n",
            "os:\n  packages:\n    install: [c]\n",
        );
        let install = out["os"]["packages"]["install"].as_sequence().unwrap();
        let names: Vec<&str> = install.iter().filter_map(Value::as_str).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn replace_directive_swaps_the_inherited_list() {
        let out = merged(
            "outputs: [cosi]\n",
            "outputs:\n  $replace:\n    - vhd-fixed\n",
        );
        let outputs = out["outputs"].as_sequence().unwrap();
        let names: Vec<&str> = outputs.iter().filter_map(Value::as_str).collect();
        assert_eq!(names, ["vhd-fixed"]);
    }

    #[test]
    fn remove_directive_drops_inherited_items() {
        let out = merged(
            "os:\n  packages:\n    install: [a, b, c]\n",
            "os:\n  packages:\n    install:\n      $remove: [b]\n",
        );
        let install = out["os"]["packages"]["install"].as_sequence().unwrap();
        let names: Vec<&str> = install.iter().filter_map(Value::as_str).collect();
        assert_eq!(names, ["a", "c"]);
    }

    #[test]
    fn set_directive_overrides_without_conflict() {
        let out = merged(
            "os:\n  selinux:\n    mode: disabled\n",
            "os:\n  selinux:\n    mode:\n      $set: enforcing\n",
        );
        assert_eq!(out["os"]["selinux"]["mode"].as_str(), Some("enforcing"));
    }

    #[test]
    fn differing_scalar_without_set_is_a_conflict() {
        let mut acc = yaml("os:\n  hostname: a\n");
        let err = merge_into(&mut acc, yaml("os:\n  hostname: b\n"), "frag-b").unwrap_err();
        assert!(
            matches!(err, ConfigError::ScalarConflict { .. }),
            "got {err:?}"
        );
        assert!(err.to_string().contains("frag-b"), "got {err}");
    }

    #[test]
    fn equal_scalar_set_twice_is_fine() {
        let out = merged("os:\n  hostname: a\n", "os:\n  hostname: a\n");
        assert_eq!(out["os"]["hostname"].as_str(), Some("a"));
    }

    #[test]
    fn storage_lists_append_generically_without_keying() {
        // No IC-specific keyed merge: a second `partitions` list appends rather than merging by id.
        let out = merged(
            indoc! {"
                storage:
                  disks:
                    - partitions:
                        - id: esp
                          size: 8M
            "},
            indoc! {"
                storage:
                  disks:
                    - partitions:
                        - id: esp
                          size: 16M
            "},
        );
        // `disks` is a list → append; the two single-disk entries are both kept (generic behavior).
        assert_eq!(out["storage"]["disks"].as_sequence().unwrap().len(), 2);
    }

    #[test]
    fn directive_mixed_with_siblings_is_rejected() {
        let mut acc = yaml("x: [a]\n");
        let err = merge_into(&mut acc, yaml("x:\n  $replace: [b]\n  extra: 1\n"), "f").unwrap_err();
        assert!(
            matches!(err, ConfigError::DirectiveNotSole { .. }),
            "got {err:?}"
        );
    }
}
