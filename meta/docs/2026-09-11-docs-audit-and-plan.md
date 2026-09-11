# Documentation audit & resolution plan

Status: Audit complete — resolution pending review
Scope: All public docs (`README.md`, `docs/**`, `COMPATIBILITY.md`, `CHANGELOG.md`,
`.github/docs/mkdocs.yml`). Docs-only — no code changes in this pass.
Method: a mechanical pass (nav/link/hygiene/inventory) plus three parallel
read-only audit agents cross-checking docs against the source (reference-vs-code,
tutorials+how-to, explanation+top-level). All findings carry `file:line` citations.

## Verdict

The docs are broad and mostly high-quality, but the audit found: **one
site-breaking navigation bug**, a cluster of **post-1.0 staleness** (version
strings, tag convention, `lock`/`update`, signing status, `buildDirBase`), several
**completeness gaps** (undocumented commands/flags/config fields and missing
how-tos for shipped features), and **cohesiveness issues** (terminology drift,
explanation duplication, public→internal `meta/docs` links, weak learning-path
trails). None are hard to fix; they cluster into four phases below.

Two items require **code** changes and are therefore **out of scope** here — listed
at the end as follow-ups.

---

## Findings

### P0 — Site-breaking / structural

1. **Broken nav + orphaned page (Critical).** `.github/docs/mkdocs.yml` and
   `docs/explanation/README.md:9` both link to `explanation/2026-06-22-architecture.md`,
   which **does not exist**; the real `docs/explanation/architecture.md` is
   **orphaned** (not in nav). The "Architecture" page 404s on the published site.
2. **`architecture.md` vs `architectures.md` name collision (High).** Singular =
   crate/software architecture (`architecture.md`, ~24 lines); plural = target CPU
   architectures / the reserved `arch` axis (`architectures.md`, ~75 lines).
   Near-identical names caused the rename drift above. Rename to unambiguous names
   (e.g. `crate-architecture.md` / `target-architectures.md`) or consolidate.

### P1 — Factual errors / outdated (mistakes)

3. **Stale version examples.** `docs/installation.md:39` shows `tailor 0.2.0+…`;
   `docs/tutorials/getting-started.md:22` shows `tailor 0.1.0+…`. Current is 1.0.1.
   Use a version-neutral `tailor <version>+<commit>.<date>`.
4. **CHANGELOG release links use the wrong tag convention.** `CHANGELOG.md:126-136`
   mixes `v*` and `tailor-v*`. The current convention is `tailor-v*`
   (`README.md`, `docs/how-to/embed-in-a-monorepo.md`). Reconcile (historical `v*`
   tags are real for ≤1.0.0; new ones are `tailor-v*`).
5. **`lock` vs `update` presented as interchangeable.** `docs/reference/cli.md:148-149`
   and `docs/how-to/pin-the-ic-version.md:35-46` don't convey the now-distinct
   semantics (`lock` = freeze existing pins, resolve only new; `update` = re-resolve
   all). Code: `crates/tailor/src/run.rs` lock/update split.
6. **Signing status is inconsistent.** `docs/how-to/sign-an-image.md` says
   "foundation only … stops with a clear error"; the changelog says signing is
   preview-gated; an old `0.2.0` changelog entry claims "end-to-end signing." State
   one authoritative status: *signing configuration + preflight are preview-gated
   (`previewFeatures: [signing]`); signed-image execution is not yet available.*
7. **`tailor explain` example missing `--with-config`.** `docs/tutorials/your-first-matrix.md:98`
   claims it "shows the fully merged IC config" but omits the flag that actually
   prints it.
8. **`export` overstated.** `docs/how-to/export-configs-for-a-pipeline.md:53-55`
   says it "always succeeds for any cell"; it still requires a renderable config.
9. **`--build-dir-base` help mismatch.** `docs/reference/cli.md:48` (correctly
   updated: "not `/`, a system dir, or `$HOME`") disagrees with the **clap help
   string in code** (`crates/tailor/src/cli.rs`, still "same filesystem as `/`").
   Doc is right; the code help is stale → **code follow-up (below)**.

### P1 — Completeness gaps (missing things)

