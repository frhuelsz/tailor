# `tailor.yaml` reference

`tailor.yaml` is the workspace manifest: it configures toolchains, runtime defaults, and image discovery. tailor finds it by walking up from the current directory, like Cargo.

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
| `runtime.privileged` | bool | no | Default `true`; IC requires privileged container execution. |
| `runtime.mounts.hostRoot` | path | no | Default `/host`; host `/` bind target and path-translation prefix. |
| `runtime.mounts.dev` | bool | no | Default `true`; bind `/dev:/dev`. |
| `runtime.buildDir` | string | no | Container-internal IC build dir. Default `/tmp`. |
| `runtime.logLevel` | enum | no | IC log level: `panic`, `fatal`, `error`, `warn`, `info`, `debug`, `trace`. |
| `runtime.imageCacheDir` | path | conditional | Required for `oci`/`azureLinux` bases. |
| `runtime.janitorImage` | `{container, tag?}` | no | Minimal image used for sudo-free ownership cleanup. |
| `defaults.architectures` | arch list | no | Inherited by images without `architectures`. |
| `defaults.outputs` | output list | no | Inherited by images without `outputs`. |
| `images` | object | no | Omit to auto-discover every immediate `*/image.yaml`. |

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
