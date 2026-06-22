//! Fragments and conditional composition (`meta/docs/image-definitions.md` §6, §9.1).
//!
//! A fragment is a partial document: tailor fields at the top level (`base`, `outputs`, `params`,
//! `rpmSources`, an inline `match`) plus an opaque `config:` delta. The recommended layout encodes
//! the match in the path — `by-<axis>/<value>.yaml` applies when that axis equals that value, and
//! `by-feature/<name>.yaml` when that feature is enabled — so single-condition fragments carry no
//! `match:` boilerplate. An inline `match:` is ANDed with the path predicate.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use serde::Deserialize;
use serde_yaml_ng::Value;

use crate::{error::ConfigError, schema::Matrix, types::ParamValue};

const FRAGMENT_DIR_PREFIX: &str = "by-";
const FEATURE_AXIS: &str = "feature";
const BASE_DOCUMENT: &str = "image.yaml";
const YAML_EXT: &str = "yaml";

/// The tailor-field deltas and opaque `config:` a fragment (or the base document) contributes.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Fragment {
    #[serde(default, rename = "match")]
    pub(crate) match_expr: Option<Match>,
    #[serde(default)]
    pub(crate) base: Option<Value>,
    #[serde(default)]
    pub(crate) outputs: Option<Value>,
    #[serde(default)]
    pub(crate) params: IndexMap<String, ParamValue>,
    #[serde(default)]
    pub(crate) rpm_sources: Vec<PathBuf>,
    #[serde(default)]
    pub(crate) config: Option<Value>,
}

/// A boolean condition over a cell's axis values and the image's enabled features
/// (`meta/docs/image-definitions.md` §6).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum Match {
    All { all: Vec<Match> },
    Any { any: Vec<Match> },
    Not { not: Box<Match> },
    Leaf(IndexMap<String, MatchValue>),
}

/// A `match` leaf value: a single value (equality) or a list (set membership).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum MatchValue {
    One(String),
    Many(Vec<String>),
}

impl Match {
    pub(crate) fn evaluate(&self, axes: &BTreeMap<String, String>, features: &[String]) -> bool {
        match self {
            Match::All { all } => all.iter().all(|m| m.evaluate(axes, features)),
            Match::Any { any } => any.iter().any(|m| m.evaluate(axes, features)),
            Match::Not { not } => !not.evaluate(axes, features),
            Match::Leaf(leaf) => leaf
                .iter()
                .all(|(key, value)| leaf_matches(key, value, axes, features)),
        }
    }
}

fn leaf_matches(
    key: &str,
    value: &MatchValue,
    axes: &BTreeMap<String, String>,
    features: &[String],
) -> bool {
    if key == FEATURE_AXIS {
        return match value {
            MatchValue::One(name) => features.iter().any(|f| f == name),
            MatchValue::Many(names) => names.iter().any(|name| features.iter().any(|f| f == name)),
        };
    }
    let Some(actual) = axes.get(key) else {
        return false;
    };
    match value {
        MatchValue::One(expected) => actual == expected,
        MatchValue::Many(expected) => expected.iter().any(|candidate| candidate == actual),
    }
}

/// The path-derived predicate of a fragment file.
#[derive(Debug, Clone)]
enum Predicate {
    Always,
    Axis { axis: String, value: String },
    Feature { name: String },
}

/// A fragment with its source label and resolved predicate, ready to test against each cell.
pub(crate) struct LoadedFragment {
    pub(crate) label: String,
    predicate: Predicate,
    pub(crate) doc: Fragment,
}

impl LoadedFragment {
    /// Whether this fragment applies to a cell: its path predicate AND any inline `match`.
    pub(crate) fn applies(&self, axes: &BTreeMap<String, String>, features: &[String]) -> bool {
        let path_ok = match &self.predicate {
            Predicate::Always => true,
            Predicate::Axis { axis, value } => axes.get(axis).is_some_and(|actual| actual == value),
            Predicate::Feature { name } => features.iter().any(|f| f == name),
        };
        path_ok
            && self
                .doc
                .match_expr
                .as_ref()
                .is_none_or(|m| m.evaluate(axes, features))
    }
}

