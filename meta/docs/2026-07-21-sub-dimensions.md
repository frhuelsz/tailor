# tailor — sub-dimensions (nested / conditional matrix axes)

> **Status:** Proposed (exploratory) · _2026-07-21_
>
> Explores letting a matrix **value carry its own nested axis** — a "sub-dimension" that exists
> **only under that value**. Motivating case: an image needs a `type` axis whose values are
> `container` and `kata`, but `kata` itself splits into `qemu` and `openvmm`. This doc theorizes the
> config surface and semantics; it is a design to continue later, not a committed plan.

## 1. The problem

Consider a `type` dimension with two conceptual types, one of which has variants:

- `container`
- `kata` → `qemu` | `openvmm`

Today tailor has two ways to model this, both unsatisfying:

- **Flat 3-value axis** `type: [container, kata-qemu, kata-openvmm]`. Anything common to *both* kata
  variants goes in a **disjunction fragment** `by-type/kata-qemu+kata-openvmm.yaml`. This grows
  awkwardly: every new kata variant must be **remembered and appended** to that filename, and the
  name balloons (`kata-qemu+kata-openvmm+kata-clh+…`). The "these are all kata" relationship is
  implicit and fragile.
- **Two independent axes** `type: [container, kata]` + `hypervisor: [qemu, openvmm]`. Now "common
  kata" is naturally `by-type/kata.yaml` — but `hypervisor` is meaningless for `container`, so you
  must **exclude it from every container cell** (`selectors.exclude`), and you get phantom
  `container × qemu`/`container × openvmm` cells unless you carefully carve them out. The exclusion
  is boilerplate you must keep in sync.

Neither expresses the actual shape: **`kata` *has* variants; `container` does not.**

## 2. The concept

A **sub-dimension** lets a value open a nested axis scoped to itself. The matrix stops being a flat
cartesian product and becomes a **tree**: most values are leaves, but a value may branch into a
nested matrix. Equivalently: the sub-axis is a **conditional axis** — present only in the cells
descended from its parent value.

For the example, cells become:

```
type=container
type=kata, runtime=qemu
type=kata, runtime=openvmm
```

`container` cells simply **have no `runtime` axis**; `kata` cells do. That is the whole idea, and it
maps cleanly onto how ACL-style images actually branch.

## 3. Config surface

**Recommended spelling — value-with-nested-axis (a heterogeneous value list):**

```yaml
matrix:
  type:
    - container                     # leaf value
    - kata:                         # branch value → opens a nested axis
        runtime: [qemu, openvmm]
```

Read aloud: *"type is container, or kata with runtime qemu/openvmm."* A value is either a **string**
(leaf) or a **single-key map** `{ <value>: <nested matrix> }`. The nested block is itself a
`matrix:` fragment, so it generalizes:

- **Multiple sub-axes** under one value (they cross like a normal matrix):
  ```yaml
  - kata:
      runtime: [qemu, openvmm]
      debug:   [on, off]           # kata → 2 × 2 = 4 sub-cells
  ```
- **Recursion** (a sub-value can itself branch) — allowed by the model, but discourage depth > 1 for
  readability.

This composes with other top-level axes as usual: `arch: [amd64, arch64]` × the `type` tree.

**Alternatives considered (not recommended):**

- **Nested map throughout** — `type: { container: ~, kata: { runtime: [...] } }`. The `container: ~`
  reads poorly and drops the list form.
- **Qualified sub-axis key** — keep `type: [container, kata]` flat and declare
  `kata.runtime: [qemu, openvmm]` as a sibling. Minimal schema change, but the dotted key is magic
  and separates the sub-axis from the value it belongs to.

## 4. Semantics

### 4.1 Expansion

Expand the `type` tree depth-first: each leaf value yields one branch; each branch value expands its
nested matrix and prefixes every sub-cell with `(type = value)`. Then cross the resulting set with
the other top-level axes. `container` contributes 1 cell; `kata` contributes 2 (or 2×2 with two
sub-axes).

### 4.2 Fragment layout — recursive nested (`by-<axis>/<value>/by-<subaxis>/<subvalue>.yaml`)

