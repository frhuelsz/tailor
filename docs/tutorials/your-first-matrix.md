# Your first matrix

This tutorial shows how a single image definition expands into multiple cells.

## 1. Scaffold an advanced workspace

```bash
mkdir matrix-demo
cd matrix-demo
tailor init gizmo advanced
```

The `advanced` template creates:

```text
tailor.yaml
gizmo/image.yaml
gizmo/by-arch/amd64.yaml
gizmo/by-arch/arm64.yaml
gizmo/by-variant/minimal.yaml
gizmo/by-variant/full.yaml
```

Its matrix declares `arch` first, then `variant` — axes go widest → most-specific, and that order sets
both the slug order and fragment precedence (see [Architectures](../explanation/architectures.md)).

## 2. Inspect the matrix

```bash
tailor matrix gizmo --format slugs
```

Expected output:

```text
gizmo_amd64_minimal_cosi
gizmo_amd64_full_cosi
gizmo_arm64_minimal_cosi
gizmo_arm64_full_cosi
```

The slug format is `<image>_<axis values in matrix order>_<format>`.

## 3. Add another axis

```bash
tailor add axis gizmo channel
```

This appends a placeholder value to `gizmo/image.yaml` and creates `gizmo/by-channel/`. Edit the new matrix entry:

```yaml
matrix:
  arch:    [amd64, arm64]
  variant: [minimal, full]
  channel: [stable, edge]
```

Add channel fragments:

```bash
cat > gizmo/by-channel/stable.yaml <<'EOF'
params:
  repoChannel: stable
config:
  os:
    packages:
      install:
        - gizmo-stable
EOF

cat > gizmo/by-channel/edge.yaml <<'EOF'
params:
  repoChannel: edge
config:
  os:
    packages:
      install:
        - gizmo-edge
EOF
```

## 4. Watch cells multiply

```bash
tailor matrix gizmo --format slugs
```

Expected shape: eight slugs, because `arch[2] × variant[2] × channel[2] × outputs[1] = 8`. The last
axis varies fastest:

```text
gizmo_amd64_minimal_stable_cosi
gizmo_amd64_minimal_edge_cosi
gizmo_amd64_full_stable_cosi
gizmo_amd64_full_edge_cosi
gizmo_arm64_minimal_stable_cosi
gizmo_arm64_minimal_edge_cosi
gizmo_arm64_full_stable_cosi
gizmo_arm64_full_edge_cosi
```

## 5. Inspect the merge order

```bash
tailor explain gizmo -s arch=amd64,variant=full,channel=edge
```

`explain` picks the selected cell and prints the ordered list of files that merge to produce it (top =
base, bottom wins) — the same order the axes are declared in:

```text
cell  gizmo_amd64_full_edge_cosi   (arch=amd64, channel=edge, variant=full)

merge order (top = base, bottom wins):
   1  image.yaml            base
   2  by-arch/amd64.yaml    arch=amd64
   3  by-variant/full.yaml  variant=full
   4  by-channel/edge.yaml  channel=edge
```

Add `--with-config` to also print the fully merged Image Customizer config for the cell:

```bash
tailor explain gizmo -s arch=amd64,variant=full,channel=edge --with-config
```

## 6. Dry-run one selected cell

```bash
tailor build gizmo -s arch=amd64,variant=full,channel=edge --dry-run
```

You now have a small workspace that demonstrates axes, fragments, interpolation, cell selection, and dry-run builds.
