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
