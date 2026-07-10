# Use a tools dir for sealed images

Some sealed/minimal bases do not contain `tdnf`/`dnf`, so IC needs an external tools dir. Declare a
named source in `tailor.yaml`:

```yaml
toolsDirSources:
  - name: acl
    container: mcr.microsoft.com/azurelinux/base/core
    tag: "3.0"
```

Then opt in from the image:

```yaml
toolsDir:
  source: acl

config:
  previewFeatures:
    - tools-dir
```

tailor resolves the source digest, exports it to `runtime.imageCacheDir/tools-dirs/<digest>`, binds it
read-only, and passes the translated path to IC customize passes. It never emits `--tools-dir /`.

Use writable access only when IC must mutate the tools dir:

```yaml
toolsDir:
  source: acl
  access: rw
```

`access: rw` requires `runtime.buildDirBase`; tailor copies the shared cache to a per-cell disposable
`<buildDirBase>/<slug>/tools-dir` and binds that copy writable.
