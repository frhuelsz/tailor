# tailor documentation review — 2026-07-06

Reviewer: docs audit pass over `README.md`, `docs/` (user-facing, Diátaxis) and
`meta/docs/` (design). Claims were verified against the installed `tailor`
binary (`tailor 0.2.0+a0e5fa4`) and the config schema
(`crates/tailor-config/src/schema.rs`, `crates/tailor-config/src/types.rs`).
Where a finding says **[verified]**, it was checked by running the CLI or reading
the code, not just by reading the prose.

---

## 1. Executive summary

The documentation is, overall, in **good** shape: it is well-structured
(Diátaxis with per-section indexes), the mental-model explanations are strong,
and most reference material matches the code exactly. The single biggest
strength is `docs/explanation/architectures.md` — a rigorous, *correct* account
of the reserved `arch` axis and effective-arch reconciliation that matches the
orchestrator code line-for-line.

However, two feature changes have left **stale, actively-misleading content**
that a user will hit immediately:

1. **The catalogue base key was renamed `image:` → `ref:`, but many docs still
   say `image:`.** `base: { image: <name> }` **fails to parse** today
   [verified]; the correct key is `ref`. Wrong in `docs/reference/image-yaml.md`,
   two how-tos, and two `meta/docs/schemas/reference/` files.
2. **Signing is fully implemented, but `docs/how-to/sign-an-image.md` says it is
   "foundation only / not yet implemented"** and describes a "pure-Rust" CA.
   Signing actually runs (customize→sign→inject-files) using **`openssl` +
   `sbsign`** [verified].
3. **`docs/tutorials/your-first-matrix.md` §5 tells the user to run
   `tailor explain …` and expect merged IC config**, but `explain` prints the
   *merge order* unless you add `--with-config`, and the shown output format is
   fabricated [verified].
4. **The `pull: always|missing|never` toolchain policy is undocumented in
   `docs/`** — a notable, shipped feature with no user-facing home.
5. **`docs/tutorials/getting-started.md` shows a `simple` `image.yaml` that no
   longer matches the real scaffold** (missing `previewFeatures`, `storage`) and
   would not build [verified].

Fixing items 1–3 is high priority: they are the first things a beginner and a
catalogue/signing user respectively will trip over.

---

## 2. What's working well (preserve & replicate)

- **[praise] `docs/explanation/architectures.md`** — the model page to emulate.
  It states the arch resolution order (axis → base arch → `amd64`) exactly as
  the orchestrator implements it (`crates/tailor-core/src/orchestrator.rs:293-302`)
  [verified], and the **effective-arch matrix table** (§"The effective-arch
  matrix") is a genuinely excellent way to communicate a 2-D reconciliation rule.
  It correctly stresses "default is `amd64`, never the host arch." Replicate this
  "table + resolution-order list" pattern wherever behavior is a small matrix.
- **[praise] `docs/explanation/base-images.md`** — the "registry bases vs file
  bases" table and the "catalogue = local cache keyed on the *file*" framing make
  a subtle model legible; it uses the correct `ref:` key throughout.
- **[praise] `docs/reference/directives.md` and `docs/reference/output-formats.md`**
  — both match the code exactly: every directive (`$set/$replace/$remove/$prepend/
  $append/$unset/$include`, `$select` reserved) matches
  `crates/tailor-config/src/merge.rs:14-22` [verified], and every output format
  matches the `OutputFormat` enum (`crates/tailor-config/src/types.rs:32-43`)
  [verified]. This is the standard the other reference pages should meet.
- **[praise] `README.md`** — strong "why" narrative, a real end-to-end standalone
  example, and Mermaid (not ASCII) diagrams. The "Just an `image.yaml`" section is
  an effective on-ramp.
- **[praise] Diátaxis structure & navigation** — clean tutorials/how-to/reference/
  explanation split, each with a `README.md` index; `docs/how-to/README.md` lists
  all 13 how-tos and all targets exist [verified]. No orphaned how-to pages.
- **[praise] `meta/docs/arch-and-platform.md`** — Status "Implemented", and it is:
  it agrees with `architectures.md` and the code, and explicitly documents that
  `architectures:` is gone. Good example of a design doc kept in sync.

---

## 3. Findings — must-fix (correctness / broken)

### 3.1 [must-fix] Catalogue base key is `ref:`, not `image:` (pervasive stale rename)
`base: { image: <name> }` **fails to parse** — `data did not match any variant of
untagged enum BaseSource`; `base: { ref: <name> }` validates [verified]. The
`BaseSource::Ref` variant renames the field to `ref`
(`crates/tailor-config/src/schema.rs:480-483`), and `tailor.schema.json:382-384`
correctly requires `ref`. The prose docs drifted. Fix every occurrence to `ref`:

- `docs/reference/image-yaml.md:12` — table row: *"one of `path`, `oci`,
  `azureLinux`, `image` … `image: <name>` references a `baseImages:` slot."* →
  use `ref`. (Note the example at `:110-111` already correctly uses `ref:`, so the
  page is internally self-contradictory.)
- `docs/reference/image-yaml.md:131` — *"An `image:` base references a named
  slot…"* → `ref:`.
- `docs/how-to/override-a-base-per-axis.md:36` — *"`image: <name>` — a named slot
  from the workspace `baseImages:` catalogue"* → `ref: <name>`.
- `docs/how-to/use-a-base-image-catalogue.md:73` — link text *"the `image` base
  kind"* → *"the `ref` base kind"* (the page's own examples at `:37,:46` use
  `ref:`).
