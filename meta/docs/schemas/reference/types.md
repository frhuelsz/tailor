# Shared types

Reusable types referenced by [`tailor.yaml`](./tailor-yaml.md), [`image.yaml`](./image-yaml.md), and
[fragments](./fragments.md). Schema `$defs` live in [`../tailor.schema.json`](../tailor.schema.json).

## ToolchainEntry

A pinned Image Customizer container. The resolved registry **digest** (not the tag) is written to
`tailor.lock`.

| Field | Type | Req | Notes |
| ----- | ---- | --- | ----- |
| `container` | string | **yes** | Registry path, e.g. `mcr.microsoft.com/azurelinux/imagecustomizer`. |
| `version` | string (semver) | no | Optional, informational IC version (tailor does not gate IC versions). Used as the pull tag when `tag` is absent. |
| `tag` | string | no | Registry tag pulled. Default = `version` (MCR publishes unprefixed tags, e.g. `:1.3.0`), else `latest` when neither is set. |

## ToolchainRef

How an [image](./image-yaml.md) selects its toolchain (the `toolchain:` field). One of:

- **string id** — references a `toolchains.entries` key in `tailor.yaml` (workspace mode):
  `toolchain: ic-1.1`
- **inline [ToolchainEntry](#toolchainentry)** — self-contained (standalone mode, no `tailor.yaml`):
  ```yaml
  toolchain:
    container: mcr.microsoft.com/azurelinux/imagecustomizer
    version: "1.3.0"
  ```
- **omitted** — use the workspace `toolchains.default`, or tailor's built-in default when standalone.

## BaseSource

The base OS image. **Exactly one** of four kinds. Drives IC's input image via the command line
(overrides any `input.image` in the IC config).

| Kind | Shape | Notes |
| ---- | ----- | ----- |
| local file | `path: ./artifacts/core.vhdx` | Single-arch; for per-arch local files use the `baseImages` catalogue with one slot per arch. |
| named catalogue image | `image: azure-linux-3-amd64` | References a `tailor.yaml` `baseImages` entry. |
| OCI | `oci: { uri, platform? }` | Any registry; `platform` defaults to `linux/<arch>` per cell. Digest pinned in lock. |
| Azure Linux (MCR) | `azureLinux: { version, variant }` | Sugar over `oci`. Multi-arch manifest ⇒ one source covers every arch. |

```yaml
base:
  azureLinux:
    version: "3.0"
    variant: minimal-os
```

For per-arch local files, define one `baseImages` catalogue slot per architecture in `tailor.yaml`,
then use `by-arch/<arch>.yaml` fragments to set `base: { ref: <slot> }`.

## OutputSpec

One output format. Each output × architecture is a separate IC invocation → one artifact.

| Field | Type | Req | Notes |
| ----- | ---- | --- | ----- |
| `format` | [OutputFormat](#outputformat) | **yes** | The image format. |
| `cosiCompressionLevel` | int 1–22 | no | zstd level; **only** for `format: cosi` (requires IC ≥ 1.2). |
| `name` | string | no | Output-name **template** that overrides the default [cell slug](#output-naming-cell-slug). Supports `${...}` (axis values + `${name}`/`${arch}`/`${format}`). |

```yaml
outputs:
  - format: cosi
    cosiCompressionLevel: 22
  - format: vhdx
```

## OutputFormat

`cosi` · `vhd` · `vhd-fixed` · `vhdx` · `qcow2` · `raw` · `iso` · `pxe-dir` · `pxe-tar` ·
`baremetal-image`

The `convert` [operation](#operation) supports only a subset: `vhd`, `vhd-fixed`, `vhdx`, `qcow2`,
`raw`, `cosi`, `baremetal-image` (`iso`/`pxe-*` are customize-only).

## Output naming (cell slug)

Every built artifact is named by its **cell slug** — the cell's full coordinate — so cells that differ
in *any* axis never collide. The default basename is:

```text
<image-name>_<value of each declared axis, in matrix order>_<format>.<ext>
```

- **Every declared axis appears**, even single-valued ones (stable, predictable paths).
- Segments are joined by `_`, which is **reserved**: image names and axis values must match
  `[A-Za-z0-9.-]+` (no `_`). Kebab (`-`) and dotted (`3.0`) values are still fine, so the slug stays
  unambiguous and reversible (`split('_')`).
- `outputs[].name` overrides the basename with a `${...}` template (default = the full slug). tailor
  computes all paths up front and **rejects collisions**.

```text
trident-vm-testimage_grub_amd64_4.0_base_cosi.cosi
trident-vm-testimage_vm-img_amd64_4.0_base_vhd-fixed.vhd
```

The same slug keys the working-copy config, the build stamp, and the rendered golden (design.md
§7.6/§9.2/§10). `build --clones N` appends a clone index. `pxe-dir` is a directory (no extension),
`pxe-tar` a tarball. Artifacts land in `<output-dir>` (default `<manifest-dir>/artifacts/`).

## Matrix

An image's axis declaration **and** build set. Each user-defined axis maps to its **closed** list of
values; the cartesian product (minus `exclude`, plus `include`) is the set of cells built. Axis names
match `^[A-Za-z][A-Za-z0-9_-]*$` and exclude the reserved keys `include`/`exclude`. **Axis values must
match `[A-Za-z0-9.-]+`** (no `_` — it is the reserved [cell-slug](#output-naming-cell-slug) separator),
so every value is safe as both a fragment filename and an output-name segment.

| Key | Type | Notes |
| --- | ---- | ----- |
| *(axis name)* | [string] (≥ 1, unique) | The axis's closed value set. |
| `include` | [CellSelector] | Extra cells to add beyond the product. |
| `exclude` | [CellSelector] | Cells to remove from the product. |

A **CellSelector** is a partial cell — a map of axis name → a single pinned value:

```yaml
matrix:
  variant: [grub, root-verity, usr-verity]
  arch:    [amd64, arm64]
  release: ["3.0", "4.0"]
  exclude:
    - variant: usr-verity
      release: "3.0"          # this combination isn't built
  include:
    - variant: grub
      arch: arm64
      release: "4.0"          # add one extra cell
```

## Params

Named **scalar** constants for `${...}` interpolation into `config:` string values (and into other
params / axis values). Values are scalars only — never structure.

```yaml
params:
  efiArch: x64                          # a plain constant
  grubEfiPkg: "grub2-efi-${efiArch}"    # interpolates another param → grub2-efi-x64
```

Map of `string → string | number | boolean`. Axis values (`${arch}`, `${release}`, …) are available
to interpolation without being declared.

## Operation

`customize` (default) | `convert`. The tailor-level IC operation.

- **`customize`** — the normal path; `config` required (per cell), `base` required, full
  output-format set.
- **`convert`** — `config` **forbidden**, base must be a local `path`, `rpmSources`/`injectFiles`
  forbidden, output formats restricted to the convert subset (see [OutputFormat](#outputformat)).

## Arch

`amd64` | `arm64`. Maps to the container platform `linux/<arch>`.

## LogLevel

`panic` · `fatal` · `error` · `warn` · `info` (default) · `debug` · `trace`. IC `--log-level`.
