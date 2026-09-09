# tailor — inter-image dependencies (design proposal)

> **Status:** _proposal · 2026-09-09_ · not implemented. Motivated by two recurring shapes that
> today force either a hand-written cross-image path (fragile) or a **nested workspace** built by two
> ordered `tailor build` invocations (a workaround). This doc proposes a first-class dependency edge
> between workspace images, its config surface, and — the crux — how it stays **incrementally
> correct** under tailor's plan-then-build architecture. Open decisions in §9.

## 0. Motivation

Two images in one workspace often relate as **producer → consumer**:

1. **Embed** — the consumer bundles the producer's *artifact* as an input. E.g. an installer ISO that
   embeds a payload COSI via an IC `additionalFiles` source and streams it to disk at boot; or a
   consumer that pulls the producer's RPMs via `rpmSources`.
2. **Base-of** — the producer's *output image* is the consumer's **base**. E.g. a hardened/base OS
   image that several derived images customize further.

Both are expressible today only by **hand-writing a path** into the consumer that points at the
producer's output directory:

```yaml
# consumer/image.yaml  — today
config:
  iso:
    additionalFiles:
      - source: ../producer/artifacts/producer_amd64_cosi.cosi   # produced by the other image
        destination: /images/payload.cosi
```

This has three problems:

- **No ordering.** A plain `tailor build` builds members in an unspecified order
  (`crates/tailor-config/src/workspace.rs` discovery order). If the consumer builds first, the source
  is missing — or worse, silently embeds a **stale** artifact from a previous run.
- **A duplicated, brittle path.** The consumer encodes the producer's slug (`producer_amd64_cosi`),
  its format extension, and its output-dir layout. Any of those changing (a matrix axis, a format, a
  `--output-dir`, adding `compression:` → `.cosi.zst`) breaks the reference silently.
- **The staleness hole is exactly the one the fingerprint work closed — reopened at the workspace
  level.** `extraDependencies`/`rpmSources` are content-hashed into the per-cell fingerprint
  (`crates/tailor-core/src/deps.rs`), so a consumer *can* declare the embedded artifact and rebuild
  when it changes. But `plan()` fingerprints **every** selected cell up front
  (`crates/tailor-core/src/orchestrator.rs::plan`), then `build()` runs. Within one invocation the
  consumer can hash the **old** producer artifact, build against it, and only then rebuild the
  producer — leaving a consumer whose fingerprint says "up to date" but whose embedded/based-on bytes
  no longer exist on disk.

**Current workaround.** Nest a second workspace inside the first (`producer/` as its own
`tailor.yaml` with `members: ["."]`) and drive **two** `tailor build` invocations in the right order
from a justfile. It works, but the nested workspace exists *only* to impose ordering, and the two
invocations are manual.

This proposal makes producer→consumer a **declared edge**, so a single `tailor build`:
orders the DAG, fails fast on cycles, resolves the producer's artifact path for the consumer (one
source of truth), and — critically — keeps the consumer's incremental verdict correct when the
producer rebuilds.

## 1. Principles

1. **One source of truth for the path.** The consumer never re-encodes the producer's slug/format/
   output layout. It names the *image* (and, if ambiguous, the *output* and *cell*); tailor resolves
   the path.
2. **Inputs are declared, then referenced by name.** A top-level `inputs:` list binds a `name` to a
   typed source; the use-site is a bare `${inputs.<name>}`. An `image`-kind input *is* the
   dependency edge — no separate restatement. `dependsOn:` exists only for order-only edges that
   reference nothing (§2.3).
3. **Config stays opaque.** tailor does not learn IC schema. At the use-site it only ever substitutes
   a bare `${inputs.<name>}` into a string value (as it already interpolates `${param}` —
   `crates/tailor-config/src/interpolate.rs`) and **registers the resolved path as a content-hashed
   dependency**. It never parses `additionalFiles` or reaches into IC structure.
4. **Incremental correctness is non-negotiable.** A producer rebuild must force any dependent to
   re-fingerprint against the fresh bytes (§4). Fail-closed: a missing/failed producer fails its
   dependents rather than embedding stale bytes.
