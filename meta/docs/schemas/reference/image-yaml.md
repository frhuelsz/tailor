# `image.yaml` — image definition

One buildable image — the **base document** that sibling [fragments](./fragments.md) layer onto. The
**top level is tailor fields only**; **all Image Customizer config lives under `config:`** and is
opaque to tailor. An image may declare a [`matrix:`](./types.md#matrix) that expands into many cells.

Schema: [`#/$defs/ImageDefinition`](../tailor.schema.json).

## Top-level fields

| Field | Type | Req | Default | Notes |
| ----- | ---- | --- | ------- | ----- |
| `name` | string | **yes** | — | Unique image id (`^[A-Za-z0-9][A-Za-z0-9.-]*$`, no `_`); the first segment of every output [cell slug](./types.md#output-naming-cell-slug), and the CLI handle. |
| `toolchain` | [ToolchainRef](./types.md#toolchainref) | no | workspace `default` (or built-in) | Select/override the IC version. Id ref in a workspace, or inline when standalone. |
| `architectures` | [[Arch](./types.md#arch)] | no | `defaults.architectures` | Target arches (the arch axis). |
| `matrix` | [Matrix](./types.md#matrix) | no | one cell | Axes + values whose product is the set of cells built. |
| `outputs` | [[OutputSpec](./types.md#outputspec)] | no | `defaults.outputs` | Output formats; each cell × format is one IC run → one artifact, named by its [cell slug](./types.md#output-naming-cell-slug). |
| `base` | [BaseSource](./types.md#basesource) | cond | — | The base image. **Exactly one** of `base` / `baseByArch` (may instead be supplied by a fragment). |
| `baseByArch` | map<[Arch](./types.md#arch) → [BaseSource](./types.md#basesource)> | cond | — | Per-arch bases (for local files that differ by arch). Mutually exclusive with `base`. |
| `features` | [string] | no | — | Image-level **feature flags** (e.g. `pcrlock-static-files`); enable `by-feature/<name>.yaml` fragments. |
| `params` | [Params](./types.md#params) | no | — | Named scalar constants for `${...}` interpolation. |
| `rpmSources` | [string] | no | — | Extra IC `--rpm-source` entries (a dir of RPMs or a `.repo` file). |
| `operation` | `customize` \| `convert` | no | `customize` | tailor-level IC operation (see [Operation](./types.md#operation)). |
| `injectFiles` | bool | no | `false` | Run IC inject-files signing when the IC config sets `output.artifacts`. |
| `extraDependencies` | [string] | no | — | Extra input paths for the incremental up-to-date check — the only way to track files referenced from inside the opaque `config:`. |
| `config` | object \| string | no | — | **Image Customizer config** — inline mapping or path. **Opaque** (not validated here). |

> **`features` vs operation flags.** `features:` here is the **flag list** that gates `by-feature/`
> fragments. The IC *operation* knobs are the separate top-level `operation:` and `injectFiles:`
> fields (this differs from an earlier draft that nested them under `features:`).

> **`base` / `baseByArch` may come from a fragment.** An image with a `matrix` whose base differs per
> axis often sets no `base` at the top level — `by-arch/`/`by-release/` fragments supply it. The
> *resolved* cell must still end up with exactly one base.

## Minimal example (standalone)

```yaml
name: appliance
toolchain:                          # inline (no tailor.yaml to reference an id); omit → built-in default
  container: mcr.microsoft.com/azurelinux/imagecustomizer
  version: "1.3.0"
outputs:
  - format: cosi
base:
  azureLinux:
    version: "3.0"
    variant: minimal-os
config:                             # ← everything below is opaque IC config
  os:
    hostname: appliance
    packages:
      install: [openssh-server, vim, chrony]
  storage:
    bootType: efi
    disks:
      - partitionTableType: gpt
        partitions:
          - id: esp
            type: esp
            size: 8M
          - id: rootfs
            size: grow
    filesystems:
      - deviceId: esp
        type: fat32
        mountPoint:
          path: /boot/efi
          options: "umask=0077"
      - deviceId: rootfs
        type: ext4
        mountPoint:
          path: /
```

## Matrix example (multi-variant)

```yaml
name: trident-vm-testimage
features: [pcrlock-static-files]
matrix:
  variant: [grub, root-verity, usr-verity]
  arch:    [amd64, arm64]
  release: ["3.0", "4.0"]
  phase:   [base]
outputs:
  - format: cosi
config:
  os:
    # ...the shared IC config; per-axis deltas live in by-*/ fragments
    hostname: trident-vm-testimg
```

The per-axis deltas (`by-variant/grub.yaml`, `by-release/4.0.yaml`, …) are
[fragments](./fragments.md). See [`examples/trident-vm-testimage`](../../examples/trident-vm-testimage)
for the complete, verified image.
