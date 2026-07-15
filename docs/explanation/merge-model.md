# Merge model

The merge model is designed to be deterministic and reviewable.

```mermaid
flowchart LR
  I["image.yaml"] --> M["merge"]
  A1["by-first-axis/value.yaml"] --> M
  A2["by-later-axis/value.yaml"] --> M
  M --> R["rendered IC config"]
```

## Precedence is axis declaration order

Fragments apply in the order axes are declared in `matrix:`. A later axis wins later `$set` conflicts and appends later to lists.

```yaml
matrix:
  edition: [lite, pro]
  channel: [stable, edge]
```

Here `by-edition/...` applies before `by-channel/...`. This is intentional: adding a directory or changing alphabetical order cannot silently change precedence. Authors control precedence where they declare the axes.

A fragment can target a **combination** of axes (`by-edition+arch/pro+arm64.yaml`, a conjunction) or
**several values of one axis** (`by-channel/stable+edge.yaml`, a disjunction). Such fragments sort after the
single-axis ones they refine (more axes = more specific = later), and a narrower single-value fragment wins
over a broader disjunction on the same axis. `tailor explain <image> --cell <slug>` prints the exact order;
see `docs/reference/image-yaml.md` and `meta/docs/2026-06-29-directive-design.md` §2.

## Maps, lists, and scalars

- Maps deep-merge.
- Lists append by default; `$prepend`/`$append` add to either end and `$replace`/`$remove` rewrite or trim.
- Differing scalar assignments conflict unless the later assignment uses `$set`.
- Set a key's value to the bare token `$unset` to remove the inherited key entirely.

This makes accidental double ownership loud while keeping additive IC lists easy. See
`docs/reference/directives.md` for every directive.

## Why no IC-aware merge?

tailor does not know that an IC list contains packages, partitions, filesystems, or services. It does not merge list items by `id` or deduplicate packages. If a structured list must change, own the whole list in the fragment or use `$replace`.

That keeps tailor a thin wrapper over Image Customizer rather than a second IC schema implementation.