/// Discover the base document and every `by-*/<value>.yaml` fragment for an image, in apply order:
/// the base document first, then local fragments by normalized path (`meta/docs/image-definitions.md`
/// §7). Validates that every fragment axis/value is declared (closed-axis check, §9.4).
pub(crate) fn discover(
    image_dir: &Path,
    matrix: Option<&Matrix>,
    features: &[String],
) -> Result<Vec<LoadedFragment>, ConfigError> {
    let mut fragments = vec![LoadedFragment {
        label: BASE_DOCUMENT.to_owned(),
        predicate: Predicate::Always,
        doc: parse_fragment(&image_dir.join(BASE_DOCUMENT))?,
    }];

    let mut local = Vec::new();
    for axis_dir in read_dir_sorted(image_dir)? {
        let dir_name = axis_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let Some(axis) = dir_name.strip_prefix(FRAGMENT_DIR_PREFIX) else {
            continue;
        };
        if !axis_dir.is_dir() {
            continue;
        }
        for file in read_dir_sorted(&axis_dir)? {
            if file.extension().and_then(|e| e.to_str()) != Some(YAML_EXT) {
                continue;
            }
            let value = file
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let label = format!(
                "{dir_name}/{}",
                file.file_name().unwrap_or_default().to_string_lossy()
            );
            let predicate = predicate_for(axis, &value, &label, matrix, features)?;
            local.push((label, predicate, parse_fragment(&file)?));
        }
    }
    local.sort_by(|(a, _, _), (b, _, _)| a.cmp(b));
    fragments.extend(
        local
            .into_iter()
            .map(|(label, predicate, doc)| LoadedFragment {
                label,
                predicate,
                doc,
            }),
    );
    Ok(fragments)
}

fn predicate_for(
    axis: &str,
    value: &str,
    label: &str,
    matrix: Option<&Matrix>,
    features: &[String],
) -> Result<Predicate, ConfigError> {
    if axis == FEATURE_AXIS {
        if !features.iter().any(|f| f == value) {
            return Err(ConfigError::UnknownFragmentValue {
                axis: FEATURE_AXIS.to_owned(),
                value: value.to_owned(),
                file: label.to_owned(),
            });
        }
        return Ok(Predicate::Feature {
            name: value.to_owned(),
        });
    }
    let declared =
        matrix
            .and_then(|m| m.axes.get(axis))
            .ok_or_else(|| ConfigError::UnknownFragmentAxis {
                axis: axis.to_owned(),
            })?;
    if !declared.iter().any(|v| v == value) {
        return Err(ConfigError::UnknownFragmentValue {
            axis: axis.to_owned(),
            value: value.to_owned(),
            file: label.to_owned(),
        });
    }
    Ok(Predicate::Axis {
        axis: axis.to_owned(),
        value: value.to_owned(),
    })
}

fn parse_fragment(path: &Path) -> Result<Fragment, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    serde_yaml_ng::from_str(&text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>, ConfigError> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|source| ConfigError::Read {
        path: dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| ConfigError::Read {
            path: dir.to_path_buf(),
            source,
        })?;
        entries.push(entry.path());
    }
    entries.sort();
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axes(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn parse_match(yaml: &str) -> Match {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    #[test]
    fn equality_leaf_matches_axis_value() {
        let m = parse_match("release: '4.0'");
        assert!(m.evaluate(&axes(&[("release", "4.0")]), &[]));
        assert!(!m.evaluate(&axes(&[("release", "3.0")]), &[]));
    }

    #[test]
    fn set_membership_leaf() {
        let m = parse_match("variant: [root-verity, usr-verity]");
        assert!(m.evaluate(&axes(&[("variant", "usr-verity")]), &[]));
        assert!(!m.evaluate(&axes(&[("variant", "grub")]), &[]));
    }

    #[test]
    fn all_any_not_combinators() {
        let all = parse_match("all: [{release: '3.0'}, {variant: grub}]");
        assert!(all.evaluate(&axes(&[("release", "3.0"), ("variant", "grub")]), &[]));
        assert!(!all.evaluate(&axes(&[("release", "3.0"), ("variant", "vm-img")]), &[]));

        let any = parse_match("any: [{variant: grub}, {phase: update}]");
        assert!(any.evaluate(&axes(&[("variant", "grub"), ("phase", "base")]), &[]));

        let not = parse_match("not: {release: '4.0'}");
        assert!(not.evaluate(&axes(&[("release", "3.0")]), &[]));
        assert!(!not.evaluate(&axes(&[("release", "4.0")]), &[]));
    }

    #[test]
    fn feature_leaf_consults_enabled_features() {
        let m = parse_match("feature: pcrlock-static-files");
        assert!(m.evaluate(&axes(&[]), &["pcrlock-static-files".to_owned()]));
        assert!(!m.evaluate(&axes(&[]), &[]));
    }

    #[test]
    fn compound_feature_and_release() {
        let m = parse_match("all: [{feature: pcrlock-static-files}, {release: '3.0'}]");
        let feats = ["pcrlock-static-files".to_owned()];
        assert!(m.evaluate(&axes(&[("release", "3.0")]), &feats));
        assert!(!m.evaluate(&axes(&[("release", "4.0")]), &feats));
    }
}