5. **Intra-workspace only (v1).** `image` inputs reference **member images** of the same workspace
   (`crates/tailor-config/src/workspace.rs`). Cross-workspace / remote producers are a non-goal (§7).

## 2. Config surface

`inputs:` is the primary API: a typed, named catalogue of the things a build consumes. `base:`
gains an `image` shorthand for the common "producer is my base" case; `dependsOn:` is the order-only
escape hatch.

### 2.1 The `inputs:` catalogue

A **list of named entries**, each a `name` plus a typed input source discriminated by its kind key
(one-of, like `BaseSource` — `crates/tailor-config/src/schema.rs`). This mirrors the other workspace
catalogues (`baseImages`, `toolchains.entries`, `toolsDirSources` — all lists of `{ name, … }`).
Every input, regardless of kind, resolves to a host path tailor binds read-only and content-hashes
into the fingerprint; the kind only changes *how that path is produced*. v1 ships one kind — `image`
(an artifact produced by another workspace image):

```yaml
# iso/image.yaml
inputs:
  - name: payload                # the interpolation key
    image: installer-payload     # kind: produced by a workspace member image
    output: cosi                 # the producer output's format NAME (see §2.4); optional iff single-output
    cell: { flavor: min }        # structured pin for producer axes the consumer lacks (§2.4)
```

Reference it by name anywhere a `${param}` is valid — including inside opaque `config:` strings:

```yaml
config:
  iso:
    additionalFiles:
      - source: "${inputs.payload}"        # by name; format + cell already fixed above
        destination: /images/payload.cosi
```

`${inputs.<name>}` (a) **substitutes** the resolved host path of the producer's paired-cell artifact
and (b) **registers** that path in the cell's content-hashed dependency set (the set
`extraDependencies` feeds — `crates/tailor-core/src/deps.rs`), which is what makes the embed
staleness-correct: the *text* of `config:` is hashed, but text can't see a byte change behind a stable
path — the registered content hash can. The user never writes `../producer/artifacts/…`, never
restates the artifact under `extraDependencies`, and the format/cell live once, in the declaration.

**Extensibility (why `inputs:` and not a bare `dependsOn`).** The name/path/hash machinery is
kind-agnostic, so future input kinds slot in with **no change to any use-site**:

```yaml
inputs:
  - name: payload                                        # v1 — creates a DAG edge
    image: installer-payload
    output: cosi
  - name: seed                                           # future — a named local file/dir
    path: ./seed/data.img
  - name: drivers                                        # future — an OCI artifact
    oci: example.com/drivers:1.2
    file: drivers.tar
  - name: firmware                                       # future — a fetched blob
    url: "https://…/fw.bin"
    sha256: "…"
```

Only `image` inputs create a build-order **edge** (§3); `path`/`oci`/`url` inputs are leaves — no
edge, still hashed. So "an inter-image dependency" is just "an input whose source is another image,"
one concept rather than two. `extraDependencies` (an unreferenced `path` input, hashed only) and
`rpmSources` (referenceable local sources) are the natural things this converges on later (§9);
v1 leaves them as-is. Input `name`s are unique within an image (a duplicate is a config error, like
the other catalogues).

### 2.2 Image-as-base — `base: { image: … }`

The base is a single value, not a catalogue entry, so it keeps its own structured form alongside
`path` / `oci` / `azureLinux` / `ref` (`crates/tailor-config/src/schema.rs::BaseSource`):

```yaml
# derived/image.yaml
base:
  image: hardened-base      # a member image name
  output: raw               # optional: which producer format (see §2.4)
  cell: { flavor: min }     # optional pin (§2.4)
```

It resolves like §2.1's `image` kind, then behaves exactly like a `path` base — flowing through the
existing resolver and **content-hashed** as `ResolvedBase::LocalFile { content_hash, size }`
(`crates/tailor-core/src/fingerprint.rs`), so the base case needs no new fingerprint surface. Whether
`base:` should instead reference an `inputs` name (`base: { input: payload }`) — making `inputs:` the
single source of truth for every producer ref — is Open Decision §9.

### 2.3 Explicit `dependsOn:` (order-only escape hatch)

For the rare case where the consumer needs the producer to have *run* but references nothing it
produced (e.g. a side effect in a shared output dir):

```yaml
# consumer/image.yaml
dependsOn:
  - hardened-base
```

