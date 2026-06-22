use std::{collections::BTreeMap, path::PathBuf};

use indexmap::IndexMap;
use semver::Version;
use serde::Deserialize;
use serde_yaml_ng::Value;

use crate::types::{Arch, LogLevel, Operation, OutputFormat, ParamValue};

// ===== tailor.yaml — workspace / tool config (reference/tailor-yaml.md) =====

/// `tailor.yaml` — the workspace root: toolchains, runtime, defaults, and the image catalogue.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolConfig {
    pub schema_version: u32,
    pub toolchains: Toolchains,
    #[serde(default)]
    pub runtime: Option<Runtime>,
    #[serde(default)]
    pub defaults: Option<Defaults>,
    #[serde(default)]
    pub images: Option<ImageCatalogue>,
}

/// Repo-wide Image Customizer toolchain(s). This is where the IC version lives.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Toolchains {
    pub default: String,
    pub entries: BTreeMap<String, ToolchainEntry>,
}

/// A pinned Image Customizer container.
///
/// `version` is optional, informational metadata (tailor does not gate IC versions — `meta/docs/design.md`
/// §8). The registry tag actually pulled is `tag`, else `version`, else `latest` — see
/// [`ToolchainEntry::effective_tag`].
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolchainEntry {
    pub container: String,
    #[serde(default)]
    pub version: Option<Version>,
    #[serde(default)]
    pub tag: Option<String>,
}

impl ToolchainEntry {
    /// The registry tag to pull: an explicit `tag`, else the `version` string, else the built-in
    /// [`DEFAULT_IC_TAG`](crate::defaults::DEFAULT_IC_TAG) (`meta/docs/design.md` §5.1).
    pub fn effective_tag(&self) -> String {
        if let Some(tag) = &self.tag {
            return tag.clone();
        }
        self.version.as_ref().map_or_else(
            || crate::defaults::DEFAULT_IC_TAG.to_owned(),
            Version::to_string,
        )
    }
}

/// How an image selects its toolchain: an id (workspace) or an inline definition (standalone).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ToolchainRef {
    Id(String),
    Inline(ToolchainEntry),
}

/// Container-runtime (bollard) knobs.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Runtime {
    #[serde(default)]
    pub privileged: Option<bool>,
    #[serde(default)]
    pub mounts: Option<Mounts>,
    #[serde(default)]
    pub build_dir: Option<String>,
    #[serde(default)]
    pub log_level: Option<LogLevel>,
    #[serde(default)]
    pub image_cache_dir: Option<PathBuf>,
    #[serde(default)]
    pub janitor_image: Option<JanitorImage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mounts {
    #[serde(default)]
    pub host_root: Option<PathBuf>,
    #[serde(default)]
    pub dev: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JanitorImage {
    pub container: String,
    #[serde(default)]
    pub tag: Option<String>,
}

/// Defaults inherited by every image that does not set the field itself.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    #[serde(default)]
    pub architectures: Option<Vec<Arch>>,
    #[serde(default)]
    pub outputs: Option<Vec<OutputSpec>>,
}

/// Which images belong to the workspace. Omitted ⇒ auto-discover `*/image.yaml` at depth 1.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageCatalogue {
    #[serde(default)]
    pub members: Option<Vec<String>>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub inline: Vec<ImageDefinition>,
}

// ===== image.yaml — image definition base document (reference/image-yaml.md) =====

/// `image.yaml` — one image definition.
///
/// This models the **base document**; fragment directives are layered in a later phase. The
/// `config:` value is opaque (an inline IC config mapping or a path string) — never modeled here.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageDefinition {
    pub name: String,
    #[serde(default)]
    pub toolchain: Option<ToolchainRef>,
    #[serde(default)]
    pub architectures: Option<Vec<Arch>>,
    #[serde(default)]
    pub matrix: Option<Matrix>,
    #[serde(default)]
    pub outputs: Option<Vec<OutputSpec>>,
    #[serde(default)]
    pub base: Option<BaseSource>,
    #[serde(default)]
    pub base_by_arch: Option<BTreeMap<Arch, BaseSource>>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub params: IndexMap<String, ParamValue>,
    #[serde(default)]
    pub rpm_sources: Vec<PathBuf>,
    #[serde(default)]
    pub operation: Option<Operation>,
    #[serde(default)]
    pub inject_files: Option<bool>,
    #[serde(default)]
    pub extra_dependencies: Vec<PathBuf>,
    #[serde(default)]
    pub config: Option<Value>,
}

// ===== shared types (reference/types.md) =====

/// A base OS image source. Exactly one of three kinds, keyed by property name (a schema `oneOf`).
///
/// Modeled as an untagged enum so the YAML surface is `{ path: … }` / `{ oci: … }` /
/// `{ azureLinux: … }` (serde's externally-tagged form would instead require YAML `!tags`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum BaseSource {
    /// A local base image file, resolved relative to the image directory.
    Path { path: PathBuf },
    /// A base image pulled from an OCI registry.
    Oci { oci: OciBase },
    /// Microsoft Container Registry sugar for an Azure Linux base.
    #[serde(rename_all = "camelCase")]
    AzureLinux { azure_linux: AzureLinuxBase },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciBase {
    pub uri: String,
    #[serde(default)]
    pub platform: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzureLinuxBase {
    pub version: String,
    pub variant: String,
}

/// One output: a format plus optional knobs.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputSpec {
    pub format: OutputFormat,
    #[serde(default)]
    pub cosi_compression_level: Option<u8>,
    #[serde(default)]
    pub name: Option<String>,
}

/// A partial cell: a map of axis name to a single pinned value.
pub type CellSelector = IndexMap<String, String>;

/// An image's matrix: user-defined axes (order-preserving) plus reserved `include`/`exclude`.
///
/// `include`/`exclude` are named fields; every other key flattens into `axes` (the product),
/// preserving the declaration order that the cell slug depends on (`meta/docs/design.md` §10).
#[derive(Debug, Clone, Deserialize)]
pub struct Matrix {
    #[serde(default)]
    pub include: Vec<CellSelector>,
    #[serde(default)]
    pub exclude: Vec<CellSelector>,
    #[serde(flatten)]
    pub axes: IndexMap<String, Vec<String>>,
}
