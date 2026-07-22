# tailor — fragment/value-level `skip` (drop cells that pick a value, unless pinned)

> **Status:** Proposed · _2026-07-22_
>
> Extends the image-level `skip:` (already shipped — `image.yaml: skip: true` excludes a whole image
> from bulk selection) down to **a specific dimension value**: mark the value's fragment file and
> every cell that picks that value is dropped from bulk selection — unless the run **specifically
> requests it**. Motivating case: experimenting with a config (a `by-<axis>/<value>.yaml` fragment)
> that needs a package that doesn't exist yet — you want it in the tree, iterable locally, but never
> grabbed by the pipeline (which runs `build`/`matrix`/`export` over everything).

## 1. Model (agreed)

`skip` is a **regular, mergeable per-cell setting**. Every candidate cell is expanded and **rendered
normally** — it fully exists, with a resolved `skip` value. Then, **as a filter step before the build
starts**, cells with resolved `skip == true` are **dropped from the list**, unless the cell was
**specifically requested**.

Two independent entry points, resolved at their natural phase:

| Where | Field | Scope | Filtered at | Overridden by |
| --- | --- | --- | --- | --- |
| `image.yaml` | `skip: true` | whole image | `build_targets` (selection of images) | naming the image (`tailor build <name>`) |
| `by-<axis>/<value>.yaml` fragment | `skip: true` | cells that pick that value | `cells_selected` (per-cell) | `--cell <slug>` **or** a `-s` that **pins the skip value** |

The image-level case ships today. This doc is the **fragment/value-level** case.

## 2. Fragment `skip` is mergeable, with provenance

`skip` becomes a tailor-level field on a fragment (alongside `base`/`outputs`/`params`/`rpmSources`),
so it merges per cell like any other fragment field:

```yaml
# by-type/kata-experimental.yaml
skip: true                 # cells with type=kata-experimental are dropped from bulk
config:
  os:
    packages:
      install: [not-yet-a-real-package]
```

Merge is **last-wins in apply order** (base → most-specific), so a more-specific fragment may flip it
back (`skip: false`) to un-skip a narrower slice. When a fragment sets `skip: true`, tailor records
that fragment's predicate coordinates as the cell's **`skip_pins`** — the `(axis, value)` pairs that,
if pinned by `-s`, count as "specifically requesting" the cell. For `by-type/kata-experimental.yaml`
that is `[(type, kata-experimental)]`.

## 3. The "specifically requested" rule (strict)

A cell with resolved `skip == true` (from a fragment, i.e. `skip_pins` non-empty) is **kept** iff:

- `--cell <slug>` names it exactly, **or**
- a `-s axis=value` selector pins at least one of the cell's `skip_pins`.

Everything else drops it — including **unqualified bulk** *and* a non-pinning selector like
`-s arch=amd64` (it matches the cell but does not pin `type=kata-experimental`, so the experimental
cell is **not** resurrected). This is the strict interpretation chosen for this feature.

```
tailor build                          # bulk → dropped
tailor matrix                         # bulk → dropped (so the pipeline never sees it)
tailor build -s arch=amd64            # matches but doesn't pin the value → dropped
tailor build -s type=kata-experimental  # pins the skip value → BUILT
tailor build --cell myimage_kata-experimental_amd64_vhd  # names it → BUILT
```

## 4. Implementation sketch

- **`tailor-config`**
  - `Fragment { …, skip: Option<bool> }`.
  - `LoadedFragment::pins()` → the `(axis, value)` equality coordinates of its path predicate (and
    equality leaves of an inline `match`).
  - `render_cell`: fold `skip` over the matched fragments (last-wins); when the winning value is
    `true`, set `skip_pins` = that fragment's `pins()`, else clear.
  - `RenderedCell { …, skip: bool, skip_pins: Vec<(String, String)> }`.
- **`tailor-core`**
  - `Cell { …, skip: bool, skip_pins: Vec<(String, String)> }`, populated by `cells()`.
  - `Selector::requests_skip_cell(cell)` → `slugs` names the cell **or** `axes` pins a member of
    `cell.skip_pins`.
  - `cells_selected`: after `selector.matches`, additionally drop any cell where
    `cell.skip && !cell.skip_pins.is_empty() && !selector.requests_skip_cell(cell)`. Apply for both
    empty (bulk) and non-empty selectors. Keep `NoCellsSelected` only for a **non-empty selector that
    matched nothing** (typo-catch); a bulk run whose cells were all skip-dropped returns empty (the
    image simply contributes no cells).
- **Image-level `skip` unchanged** — it stays in `build_targets` (whole-image, bulk vs named). Because
  image-level skip carries **no** `skip_pins`, `cells_selected` never double-drops a named skip
  image's cells.

## 5. Interaction notes

- **Composition/disjunction:** a disjunction fragment `by-axis/v1+v2.yaml` with `skip: true` pins both
  `(axis,v1)` and `(axis,v2)`; pinning either resurrects. A composite `by-a+b/va+vb.yaml` pins both
  coordinates; pinning either counts (lenient) — could be tightened to require all, TBD.
- **`selectors.exclude` still exists** and is the *unconditional* drop (no override). `skip` is the
  *conditional* drop (overridable by pinning). Keep both.
- **Sub-dimensions** (`2026-07-21-sub-dimensions.md`): a nested sub-axis value's fragment can carry
  `skip` the same way; `skip_pins` would be the nested `(subaxis, value)`.

## 6. Open questions

1. Composite-predicate skip: pinning **any** vs **all** coordinates to resurrect? (Lean: any.)
2. Should a bulk run where an image's cells are *entirely* skip-dropped warn (vs silently contribute
   nothing)?
3. Do we want fragment `skip: false` to un-skip cells dropped by an **image-level** `skip: true`? (No
   — image-level is a whole-image bulk decision resolved before cells render; documented limitation.)
