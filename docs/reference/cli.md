# CLI reference

Global options:

| Option | Meaning |
| --- | --- |
| `--manifest <PATH>` | Path to `tailor.yaml`, or a directory to search from. Default: walk up from the current directory. |
| `--strict` | Promote authority/confinement warnings to errors. |
| `-v`, `--verbose` | Increase verbosity; repeatable. |
| `-q`, `--quiet` | Decrease verbosity; repeatable. |
| `--version` | Print version, including commit/build metadata. |

## `tailor init <name> [base|simple|advanced]`

Scaffold a project. Omitted template is `base`.

| Template | Creates |
| --- | --- |
| `base` | `tailor.yaml` plus `<name>/image.yaml`. |
| `simple` | Standalone `./image.yaml`; no `tailor.yaml`; uses built-in default IC toolchain. |
| `advanced` | Like `base`, plus `variant` and `arch` axes, `by-variant/`, `by-arch/`, and `${efiArch}` interpolation. |

## `tailor add image <name>`

Add a member image to an existing workspace. Requires a `tailor.yaml` in the current directory or a parent. Creates `<name>/image.yaml` in the current directory and registers it in `tailor.yaml`.

## `tailor add axis [<image>] <axis>`

Append an axis to an image's `matrix:` and create `by-<axis>/`. The image argument is optional when the workspace has a single image. A placeholder value is inserted so the matrix stays non-empty.

## `tailor build [images...]`

Resolve and run Image Customizer for selected images. Default: all images.

| Flag | Meaning |
| --- | --- |
| `-s`, `--select AXIS=VALUE` | Constrain matrix axes. Repeatable. Comma-separated axis pairs are accepted, for example `-s variant=full,arch=amd64`. |
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

## `tailor matrix [images...] [--format json|slugs]`

Emit selected matrix cells. Default format is `json`.

JSON entries contain `image`, `slug`, `axes`, and `format`.

## `tailor slugs [images...]`

Print one selected cell slug per line. Equivalent to `tailor matrix --format slugs`.

## `tailor explain <image>`

Print the rendered Image Customizer config per selected cell. Accepts `-s/--select` and `--cell`.

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

## `tailor version`

Print version information. Same source as `tailor --version`.
