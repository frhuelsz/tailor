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

### 4.2 Cells, slug, and the key insight

A cell is still an ordered list of `(axis, value)` pairs — but **different cells carry different axis
sets** (`container` cells lack `runtime`). tailor's cell model (`AxisTuple` = `Vec<(axis, value)>`)
**already supports this** — the slug just joins the values it has:

```
myimage_container_vhd
myimage_kata_qemu_vhd
myimage_kata_openvmm_vhd
```

The sub-value follows its parent value in slug order (parent first, then nested-axis order).

### 4.3 Fragment layout — mostly already works

Because a `by-<axis>/<value>.yaml` fragment applies **iff the cell has that axis=value**, the
existing mechanism already does the right thing for conditional axes:

- `by-type/container.yaml` — container only.
- `by-type/kata.yaml` — **common to every kata cell** (applies whenever `type=kata`, regardless of
  the sub-value). ← this is the "common between the katas" the flat approach struggled with, now a
  plain parent-value fragment.
- `by-runtime/qemu.yaml`, `by-runtime/openvmm.yaml` — variant-specific. They only match cells that
  *have* a `runtime` axis (i.e. kata cells), so they never touch `container`.

Adding a new kata variant is then **local**: add it to the `runtime:` list and drop a
`by-runtime/<new>.yaml`. `by-type/kata.yaml` keeps applying automatically. No composite filename to
grow, nothing to remember.

**Optional scoped layout.** For readability (and to disambiguate a sub-axis name reused under
different parents), also allow a **nested fragment path** that ties the sub-value to its parent:

```
by-type/
  container.yaml
  kata.yaml            # common to all kata
  kata/
    qemu.yaml          # = by-type where type=kata AND runtime=qemu
    openvmm.yaml
```

`by-type/kata/<sub>.yaml` reads as "kata's qemu variant" and scopes the fragment under its parent.
Both forms can coexist; the nested form is sugar over "the cell has `type=kata` and `runtime=qemu`".

### 4.4 Selectors

- `-s type=kata` → both kata cells.
- `-s runtime=qemu` → the qemu cell (and, since only kata cells have `runtime`, implicitly kata).
- `-s type=container` → the container cell (selecting `runtime=*` with `type=container` matches
  nothing).

Validation: a selector on a sub-axis is valid; applying it to cells that lack that axis simply
doesn't match (consistent with today's `section_matches`).

### 4.5 Merge precedence

Per cell, apply base → parent-value fragment (`by-type/kata.yaml`) → sub-value fragment
(`by-runtime/qemu.yaml` or `by-type/kata/qemu.yaml`) → other axes' fragments → composites, later
wins. The parent-then-sub ordering means a variant can override the common-kata layer, which is the
intuitive precedence.

## 5. Why this is better than the two alternatives

| | Flat 3-value + disjunction file | Two independent axes + excludes | **Sub-dimension** |
| --- | --- | --- | --- |
| "common to all kata" | `by-type/kata-qemu+kata-openvmm.yaml` (grows, must be remembered) | `by-type/kata.yaml` ✅ | `by-type/kata.yaml` ✅ |
| add a new kata variant | edit the composite filename + the axis | add value + **update excludes** | add value + one `by-runtime/<v>.yaml` ✅ |
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
- **Validation:** nested-axis name uniqueness rules (§7), selector validation across conditional
  axes.
- **(Optional) nested fragment path** `by-<parent>/<value>/<sub>.yaml` in `fragment::discover`.

Slug, selectors, and the core fragment apply-rule likely need **little or no change**.

## 7. Edge cases & rules to decide

- **Sub-axis name scope.** Is `runtime` global (one `by-runtime/`, shared if two parents both use
  `runtime`) or strictly scoped to its parent? Recommend: names are **declared per parent** but live
  in the same cell namespace; if you want isolation, use the nested `by-type/kata/<sub>.yaml` form or
  a unique sub-axis name. Disallow a sub-axis name that collides with a top-level axis.
- **Depth.** Allow recursion but lint against depth > 1.
- **Interaction with features / composites / disjunctions.** A sub-axis is a normal axis for those
  mechanisms: `by-runtime/qemu+clh.yaml` (disjunction) and `by-arch+runtime/arm64+qemu.yaml`
  (composite across a top-level and a sub-axis) should work — confirm the composite validator handles
  a cell that *lacks* one of the named axes (it must simply not match, not error).
- **Matrix order / slug determinism.** Fix the sub-value's slug position (immediately after its
  parent) and the nested-axis order (declared order) so slugs stay stable.
- **Empty / selection.** Selecting only a sub-axis value that no selected parent provides → empty
  (existing `EmptySelection` error).

## 8. Open questions

1. Config spelling: confirm the value-with-nested-axis list form (§3) over the qualified-key
   alternative.
2. Sub-value fragment home: flat `by-<subaxis>/<value>.yaml` (reuses existing mechanism, zero new
   rules) vs the scoped nested `by-<parent>/<value>/<sub>.yaml` (more readable) — support both, or
   pick one?
3. Sub-axis naming: require globally-unique axis names (simplest, avoids the scope question), or
   allow reuse with the nested form for disambiguation?
4. How far to generalize now: single-level sub-dimensions only (covers the kata case), or the full
   recursive tree from day one?
5. Does anything downstream assume a *fixed* axis set across an image's cells (reporting, `matrix`
   output schema, ADO matrix legs)? Audit those for per-cell varying axes.