The layout **mirrors the dimension tree**, applying the same `by-<axis>/<value>` rule at every level.
One rule, recursive, arbitrary depth:

> For a value `V` of axis `A` at any nesting level:
> - **`by-A/V.yaml`** is `V`'s fragment — it applies to **every cell under `V`** (the "common" layer);
> - **`by-A/V/`** is the directory holding `V`'s **nested dimensions**, each declared exactly the same
>   way: `by-B/W.yaml` (+ `by-B/W/` for deeper nesting).

For the example:

```
by-type/
  container.yaml                       # type=container
  kata.yaml                            # common to ALL kata (type=kata, any runtime)
  kata/
    by-runtime/
      qemu.yaml                        # type=kata AND runtime=qemu
      openvmm.yaml                     # type=kata AND runtime=openvmm
```

A leaf's full path spells the whole predicate: `by-type/kata/by-runtime/qemu.yaml` = "type=kata AND
runtime=qemu". Deeper nesting continues the same way, e.g.
`by-type/kata/by-runtime/qemu/by-debug/on.yaml`. Independent **top-level** axes stay at the root
(`by-arch/arm64.yaml`); only **nested** axes live under their parent value's directory — so `runtime`
never appears at the root and never touches `container`.

This is why sub-dimensions read well and stay maintainable:

- **"common to all kata"** is just `by-type/kata.yaml` — the parent value's own fragment.
- **adding a variant** is local: add it to the `runtime:` list and drop
  `by-type/kata/by-runtime/<new>.yaml`; `by-type/kata.yaml` keeps applying automatically. No composite
  filename to grow, no exclude to maintain.
- **arbitrary breadth and depth**: any value can open any number of nested axes, each nested to any
  depth, all with the one `by-<axis>/<value>[/…]` rule.

Backward-compatible: an image with no sub-dimensions is exactly today's flat `by-<axis>/<value>.yaml`
layout (a top-level value simply has no nested directory).

### 4.3 Slugs — what they look like now

Slugs are the thing to get right. The rule is the **minimal, backward-compatible** extension of
today's "join the cell's axis values with `_`": walk the cell's axis **tree depth-first in declared
order** and join **every present value** with `_`, including the parent value of each nested axis.

For `matrix: { type: [container, kata:{runtime:[qemu,openvmm]}] }` (image `myimage`, format `vhd`):

```
myimage_container_vhd                    # type=container            → 1 value
myimage_kata_qemu_vhd                    # type=kata, runtime=qemu   → 2 values (parent + sub)
myimage_kata_openvmm_vhd                 # type=kata, runtime=openvmm
```

With a top-level `arch` axis declared first, and deeper nesting:

```
myimage_arm64_container_vhd
myimage_arm64_kata_qemu_vhd
myimage_arm64_kata_qemu_on_vhd           # …_kata_qemu_debug=on, if kata→runtime→debug is nested
```

Properties and the honest caveats:

- **Unique & filesystem-safe.** Two cells never collide, because their value paths differ. `container`
  vs `kata_qemu` are distinct; the fixed tree means no ambiguity *within an image*.
- **The parent value is always included** (`kata_qemu`, never a bare `qemu`), so a sub-value keeps its
  context and can't collide with a same-named sub-value under a different parent.
- **Variable width — the thing to accept.** `container` cells have fewer `_`-components than `kata`
  cells. Slugs were already **positional-by-matrix-order and meant to be opaque IDs**; with
  conditional axes they are no longer fixed-width, so **do not parse slugs positionally**. Anything
  that needs "which value is which axis" should read the structured `axes` from `tailor matrix --json`
  (which carries the full `(axis,value)` map per cell), not split the slug. This is the one real
  behavior change to socialize.

**Alternative if fixed-width-per-top-level-axis matters** (not recommended): collapse each nested path
into a **single compound token per top-level axis** using a nesting separator, e.g.
`myimage_arm64_kata-qemu_vhd` / `myimage_arm64_container_vhd` — one token per top-level axis regardless of
depth, so positional parsing survives. The catch: `-` (and `.`) are legal *value* characters today, so
a compound token `kata-qemu` is ambiguous with a flat value literally named `kata-qemu`. It would need
a reserved nesting separator not allowed in values. Given slugs are opaque IDs and structured data
lives in `matrix --json`, the plain `_`-join (variable width) is cleaner; flag the compound-token
option only if a downstream consumer truly requires fixed columns.

