# `image.yaml` reference

An image definition lives in an `image.yaml`. The top level belongs to tailor. The `config:` value is opaque Image Customizer YAML: tailor merges it structurally and passes it to IC.

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `name` | string | yes | Image id used by CLI and slugs. Use `[A-Za-z0-9.-]+`; `_` is reserved as the slug separator. |
| `toolchain` | string or `{name, container, version?, tag?, pull?}` | no | Workspace toolchain name or inline standalone toolchain. Defaults to workspace default or built-in `latest`; `pull` defaults to `missing`. |
| `toolsDir` | `{source, access?}` | no | Tailor-managed IC `--tools-dir`. `source` is a `toolsDirSources` name or inline `{container, tag?, pull?}`. `access` defaults to `ro`; `rw` requires `runtime.buildDirBase`. |
| `matrix` | ordered map `axis: [values]` | no | User-defined axes; their cartesian product is the candidate cells. Omit for one cell. Declaration order controls slug order and fragment precedence — order axes widest → most-specific (so `arch` is first). |
| `selectors` | `{ include?, exclude? }` | no | Which cells of the `matrix:` product to build. Lists of **selectors** (sub-cubes); `include` is an allowlist, `exclude` a denylist. Requires `matrix:`; omitted ⇒ the full product. |
| `outputs` | list of output specs | no | Defaults from workspace or built-in `cosi`. One artifact per cell × output. |
| `base` | one of `path`, `oci`, `azureLinux`, `ref` | conditional | Exactly one base resolves per cell. `ref: <name>` references a `baseImages:` slot. |
| `features` | string list | no | Enables matching `by-feature/<name>.yaml` fragments. Does not multiply cells. |
| `params` | scalar map | no | Values interpolated into `config:` strings with `${name}`. Params may reference other params. |
| `rpmSources` | path list | no | Each path is a directory of RPMs or a `.repo` file; passed as IC `--rpm-source`. |
| `operation` | `customize` or `convert` | no | Default: `customize`. |
| `signing` | `true` or profile id | no | Opt in to the signed-image pipeline. `true` ⇒ the workspace `signing.default` profile; a string ⇒ that named profile; omitted ⇒ unsigned. See [Sign an image](../how-to/sign-an-image.md). |
| `injectFiles` | boolean | no | Inert placeholder, superseded by `signing:`. Currently a no-op; do not rely on it. |
| `extraDependencies` | path list | no | Extra files/directories to hash for incremental checks; use for IC-config-referenced assets. |
| `config` | mapping or path string | conditional | Required for `customize`, forbidden for `convert`. Opaque IC config. |

## Tools dir

Use `toolsDir:` when the image needs an external package-manager userspace for IC operations. The
`source` is either a name from workspace `toolsDirSources:` or an inline container source. `access`
defaults to `ro`; `rw` gets a per-cell disposable copy under `runtime.buildDirBase` and therefore
requires that setting. Inline sources accept the same `pull: always | missing | never` policy as
workspace `toolsDirSources`; local-only images without a `RepoDigest` run by image `Id` and are not
lockable. The inline IC `config.previewFeatures` list must include `tools-dir`.

```yaml
toolsDir:
  source: acl

config:
  previewFeatures:
    - tools-dir
```

```yaml
toolsDir:
  source:
    container: quay.io/fedora/fedora
    tag: "42"
    pull: missing
  access: rw

config:
  previewFeatures:
    - tools-dir
```

tailor exports the source container to `runtime.imageCacheDir/tools-dirs/<digest>` and passes the
translated `/host/...` path to customize passes only. It never emits `--tools-dir /`, and convert or
inject-files passes do not receive the flag.

## Matrix

`matrix:` declares the axes; their cartesian product (in declaration order) is the candidate cells.
The optional `selectors:` block chooses which of those cells to actually build.

```yaml
matrix:                   # axes only — order widest → most-specific (arch first)
  arch: [amd64, arm64]
  edition: [lite, pro]
  channel: [stable, edge]

selectors:                # omit entirely ⇒ build the full product
  include:                # allowlist: keep cells matched by any selector (full product if absent)
    - { arch: amd64 }                        # every amd64 cell
    - { arch: arm64, edition: lite }         # plus the lite arm64 cells (channel expands)
  exclude:                # denylist: then drop cells matched by any selector (exclude wins)
    - { edition: pro, channel: [stable, edge] }   # a value may be a list
```

