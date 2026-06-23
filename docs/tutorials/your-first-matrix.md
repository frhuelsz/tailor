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
gizmo/by-variant/minimal.yaml
gizmo/by-variant/full.yaml
gizmo/by-arch/amd64.yaml
gizmo/by-arch/arm64.yaml
```

## 2. Inspect the matrix

```bash
tailor matrix gizmo --format slugs
```

Expected output:

```text
gizmo_minimal_amd64_cosi
gizmo_minimal_arm64_cosi
gizmo_full_amd64_cosi
gizmo_full_arm64_cosi
```

The slug format is `<image>_<axis values in matrix order>_<format>`.

## 3. Add another axis

```bash
tailor add axis gizmo channel
```

This appends a placeholder value to `gizmo/image.yaml` and creates `gizmo/by-channel/`. Edit the new matrix entry:

```yaml
matrix:
  variant: [minimal, full]
  arch:    [amd64, arm64]
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

Expected shape: eight slugs, because `variant[2] × arch[2] × channel[2] × outputs[1] = 8`.

```text
gizmo_minimal_amd64_stable_cosi
gizmo_minimal_amd64_edge_cosi
...
gizmo_full_arm64_edge_cosi
```

## 5. Inspect rendered Image Customizer YAML

```bash
tailor explain gizmo -s variant=full,arch=amd64,channel=edge
```

Expected shape:

```text
gizmo: 1 cell(s)

── gizmo_full_amd64_edge_cosi ──
os:
  hostname: gizmo
  packages:
    install:
      - openssh-server
      - grub2-efi-x64
      - vim
      - git
      - gizmo-edge
```

The exact config depends on your edits. The important point: `explain` shows the fully merged IC config for selected cells.

## 6. Dry-run one selected cell

```bash
tailor build gizmo -s variant=full,arch=amd64,channel=edge --dry-run
```

You now have a small workspace that demonstrates axes, fragments, interpolation, cell selection, and dry-run builds.