### 4.4 Selectors

- `-s type=kata` → both kata cells.
- `-s runtime=qemu` → the qemu cell (and, since only kata cells have `runtime`, implicitly kata).
- `-s type=container` → the container cell (selecting `runtime=*` with `type=container` matches
  nothing).

Validation: a selector on a sub-axis is valid; applying it to cells that lack that axis simply
doesn't match (consistent with today's `section_matches`).

### 4.5 Merge precedence

Per cell, apply base → parent-value fragment (`by-type/kata.yaml`) → sub-value fragment
(`by-type/kata/by-runtime/qemu.yaml`) → other axes' fragments → composites, later wins. The
parent-then-sub ordering means a variant can override the common-kata layer, which is the intuitive
precedence.

### 4.6 Composition & disjunction under nesting

The two advanced fragment mechanisms — **disjunction** (`by-A/v1+v2.yaml`, OR within an axis) and
**composition/conjunction** (`by-A+B/v1+v2.yaml`, across axes) — both still work, but the first thing
to notice is that **nesting subsumes most of what they were used for.** Both exist to express
conjunction/grouping without a fragment explosion; nesting already expresses hierarchical conjunction
*by path* (`by-type/kata/by-runtime/qemu.yaml` **is** "kata AND runtime=qemu"), and gives
"common to all kata" as the parent fragment `by-type/kata.yaml` — which is exactly the
`kata-qemu+kata-openvmm.yaml` disjunction pain that motivated this. So inside a nested subtree you
rarely reach for either.

**Disjunction — fully supported, at every level.** The `+`-in-stem rule is unchanged in any `by-<axis>/`
directory, nested or not:

- `by-type/kata/by-runtime/qemu+clh.yaml` — kata AND (runtime = qemu OR clh): a fragment for a
  *subset* of a sub-axis's values.
- `by-type/container+kata.yaml` — type ∈ {container, kata}. A **branch value in a disjunction covers
  its whole subtree** (every kata sub-cell matches), which is an intuitive, useful property.

**Composition — supported, with one strained case:**

- *Two top-level axes* (neither nested): unchanged — `by-arch+type/arm64+kata.yaml` at the root
  (`type=kata` matches all kata sub-cells). ✅
- *Two sub-axes under the same parent*: lives under that parent by the same recursive rule —
  `by-type/kata/by-runtime+debug/qemu+on.yaml`. ✅
