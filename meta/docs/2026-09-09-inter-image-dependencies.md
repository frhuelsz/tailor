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
   output layout. It names the *image* (and, if ambiguous, the *output*); tailor resolves the path.
2. **The edge is implied by use, with an explicit escape hatch.** Referencing a producer artifact
   (as a base, or via interpolation in `config:`) *is* the dependency — no need to also restate it in
   a `dependsOn:` list. `dependsOn:` exists only for order-only edges that aren't otherwise expressed.
3. **Config stays opaque.** tailor does not learn IC schema. It substitutes a resolved path into the
   user's `config:` string values (as it already interpolates `${param}` —
   `crates/tailor-config/src/interpolate.rs`) and **registers that path as a content-hashed
   dependency**. It never parses `additionalFiles`.
4. **Incremental correctness is non-negotiable.** A producer rebuild must force any dependent to
   re-fingerprint against the fresh bytes (§4). Fail-closed: a missing/failed producer fails its
   dependents rather than embedding stale bytes.
5. **Intra-workspace only (v1).** Edges reference **member images** of the same workspace
   (`crates/tailor-config/src/workspace.rs`). Cross-workspace / remote producers are a non-goal (§7).

## 2. Config surface

Three entry points; the first two are the ergonomic primary API (each *implies* the edge), the third
is the escape hatch.

### 2.1 Image-as-base — `base: { image: … }`

A new `BaseSource` kind alongside `path` / `oci` / `azureLinux` / `ref`
(`crates/tailor-config/src/schema.rs::BaseSource`):

```yaml
# derived/image.yaml
base:
  image: hardened-base      # a member image name in this workspace
  output: raw               # optional: which of the producer's formats to consume (see §2.4)
```

Resolves to the producer's published artifact **for the paired cell** (§2.4), then behaves exactly
like a `path` base — so it flows through the existing base resolver and is **content-hashed into the
fingerprint** as `ResolvedBase::LocalFile { content_hash, size }`
(`crates/tailor-core/src/fingerprint.rs`). No new fingerprint surface needed for the base case.

### 2.2 Image-as-input — artifact interpolation

Expose each producer artifact as an interpolation token usable anywhere a `${param}` is
(`crates/tailor-config/src/interpolate.rs`), including inside opaque `config:` string values and in
`rpmSources` / `extraDependencies`:

```yaml
# iso/image.yaml
config:
  iso:
    additionalFiles:
      - source: "${image.installer-payload.cosi}"   # resolved to the producer's artifact path
        destination: /images/payload.cosi
```

`${image.<name>.<format>}` (or `${image.<name>}` when the producer has exactly one output) does two
things:

1. **Substitutes** the resolved host path of the producer's paired-cell artifact (§2.4), translated
   like any other path the executor binds.
2. **Registers** that path in the consuming cell's content-hashed dependency set (the same set
   `extraDependencies` feeds — `crates/tailor-core/src/deps.rs`). This is what makes the embed
   staleness-correct: the *text* of `config:` is hashed, but text alone can't see a byte change behind
   a stable path; the registered content hash can.

So a user never writes `../producer/artifacts/…` again, and never separately lists the artifact under
`extraDependencies`.

### 2.3 Explicit `dependsOn:` (order-only escape hatch)

For the rare case where the consumer depends on the producer having *run* but doesn't reference its
artifact by base or interpolation (e.g. a side effect in a shared output dir):

```yaml
# consumer/image.yaml
dependsOn:
  - hardened-base
  - installer-payload
```

`dependsOn` only adds edges to the DAG (§3). It contributes **no** fingerprint input by itself — an
order-only dependency that changes nothing the consumer reads must not force a rebuild.

### 2.4 Output selection & matrix pairing

A producer may have multiple outputs and/or a matrix. Resolution rules for both §2.1 and §2.2:

- **Output.** If the producer declares exactly one output, `output:`/the `.<format>` segment is
  optional. Otherwise it is **required**; an unknown/ambiguous format is a config error.
