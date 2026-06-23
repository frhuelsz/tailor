# Example: a **standalone** image (`image.yaml` only, no `tailor.yaml`)

This shows **standalone mode** — a single self-contained `image.yaml` with **no `tailor.yaml`
anywhere above it**. It's the one-image counterpart to
[`../workspace-two-images`](../workspace-two-images), and the lowest-friction way to build one image.

## File tree

```
standalone-image/
└── image.yaml      # everything in one file: toolchain + base + IC config
```

## How discovery works

`tailor build` walks **up** from the current directory looking for a `tailor.yaml`. Finding none, it
falls back to **single-image mode** and uses the `image.yaml` in the current directory. No workspace,
no catalogue, no repo-wide settings — just this image.

## Where the IC version lives (when there's no workspace)

With no `tailor.yaml` to define `toolchains:`, the image carries its own:

- **Inline `toolchain:`** — a `{ container, version }` definition right in `image.yaml` (what this
  example does, pinning IC `1.3.0`).
- **…or omit `toolchain:` entirely** to use tailor's **built-in default** IC version — the truly
  minimal case.

Either way, tailor resolves the toolchain (and the base) to registry digests and writes a
`tailor.lock` **next to `image.yaml`**, so a standalone build is just as reproducible as a workspace
one.

## Building

```
tailor build                  # build this image
tailor build --clones 2       # two isolated, identical copies in parallel
```

## Relationship to the other examples

| Example | Mode | Focus |
| ------- | ---- | ----- |
| this one | standalone (`image.yaml`, no `tailor.yaml`) | **topology**: a complete image with an inline/defaulted toolchain |
| [`../minimal-single-image`](../minimal-single-image) | standalone | the **image-definition entry level** (one file, `config:` namespacing) |
| [`../workspace-two-images`](../workspace-two-images) | workspace (`tailor.yaml` + members) | repo-wide toolchains + per-image override + discovery |
| [`../trident-vm-testimage`](../trident-vm-testimage) | (one image) | the full `matrix:` + `by-*/` fragment mechanism |

> **Illustrative only** — package names aren't verified against a live repo; this example is about the
> standalone shape, not the package set.
