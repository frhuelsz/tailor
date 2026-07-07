# CLI reference

Global options:

| Option | Meaning |
| --- | --- |
| `--manifest <PATH>` | Path to `tailor.yaml`, or a directory to search from. Default: walk up from the current directory. |
| `--engine <docker\|podman\|auto>` | Container engine for this invocation. Overrides `runtime.engine`. See [Select a container engine](../how-to/select-a-container-engine.md). |
| `--host <ENDPOINT>` | Engine endpoint for this invocation (`unix://…` or `tcp://…`). Overrides `runtime.host` and `DOCKER_HOST` / `CONTAINER_HOST`. |
| `--strict` | Promote authority/confinement warnings to errors. |
| `-v`, `--verbose` | Increase verbosity; repeatable. |
| `-q`, `--quiet` | Decrease verbosity; repeatable. |
| `--timestamps <elapsed\|time\|off>` | Leading timestamp on status/log lines: `elapsed` seconds since start (default), wall-clock `time` (`HH:MM:SS`), or `off`. |
| `--log-dir <PATH>` | Persist each cell's full IC debug log to `<PATH>/<slug>.log`. |
| `--build-dir-base <PATH>` | Absolute host directory under which each cell's IC build-dir is created (`--build-dir /host/<PATH>/<slug>`). Overrides `runtime.buildDirBase`. Use a data mount (separate filesystem from the container rootfs) to avoid IC's ACL overlay `ELOOP`; needs room for a multi-GB raw copy of the base. |
| `--ic-log-level <LEVEL>` | Set IC's own `--log-level` (default `debug`), independent of `-v`/`-q`. |
| `--version` | Print version, including commit/build metadata. |

## `tailor init <name> [base|simple|advanced]`

Scaffold a project. Omitted template is `base`.

| Template | Creates |
| --- | --- |
| `base` | `tailor.yaml` plus `<name>/image.yaml`. |
| `simple` | Standalone `./image.yaml`; no `tailor.yaml`; uses built-in default IC toolchain. |
| `advanced` | Like `base`, plus `arch` and `variant` axes, `by-arch/`, `by-variant/`, and `${efiArch}` interpolation. |

## `tailor add image <name>`

Add a member image to an existing workspace. Requires a `tailor.yaml` in the current directory or a parent. Creates `<name>/image.yaml` in the current directory and registers it in `tailor.yaml`.

## `tailor add axis [<image>] <axis>`

Append an axis to an image's `matrix:` and create `by-<axis>/`. The image argument is optional when the workspace has a single image. A placeholder value is inserted so the matrix stays non-empty.

## `tailor build [images...]`

Resolve and run Image Customizer for selected images. Default: all images.

| Flag | Meaning |
| --- | --- |
| `-s`, `--select AXIS=VALUE` | Constrain matrix axes. Repeatable. Comma-separated axis pairs are accepted, for example `-s arch=amd64,variant=full`. |
| `--cell SLUG` | Select exact cells by slug. Repeatable. |
| `--locked` | Require a complete `tailor.lock`; fail on missing entries or registry drift. |
| `--force` | Ignore incremental up-to-date checks. |
| `--arch ARCH` | Restrict build to architecture(s). Repeatable. |
| `--output-dir PATH` | Output directory. Default: `<workspace>/artifacts`. |
| `--dry-run` | Render each selected container/IC invocation without running it. |
| `--jobs N` | Reserved; currently sequential. |
| `--clones N` | Build N identical clones of each cell. Default: `1`. |

## `tailor validate [images...]`

Render every selected cell without building. Catches tailor-owned config and merge errors. Accepts `-s/--select` and `--cell`.

## `tailor matrix [images...] [--format json|slugs|ado] [--ado <VAR>]`

Emit selected matrix cells. Default format is `json`.

- `json` — a JSON array whose entries contain `image`, `slug`, `axes`, and `format`, plus `baseImage`
  when the cell binds to a `baseImages:` catalogue slot.
- `slugs` — one cell slug per line (same as `tailor slugs`).
- `ado` — the bare Azure DevOps matrix object (`{ leg: { var: string, … } }`) for a pipeline
  `strategy: matrix`. `--ado <VAR>` wraps it in a `##vso[task.setvariable variable=<VAR>;isOutput=true]`
  logging command so a later stage can consume it; an empty selection fails non-zero.

## `tailor slugs [images...]`

Print one selected cell slug per line. Equivalent to `tailor matrix --format slugs`.

## `tailor explain <image>`

Print the **merge order** for each selected cell: the ordered list of fragment files that merge into it
(base first, later files win), each annotated with why it applies and any `$include`d libraries. This makes
the fragment precedence model legible. Add `--with-config` to also print the merged Image Customizer
config. Accepts `-s/--select` and `--cell`; read-only and offline.

```text
$ tailor explain gizmo --cell gizmo_arm64_pro_stable_cosi
cell  gizmo_arm64_pro_stable_cosi   (arch=arm64, channel=stable, edition=pro)

merge order (top = base, bottom wins):
   1  image.yaml                      base
   2  by-arch/arm64.yaml              arch=arm64
   3  by-edition/pro.yaml             edition=pro
   4  by-channel/stable.yaml          channel=stable
   5  by-arch+edition/arm64+pro.yaml  arch=arm64 ∧ edition=pro
```

## `tailor show <image> [field]`

Show dimensions and cell count for one image. Optional fields currently include `name`, `dir`, `outputs`, and `features`.

## `tailor list`

List images and toolchains.

## `tailor render [images...]`

Write golden snapshots for selected cells. Accepts `-s/--select` and `--cell`.

## `tailor lock`

Resolve registry inputs and write `tailor.lock` without building.

## `tailor update`

Re-resolve and rewrite `tailor.lock`.

## `tailor resolve [images...]`

Resolve digests/hashes and print the lockfile content without writing it.

## `tailor clean [images...]`

Remove generated artifacts and build stamps for selected cells. Accepts `-s/--select` and `--cell`.

## `tailor bases list`

List base-image catalogue slots: each slot's arch, source, on-disk presence, and path. Requires a
`baseImages:` catalogue in `tailor.yaml`.

## `tailor bases download [names...] [--force]`

Materialise base-image catalogue slots from their `source`. Default (no names): every slot that has a
`source` and whose file is missing. Naming a sourceless slot is an error; `--force` re-pulls present
files. Requires a `baseImages:` catalogue in `tailor.yaml`.

## `tailor bases verify [names...]`

Assert base-image slot files exist on disk, failing with the missing names and paths. Default scope is
every slot referenced by the workspace's images; pass names to check only those. The pipeline's "is the
feed download wired?" gate. See [Use a base-image catalogue](../how-to/use-a-base-image-catalogue.md).

## `tailor version`

Print version information. Same source as `tailor --version`.