- **Compression.** The resolved path is the *published* artifact
  (`crates/tailor-core/src/orchestrator.rs::published_artifact_name`), so a producer output with
  `compression: zstd` resolves to `…​.<ext>.zst` automatically — the consumer says nothing about it.
- **Matrix pairing.** Producer and consumer are matched **cell-wise on shared axes**:
  - Producer has no matrix (single cell) ⇒ every consumer cell references that one artifact.
  - Producer and consumer share an axis (canonically `arch`) ⇒ pair by value: the consumer's
    `arch=arm64` cell references the producer's `arch=arm64` artifact. This is the common case
    (arch-paired base or payload).
  - Producer has an axis the consumer lacks (so pairing is ambiguous) ⇒ config error, unless the
    reference pins it: `base: { image: p, cell: { flavor: min } }` / `${image.p.min.cosi}` (exact
    form TBD, §9).

## 3. Build semantics (the DAG)

- **Nodes.** Each workspace **image** is a node (image-level DAG; §9 discusses cell-level).
- **Edges.** Union of the edges implied by §2.1/§2.2 and any explicit §2.3 `dependsOn`.
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

Surfaced by `tailor validate` (and at plan time):

- unknown image name in `base.image` / `${image.X…}` / `dependsOn`;
- unknown/ambiguous `output` format for a multi-output producer;
- ambiguous matrix pairing (producer axis the consumer can't pin);
- dependency **cycle** (names the cycle);
- a `dependsOn`/reference that escapes the workspace (a non-member) — rejected (§7);
- arch mismatch in a paired cell (producer has no `arch=<x>` cell for the consumer's `arch=<x>`).

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
- **Fully implicit (scan `config:` for paths under sibling images' output dirs).** Magic, fragile,
  and violates config-opacity. Rejected in favor of explicit references (§2.1/§2.2).
- **Cell-level DAG from day one.** More precise (a consumer arch cell depends only on the producer's
  same-arch cell) but heavier to model and explain. Start image-level with cell-wise *pairing* (§2.4);
  revisit if per-cell parallelism demands finer edges (§9).

## 9. Open decisions

1. **Cell-pin syntax** for pairing against a producer axis the consumer lacks —
   `base: { image, cell: { axis: value } }` and the interpolation analog `${image.p.<coord>.<fmt>}`.
2. **Image-level vs cell-level DAG** for scheduling/parallelism (§8). Image-level is proposed; does
   any real case need cell-level edges?
3. **`dependsOn` spelling** — `dependsOn` vs `needs` vs `after`. `dependsOn` reads well and matches
   the common ecosystem term; confirm.
4. **`--no-deps` semantics** — hard error vs warn when a required upstream artifact is absent.
5. **Interpolation namespace** — `${image.<name>.<format>}` vs `${images.<name>.<format>}` vs a
   distinct `${artifact:…}`; must not collide with user `params`.

## 10. Implementation sketch

- **Schema** (`crates/tailor-config/src/schema.rs`): add `BaseSource::Image { image, output?, cell? }`
  and a top-level `dependsOn: Vec<String>` on `ImageDefinition`. Extend the interpolation grammar
  (`interpolate.rs`) to recognize `image.<name>[.<coord>].<format>` and emit both a substitution and a
  registered dependency path.
- **DAG** (new, `tailor-config` or `tailor-core`): build image→image edges from resolved base-image
  refs + interpolation refs + `dependsOn`; topo-sort; cycle error. Reuse `Workspace`
  (`workspace.rs`) for the member set.
- **Resolution**: a producer artifact path = `<output_dir>/published_artifact_name(producer_slug,
  format, compression)` (`orchestrator.rs`), with `output_dir` the workspace artifacts dir
  (`run.rs::ARTIFACTS_DIR`, default `artifacts/`).
- **Scheduling** (`orchestrator.rs` + `run.rs`): replace the single whole-workspace
  `plan()`→`build()` with a per-topological-node `plan(node)`→`build(node)` loop (§4). The base and
  embed fingerprint inputs already exist; only the *order* of planning changes.
- **Validate** (`tailor validate`): run DAG construction + all §6 checks without building.