A **selector** is a partial assignment over the axes: each axis is pinned to a value or a **list** of
values, and **omitted axes match every value**. The final cell set is the union of the `include`
selectors (or the full product when `include` is empty), minus the union of the `exclude` selectors.

Axes are closed: every selector and `by-<axis>/<value>.yaml` fragment path must use declared axis
names and values. Selecting zero cells from a non-empty matrix is an error.

## Architectures

`arch` is the one **reserved** axis. Its values are closed to `amd64` and `arm64`, and the cell's
arch drives `--platform linux/<arch>`, per-arch base selection, the slug, and `${arch}`. Each cell
has exactly one arch, resolved in this order:

1. the `arch` matrix axis, one cell per value (`matrix.arch: [amd64, arm64]`);
2. else the **base image's own arch** — a `baseImages:` slot's `arch`, a local `path` base's `arch`,
   or an `oci.platform`'s arch component;
3. else **`amd64`**.

There is no `architectures:` field — neither per-image nor a workspace default. Declare a non-default
arch with the axis, or let the base image's own arch supply it. The default is fixed at `amd64` and
never the host arch, so a workspace builds the same set everywhere. See
[Architectures](../explanation/architectures.md)
and [Cross-arch building](../how-to/cross-arch-building.md).

## Fragments

Per-cell deltas live in `by-*/` files whose **path** is the condition — no inline `match:` needed. A
fragment applies to a cell when its path predicate holds:

| Path | Applies when | Kind |
| --- | --- | --- |
| `by-arch/amd64.yaml` | `arch == amd64` | single axis, single value |
| `by-mode/dev+test.yaml` | `mode ∈ {dev, test}` | single axis, **disjunction** |
| `by-boot+verity/uki+root.yaml` | `boot == uki` **and** `verity == root` | multi-axis **conjunction** |
| `by-feature/<name>.yaml` | the feature is enabled | feature flag |

`+` joins axes in the directory and values in the file. A directory naming **one** axis lets the file list
several values (a disjunction, in the axis's declared value order); a directory naming **several** axes
takes exactly one value per axis, positionally, with the axes in matrix-declared order. `image.yaml` is the
base (applies to every cell).

Apply order is merge precedence (later wins for scalars, extends lists). Fragments are sorted by: **arity**
(more axes apply later — a composite refines the singles it builds on), then **axis-declaration order**
(cross-axis precedence follows the matrix), then **breadth** (a broader disjunction applies before a
narrower single value on the same axis, so the more specific one wins). Run `tailor explain <image> --cell
<slug>` to print the exact merge order for a cell. See `meta/docs/directive-design.md` §2 for the full
model.

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

```yaml
base:
  ref: baremetal          # a named slot in tailor.yaml `baseImages:`
```

For an `oci` or `azureLinux` base, tailor resolves the registry digest and passes IC a digest-pinned
`--image oci:<repo>@sha256:…` (so the build is reproducible). Image Customizer downloads OCI input
images behind a **preview feature**, and tailor never edits your IC `config:`, so you must enable it
yourself in the image's `config:`:

```yaml
config:
  previewFeatures:
    - input-image-oci
```

Registry bases also need an image cache directory; tailor defaults `runtime.imageCacheDir` to
`<workspace>/.tailor/cache` when you set none (see [tailor.yaml](tailor-yaml.md)).

The `arch` component of an `oci.platform` must match the cell's arch, so `linux/${arch}` is the safe
spelling. Pinning a fixed `platform: linux/arm64` on an amd64 cell is a validate-time error.

A `ref:` base references a named slot from the workspace `baseImages:` catalogue and resolves to
that slot's local file (the path lives once, in `tailor.yaml`). Use it for the file-based, registry-pull-free
flow Trident needs — see [`baseImages` in tailor.yaml](tailor-yaml.md), [Use a base-image catalogue](../how-to/use-a-base-image-catalogue.md),
and [Base images](../explanation/base-images.md).

## Output spec

```yaml
outputs:
  - format: cosi
    cosiCompressionLevel: 6
    name: "${name}-${arch}"
```

`format` is required. `cosiCompressionLevel` and `name` are optional.
