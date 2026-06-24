# `image.yaml` reference

An image definition lives in an `image.yaml`. The top level belongs to tailor. The `config:` value is opaque Image Customizer YAML: tailor merges it structurally and passes it to IC.

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `name` | string | yes | Image id used by CLI and slugs. Use `[A-Za-z0-9.-]+`; `_` is reserved as the slug separator. |
| `toolchain` | string or `{container, version?, tag?}` | no | Workspace toolchain id or inline standalone toolchain. Defaults to workspace default or built-in `latest`. |
| `architectures` | `amd64`/`arm64` list | no | Defaults from `tailor.yaml`; equivalent to an `arch` axis when no matrix `arch` is declared. |
| `matrix` | ordered map of axes plus `include`/`exclude` | no | User-defined axes. Omit for one cell. Axis declaration order controls slug order and fragment precedence. |
| `outputs` | list of output specs | no | Defaults from workspace or built-in `cosi`. One artifact per cell × output. |
| `base` | one of `path`, `oci`, `azureLinux` | conditional | Exactly one of `base` or `baseByArch` must resolve per cell. |
| `baseByArch` | map from arch to base | conditional | Per-arch base sources, usually for local files. |
| `features` | string list | no | Enables matching `by-feature/<name>.yaml` fragments. Does not multiply cells. |
| `params` | scalar map | no | Values interpolated into `config:` strings with `${name}`. Params may reference other params. |
| `rpmSources` | path list | no | Each path is a directory of RPMs or a `.repo` file; passed as IC `--rpm-source`. |
| `operation` | `customize` or `convert` | no | Default: `customize`. |
| `signing` | `true` or profile id | no | Opt in to the signed-image pipeline. `true` ⇒ the workspace `signing.default` profile; a string ⇒ that named profile; omitted ⇒ unsigned. See [Sign an image](../how-to/sign-an-image.md). |
| `injectFiles` | boolean | no | Inert placeholder, superseded by `signing:`. Currently a no-op; do not rely on it. |
| `extraDependencies` | path list | no | Extra files/directories to hash for incremental checks; use for IC-config-referenced assets. |
| `config` | mapping or path string | conditional | Required for `customize`, forbidden for `convert`. Opaque IC config. |

## Matrix

```yaml
matrix:
  edition: [lite, pro]
  arch: [amd64, arm64]
  channel: [stable, edge]
  exclude:
    - edition: pro
      channel: stable
  include:
    - edition: lite
      arch: arm64
      channel: edge
```

Axes are closed: selectors and fragment paths must use declared axis names and values. `include` entries must pin every axis.

## Base sources

```yaml
base:
  path: ./bases/gizmo-amd64.img
```

```yaml
base:
  oci:
    uri: "registry.example/gizmo/base:edge"
    platform: "linux/${arch}"
```

```yaml
base:
  azureLinux:
    version: "3.0"
    variant: minimal-os
```

## Output spec

```yaml
outputs:
  - format: cosi
    cosiCompressionLevel: 6
    name: "${name}-${arch}"
```

`format` is required. `cosiCompressionLevel` and `name` are optional.
