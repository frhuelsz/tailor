# tailor — near-term: declarative configs-only eject + `eject --check`

> **Status:** Proposed · _2026-07-20_
>
> A focused, buildable subset of the eject design (`2026-07-16-render-ahead-eject.md`) for the
> immediate need: **emit the merged IC config YAML per cell** into a committed directory, and a
> **`tailor eject --check`** drift gate. The eject target is **declared in `tailor.yaml`**, so
> `tailor eject` and `tailor eject --check` take **no arguments** — the check becomes a trivial,
> zero-config command for a pre-commit hook and CI. Everything else in the parent docs (standalone
> build scripts, manifest, `--limited`, native-DSL emitters) is **out of scope here** and deferred.

## 1. Why this subset first

The signed pipeline cannot run tailor (`2026-07-17-pipeline-handoff-strategy.md`), but it *can*
consume committed, reviewed YAML. The merged IC config is the highest-value, most portable artifact,
and — because the tailor-managed capabilities (tools-dir, base, rpm-sources, signing) live in the IC
**invocation flags, not the `config:` tree** — a configs-only eject is **always fully ejectable**: no
strict/limited gating, no invocation portability problems. The pipeline owns the invocation and just
reads the per-cell config YAML. This is the smallest thing that unblocks the pipeline today.

## 2. Scope

**In:**

- Render every selected cell's merged IC config to a **committed** directory.
- `tailor eject` (write) and `tailor eject --check` (verify, no writes, non-zero on drift).
- A declarative **`eject:` block in `tailor.yaml`** that both commands read, so `--check` needs no
  flags.

**Out (deferred to the parent docs):** standalone `build.sh` scripts, `manifest.json`, `build-all.sh`,
`--limited` mode, native-DSL/matrix emitters, IC image pinning inside the artifact.

## 3. Declarative eject config (`tailor.yaml`)

A new optional top-level block. Its presence is what makes `eject`/`eject --check` argument-free:

```yaml
eject:
  outputDir: rendered      # committed output dir, relative to the workspace root (the only required field)
  # scope: configsOnly     # OPTIONAL — defaults to configsOnly; state it only if a non-default scope exists
  # images: [myimage]        # optional; default = all images in the workspace
```

- **`outputDir`** — the committed output root, and the only field normally needed. Distinct from
  today's per-image `.rendered/` diagnostic goldens; this is the reviewed artifact the pipeline
  consumes. Absolutized against the workspace root.
- **`scope`** — **optional, defaults to `configsOnly`**. It is an enum so a future non-default scope
  (e.g. `scripts`, parent doc §3.2) can be added without a breaking change, but it should be *omitted*
  today: `eject: { outputDir: rendered }` fully implies `scope: configsOnly`.
- **`images`** — restrict to a subset; omitted ⇒ all images. (Cell selection stays "all cells of each
  listed image"; matrix selectors can be added later if needed.)

If the `eject:` block is absent, `tailor eject` still works with CLI args (image list + `--output-dir`),
but `tailor eject --check` in CI is expected to rely on the block so it is a bare command.

## 4. Output layout

```
<outputDir>/
  <slug>.yaml     # merged IC config for one cell
```

**Flat, one `<slug>.yaml` per cell.** The slug is `cell_slug(image, axes, format)` (`matrix.rs`), so it
already encodes the image name and is **globally unique across the workspace** — no per-image subdir is
needed, and the filename matches the existing golden (`write_golden`, `render.rs` → `{slug}.yaml`).

## 5. `tailor eject` (write)

1. Load the workspace, resolve `eject.images` (default all), expand each image's matrix to cells.
2. Render each cell's IC config (pure, offline — no base/toolchain resolution, no Docker).
3. Write `<outputDir>/<slug>.yaml`.
4. **Prune stale files:** remove any `*.yaml` under `<outputDir>` that no cell in this run produces (so
   a removed cell/axis does not leave an orphaned committed file). Only files matching the eject naming
   pattern are pruned — never unrelated content under `<outputDir>`.

Deterministic: stable cell order, stable YAML key order, LF newlines, no timestamps or absolute paths,
so re-running with unchanged configs is a no-op and the committed diff is minimal.

## 6. `tailor eject --check` (drift gate)

The trivial CI/pre-commit command. With the `eject:` block present it takes **no arguments**:

1. Render the same set **to a temp dir** (never touches `<outputDir>`).
2. Compare against the committed `<outputDir>` **byte-for-byte**, detecting three kinds of drift:
   - **changed** — a committed file differs from freshly rendered;
   - **missing** — a cell renders but no committed file exists;
   - **extra** — a committed `*.yaml` exists that no cell produces (stale).
3. On any drift: print the offending paths and **exit non-zero**. On none: exit 0, silent.

Usage:

- **Pre-commit hook:** `tailor eject && git add <outputDir>` (regenerate) or `tailor eject --check`
  (block the commit on drift). tailor can scaffold the hook via `tailor init`.
- **CI gate:** `tailor eject --check` fails the PR if `<dir>` is stale — the safety net for anyone who
  bypasses the hook. This is the mechanism that lets the pipeline trust the committed YAML
  (`2026-07-17` §2).

## 7. The one correctness caveat — in-config paths

If a merged config references files (`os.additionalFiles`, `sshPublicKeyPaths`, …), those paths are
resolved by IC relative to the config-file location (or build dir). The consuming pipeline must place
the config and mount the repo so those references still resolve. For the near-term use this is fine if
either (a) the configs carry no such file references, or (b) the pipeline mounts the repo at a
consistent root. **Verify IC's resolution rule** before relying on file-referencing configs; document
the required mount root for consumers. (Same caveat as `2026-07-16` §4, narrowed to configs-only.)

## 8. Code touch-points

- `tailor-config` — an `Eject { output_dir, scope: EjectScope, images }` struct on the workspace
  config; `scope` **defaults to `EjectScope::ConfigsOnly`** (serde default, so it may be omitted);
  validation (unknown images rejected; `output_dir` absolutized vs workspace root).
- `tailor` CLI — an `eject` verb with `--check` (and optional `--output-dir`/image args as a fallback
  when no `eject:` block). Reuse `render_image`; add temp-render + byte-diff for `--check`; add the
  stale-file prune for the write path.
- Reuse unchanged: matrix expansion, fragment merge, param interpolation, golden serialization.
- **No new crate, no executor/runtime changes** — this is entirely offline config rendering.

## 9. Phasing

- **This doc (near-term):** `eject:` block, `tailor eject`, `tailor eject --check`, configs-only
  (default scope), prune, drift on changed/missing/extra.
- **Later (parent docs):** `scope: scripts` (standalone build scripts + manifest), `--limited`,
  native-DSL/matrix hand-off, IC image pinning in the artifact.

## Open questions

1. Should `eject --check` be folded into `tailor validate` (which already renders every cell) as an
   extra assertion, so one CI command covers both? Or kept separate for a clear failure signal?
   (Lean: separate, for an unambiguous drift signal.)

## Resolved

- **Output dir:** a single top-level committed `outputDir` (default `rendered/`), distinct from the
  per-image `.rendered/` diagnostic goldens — one place for the pipeline to consume.
- **Layout / filename:** flat `<outputDir>/<slug>.yaml` — the slug already encodes image+axes+format
  (globally unique) and matches the existing golden filename.
- **`scope`:** optional, defaults to `configsOnly`; omitted in practice.
