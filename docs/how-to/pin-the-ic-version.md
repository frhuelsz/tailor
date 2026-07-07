# Pin the Image Customizer version

Pin Image Customizer in `tailor.yaml` under `toolchains:`.

```yaml
schemaVersion: 1

toolchains:
  default: ic-1.3
  entries:
    ic-1.3:
      container: mcr.microsoft.com/azurelinux/imagecustomizer
      version: "1.3.0"
```

`tag` defaults to `version`, or to `latest` when neither `tag` nor `version` is set. Use `tag` if the registry tag is not the version string:

```yaml
toolchains:
  default: nightly
  entries:
    nightly:
      container: mcr.microsoft.com/azurelinux/imagecustomizer
      tag: latest
```

An image can select a non-default toolchain:

```yaml
# db/image.yaml
name: db
toolchain: ic-1.3
```

## Control when tailor pulls

Each toolchain entry takes an optional `pull:` policy — a pin-aware take on Docker's `--pull` and
Kubernetes' `imagePullPolicy`:

```yaml
toolchains:
  default: local-ic
  entries:
    local-ic:
      container: acl-imagecustomizer   # a locally-built image, never pushed to a registry
      tag: local
      pull: never
```

| Policy | Behaviour |
| --- | --- |
| `missing` (default) | Use the locked digest if present, else the local image if present, else resolve + pin + pull. |
| `always` | Always resolve a fresh registry digest, pin it, and pull (best for a mutable tag like `:latest` in CI). |
| `never` | Require the image already present locally; never resolve or pull (air-gapped / local dev with a locally-built IC). |

With `missing` or `never`, a locally-built image is pinned by its `RepoDigest` (portable, written to
the lock) or its config id (local-only, tracked for drift but not written to the portable lock).

## Lock and build reproducibly

Write or refresh the lockfile:

```bash
tailor lock
# Later, deliberately refresh floating tags:
tailor update
```

Build reproducibly from the lock:

```bash
tailor build --locked
```
