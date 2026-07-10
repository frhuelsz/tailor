# `tailor.yaml` — workspace / tool config

The **workspace root**. It holds repo-wide settings — the Image Customizer **toolchain(s)**, the
container-runtime knobs, and **defaults** applied to every image — plus the image catalogue. Found by
walking **up** from the current directory (Cargo-style); its directory is the workspace root, and all
relative paths inside it resolve against that directory.

Schema: [`#/$defs/ToolConfig`](../tailor.schema.json).

## Top-level fields

| Field | Type | Req | Default | Notes |
| ----- | ---- | --- | ------- | ----- |
| `schemaVersion` | int (`1`) | **yes** | — | Manifest schema version. Only `tailor.yaml`/`tailor.lock` carry it. |
| `toolchains` | [Toolchains](#toolchains) | **yes** | — | The IC version(s). This is where the IC version lives. |
| `runtime` | [Runtime](#runtime) | no | (built-in) | bollard/Docker knobs reproducing IC's invocation contract. |
| `defaults` | [Defaults](#defaults) | no | — | Field defaults inherited by every image. |
| `images` | [ImageCatalogue](#images) | no | auto-discover | Which images are members. Omit ⇒ auto-discover `*/image.yaml` at depth 1. |

## toolchains

The Image Customizer container(s), repo-wide. Each entry resolves to a registry **digest** pinned in
`tailor.lock`.

| Field | Type | Req | Notes |
| ----- | ---- | --- | ----- |
| `default` | string (name) | **yes** | Toolchain name used by any image that omits `toolchain:`. |
| `entries` | list of [ToolchainEntry](./types.md#toolchainentry) | **yes** | One or more pinned IC containers (≥ 1); each `name` must be unique. |

```yaml
toolchains:
  default: ic-1.3
  entries:
    - name: ic-1.3
      container: mcr.microsoft.com/azurelinux/imagecustomizer
      version: "1.3.0"          # optional, informational; omit to track `latest`
    - name: ic-1.1
      container: mcr.microsoft.com/azurelinux/imagecustomizer
      version: "1.1.0"
```

An image selects one with the top-level [`toolchain:`](./image-yaml.md#top-level-fields) field
(`toolchain: ic-1.1`), or omits it to use `default`. See [ToolchainRef](./types.md#toolchainref).

## runtime

Container-runtime settings. Sensible defaults; rarely overridden.

| Field | Type | Req | Default | Notes |
| ----- | ---- | --- | ------- | ----- |
| `privileged` | bool | no | `true` | IC requires a privileged container. |
| `mounts.hostRoot` | string | no | `/host` | Container-side mount of host `/`; the prefix for all host→container path translation. |
| `mounts.dev` | bool | no | `true` | Bind `/dev:/dev`. |
| `buildDir` | string | no | `/tmp` | IC `--build-dir` (container-internal; not host-translated). |
| `logLevel` | [LogLevel](./types.md#loglevel) | no | `info` | IC `--log-level`. |
| `imageCacheDir` | string (host path) | cond | — | Host dir for IC `--image-cache-dir` (caches registry-downloaded base images). Needed only when an image uses an `oci`/`azureLinux` base; tailor may auto-default it (e.g. `./.tailor/cache`). |
| `janitorImage` | {`container` (req), `tag`?} | no | a busybox-class image | Digest-pinned minimal image for sudo-free ownership/cleanup of IC's root-owned outputs. |

```yaml
runtime:
  imageCacheDir: ./.tailor/cache   # required here because the images use azureLinux (MCR) bases
```

## defaults

Applied to any image that does not set the field itself.

| Field | Type | Req | Notes |
| ----- | ---- | --- | ----- |
| `architectures` | [[Arch](./types.md#arch)] | no | Inherited by images lacking `architectures:`. |
| `outputs` | [[OutputSpec](./types.md#outputspec)] | no | Inherited by images lacking `outputs:` (≥ 1). |

```yaml
defaults:
  architectures: [amd64]
  outputs:
    - format: cosi
```

## images

Which images belong to the workspace. **Omit the whole key** to auto-discover every `*/image.yaml`
at depth 1 (the common case). Provide it to curate the set or to inline a trivial image.

| Field | Type | Req | Default | Notes |
| ----- | ---- | --- | ------- | ----- |
| `members` | [string] (paths/globs) | no | `["*/"]` | Member directories (each with an `image.yaml`) or `image.yaml` files, relative to `tailor.yaml`. |
| `exclude` | [string] (paths/globs) | no | — | Members to drop from the discovered/listed set. |
| `inline` | [[ImageDefinition](./image-yaml.md)] | no | — | Trivial images defined directly here (no own directory). |

```yaml
# images:                          # omitted entirely above → auto-discovery
#   members: [./webserver/, ./database/]
#   exclude: [./scratch/]
```

## Full example

```yaml
schemaVersion: 1

toolchains:
  default: ic-1.3
  entries:
    - name: ic-1.3
      container: mcr.microsoft.com/azurelinux/imagecustomizer
      version: "1.3.0"
    - name: ic-1.1
      container: mcr.microsoft.com/azurelinux/imagecustomizer
      version: "1.1.0"

runtime:
  imageCacheDir: ./.tailor/cache

defaults:
  architectures: [amd64]
  outputs:
    - format: cosi
```

(From [`examples/workspace-two-images`](../../examples/workspace-two-images).)