- `meta/docs/schemas/reference/image-yaml.md:17` — *"Includes the `image: <name>`
  kind for `tailor.yaml` `baseImages` entries."* → `ref`.
- `meta/docs/schemas/reference/types.md:39` — table row *"named catalogue image |
  `image: azure-linux-3-amd64`"* → `ref: …` (`:51` already uses `ref`).

### 3.2 [must-fix] `docs/how-to/sign-an-image.md` — signing is implemented; status/notes are stale
Signing execution ships: `crates/tailor-sign/src/lib.rs` mints the CA/leaf and
signs with **`openssl`** (`OPENSSL` at `:18`, `Command::new(OPENSSL)` at
`:162-216, :330`) and `sbsign`; `meta/docs/signing.md` is "Implemented (S1)" and
`meta/docs/signing-status.md` is "Implemented". A signed `tailor validate` / `build
--dry-run` reports *"✓ signing profile `test-ca` ready"* with **no** "not
implemented" note [verified]. Fix:

- `:7-12` — the **"Status — foundation only … signing execution … is a later
  milestone … stops with a clear error rather than produce a silently-unsigned
  image"** blockquote is false. Remove/replace with an "Implemented" status.
- `:96` — the dry-run example line *"note: signing execution is not yet
  implemented; this dry-run shows the unsigned customize invocation."* does not
  appear in real output [verified]; delete it.
- `:38` and `:83` — *"Pure-Rust self-signed CA + leaf minted per build"* and
  *"keys are minted in pure Rust at sign time"* are wrong: the CA/leaf are minted
  with `openssl` at sign time [verified]. Correct the mechanism (and note the
  `openssl`/`sbsign` runtime dependency, which is currently unstated — a user on a
  minimal host will hit a missing-tool error).

### 3.3 [must-fix] `docs/tutorials/your-first-matrix.md` §5 — wrong `explain` command & output
`tailor explain <image>` prints the **merge order** (numbered fragment list);
the **merged IC config** requires `--with-config` [verified]. The tutorial at
`:97-98` runs `tailor explain gizmo -s …` (no `--with-config`) but then shows
merged `os:`/`packages:` config at `:103-116`. The shown header
(`gizmo: 1 cell(s)` / `── gizmo_full_amd64_edge_cosi ──`) is also fabricated; the
real output is:

```text
cell  gizmo_full_amd64_cosi   (arch=amd64, variant=full)

merge order (top = base, bottom wins):
   1  image.yaml            base
   ...
merged config:
previewFeatures:
- input-image-oci
os:
  hostname: gizmo
  ...
```

Fix: add `--with-config` to the command and replace the expected-output block with
the real `cell … / merge order / merged config:` shape (matches
`docs/reference/cli.md:74-84`).

### 3.4 [must-fix] `docs/tutorials/getting-started.md` §3 — shown `simple` scaffold is out of date
The real `tailor init solo simple` scaffold contains `previewFeatures:
[input-image-oci]`, `os.bootloader.resetType: hard-reset`, and a full `storage:`
block [verified]; the tutorial (`:48-65`) shows a stripped `image.yaml` with none
of those. As shown, the config would fail an actual build (azureLinux base needs
the `input-image-oci` preview; the package install needs the grown rootfs). Fix:
paste the current scaffold output (or trim it explicitly and say so).

---

## 4. Findings — flow & structure

- **[should-fix] Contradictory axis-ordering guidance.** `docs/reference/image-yaml.md:9`
  and `docs/explanation/architectures.md:49` tell authors to *"order `arch` first"*,
  but every example and the shipped `advanced` scaffold declare a non-arch axis
  first: `concepts.md:26-27` (`edition, arch`), `your-first-matrix.md:51-52`
  (`variant, arch`), and `tailor init … advanced` itself (`variant, arch`)
  [verified]. Pick one convention and make the guidance and examples agree; today a
  careful reader gets whiplash.
- **[should-fix] `getting-started.md:22` version drift.** Expected output shows
  `tailor 0.1.0+...`; the binary is `0.2.0` [verified]. Also `README.md:110`
  pins `version="v0.1.0"` in the release-download snippet. Use a
  version-agnostic placeholder or bump.
