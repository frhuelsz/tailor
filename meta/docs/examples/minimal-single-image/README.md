# The entry level: one file

**This is where you start.** A simple, one-off image is a **single `image.yaml`** — a small tailor
header (`name` / `base` / `outputs`) plus the Image Customizer config under `config:`. That's it:

- no `base.yaml`
- no `by-variant/` / `by-release/` / `by-arch/`
- no `matrix`, no `$select`, no `$include`

See [`image.yaml`](./image.yaml) — the whole image is that one file. Top level is tailor config; the
`config:` block is the IC config, kept in its own namespace (they never mix).

## The three forms of "entry level" (pick whichever fits)

**1. Inline (start from scratch)** — one file: tailor header + the IC config under `config:` (this
example):

```yaml
name: my-image
base: { path: ./artifacts/core.vhdx }
outputs: [{ format: cosi }]
config:                                  # ← all IC config lives here
  os: { hostname: my-image, packages: { install: [vim] } }
  storage: { bootType: efi, disks: [ ... ], filesystems: [ ... ] }
```

**2. Reference (you already have an IC config)** — point `config:` at your existing IC YAML, used
as-is (same key, a path string instead of an inline mapping):

```yaml
name: my-image
base: { path: ./artifacts/core.vhdx }
outputs: [{ format: cosi }]
config: ./baseimg.yaml        # your hand-written IC config, unchanged
```

**3. No image-def layer at all** — just a tailor target in `tailor.yaml`
([design.md §5.2](../../design.md)). Use this for a true one-off; the image-definition mechanism
isn't even involved:

```yaml
targets:
  - name: my-image
    config: ./baseimg.yaml
    base: { path: ./artifacts/core.vhdx }
    outputs: [{ format: cosi }]
```

## How it grows (and only if it needs to)

You reach for the bigger structure **only** when an image sprouts variants. The progression is
additive — you never rewrite what you have:

| Level | What you add | Files |
| ----- | ------------ | ----- |
| **Entry** | nothing | one `image.yaml` (this dir) |
| +variants | split the IC config into `base.yaml` + `by-variant/<v>.yaml` | a handful of small files |
| +releases/arch | a `matrix:` block + `by-release/*.yaml` / `by-arch/*.yaml` | one tiny file per axis value |
| full matrix | shared `layouts/`, `features`, `include`/`exclude` | see [`../trident-vm-testimage`](../trident-vm-testimage) |

The [`trident-vm-testimage`](../trident-vm-testimage) example is the **far end** of this ladder (a
multi-variant family). This directory is the **near end** — the single file you actually start with.
