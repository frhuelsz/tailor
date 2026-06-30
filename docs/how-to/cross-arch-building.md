# Cross-arch building

tailor targets `amd64` by default. Build `arm64` by **declaring** it — nothing is inferred from the
host, so a workspace builds the same set on any machine.

## Build one arm64 image

Add an `arch` axis to the image:

```yaml
# image.yaml
name: gizmo
matrix:
  arch: [arm64]
base:
  oci:
    uri: "registry.example/gizmo/base:edge"
    platform: "linux/${arch}"      # ${arch} → linux/arm64
outputs:
  - format: cosi
config:
  os:
    hostname: gizmo
```

```sh
tailor build gizmo          # → gizmo_arm64_cosi.cosi
```

## Build both arches

List both values; tailor expands one cell per arch:

```yaml
matrix:
  arch: [amd64, arm64]       # → gizmo_amd64_cosi, gizmo_arm64_cosi
base:
  path: ./bases/gizmo-${arch}.img
```

## Amd64 is the default

With no `arch` axis and no workspace override, an image builds a single `amd64` cell:

```yaml
name: gizmo
base:
  path: ./bases/gizmo.img      # → gizmo_amd64_cosi
```

## Change the default for a workspace

Set `defaults.architectures` in `tailor.yaml` to override `amd64` for every image that has no `arch`
axis:

```yaml
# tailor.yaml
defaults:
  architectures: [arm64]       # workspace builds arm64 unless an image declares its own axis
```

## Platform must match the arch

The `arch` component of an `oci.platform` must equal the cell's arch. Always write `linux/${arch}` so
each cell pulls its own manifest; a fixed `platform: linux/arm64` on an `amd64` cell fails at
`validate` before any pull. `path` and `azureLinux` bases declare no arch, so the cell arch decides.

See [Architectures](../explanation/architectures.md) for the full model.
