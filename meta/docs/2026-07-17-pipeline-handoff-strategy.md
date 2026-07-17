# tailor — pipeline hand-off under a "reviewed-artifacts-only" signed pipeline

> **Status:** Proposed · _2026-07-17_
>
> Constraint (decided): tailor is currently an **unofficial, personal skunkworks project** (personal
> GitHub) — **not** an org-owned, org-built, trust-chained tool. So there is **no provenance/trust
> chain that would let it run inside an official signed release pipeline** — not as a dependency, a
> downloaded binary, or a container step. tailor therefore runs **only in dev and PR-gate CI**, and
> the official pipeline consumes **only artifacts committed to the repo and approved through normal
> PR review** — their trust derives from a human reviewing the committed *output*, not from tailor's
> provenance. This doc frames tailor as a **build-input generator** whose output is reviewed like
> source, and ranks the artifact formats it can emit. It generalizes the eject proposal
> (`2026-07-16-render-ahead-eject.md`), which is one such format.
>
> **Transitional by design.** This constraint is expected to relax if/when tailor gains an official
> trust chain; the strategy is deliberately a **bridge** (see §7), not a permanent architecture that
> would be painful to unwind once tailor can run in pipelines directly.

## 1. The reframe — where tailor runs vs what the pipeline consumes

Three loci for tailor's config expansion:

1. **In the signed pipeline** — blocked: tailor has no provenance/trust chain there **yet** (it is an
   unofficial personal project).
2. **In an adjacent trusted context** — the dev inner loop and PR-gate CI. tailor is allowed here.
3. **Never runs** — its logic re-expressed in the pipeline's own tooling (out of scope; needs a
   reimplementation).

This design lives in **locus 2**: tailor expands the matrix, merges fragments, resolves params, and
**emits committed artifacts**; the signed pipeline is a pure **consumer** of those artifacts. tailor
is a compiler whose object code is checked in and code-reviewed — like generated protobufs or a
vendored lockfile. Crucially, the artifacts' trustworthiness comes from **PR review of the committed
output** (plus §2), so it does **not** depend on tailor itself being trust-chained — which is exactly
why this works while tailor is still unofficial.

## 2. The trust mechanism (the linchpin)

"Consume only reviewed artifacts" is only safe if every consumed artifact is **both**:

1. **Committed and PR-reviewed** — it appears in the diff, so a human approves the actual bytes the
   pipeline will run; and
2. **Proven to match its source config** — tailor runs in **PR-gate CI** in `--check` mode
   (render/emit to a temp dir, byte-diff against the committed artifacts, **fail on drift**), so a
   reviewer can trust that the committed generated files faithfully reflect the tailor configs and
   were not hand-edited or left stale.

Without (1) the pipeline runs unreviewed content; without (2) the committed artifacts rot. Both are
mandatory and must ship together. This is the same drift-check described for eject
(`2026-07-16-render-ahead-eject.md` §5), applied to whatever artifact format is chosen below.

## 3. Artifact-format spectrum (what to commit), ranked

All three emit the **committed rendered IC config per cell** (today's deterministic golden,
`render.rs`). They differ in *how the pipeline is told to run IC*.

### 3.1 Compile to the pipeline's native DSL — **recommended**

tailor generates the signed pipeline's **own manifest** — e.g. ADO stages/template parameters
(building on the existing `tailor matrix --ado` runtime matrix, `ado.rs` /
`2026-06-29-ado-matrix.md`), or Make/`doit` tasks — committed alongside the rendered configs. The
pipeline then drives IC with its **existing, already-approved invocation machinery**, parameterized
per cell (config path, IC image digest, base, rpm-sources, output).

Why it's the best fit: the pipeline's IC-invocation step is presumably **already reviewed and
blessed**; tailor adds *no* new runtime tool and *no* bespoke runner — it only fills in reviewed
pipeline config (the matrix + per-cell parameters). The review surface is "pipeline YAML/Make diff,"
which the pipeline owners already review.

### 3.2 Structured build plan + the pipeline's own generic runner

tailor commits a machine-readable **`plan.json`**: per cell → IC image digest, the full arg vector,
mounts, and config path (`build --dry-run` + `matrix --json` are ~80% of this, minus the absolute-path
portability fix from the eject doc §4). The signed pipeline has a small **generic runner in its own
stack** (bash/python) — reviewed once — that iterates the plan and runs IC. Data is reviewed per
change; the runner rarely changes. Good when the pipeline prefers a data-driven loop over generated
DSL.

