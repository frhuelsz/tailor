//! The per-cell render pipeline: gather matched fragments, resolve `$include`, merge the config and
//! tailor fields, interpolate `${…}`, and emit one runnable cell (`meta/docs/image-definitions.md` §7,
//! §9.3). Pure and deterministic, so the emitted config is a stable golden snapshot.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde_yaml_ng::{Mapping, Value};

use crate::{
    error::ConfigError,
    fragment::{self, LoadedFragment},
    include, interpolate, matrix,
    matrix::AxisTuple,
    merge,
    schema::{BaseSource, ImageDefinition, OutputSpec},
    types::ParamValue,
};

const BASE_FIELD: &str = "base";
const OUTPUTS_FIELD: &str = "outputs";
const RENDERED_DIR: &str = ".rendered";

/// Write a cell's normalized golden snapshot to `<image_dir>/.rendered/<slug>.yaml` (for review and
/// CI blast-radius diffing, `meta/docs/image-definitions.md` §9.3). Returns the written path.
pub fn write_golden(
    image_dir: &Path,
    slug: &str,
    ic_config: &Value,
) -> Result<PathBuf, ConfigError> {
    let dir = image_dir.join(RENDERED_DIR);
    std::fs::create_dir_all(&dir).map_err(|source| ConfigError::Write {
        path: dir.clone(),
        source,
    })?;
    let path = dir.join(format!("{slug}.yaml"));
    let text = serde_yaml_ng::to_string(ic_config).map_err(|source| ConfigError::Parse {
        path: path.clone(),
        source,
    })?;
    std::fs::write(&path, text).map_err(|source| ConfigError::Write {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// One fully rendered matrix cell, ready for execution or golden snapshotting.
#[derive(Debug, Clone)]
pub struct RenderedCell {
    /// The cell's axis coordinate (matrix-declared order).
    pub tuple: AxisTuple,
    /// The merged, interpolated Image Customizer config (the `config:` tree).
    pub ic_config: Value,
    /// The single resolved base image source.
    pub base: BaseSource,
    /// The output formats to build for this cell.
    pub outputs: Vec<OutputSpec>,
    /// Local RPM sources (directories or `.repo` files) passed to IC as `--rpm-source`.
    pub rpm_sources: Vec<PathBuf>,
}

/// Render every cell of an image. `$include` paths resolve relative to `image_dir`.
pub fn render_image(
    image: &ImageDefinition,
    image_dir: impl AsRef<Path>,
) -> Result<Vec<RenderedCell>, ConfigError> {
    let image_dir = image_dir.as_ref();
    let fragments = fragment::discover(image_dir, image.matrix.as_ref(), &image.features)?;
    let cells = match &image.matrix {
        Some(matrix) => matrix::expand(matrix)?,
        None => vec![AxisTuple { values: Vec::new() }],
    };
    cells
        .into_iter()
        .map(|tuple| render_cell(image, image_dir, &fragments, tuple))
        .collect()
}

fn render_cell(
    image: &ImageDefinition,
    image_dir: &Path,
    fragments: &[LoadedFragment],
    tuple: AxisTuple,
) -> Result<RenderedCell, ConfigError> {
    let axes: BTreeMap<String, String> = tuple.values.iter().cloned().collect();
    let matched: Vec<&LoadedFragment> = fragments
        .iter()
        .filter(|f| f.applies(&axes, &image.features))
        .collect();

    let params = merge_params(&matched)?;
    let context = interpolate::build_context(&axes, &params)?;

    let mut config = Mapping::new();
    for fragment in &matched {
        let Some(delta) = fragment.doc.config.clone() else {
            continue;
        };
        let mut delta = delta;
        include::resolve_includes(&mut delta, image_dir)?;
        let Value::Mapping(delta) = delta else {
            return Err(ConfigError::InvalidField {
                slug: slug(image, &tuple),
                field: "config",
                detail:
                    "expected a mapping (inline IC config); path-string config is not yet supported"
                        .to_owned(),
            });
        };
        merge::merge_into(&mut config, delta, &fragment.label)?;
    }
    let mut ic_config = Value::Mapping(config);
    interpolate::interpolate_tree(&mut ic_config, &context)?;

    let base = resolve_base(image, &tuple, &matched, &context)?;
    let outputs = resolve_outputs(image, &tuple, &matched)?;
    let rpm_sources = matched
        .iter()
        .flat_map(|f| f.doc.rpm_sources.clone())
        .collect();

    Ok(RenderedCell {
        tuple,
        ic_config,
        base,
        outputs,
        rpm_sources,
    })
}

fn merge_params(matched: &[&LoadedFragment]) -> Result<IndexMap<String, String>, ConfigError> {
    let mut params: IndexMap<String, String> = IndexMap::new();
    for fragment in matched {
        for (name, value) in &fragment.doc.params {
            let value = param_string(value);
            match params.get(name) {
                Some(existing) if existing != &value => {
                    return Err(ConfigError::ParamConflict {
                        name: name.clone(),
                        existing: existing.clone(),
                        incoming: value,
                    });
                }
                _ => {
                    params.insert(name.clone(), value);
                }
            }
        }
    }
    Ok(params)
}

fn resolve_base(
    image: &ImageDefinition,
    tuple: &AxisTuple,
    matched: &[&LoadedFragment],
    context: &interpolate::Context,
) -> Result<BaseSource, ConfigError> {
    let mut base: Option<Value> = None;
    for fragment in matched {
        if let Some(value) = fragment.doc.base.clone() {
            base = Some(merge::merge_field(
                base,
                value,
                BASE_FIELD,
                &fragment.label,
            )?);
        }
    }
    let Some(mut base) = base else {
        return Err(ConfigError::MissingBase {
            slug: slug(image, tuple),
        });
    };
    interpolate::interpolate_tree(&mut base, context)?;
    ensure_single_base_kind(&base, image, tuple)?;
    deserialize_field(base, image, tuple, BASE_FIELD)
}

/// A base is a `oneOf` (`path` | `oci` | `azureLinux`). Two fragments merging incompatible base
/// *kinds* without `$set` would otherwise silently keep the first; reject the ambiguity loudly.
fn ensure_single_base_kind(
    base: &Value,
    image: &ImageDefinition,
    tuple: &AxisTuple,
) -> Result<(), ConfigError> {
    const KINDS: [&str; 3] = ["path", "oci", "azureLinux"];
    let present: Vec<&str> = KINDS
        .into_iter()
        .filter(|kind| base.get(kind).is_some())
        .collect();
    match present.len() {
        1 => Ok(()),
        0 => Err(ConfigError::MissingBase {
            slug: slug(image, tuple),
        }),
        _ => Err(ConfigError::AmbiguousBase {
            slug: slug(image, tuple),
            kinds: present.join(", "),
        }),
    }
}

fn resolve_outputs(
    image: &ImageDefinition,
    tuple: &AxisTuple,
    matched: &[&LoadedFragment],
) -> Result<Vec<OutputSpec>, ConfigError> {
    let mut outputs: Option<Value> = None;
    for fragment in matched {
        if let Some(value) = fragment.doc.outputs.clone() {
            outputs = Some(merge::merge_field(
                outputs,
                value,
                OUTPUTS_FIELD,
                &fragment.label,
            )?);
        }
    }
    match outputs {
        Some(value) => deserialize_field(value, image, tuple, OUTPUTS_FIELD),
        None => Ok(Vec::new()),
    }
}

fn deserialize_field<T: DeserializeOwned>(
    value: Value,
    image: &ImageDefinition,
    tuple: &AxisTuple,
    field: &'static str,
) -> Result<T, ConfigError> {
    serde_yaml_ng::from_value(value).map_err(|source| ConfigError::InvalidField {
        slug: slug(image, tuple),
        field,
        detail: source.to_string(),
    })
}

fn param_string(value: &ParamValue) -> String {
    match value {
        ParamValue::Bool(b) => b.to_string(),
        ParamValue::Int(i) => i.to_string(),
        ParamValue::Float(f) => f.to_string(),
        ParamValue::Str(s) => s.clone(),
    }
}

fn slug(image: &ImageDefinition, tuple: &AxisTuple) -> String {
    let coordinate = tuple.coordinate();
    if coordinate.is_empty() {
        image.name.clone()
    } else {
        format!("{}_{coordinate}", image.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use indoc::indoc;
    use tempfile::TempDir;

    use crate::{loader::load_image, types::OutputFormat};

    /// Write `body` to `<root>/<rel>`, creating parent directories as needed.
    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// A small matrix image exercising every render operation: list append, `$remove`, `$replace`,
    /// `$set`, `$include`, and (nested) parameter interpolation across by-arch/by-edition fragments.
    fn mini_image(root: &Path) -> ImageDefinition {
        write(
            root,
            "image.yaml",
            indoc! {"
                name: mini
                matrix:
                  arch: [amd64, arm64]
                  edition: [lite, pro]
                outputs:
                  - format: cosi
                config:
                  os:
                    packages:
                      install:
                        - core
                        - \"${bootPkg}\"
            "},
        );
        write(
            root,
            "by-arch/amd64.yaml",
            "base:\n  path: ./amd64.img\nparams:\n  efiArch: x64\n",
        );
        write(
            root,
            "by-arch/arm64.yaml",
            "base:\n  path: ./arm64.img\nparams:\n  efiArch: aa64\n",
        );
        write(
            root,
            "by-edition/lite.yaml",
            indoc! {"
                params:
                  bootPkg: \"boot-${efiArch}\"
                config:
                  storage:
                    $include: layouts/disk.yaml
            "},
        );
        write(
            root,
            "by-edition/pro.yaml",
            indoc! {"
                outputs:
                  $replace:
                    - format: raw
                params:
                  bootPkg: boot-pro
                base:
                  $set:
                    oci:
                      uri: registry.example/mini:pro
                      platform: \"linux/${arch}\"
                config:
                  os:
                    packages:
                      install:
                        $remove:
                          - core
            "},
        );
        write(root, "layouts/disk.yaml", "bootType: efi\n");
        load_image(root.join("image.yaml")).unwrap()
    }

    fn cell<'a>(cells: &'a [RenderedCell], edition: &str, arch: &str) -> &'a RenderedCell {
        cells
            .iter()
            .find(|c| c.tuple.get("edition") == Some(edition) && c.tuple.get("arch") == Some(arch))
            .unwrap_or_else(|| panic!("no {edition}/{arch} cell"))
    }

    fn install(cell: &RenderedCell) -> Vec<&str> {
        cell.ic_config["os"]["packages"]["install"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(serde_yaml_ng::Value::as_str)
            .collect()
    }

    #[test]
    fn renders_one_cell_per_matrix_point() {
        let tmp = TempDir::new().unwrap();
        let image = mini_image(tmp.path());
        let cells = render_image(&image, tmp.path()).unwrap();
        assert_eq!(cells.len(), 4); // edition[2] × arch[2]
    }

    #[test]
    fn lite_cell_keeps_path_base_appends_packages_and_resolves_include_and_nested_params() {
        let tmp = TempDir::new().unwrap();
        let image = mini_image(tmp.path());
        let cells = render_image(&image, tmp.path()).unwrap();
        let lite = cell(&cells, "lite", "amd64");

        // by-arch supplies a local path base; the inherited cosi output is unchanged.
        assert!(matches!(&lite.base, BaseSource::Path { .. }));
        assert_eq!(lite.outputs.len(), 1);
        assert_eq!(lite.outputs[0].format, OutputFormat::Cosi);
        // List append + nested interpolation: bootPkg = "boot-${efiArch}", efiArch = x64.
        assert_eq!(install(lite), ["core", "boot-x64"]);
        // $include splices the storage layout as the value of `storage`.
        assert_eq!(lite.ic_config["storage"]["bootType"].as_str(), Some("efi"));
    }

    #[test]
    fn pro_cell_applies_set_base_replace_outputs_and_remove() {
        let tmp = TempDir::new().unwrap();
        let image = mini_image(tmp.path());
        let cells = render_image(&image, tmp.path()).unwrap();
        let pro = cell(&cells, "pro", "arm64");

        // $set overrides the by-arch path base wholesale; ${arch} interpolates into the platform.
        match &pro.base {
            BaseSource::Oci { oci } => assert_eq!(oci.platform.as_deref(), Some("linux/arm64")),
            other => panic!("expected an OCI base, got {other:?}"),
        }
        // $replace swaps the whole output list.
        assert_eq!(pro.outputs.len(), 1);
        assert_eq!(pro.outputs[0].format, OutputFormat::Raw);
        // $remove drops `core`, leaving only the interpolated boot package.
        assert_eq!(install(pro), ["boot-pro"]);
    }

    #[test]
    fn an_image_with_no_base_is_an_error() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "image.yaml",
            "name: nobase\nconfig:\n  os:\n    hostname: x\n",
        );
        let image = load_image(tmp.path().join("image.yaml")).unwrap();
        let err = render_image(&image, tmp.path()).unwrap_err();
        assert!(
            matches!(err, ConfigError::MissingBase { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn merge_precedence_follows_axis_declaration_order_not_directory_names() {
        // Two axes whose fragments both `$set` the same scalar. The axis declared LAST wins,
        // regardless of how the `by-*` directories sort on disk (by-aa < by-zz alphabetically).
        fn rendered_hostname(first_axis: &str, second_axis: &str) -> String {
            let tmp = TempDir::new().unwrap();
            write(
                tmp.path(),
                "image.yaml",
                &format!(
                    "name: ord\nmatrix:\n  {first_axis}: [x]\n  {second_axis}: [x]\n\
                     outputs:\n  - format: cosi\nbase:\n  path: ./b.img\n\
                     config:\n  os:\n    hostname: from-base\n"
                ),
            );
            write(
                tmp.path(),
                "by-aa/x.yaml",
                "config:\n  os:\n    hostname:\n      $set: from-aa\n",
            );
            write(
                tmp.path(),
                "by-zz/x.yaml",
                "config:\n  os:\n    hostname:\n      $set: from-zz\n",
            );
            let image = load_image(tmp.path().join("image.yaml")).unwrap();
            let cells = render_image(&image, tmp.path()).unwrap();
            cells[0].ic_config["os"]["hostname"]
                .as_str()
                .unwrap()
                .to_owned()
        }
        // Declaring `aa` before `zz` makes `zz` win; reversing the declaration flips the winner —
        // even though the on-disk directory names (by-aa, by-zz) are identical in both cases.
        assert_eq!(rendered_hostname("aa", "zz"), "from-zz");
        assert_eq!(rendered_hostname("zz", "aa"), "from-aa");
    }
}