- **[nice-to-have] Signing not in `README.md` Features list** (`:144-153`) even
  though it is a headline capability; add a bullet and link to the how-to.
- **[nice-to-have] `pin-the-ic-version.md`** is the natural home to introduce the
  `pull:` policy (local/registry resolution) and a `--locked` cross-link; today it
  stops at `tag`/`version`.

---

## 5. Findings — completeness gaps

- **[should-fix] `pull: always|missing|never` toolchain policy is undocumented in
  `docs/`.** No mention anywhere under `docs/` [verified: `grep`], though it is a
  first-class schema field (`schema.rs:46-77`, default `missing`) enabling
  air-gapped/locally-built IC. *Hurts advanced users.* Add to
  `docs/reference/tailor-yaml.md` (toolchains.entries) and ideally a short how-to
  ("Use a locally-built / air-gapped Image Customizer").
- **[should-fix] `docs/reference/cli.md` omits `tailor bases list`.** The
  subcommand exists (`bases list|download|verify`) [verified] but only `download`
  and `verify` are documented (`:114-124`). *Hurts both personas.*
- **[should-fix] `docs/reference/cli.md` `matrix` omits the `ado` format and
  `--ado` flag.** `:56-61` documents only `--format json|slugs`, but `matrix`
  supports `--format ado` and `--ado <VAR_NAME>` [verified] (there is a whole
  `meta/docs/ado-matrix.md`). *Hurts advanced/CI users.*
- **[should-fix] `docs/reference/image-yaml.md` omits `outputArtifacts`.** The
  parse-error field list confirms it is a valid image field [verified], with a
  three-value policy (`managed|scratch|strip`,
  `crates/tailor-config/src/types.rs:83-92`). *Hurts advanced users* (relevant to
  signing scratch handling).
- **[should-fix] `docs/reference/tailor-yaml.md` omits `runtime.logDir` and
  `defaults.outputArtifacts`.** Both exist in the schema (`schema.rs:182-183`,
  `:359-360`). The `runtime.logLevel`/`imageCacheDir`/`janitorImage` rows are
  present, so these two are conspicuous holes.
- **[nice-to-have] XXH3-128 local-base hashing** (cache under
  `<output>/.tailor/base-hashes/`, keyed on `(path,size,mtime)`) is not mentioned;
  it is relevant to how incremental builds detect base changes and complements the
  `extraDependencies` note. A sentence in `explanation/base-images.md` would close
  the loop for advanced users.

---

## 6. Beginner journey assessment

Walking `README.md` → `docs/README.md` → `tutorials/getting-started.md` →
`your-first-matrix.md` → first how-tos:

- **README → hub:** smooth. The standalone example is compelling and the
  60-second quickstart (`tailor init myimage advanced` → `matrix` → `build
  --dry-run`) works verbatim [verified].
- **getting-started (standalone):** mostly good; `init simple`, `validate`,
  `build --dry-run` all work as written [verified]. **Snag:** the printed
  `image.yaml` (§3) doesn't match what the tool actually scaffolds (3.4) — a
  beginner comparing the page to their file will be confused, and copy-pasting the
  page's config would not build. **Minor snag:** the `0.1.0` version string (§1).
- **your-first-matrix:** the scaffold, `matrix --format slugs`, `add axis`, and
  cell multiplication all match reality [verified]. **Hard stop at §5:** the
  `explain` command as written won't show the promised config, and the expected
  output looks nothing like reality (3.3). This is exactly the step where a learner
  double-checks their understanding, so a wrong example here is costly.
- **first how-tos:** `use-a-base-image-catalogue.md` examples work (`ref:`), but a
  beginner who reads the reference (`image-yaml.md`) first and writes `base: {
  image: … }` will get an opaque `untagged enum BaseSource` parse error (3.1).
  `sign-an-image.md` will actively mislead them into thinking signing is a no-op
  (3.2).

Net: the happy path is close to seamless; the two tutorial snags (3.3, 3.4) and
the `image:`/`ref:` reference bug (3.1) are the concrete places a newcomer gets
stuck.

---

## 7. Advanced-user / reference assessment

Findability is good (clear reference split, schema mirror under
`meta/docs/schemas/`). Depth is high in the design docs. Correctness is strong
except for the pervasive `image:`/`ref:` drift (3.1) and the gaps in §5.

**`arch` case study (focused).** This is handled *well and consistently* where it
is documented:

- `docs/explanation/architectures.md` — resolution order (axis → base arch →
  `amd64`) and the reconciliation matrix exactly match
  `orchestrator.rs:293-305` and `check_platform_arch`/`reconcile_slot_arch`
  [verified].