### 3.3 Standalone eject scripts — see the companion doc

tailor commits standalone per-cell build scripts (`2026-07-16-render-ahead-eject.md`). Most
self-contained, but introduces a **new script surface** to review and maintain instead of reusing the
pipeline's approved machinery — so it's the least preferred under this constraint unless the pipeline
has no reusable IC-invocation step to target.

## 4. Where tailor runs

- **Dev inner loop:** `tailor build` / `render` / `matrix` for fast iteration.
- **PR-gate CI (trusted):** on every PR, tailor **regenerates** the committed artifacts and runs the
  `--check` drift gate + the golden diff, so a config change and its regenerated artifacts land in the
  **same reviewed PR**. This is where the "faithful & fresh" guarantee is produced.
- **Signed release pipeline:** consumes the committed artifacts only. tailor is never present.

## 5. Relationship to the eject proposal

- Eject (`2026-07-16-render-ahead-eject.md`) is **artifact format §3.3**. This doc adds formats §3.1
  (native-DSL compile, recommended) and §3.2 (structured plan) and the *where-tailor-runs* + *trust*
  framing.
- The **ejectability hard-error rule** (`…eject` §6) applies to **every** format: the signed pipeline
  can only consume cells whose inputs are fully static/local (local `path:` base, `.repo` rpm-sources,
  no tools-dir, no signing). Any cell needing a tailor-managed build-time step (tools-dir export,
  RPM-dir createrepo farm, OCI/azureLinux base pull, signing) is a **hard error at generation time in
  PR CI** — because the signed pipeline cannot reproduce that step, and no unreviewed tool may run
  there to do it. This must be surfaced as a fail-fast, not a silent partial artifact.
- The **IC image pin** comes from a committed `tailor.lock` digest, so the pipeline runs the exact
  reviewed IC.

## 6. Risks / non-goals

- **Two sources of truth.** The config and its generated artifact can diverge; mitigated *only* by the
  §2 PR-CI drift gate. Non-negotiable.
- **Per-flavor emitter maintenance.** A native-DSL emitter (§3.1) is specific to each pipeline system
  (ADO, Make, GH Actions…). Start with the one flavor in use; don't build a generic pipeline compiler
  speculatively.
- **The pipeline's IC step must accept parameterization.** §3.1/§3.2 assume the approved invocation
  step can take a per-cell config path, image digest, base, and rpm-sources. If it's hard-coded,
  adapting it is itself a (one-time) pipeline review.
- **Non-goal (for now):** running tailor in the signed pipeline in *any* form — while tailor is
  unofficial. This is the transitional constraint, not a permanent stance (§7). **Non-goal:**
  re-implementing tailor's engine in the pipeline's language (locus 3).

## 7. Transitional — this is a bridge, not a destination

The whole "generate committed artifacts" posture exists **because tailor is not yet trust-chained**.
If tailor becomes officially owned/built with an accepted provenance chain, locus 1 opens up and the
pipeline could invoke tailor directly. Design so that transition is cheap:

- **Keep the source configs authoritative and the artifacts purely derived.** Nothing pipeline-side
  should ever be hand-authored on top of the generated output, so "stop committing artifacts and run
  tailor directly" is a clean deletion, not a migration.
- **The generated artifact is a thin projection.** Whether it's native-DSL, `plan.json`, or scripts,
  it should carry no logic that isn't reproducible by running tailor — so it can be dropped wholesale
  later.
- **The drift gate (§2) already proves equivalence** between "committed artifact" and "tailor run,"
  which is exactly the evidence needed to later justify running tailor live.

Until then, this bridge lets the team use tailor's config flexibility today without putting an
unofficial tool in the trust-sensitive path.

## Open questions

1. Which native DSL to target first — the ADO path (leveraging `ado.rs`), or the Make/`doit` tasks
   the current build uses?
2. Primary format: native-DSL compile (§3.1) or structured `plan.json` (§3.2)? (Lean: §3.1, to reuse
   the already-approved invocation step and minimize new review surface.)
3. Does the signed pipeline's existing IC step already accept a per-cell config path + image digest,
   or does enabling this require a one-time change to that (reviewed) step?
4. Should the committed artifacts live in a dedicated reviewed directory (clear ownership / CODEOWNERS)
   so pipeline owners review generated pipeline config separately from source config changes?