Reference (`docs/reference/`):
10. **`tailor bases list` undocumented** — exists (`cli.rs` `BasesCommand::List`),
    only `download`/`verify` are in `cli.md:155-168`.
11. **Exit-code taxonomy absent** — `0/1/2/130` (`crates/tailor/src/error.rs`) is
    documented nowhere in the reference. Add an "Exit codes" section to `cli.md`.
12. **`matrix --format ado` and `--ado VAR` undocumented** (`cli.rs` `MatrixFormat::Ado`,
    `MatrixArgs.ado`).
13. **Undocumented config fields:** `runtime.logDir` (+ `TAILOR_LOG_DIR` precedence),
    `defaults.outputArtifacts` and per-image `outputArtifacts` with values
    `managed`/`scratch`/`strip`, signing-profile fields (`publishCaCert`, `vault`,
    `certificate`), `inputs.cell` pinning semantics, and the accepted `previewFeatures`
    values (only `signing`; unknown values rejected).
14. **`tailor notice`** — only lightly covered in `cli.md`; state it's stdout-only.
15. **`output-formats.md`** — add that `format` is required and unknown formats are
    rejected.

New how-to/tutorial guides to create (shipped features with no guide):
16. **`--clones`** (distinct `<slug>_clone<n>`, always rebuilds).
17. **`tailor notice` / third-party licensing** (or link `explanation/licensing.md`
    from the how-to index).
18. **Exit codes for CI/scripting.**
19. **`previewFeatures` overview** (and the signing preview gate).
20. **`buildDirBase` default behavior** (optional; defaults to `<output>/.tailor/build`)
    — expand beyond the single mention in `use-a-tools-dir.md`.
21. **lock/update reproducibility workflow** — expand `pin-the-ic-version.md`
    (first-time freeze, refresh, `--locked`, local-image caveat).

Top-level:
22. **README feature list is stale/incomplete** (`README.md:182-191`): omits
    `notice`, `lock`/`update`, preview-gated signing, tools-dir, inter-image
    deps/`inputs`, compression, `convert`/`export`, base catalogue, exit codes;
    doesn't state the current version.
23. **`docs/README.md` landing page** lacks install/compat/changelog/release links
    and a "start here" path.
24. **`COMPATIBILITY.md`** should (a) state preview features are outside the semver
    guarantee until promoted, and (b) cross-link the release-verification/`tailor-v*`
    convention.
25. **`CHANGELOG.md`** should mention `tailor notice` + the tag convention and
    reconcile the signing wording with the how-to.

### P2 — Flow / learning path

26. **No trail out of the tutorials.** `docs/tutorials/README.md` and the end of
    `your-first-matrix.md` / `getting-started.md` don't route into how-to/reference.
    Add "Next steps" links.
27. **Unexplained standalone→workspace transition** between `getting-started.md`
    (standalone `image.yaml`) and `your-first-matrix.md` (full workspace). Explain
    `tailor.yaml`, when `schemaVersion` is required, members, toolchains/runtime.
28. **`your-first-matrix.md` uses axis/fragment/cell/matrix before defining them.**
    Add a short vocabulary paragraph.
29. **`how-to/README.md` is a flat list** — group by task and note prerequisites
    (workspace / engine / registry / catalogue).
30. **`-s` vs `--select` inconsistency** — introduce `--select` first, then the
    `-s` shorthand.
31. **Dry-run vs real build blur** in `getting-started.md` — call the dry-run a
    "build plan render," clarify it contacts no engine.

### P2 — Cohesiveness

32. **Public docs link to internal `meta/docs/` planning files** (dead-ends for site
    readers): `docs/explanation/merge-model.md:29`, `docs/how-to/sign-an-image.md:9`,
    `docs/reference/image-yaml.md:153`. Replace with public cross-refs; keep the
    `meta/docs` link (if any) only as clearly-labelled implementation background.
33. **Terminology overload.** "base" (OS base / catalogue slot / IC toolchain /
    tools-dir source), plus cell/slug/axis/matrix/fragment/toolchain/tools-dir, are
    introduced inconsistently and not cross-linked. Add a terminology note and
    consistent qualifiers; define `arch` as the one reserved/typed axis in
    `concepts.md`.
