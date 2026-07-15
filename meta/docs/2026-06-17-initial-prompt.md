# tailor — initial design + implementation prompt

> Kickoff prompt for the agent that will design and build `tailor`. Authored 2026-06-17.

## Mission

You are designing and implementing **tailor**: a Cargo-style front-end for the Azure Linux
Image Customizer. Image Customizer (`imagecustomizer`, in `azure-linux-image-tools`) is the
low-level engine that, given a base image plus a YAML config, produces a customized OS image.
`tailor` is the manifest-driven layer on top: it handles **configuration management and
execution orchestration** for image builds. It is NOT a dependency manager and has NO registry.
The Cargo analogy is about structure (one central tool config + per-target configs + reproducible
execution), not dependency resolution. Treat `tailor` as a working name — keep it trivially
renameable.

## Firm requirements (decided — do not relitigate)

- **Language: Rust.** Decided after weighing Go: the tailor<->IC coupling is a container CLI
  contract (not a library link), and IC is container-only, so Go offered no in-process integration
  advantage. Rust was chosen for alignment with the Trident (Rust) ecosystem and maintainer
  preference. Prefer Rust crates over shelling out.
- **Execution is container-only.** Upstream IC no longer supports raw-binary execution. Drive the
  IC **container via Docker using the Rust `bollard` crate**, not the `docker` CLI.
- **Support a few of the most recent IC versions.** IC is tagged v0.3.0 … v1.3.0; pin the
  container by tag + digest. Version/compat handling is first-class.
- **Manifest format: YAML.**
- **Base image source = a local file OR an OCI location.** This maps directly onto IC's native
  `input.image` (`path` for a local file, `oci` for a registry image). There is no module or
  dependency graph.
- **Multi-arch / multi-output: yes**, via Docker `--platform` and via configuration.

## Required architecture (the shape we want)

