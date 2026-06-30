# `tailor.yaml` reference

`tailor.yaml` is the workspace manifest: it configures toolchains, runtime defaults, and image discovery. tailor finds it by walking up from the current directory.

```yaml
schemaVersion: 1

toolchains:
  default: ic
  entries:
    ic:
      container: mcr.microsoft.com/azurelinux/imagecustomizer
      # version: "1.3.0"
      # tag: "1.3.0"

defaults:
  architectures: [amd64]
  outputs:
    - format: cosi
```

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `schemaVersion` | integer | yes | Current value: `1`. |
| `toolchains.default` | string | yes | Default toolchain id for images that omit `toolchain:`. |
| `toolchains.entries` | map | yes | Toolchain definitions. `tag` defaults to `version`, else `latest`. |
| `runtime.engine` | enum | no | Container engine: `docker` (default), `podman`, or `auto`. See [Select a container engine](../how-to/select-a-container-engine.md). |
| `runtime.host` | string | no | Explicit engine endpoint (`unix://…`, a bare socket path, or `tcp://…`), overriding the engine default and `DOCKER_HOST` / `CONTAINER_HOST`. |
| `runtime.privileged` | bool | no | Default `true`; IC requires privileged container execution. |
| `runtime.mounts.hostRoot` | path | no | Default `/host`; host `/` bind target and path-translation prefix. |
| `runtime.mounts.dev` | bool | no | Default `true`; bind `/dev:/dev`. |
| `runtime.buildDir` | string | no | Container-internal IC build dir. Default `/tmp`. |
| `runtime.logLevel` | enum | no | IC log level: `panic`, `fatal`, `error`, `warn`, `info`, `debug`, `trace`. |
| `runtime.imageCacheDir` | path | no | Cache for registry base images. Default: `<workspace>/.tailor/cache`. Required by IC for `oci`/`azureLinux` bases — tailor supplies the default so they build out of the box. |
| `runtime.janitorImage` | `{container, tag?}` | no | Minimal image used for sudo-free ownership cleanup. Default: `mcr.microsoft.com/azurelinux/base/core:3.0`. |
| `signing.default` | string | no | Signing profile used when an image says `signing: true`. See [Sign an image](../how-to/sign-an-image.md). |
| `signing.profiles` | map of `{backend, …}` | no | Named signing profiles. `backend` is `local-test-ca`, `keypair` (needs `key`+`cert`), or `azure-key-vault` (needs `vault`+`certificate`). |
| `defaults.architectures` | arch list | no | Default arch set for images without an `arch` axis. The only override of the built-in `[amd64]` default; per-image declares arch via a `matrix.arch` axis. |
| `defaults.outputs` | output list | no | Inherited by images without `outputs`. |
| `baseImages` | map of `{path, arch?, source?}` | no | Base-image catalogue: named slots an image references with `base: { ref: <name> }`. See [base-image catalogue](#base-image-catalogue). |
| `images` | object | no | Omit to auto-discover every immediate `*/image.yaml`. |

## Base-image catalogue

`baseImages:` is a map of named **slots**, each a local base-image file plus an optional remote source
`tailor bases download` pulls it from. An image references a slot by name with `base: { ref: <name> }`,
so the path lives once here instead of being repeated (with brittle `../` counts) in every image.

```yaml
baseImages:
  baremetal:
    path: bases/baremetal.vhdx      # build input + download output (workspace-root-relative)
    arch: amd64                     # amd64 | arm64; reconciles with the cell arch
    source:                         # optional: how `tailor bases download` fills the slot
      azureLinux:
        version: "3.0"
        variant: baremetal
  core_arm64:
    path: bases/core_arm64.vhdx
    arch: arm64
    source:
      oci:
        uri: registry.example/core:3.0
  qemu:
    path: bases/qemu.vhdx           # no source: filled out-of-band (e.g. a CI feed)
```

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `path` | path | yes | The slot file: build input **and** `download` output. Workspace-root-relative. |
| `arch` | `amd64`\|`arm64` | no | The base's architecture; reconciles with the referencing cell's arch. Absent ⇒ the cell decides. |
| `source` | `{oci}` or `{azureLinux}` | no | A remote source `download` pulls for `linux/<arch>`. Absent ⇒ pre-placed; `download` skips it. |

Fill slots with [`tailor bases download`](cli.md) and assert presence with `tailor bases verify`. See
[Use a base-image catalogue](../how-to/use-a-base-image-catalogue.md) and [Base images](../explanation/base-images.md).

## Image discovery

With no `images:` key, tailor discovers every `*/image.yaml` at depth 1 from the workspace root.

To curate explicitly:

```yaml
images:
  members:
    - "*"
    - tools
  exclude:
    - scratch
  inline:
    - name: tiny
      base:
        path: ./bases/tiny.img
      outputs:
        - format: cosi
      config:
        os:
          hostname: tiny
```

Relative paths in `tailor.yaml` resolve against the workspace root. Relative paths in an `image.yaml` resolve against that image directory.