- *A top-level axis × a nested sub-axis* (e.g. `arch` × `runtime`): the awkward one — `arch` is at the
  root but `runtime` exists only under `kata`. Rule: the composite lives at the **deepest nested
  location among its axes**, naming all of them — `by-type/kata/by-arch+runtime/arm64+qemu.yaml`. It
  reads slightly oddly (arch isn't "part of" kata) and is rare, so **define it but treat it as
  advanced / likely defer**: most "arm64 AND qemu" needs are already met by the `by-arch` fragment and
  the runtime fragment both applying via merge; only a true *intersection-only* fragment needs the
  composite.

**The one hard requirement that makes all of this work:** the composite/disjunction matcher must treat
*"the cell doesn't have this named axis"* as **no-match**, never an error. A root
`by-arch+runtime/…` names `runtime`, which `container` cells lack — it must simply skip them. This is
the *same* tolerance that lets conditional axes exist at all, so it's one consistent rule, not a
special case.

**My take:** keep both, but **lead with nesting**. Nesting is the right tool for the "b exists only
under a" shape and removes the composite/disjunction boilerplate there; composites stay for genuinely
independent top-level axes that need joint logic; disjunction stays orthogonal and works everywhere.
Ship order: disjunction (free), same-parent sub-axis composites (natural), and the "no-match on
missing axis" matcher rule; **defer top-level×nested composites** until a real case appears.

## 5. Why this is better than the two alternatives

| | Flat 3-value + disjunction file | Two independent axes + excludes | **Sub-dimension** |
| --- | --- | --- | --- |
| "common to all kata" | `by-type/kata-qemu+kata-openvmm.yaml` (grows, must be remembered) | `by-type/kata.yaml` ✅ | `by-type/kata.yaml` ✅ |
| add a new kata variant | edit the composite filename + the axis | add value + **update excludes** | add value + one `by-type/kata/by-runtime/<v>.yaml` ✅ |
| container has no variant | implicit | must **exclude** `runtime` from container | structural ✅ (no runtime axis) |
| phantom cells | none | `container × qemu/openvmm` unless excluded | none ✅ |
| expresses the real shape | no | no | **yes** |

## 6. What changes in tailor (scoping note)

The elegant part: **most machinery already tolerates per-cell varying axis sets** —
`AxisTuple`/`cell_slug` join whatever pairs a cell has, `by-<axis>/<value>` fragments apply only when
present, and selectors match on presence. The concentrated changes are:

- **Schema:** a matrix value becomes `Leaf(String) | Branch { name, nested: Matrix }` (recursive) —
  today `AxisValues = IndexMap<String, Vec<String>>` with string values.
- **Expansion:** `expand`/`cartesian` walk the tree (leaf → 1, branch → nested product) before
  crossing with the other axes.
- **Fragment discovery:** teach `fragment::discover` the **recursive nested path**
  `by-<axis>/<value>/by-<subaxis>/<subvalue>[/…].yaml` (§4.2) — the chosen layout.
- **Validation:** nested-axis naming rules (§7), selector validation across conditional axes.

Slug (a plain `_`-join of the cell's present values, §4.3), selectors, and the core fragment
apply-rule (`axis=value` present ⇒ applies) likely need **little or no change** — the new work is the
tree schema, tree expansion, and the nested fragment path.

## 7. Edge cases & rules to decide

- **Sub-axis names are scoped by path.** With the nested layout (§4.2), a sub-axis fragment lives at
  `by-<parent>/<value>/by-<subaxis>/…`, so two different parents may each have a `runtime` sub-axis
  without their fragments colliding — the path disambiguates. Still **disallow a sub-axis name that
  collides with a top-level axis** (keeps selectors/slug unambiguous).
- **Depth.** Allow arbitrary recursion (Paco's requirement); lint/warn only if depth gets unwieldy.
- **Composition & disjunction:** both compose with nesting — see §4.6 (disjunction works at every
  level; same-parent-sub-axis composites are natural; top-level×nested composites are the one strained
  case, deferred). The load-bearing rule is that the matcher treats a *missing* named axis as
  no-match, not an error.
- **Slug determinism (§4.3).** Emit values by **depth-first declared-axis order**, parent value
  immediately before its nested axis's value; stable across runs. Accept variable width; slugs are
  opaque IDs.
- **Empty / selection.** Selecting only a sub-axis value that no selected parent provides → empty
  (existing `EmptySelection` error).

## 8. Open questions

1. Config spelling: confirm the value-with-nested-axis list form (§3) over the qualified-key
   alternative.
2. **Slugs (the main worry, §4.3):** accept the plain `_`-join with **variable width** (recommended —
   opaque IDs, structured data via `matrix --json`), or adopt a fixed-width **compound-token** scheme
   per top-level axis (needs a reserved nesting separator, since `-`/`.` are legal value chars)?
3. Sub-axis naming: rely on **path scoping** (nested layout) so a name may recur under different
   parents, or still require globally-unique sub-axis names for simplicity?
4. How far to generalize now: single-level sub-dimensions only (covers the kata case), or the full
   arbitrary-breadth/arbitrary-depth tree from day one (Paco leans full)?
5. Does anything downstream assume a *fixed* axis set across an image's cells (reporting, `matrix`
   output schema, ADO matrix legs, the export/rendered slug filenames)? Audit those for per-cell
   varying axes — this is where variable-width slugs could bite.

**Resolved this round:** fragment layout = recursive nested `by-<axis>/<value>/by-<subaxis>/<subvalue>.yaml`
(arbitrary breadth + depth); slugs = plain `_`-join of the cell's present values (parent before sub,
variable width, opaque).