`dependsOn` only adds edges to the DAG (§3). It contributes **no** fingerprint input by itself — an
order-only dependency that changes nothing the consumer reads must not force a rebuild.

### 2.4 Output selection & cell resolution

A producer may have multiple outputs and/or a matrix. For every `image` input and every `base:
{ image }` (evaluated **per consuming cell**):

- **Output — a format *name*, not an extension.** `output:` is the value from the producer's
  `outputs[].format` (`cosi`, `raw`, `vhd-fixed`, `baremetal-image`, …), **not** the file extension.
  This matters because the extension isn't unique — `vhd` and `vhd-fixed` both write `.vhd`
  (`crates/tailor-core/src/orchestrator.rs::artifact_name`), so keying on the extension would be
  ambiguous. So `output: vhd-fixed` resolves to `…_vhd-fixed.vhd`; tailor owns the extension. Optional
  iff the producer declares exactly one output; otherwise required, and an unknown format is an error.
- **Compression is automatic.** The path is the *published* artifact
  (`published_artifact_name`), so a producer output with `compression: zstd` resolves to `….<ext>.zst`
  — the consumer says nothing about it.
- **Cell resolution — coordinate projection + explicit pins.** For a consuming cell with coordinate
  `C` (axis→value), the producer cell is:
  1. **inherit** — for every axis the producer *also* declares, take `C`'s value (match by axis name;
     canonically `arch`);
  2. **pin** — every producer axis the consumer *lacks* must be pinned by the input's `cell:` map,
     else the reference is ambiguous (an error, never an implicit "first"/"all");
  3. **format** — `output` selects which artifact of that fully-pinned producer cell.

  The resolved coordinate yields the producer slug → `<output_dir>/<slug>.<ext>[.zst]`. A coordinate
  that names no built producer cell (excluded by the producer's `selectors`/`skip`) is an error.
  Explicit pin beats inherited value. For `base:` the arch pairing is an **invariant** (you cannot
  base `arm64` on `amd64`), so a missing same-arch producer cell is a hard error, not a fallback.


## 3. Build semantics (the DAG)

- **Nodes.** Each workspace **image** is a node (image-level DAG; §9 discusses cell-level).
- **Edges.** An edge `producer → consumer` for every `image` input (§2.1), every `base: { image }`
  (§2.2), and every explicit `dependsOn` (§2.3). `path`/`oci`/`url` inputs add no edge.
- **Cycle detection.** Topologically sort at plan time; a cycle is a hard, fail-fast config error
  naming the cycle (`a → b → a`). This is the ordering-and-cycle guarantee consumers need.
- **Order.** Build in topological order. Independent nodes keep today's behavior; the only *new*
  constraint is "a producer builds before its consumers."

`tailor list` / `matrix` / `explain` gain nothing user-visible except that `explain` can print an
image's resolved dependencies and the concrete artifact path each edge resolves to.

## 4. Incremental correctness — closing the staleness hole

This is the reason the edge can't be "just ordering." tailor's `plan()` computes **all** fingerprints
up front, then `build()` runs (`orchestrator.rs`). If we plan the whole workspace once and then build
in order, a consumer is still fingerprinted against the **pre-build** producer artifact.

**Proposal: plan-and-build per topological level (or per node), not once for the workspace.**

```
for node in topological_order(dag):
    plan(node)          # fingerprint this image's cells now — AFTER its deps are built
    build(node)         # build stale cells, write stamps
```

Because a node is planned only **after** all its producers have been (re)built:

- the **base** case re-hashes the fresh base file (`ResolvedBase::LocalFile.content_hash`), and
- the **embed** case re-hashes the fresh registered dependency path (`deps.rs`),

so the consumer's fingerprint reflects the bytes that now exist. If the producer was rebuilt, its
artifact's content hash changes, the consumer's fingerprint changes, and the consumer rebuilds. If the
producer was up to date, nothing downstream is disturbed. This composes the existing fingerprint
machinery into a correct cross-image incremental build with **no new hashing** — only a change to the
plan/build *scheduling granularity*.

Trade-off: planning per node means digest/base resolution is interleaved with building rather than
done in one up-front pass. That is acceptable (resolution is cheap and cached), and it is the price of
correctness. Whole-workspace `--dry-run` still plans every node without building (it just can't show a
post-producer-rebuild fingerprint for a consumer, which is inherent).

## 5. Selection & CLI

- **`tailor build <consumer>`** implicitly builds the consumer's transitive dependencies first (like
  `cargo build -p <crate>`), in topological order. A `--no-deps` flag builds only the named image and
  errors if a dependency artifact is missing.
