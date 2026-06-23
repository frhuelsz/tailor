# Example: a tailor **workspace** (`tailor.yaml` + two images)

This shows **workspace / repo mode** — a `tailor.yaml` at the root plus two member images. It's the
multi-image counterpart to [`../standalone-image`](../standalone-image) (one `image.yaml`, no
`tailor.yaml`). The point here is the **repo topology, discovery, and where the IC version lives** —
the images themselves are deliberately tiny.

## File tree

```
workspace-two-images/
├── tailor.yaml            # WORKSPACE ROOT: toolchains (IC versions) + runtime + defaults
├── webserver/
│   └── image.yaml         # member image — uses the default toolchain, inherits defaults
└── database/
    └── image.yaml         # member image — OVERRIDES the toolchain (pins an older IC)
```

## How discovery works (Cargo-style)

1. `tailor build` (run from anywhere under this dir) walks **up** to find `tailor.yaml` — that file's
   directory is the workspace root.
2. With no `images:` key present, tailor **auto-discovers every `*/image.yaml` at depth 1** → it finds
   `webserver` and `database`. (Add an explicit `images:` list only to curate the set, point at a
   non-standard layout, or inline a trivial image.)
3. Each image is named by its `name:` field; its relative paths resolve against its own directory.

So `tailor build` builds both images; `tailor build webserver` builds one. An illustrative catalogue:

```
$ tailor images
NAME       TOOLCHAIN   IC       ARCH          BASE                       OUTPUTS
webserver  ic-1.3      1.3.0    amd64         azureLinux 3.0/minimal-os  cosi
database   ic-1.1      1.1.0    amd64, arm64  azureLinux 3.0/minimal-os  cosi
```

## Where the IC version lives

- **Repo-wide, in `tailor.yaml` `toolchains:`** — the single source of truth, pinned to a digest in
  `tailor.lock`. Here it defines two: `ic-1.3` (the `default`) and `ic-1.1`.
- **Each image may override** via the top-level `toolchain:` field: `webserver` omits it (so it gets
  `ic-1.3`), `database` sets `toolchain: ic-1.1`. That's the whole "repo-wide default + per-image
  override" model.
- The **toolchain (IC version) is independent of the base OS version** — both images run on an
  `azureLinux 3.0` base; `database` just customizes it with an older Image Customizer.

## Inheritance from `defaults:`

Both images inherit `outputs: [cosi]` from `tailor.yaml` `defaults:`. `webserver` also inherits the
default `architectures: [amd64]`; `database` **overrides** it to build **both `amd64` and `arm64`**.
Because its base is the multi-arch `azureLinux` (MCR) image, that single line is all it takes — tailor
resolves the per-arch digest at pull time, with no `baseByArch` wiring. An image overrides a default
only when it needs to (a wider arch set here, or a different output format like the
[`vm-img` variant](../trident-vm-testimage)).

## Building (and clones)

```
tailor build                     # build every discovered image
tailor build webserver           # build just one
tailor build webserver --clones 3   # 3 isolated, identical copies in parallel (suffixed outputs)
```

`--clones` is a **build-command flag**, not config — it's a build-time choice ("I need 3 identical
nodes for this test run"), so it never appears in `tailor.yaml`/`image.yaml`. Each clone gets its own
suffixed artifact and isolated working dirs; the rendered IC config is identical across clones.

> Each member image is single-cell here, but an image can also grow a full `matrix:` and `by-*/`
> fragment tree — see [`../trident-vm-testimage`](../trident-vm-testimage) for that end of the
> spectrum. The workspace topology is the same either way.

> **Illustrative only.** Package names (`nginx`, `postgresql-server`, …) aren't verified against a
> live repo the way [`../trident-vm-testimage`](../trident-vm-testimage) is; this example is about the
> workspace shape, not the package set.
