use std::path::PathBuf;

/// Errors from loading, parsing, and expanding tailor configuration.
///
/// Typed (no `anyhow`) so diagnostics can name the contributing file, axis, or selector — see
/// `meta/docs/architecture.md` §6.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file `{}`", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse YAML in `{}`", .path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },

    #[error("failed to write `{}`", .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("matrix axis `{axis}` has no values (every declared axis needs at least one)")]
    EmptyAxis { axis: String },

    #[error(
        "matrix `{selector}` selector references axis `{axis}`, which the matrix does not declare"
    )]
    SelectorUnknownAxis {
        selector: &'static str,
        axis: String,
    },

    #[error(
        "matrix `{selector}` selector pins axis `{axis}` to `{value}`, which is not a declared value"
    )]
    SelectorUnknownValue {
        selector: &'static str,
        axis: String,
        value: String,
    },

    #[error("matrix `include` selector is incomplete: it does not pin axis `{axis}`")]
    IncludeIncomplete { axis: String },

    #[error(
        "conflicting values for `{path}`: `{existing}` is already set, but {fragment} sets `{incoming}`; \
         write `{{ $set: … }}` to override on purpose"
    )]
    ScalarConflict {
        path: String,
        existing: String,
        incoming: String,
        fragment: String,
    },

    #[error(
        "type conflict at `{path}`: {fragment} merges a {incoming_kind} onto an existing {existing_kind}"
    )]
    TypeConflict {
        path: String,
        existing_kind: &'static str,
        incoming_kind: &'static str,
        fragment: String,
    },

    #[error("directive `{directive}` at `{path}` must be the sole key of its mapping")]
    DirectiveNotSole { directive: String, path: String },

    #[error("directive `{directive}` at `{path}` expects {expected}")]
    DirectiveShape {
        directive: &'static str,
        path: String,
        expected: &'static str,
    },

    #[error("unknown directive `{directive}` at `{path}`")]
    UnknownDirective { directive: String, path: String },

    #[error("directive `{directive}` at `{path}` is not supported yet")]
    UnsupportedDirective {
        directive: &'static str,
        path: String,
    },

    #[error("directive `{directive}` at `{path}` must be resolved before merge")]
    UnresolvedDirective {
        directive: &'static str,
        path: String,
    },

    #[error("undefined interpolation variable `${{{name}}}` (in `{at}`)")]
    UndefinedVar { name: String, at: String },

    #[error("unterminated `${{` interpolation in `{text}`")]
    UnterminatedInterpolation { text: String },

    #[error("parameter interpolation cycle: {chain}")]
    ParamCycle { chain: String },

    #[error("`$include` cycle detected: {chain}")]
    IncludeCycle { chain: String },

    #[error("`$include` at `{path}` must be a repo-root-relative path string")]
    IncludePathInvalid { path: String },

    #[error(
        "fragment directory `by-{axis}/` references axis `{axis}`, which the image's matrix does not declare"
    )]
    UnknownFragmentAxis { axis: String },

    #[error("fragment `{file}` selects value `{value}` for axis `{axis}`, which is not declared")]
    UnknownFragmentValue {
        axis: String,
        value: String,
        file: String,
    },

    #[error("parameter `{name}` is set to conflicting values `{existing}` and `{incoming}`")]
    ParamConflict {
        name: String,
        existing: String,
        incoming: String,
    },

    #[error("cell `{slug}` resolves to no `base`; set one in `image.yaml` or a per-axis fragment")]
    MissingBase { slug: String },

    #[error("cell `{slug}` has an invalid `{field}`: {detail}")]
    InvalidField {
        slug: String,
        field: &'static str,
        detail: String,
    },

    #[error(
        "cell `{slug}` resolves to an ambiguous `base` ({kinds}); a base is one of `path`/`oci`/`azureLinux` — \
         use `$set` to override on purpose"
    )]
    AmbiguousBase { slug: String, kinds: String },

    #[error("signing profile `{profile}` is invalid: {detail}")]
    InvalidSigningProfile { profile: String, detail: String },

    #[error(
        "unknown signing profile `{profile}`; define it under `signing.profiles` in tailor.yaml"
    )]
    UnknownSigningProfile { profile: String },

    #[error("signing is requested but misconfigured: {detail}")]
    SigningMisconfigured { detail: String },
}