- **`tailor build`** (whole workspace) plans/builds the entire DAG in order.
- **`skip`** (`meta/docs/2026-07-22-fragment-skip.md`) still removes an image/cell from *bulk*
  selection, but a skipped image that is a *declared dependency* of a requested image is still built
  (it was named transitively) — otherwise the consumer couldn't build.
- **`tailor lock` / `render` / `export`** resolve artifact *paths* statically (paths are deterministic
  pre-build), so they need ordering only for cycle detection, not for building.

## 6. Validation & errors

All checks are **static** — the matrices are known pre-build — so they fire at `tailor validate` (and
are re-checked at plan), before anything is built. Each names the consumer cell, the producer, the
offending axis/value, and a concrete fix, matching tailor's `ConfigError` style (slug + field +
detail). A **dimension mismatch** takes one of four shapes:

1. **Ambiguous** — the producer has an axis the consumer lacks and the input didn't pin it:
   ```
   error: image `iso` input `payload` → `installer-payload`, but `installer-payload` has axis
          `flavor` that `iso` does not — the producer cell is ambiguous (flavor ∈ {min, net}).
     fix: set cell: { flavor: <value> } on input `payload` — or add `flavor` to iso's matrix.
   ```
2. **Empty** — the resolved (inherited or pinned) coordinate names no built producer cell:
   ```
   error: image `iso` cell `arch=arm64` needs `installer-payload` cell `arch=arm64`,
          which does not exist (installer-payload builds: arch=amd64).
   ```
   For a base, phrased as the arch invariant:
   `error: cannot base `derived` (arch=arm64) on `base`: `base` produces no arch=arm64 output.`
3. **Bad pin** — the `cell:` pin names an axis or value the producer doesn't have:
   ```
   error: input `payload` pins axis `edition` on `installer-payload`, which has no such axis (axes: arch, flavor).
   error: input `payload` pins `flavor=xxl` on `installer-payload`, but `flavor` has no value `xxl` (min, net).
   ```
4. **Missing output** — the producer doesn't declare the requested format:
   ```
   error: input `payload` requests output `vhd-fixed` from `installer-payload`, which produces: cosi.
   ```

Plus the non-dimensional checks: unknown image name (`image`/`base.image`/`dependsOn`); a reference
that escapes the workspace (a non-member) — rejected (§7); an unresolved `${inputs.<name>}` (no such
input); and a dependency **cycle** (names the cycle, `a → b → a`).

## 7. Non-goals (v1)

- **Cross-workspace / remote producers.** Edges are intra-workspace member references only. (An OCI
  or `azureLinux` base is already the "remote producer" story; this is about *local* producer images.)
- **Injecting into opaque `config:` structurally.** tailor substitutes a path string and registers a
  hash; it does not parse or synthesize `additionalFiles`/`rpmSources` entries.
- **Fan-out / dynamic dependency lists.** The edge set is static per render.
- **Replacing the base catalogue.** `baseImages` (`meta/docs/2026-06-29-base-image-catalogue.md`) stays
  for *external* bases; `base: { image }` is for *workspace-produced* bases.

## 8. Alternatives considered