34. **Explanation duplication.** `concepts.md`, `merge-model.md`, and
    `design-rationale.md` each re-explain fragments/matrices/merging. Assign
    ownership: concepts = vocabulary; merge-model = precedence/semantics;
    design-rationale = motivation/trade-offs (link, don't repeat).
35. **`explanation/README.md` is a bare link list** — add one-line summaries, a
    reading order, and audience framing (user-facing vs contributor/security).
36. **`architecture.md` is too thin** (~24 lines) for a standalone "Architecture"
    page — expand (data flow, ports/adapters, crate boundaries) or fold into
    `design-rationale.md`.
37. **README ↔ docs/README handoff** — make both coherent entry points.

### P3 — Clarity / polish

38. **`architectures.md`** needs an orientation sentence ("most axes only select
    fragments; `arch` also controls target platform + base resolution"); state the
    effective-arch precedence once; fix the ambiguous "or the `tailor.yaml` override"
    line (`architectures.md:64-72`).
39. **`merge-model.md:25-29`** — turn the compressed ordering/precedence paragraph
    into a numbered algorithm + a worked example.
40. **`design-rationale.md`** — add an intro and a "decision / alternatives rejected
    / consequence" structure.
41. **`installation.md`** — add a link to the README cosign/provenance verification
    (currently only checksum) and fix the stale version example.
42. **`directives.md`** — cross-link `$select` ("reserved") to `image-yaml.md`
    selectors/fragments.
43. **Unused assets** — `docs/resources/logo.png` and `logo_small.png` are unused
    (only `icon.png` is wired as logo/favicon in `mkdocs.yml`). Remove or use.

---

## Resolution plan (phased)

**Phase 1 — Unbreak the site (P0, ~0.5 day).** Fix the architecture nav/orphan
(#1); rename `architecture.md`/`architectures.md` to unambiguous names and update
every link + the nav (#2); remove the unused logo assets or wire them (#43).

**Phase 2 — Correct the mistakes (P1 accuracy, ~1 day).** Version strings (#3),
CHANGELOG tag links (#4), `lock`/`update` semantics (#5), one authoritative signing
status across sign-an-image/CHANGELOG/reference (#6), `explain --with-config` (#7),
`export` wording (#8). Replace public→`meta/docs` links with public cross-refs (#32).

**Phase 3 — Close completeness gaps (P1, ~2–3 days).** Reference additions:
`bases list` (#10), exit-code section (#11), matrix `ado`/`--ado` (#12), the missing
config fields (#13), `notice` stdout note (#14), output-format required/rejected
(#15). New guides: clones (#16), notice/licensing (#17), exit-codes-for-CI (#18),
preview-features (#19), buildDirBase (#20), expand lock/update workflow (#21).
Top-level: README feature list + version (#22), docs landing "start here" (#23),
COMPATIBILITY preview + release-verification (#24), CHANGELOG reconcile (#25).

**Phase 4 — Flow, cohesiveness, clarity (P2–P3, ~2 days).** Tutorial "next steps"
+ standalone→workspace transition + vocabulary (#26–#28), how-to grouping (#29),
`--select` consistency (#30), dry-run wording (#31), terminology note + cross-links
(#33), explanation ownership boundaries + index summaries + architecture depth
(#34–#37), and the clarity polish (#38–#42).

### New docs to create
`docs/how-to/`: `build-clones.md`, `print-license-notices.md` (or link licensing),
`handle-exit-codes.md`, `use-preview-features.md`, `set-the-build-directory.md`,
and an expanded `pin-the-ic-version.md`. Each must be added to `mkdocs.yml` nav and
the how-to index.

### Code follow-ups (OUT OF SCOPE for this docs-only pass)
- **Stale clap help strings** in `crates/tailor/src/cli.rs`: `--build-dir-base`
  (and the `convert` equivalent) still say "same filesystem as `/`" after the guard
  change. One-line fix each.
- **Dead `WritableToolsDirNeedsBuildDir` path + stale comment** in
  `crates/tailor/src/run.rs` (tools-dir "requires buildDirBase") — unreachable now
  that `buildDirBase` defaults; remove or convert to a defensive invariant with an
  accurate comment.