- `docs/reference/image-yaml.md:48-62` ("Architectures") — same order, same
  "no `architectures:` field", same "both set must agree." Consistent.
- `meta/docs/arch-and-platform.md` (Status: Implemented) — agrees; explicitly
  documents removal of `architectures:`.
- Verified behaviors: a single-cell image resolves to `amd64` [verified]; a local
  `path`/slot `arch` is accepted (`schema.rs:436,470`) [verified]; a stray
  `architectures:` is rejected as an unknown field [verified].

The one arch wrinkle is the *ordering advice vs. examples* contradiction (§4),
which is stylistic, not a correctness bug in the arch model itself.

Other complex behaviors (merge model, base resolution/pinning, signing) are
rigorously documented in `meta/docs/` and mostly correct; the merge/directive
reference matches code exactly (§2). Signing's user-facing page is the exception
(3.2).

---

## 8. `docs/` vs `meta/docs/` consistency

- **Agreement is generally strong.** `meta/docs/` carries `Status:` headers and
  the arch, matrix-constraints, logging (`--timestamps`), and ADO docs match the
  user-facing docs and the code.
- **[should-fix] `meta/docs/schemas/reference/*.md` and
  `meta/docs/schemas/README.md` lack `Status:` headers** (unlike the rest of
  `meta/docs/`) [verified] — and two of them carry the stale `image:` key (3.1).
  Add headers and fix the key.
- **[should-fix] `meta/docs/base-image-catalogue.md` residual `image:` mentions.**
  The doc is correct overall (uses `ref:` at `:22,148,154,160,260,297,341,370,386`)
  and Status "Implemented", but has three stale `image:` leftovers: the design-
  rationale line `:170` ("An explicit `image:` key keeps `BaseSource` an
  unambiguous `oneOf`"), the Mermaid edge `:266` (`B -->|image: name|`), and prose
  `:283` ("a typo'd `image:` fails fast"). These are historical; update to `ref`.
- **[nice-to-have] Stale-labeled design docs are correctly marked.**
  `meta/docs/image-definitions.md` (Status: Stale) and `architecture.md` (Stale)
  are honestly flagged, so they don't mislead — good hygiene; just ensure nothing
  links to them as current.
- **[nice-to-have] `injectFiles` framing is consistent** across
  `docs/reference/image-yaml.md:18` and `sign-an-image.md:105` ("inert
  placeholder, no-op"); the schema field exists (`schema.rs:414`) and defaults to
  `false` in orchestration (`orchestrator.rs:93`). Consistent, leave as-is.

---

## 9. Prioritized action list

1. **[must-fix]** Replace `image:` → `ref:` for the catalogue base kind in
   `docs/reference/image-yaml.md:12,131`, `docs/how-to/override-a-base-per-axis.md:36`,
   `docs/how-to/use-a-base-image-catalogue.md:73`,
   `meta/docs/schemas/reference/image-yaml.md:17`,
   `meta/docs/schemas/reference/types.md:39`.
2. **[must-fix]** Rewrite the signing status/notes in `docs/how-to/sign-an-image.md`
   (`:7-12`, `:96`, `:38`, `:83`): signing is implemented and uses `openssl`/`sbsign`,
   not "pure-Rust", and does not stop with a "not implemented" error.
3. **[must-fix]** Fix `docs/tutorials/your-first-matrix.md` §5 (`:97-116`): add
   `--with-config` and replace the expected output with the real
   `cell … / merge order / merged config:` format.
4. **[must-fix]** Update `docs/tutorials/getting-started.md` §3 (`:48-65`) to match
   the current `simple` scaffold (add `previewFeatures`, `bootloader`, `storage`).
5. **[should-fix]** Document the `pull: always|missing|never` toolchain policy in
   `docs/reference/tailor-yaml.md` (+ a short air-gapped/local-IC how-to).
6. **[should-fix]** Add `tailor bases list`, and the `matrix --format ado` /
   `--ado <VAR>` options, to `docs/reference/cli.md`.
7. **[should-fix]** Add `outputArtifacts` to `docs/reference/image-yaml.md`; add
   `runtime.logDir` and `defaults.outputArtifacts` to `docs/reference/tailor-yaml.md`.
8. **[should-fix]** Resolve the "order `arch` first" advice vs. examples
   contradiction (`image-yaml.md:9`, `architectures.md:49` vs. `concepts.md`,
   `your-first-matrix.md`, the `advanced` scaffold).
9. **[should-fix]** Add `Status:` headers to `meta/docs/schemas/**` and clean the
   three residual `image:` mentions in `meta/docs/base-image-catalogue.md`
   (`:170,266,283`).
10. **[nice-to-have]** Fix the `0.1.0` version strings (`getting-started.md:22`,
    `README.md:110`); add signing to the README Features list; mention XXH3-128
    base-hash change-detection in `explanation/base-images.md`.