- **Document declaration order as a guarantee.** Cheapest, but only fixes ordering — not the path
  duplication and not the plan-then-build staleness. Rejected as insufficient (it's the trap in §0).
- **Keep the nested-workspace workaround.** Works, but institutionalizes two invocations and a
  workspace that exists only for ordering. This proposal removes the need.
- **Inline artifact interpolation — `${image.<name>.<format>}` directly in `config:`** (an earlier
  draft of §2.1). Terse, and the path lives at the use-site, but it makes tailor scan opaque `config:`
  strings, buries the dependency edge *inside a string*, forces a fragile grammar (image names may
  contain `.`, formats contain `-`, and cell pins would need `[axis=val]`), and restates the format at
  every use. Superseded by the `inputs:` catalogue: the edge becomes a visible typed declaration, the
  pin is structured YAML, and the use-site is a bare `${inputs.<name>}` — tailor substitutes only its
  own token, never IC schema.
- **tailor authors the sink** (a structured `embed: [{ image, into }]` that writes the
  `additionalFiles` entry itself). Most declarative, but tailor would have to model every sink
  (`additionalFiles` is format-nested; others aren't), breaking config-opacity and not generalizing.
  Rejected.
- **Staged stable path** (tailor reflinks each input to `./.tailor/inputs/<name>`; config points at
  the real path, no token). Purest re: opacity — tailor never edits `config:` — at the cost of a
  reflink. Kept as a possible per-input option off the *same* `inputs:` declaration (§9), not the
  default.
- **Fully implicit (scan `config:` for paths under sibling images' output dirs).** Magic, fragile,
  violates opacity. Rejected in favor of explicit `inputs:`.
- **Cell-level DAG from day one.** More precise (a consumer arch cell depends only on the producer's
  same-arch cell) but heavier to model and explain. Start image-level with cell-wise *pairing* (§2.4);
  revisit if per-cell parallelism demands finer edges (§9).

## 9. Open decisions

1. **Should `base:` reference an `inputs` name** (`base: { input: payload }`) instead of repeating
   `{ image, output, cell }`, making `inputs:` the single source of truth for every producer ref?
   (Base keeps its arch-pairing invariant either way.)
2. **Should `extraDependencies` / `rpmSources` converge into `inputs:`** — `extraDependencies` as
   unreferenced `path` inputs, `rpmSources` as referenceable ones — or stay as separate fields?
3. **Image-level vs cell-level DAG** for scheduling/parallelism (§8). Image-level is proposed; does
   any real case need cell-level edges?
4. **`dependsOn` spelling** — `dependsOn` vs `needs` vs `after`. `dependsOn` reads well and matches
   the common ecosystem term; confirm.
5. **`--no-deps` semantics** — hard error vs warn when a required upstream artifact is absent.
6. **A staged-path option** (§8) as a per-input alternative to `${inputs.<name>}` substitution, for
   users who want tailor to never edit `config:` strings.
7. **Fan-in ergonomics** — a consumer cell that embeds *several* producer cells (e.g. every flavor)
   needs one input per artifact today; is sugar warranted (e.g. an input whose name expands over an axis)?

## 10. Implementation sketch

- **Schema** (`crates/tailor-config/src/schema.rs`): add a top-level `inputs: Vec<InputSpec>` on
  `ImageDefinition`, where `InputSpec` is `{ name, <one-of kind> }` — `name` plus a source
  discriminated by kind key (`image` in v1: `{ image, output?, cell? }`; `path`/`oci`/`url` reserved),
  matching the `{ name, … }` list shape of `baseImages`/`toolchains.entries`. Names are unique per
  image (reuse the catalogue duplicate-name check). Add `BaseSource::Image { image, output?, cell? }`
  and a top-level `dependsOn: Vec<String>`.
- **Interpolation** (`interpolate.rs`): recognize the `inputs.<name>` namespace (a bare name — no
  format/coordinate grammar), substitute the resolved host path, and record the use so the resolver
  can bind + hash it. Must not collide with the `params` namespace.
- **DAG** (new, `tailor-config` or `tailor-core`): build image→image edges from `image` inputs +
  `base: { image }` + `dependsOn`; topo-sort; cycle error. Reuse `Workspace` (`workspace.rs`) for the
  member set.
- **Resolution**: a producer artifact path = `<output_dir>/published_artifact_name(producer_slug,
  format, compression)` (`orchestrator.rs`), with `output_dir` the workspace artifacts dir
  (`run.rs::ARTIFACTS_DIR`, default `artifacts/`).
- **Scheduling** (`orchestrator.rs` + `run.rs`): replace the single whole-workspace
  `plan()`→`build()` with a per-topological-node `plan(node)`→`build(node)` loop (§4). The base and
  embed fingerprint inputs already exist; only the *order* of planning changes.
- **Validate** (`tailor validate`): run DAG construction + all §6 checks without building.