- One central **"configure the tools" file**: the toolchain — which IC container version(s)/
  digests to use, Docker runtime settings, global defaults. (Analogous to Cargo `[workspace]` and
  to Trident builder's `ArtifactManifest`.)
- A set of **"configure the targets" configs**: each target primarily **points to a root IC
  config YAML** (the root of that IC invocation), plus base-image source (file | OCI), output
  format(s), architecture(s), and feature flags. (Analogous to Cargo workspace members and to
  Trident builder's `ImageConfig` list.)
- **A lockfile** pinning IC container digest(s) + resolved base-image digest for reproducible
  rebuilds.

## Ground yourself first (before any design)

1. **Study the prior art: `~/repos/trident/tests/images/builder`** (Python). `tailor` is a
   fleshed-out, Rust, config-driven generalization of it. Read `README.md`, `cli.py`, `builder.py`,
   `customize.py`. Note its proven model: declarative image definitions (`testimages.py`),
   `ArtifactManifest` (the IC container to use) vs `ImageConfig` (a target), and the pipeline
   stages: download base image → customize (run IC container) → convert (output format) → sign /
   inject-files (SecureBoot) → run. Also: image cloning, parallel builds, incremental up-to-date
   checks (output vs dependency mtimes), rpm-source mounting.

2. **Study the engine: `~/repos/azure-linux-image-tools`** (Go). Read the IC config schema under
   `docs/imagecustomizer/api/configuration/` and the published docs at
   https://microsoft.github.io/azure-linux-image-tools/. Confirm the container CLI surface and how
   versions are tagged.

3. **The IC container invocation contract** you must reproduce via the Rust `bollard` crate (taken
   verbatim from `builder/customize.py`):
   ```
   docker run --rm --privileged -v /:<HOST> -v /dev:/dev [--platform <arch>] \
     <ic_container> --config-file <root.yaml> --log-level <lvl> --build-dir <dir> \
     --image-file <base_image> --output-image-format <fmt> --output-image-file <out> \
     [--rpm-source <path> ...]
   ```
   Plus the `inject-files` and `convert` subcommands.

## Real IC config schema (top-level — the "root IC config YAML" a target points to)

The root config (IC `config` type) has these top-level keys. A target's referenced YAML IS this
object; `tailor` selects the IC container version, may inject/override `input.image` and
`output`, and runs it reproducibly.

- `input.image` — the base image. One of: `path` (local file) | `oci` (`uri` + `platform`) |
  `azureLinux` (`version` + `variant`). This is exactly the "file or OCI" source requirement.
- `storage` — disks, partitions, filesystems, verity, etc.
- `os` — hostname, `kernelCommandLine`, `packages`, `services`, users, groups, modules, selinux,
  bootloader, additional files/dirs.
- `scripts` — post/finalize customization scripts.
- `output.image` — `path`, `format` (vhd / qcow2 / iso / cosi / …), `cosi` compression; and
  `output.artifacts` — signed boot artifacts (`path`, `items`).
- `iso` / `pxe` — ISO / PXE live outputs.
- `previewFeatures` — opt-in features (e.g. inject-files).

## Cargo → tailor mapping (reframed: config management, not dependencies)

- cargo → tailor; rustc → imagecustomizer (container).
- `Cargo.toml [workspace]` → "configure the tools" YAML (IC version(s)/digests, Docker opts,
  defaults).
- member `Cargo.toml` → per-target config (root IC config YAML + base image source + output
  format(s) + arch + features).
- `Cargo.lock` → lock pinning IC container digest(s) + base-image digest for reproducible builds.
- crates / registry / dependency graph → N/A.
- workspace members → multiple targets under one tool config.
- profiles → output/arch matrix (via Docker platform + config).

## Scope — staged. Do NOT boil the ocean.

1. **Design doc first.** Write it in the repo at an agreed path. Cover: problem statement; the
   tool-config and target-config YAML schemas with one concrete annotated example of each; how the
   two relate (and resolve the open layout question below); the lockfile + reproducibility model;
   the Docker-execution layer (the `bollard` crate, how the container is run, mounts, privileged,
   platform, how IC versions are selected/pinned); the IC-version compatibility strategy; output
   formats and multi-arch; base-image resolution (local file vs OCI pull, mapping to `input.image`);
   the command/verb surface; and an explicit non-goals list (no dependency graph, no registry). Get
   sign-off before coding.

2. **MVP.** Thinnest end-to-end slice in Rust: parse a tool config + one target config → resolve
   the pinned IC container and the base image → run IC in the container via `bollard` with the
   target's root IC config YAML → produce the output artifact → write a lockfile → support a locked
   rebuild. Plus a few core verbs (e.g. `build`, `show`, `list`).

3. **Iterate**: multiple targets / matrix, multi-arch, OCI base-image sources, convert + signing
   stages, caching/incremental rebuilds. Each milestone proposed before it is built.

## Working method

- Start by asking the open questions below; do not assume.
- Design → review → implement in small, reviewable increments.
- Show the two YAML schemas + one real example early; the manifests are the heart of the tool.
- Validate against a real IC container run end-to-end as soon as the MVP can. A design that cannot
  drive the actual container is wrong.
- Match Rust + Azure Linux repo conventions (cargo build, clippy, tests, license headers).

## Open questions to resolve first

1. **Layout:** tool config and targets in one YAML file, or separate files (or support both)?
2. **Reproducibility:** exactly what the lockfile pins, and how base-image / container digests are
   resolved (local file hashing vs OCI digest).
3. **IC version range:** how many / which recent versions to support, and how version differences
   are abstracted behind one tailor manifest.
4. **Project location:** the `~/repos/tailor` repo (already initialized) — standalone, or vendored
   elsewhere later?

## First action

Read the trident builder and the IC repo to ground yourself, then return with: (a) a refined
tool-config + target-config schema sketch (YAML), (b) recommendations/answers to the open
questions, and (c) a proposed design-doc outline. Do NOT write implementation code yet.
