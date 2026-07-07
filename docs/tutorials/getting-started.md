# Getting started

In this tutorial you will create one standalone image definition and render the Image Customizer invocation without building an image.

## 1. Install tailor

Use a release binary or install from git:

```bash
cargo install --git https://github.com/frhuelsz/tailor tailor
```

Check it:

```bash
tailor --version
```

Expected shape:

```text
tailor <version>+<commit>.<date>
```

## 2. Scaffold a standalone image

Create a new empty directory, then run:

```bash
mkdir solo-demo
cd solo-demo
tailor init solo simple
```

Expected output includes:

```text
created .../solo-demo/image.yaml
Scaffolded standalone image `solo`. Try: tailor validate
```

The `simple` template creates only `./image.yaml`; there is no `tailor.yaml`. In standalone mode tailor uses its built-in default Image Customizer toolchain: `mcr.microsoft.com/azurelinux/imagecustomizer:latest`.

## 3. Read the image definition

Open `image.yaml` (the scaffold also includes explanatory comments, trimmed here):

```yaml
name: solo

outputs:
  - format: cosi

base:
  azureLinux:
    version: "3.0"
    variant: minimal-os

config:
  previewFeatures:
    - input-image-oci   # lets IC download the azureLinux/oci base
  os:
    hostname: solo
    bootloader:
      resetType: hard-reset
    packages:
      install:
        - openssh-server
    services:
      enable:
        - sshd
  # minimal-os ships tight on free space, so the scaffold grows the rootfs.
  # (Repartitioning is why os.bootloader.resetType: hard-reset is required.)
  storage:
    bootType: efi
    disks:
      - partitionTableType: gpt
        maxSize: 4G
        partitions:
          - id: esp
            type: esp
            size: 8M
          - id: rootfs
            size: grow
    filesystems:
      - deviceId: esp
        type: fat32
        mountPoint:
          path: /boot/efi
          options: "umask=0077"
      - deviceId: rootfs
        type: ext4
        mountPoint:
          path: /
```

Top-level keys (`name`, `outputs`, `base`, …) are tailor's. Everything under `config:` is Image
Customizer configuration and is passed through opaquely — tailor never interprets it.

## 4. Validate

```bash
tailor validate
```

Expected shape:

```text
✓ solo                         1 cell(s) valid
```

## 5. Dry-run the build

```bash
tailor build --dry-run
```

Expected shape:

```text
1 cell(s) (dry-run)
...
imagecustomizer ... --config-file ... --output-image-format cosi ...
```

`--dry-run` renders the container/Image Customizer invocation without starting the container. Remove `--dry-run` when you have Docker daemon access and want to build the artifact.

## Next step

Learn matrix builds in [Your first matrix](your-first-matrix.md).
