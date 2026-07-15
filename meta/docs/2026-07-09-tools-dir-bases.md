# tailor — `toolsDirSources`: tailor-managed IC `--tools-dir` from a container base

> **Status:** Implemented (commit `4659713`) · _2026-07-09_
>
> Wraps IC's `--tools-dir` capability the way tailor wraps IC's other flags: the user declares one or
> more **tools-dir sources** (a container image, local or remote) in `tailor.yaml`, an image
> references one by name **or defines one inline**, and tailor does the mechanical work — resolve the
> container, export its filesystem to a cache dir, bind it (**read-only by default**), and pass
> `--tools-dir <that dir>` to IC. tailor never passes `--tools-dir /`. This is the safe, first-class
> replacement for the manual `--tools-dir /` that caused the 2026-07-06 wipe.
>
> **Layering (tailor ↔ IC ↔ ACL):** `--tools-dir` is an *IC* capability. tailor wraps it generically
> for any distro base (Azure Linux, Fedora, …); tailor has **no ACL-specific knowledge**. ACL just
> happens to be the common case that needs it.
>
> **2026-07-09 update:** the `pull:` policy and local-image-`Id` machinery described below is now
> implemented for tools-dir sources as well as toolchains. Local images with a `RepoDigest` are
> lockable; local-only images without one run by image `Id` and are omitted from `tailor.lock`.

---

## 1. Problem

Some images can't be customized without an external package-manager userspace. IC's `--tools-dir`
"must contain a package manager (tdnf or dnf) and its runtime dependencies"
([IC create-tools-dir how-to](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/how-to/create-tools-dir.html));
IC uses it as a chroot, mounting the target image at `/_imageroot` and running
`tdnf --installroot=/_imageroot`. Sealed/minimal images (notably ACL, which ships no in-image `tdnf`)
require it for package / UKI-create / verity operations.

tailor has **no way to supply a tools-dir today**, so a user hits IC's "tools-dir required" gate and
is tempted to do what caused the incident: run IC by hand with **`--tools-dir /`** (point at the IC
container's own root, which has `tdnf`). That, combined with a host-root bind, is what let IC's failed
tools-chroot init reach and wipe the host. tailor must own tools-dir provisioning and make
`--tools-dir /` impossible.

The IC how-to's manual recipe is exactly the work to automate:

```bash
docker create --name t <image>
docker export t | tar -x -C <staging-dir>      # flatten the container fs to a directory
docker rm t
# ... customize --tools-dir <staging-dir>
```

---

## 2. Config surface

### 2.1 `tailor.yaml` — define tools-dir sources

A **list** of named tools-dir sources, each a container image reference. There is **no default**: an
image that needs a tools dir must name one explicitly (§2.2). Resolution reuses the toolchain
machinery (pull policy, digest pinning for remote, local-image `Id` for local-only builds — see
[toolchain resolution](./2026-06-22-design.md) / the `pull:` model).

```yaml
# tailor.yaml
toolsDirSources:
  - name: acl                       # a fully-remote source — pulled + digest-pinned (reproducible)
    container: mcr.microsoft.com/azurelinux/base/core
    tag: "3.0"
    pull: missing                   # always | missing (default) | never — same semantics as toolchains
  - name: acl-extended              # a LOCAL image with extra deps baked in (not pushed anywhere)
    container: acl-tools-extended
    tag: local
    pull: never                     # local-only: use the image Id (not locked)
  - name: fedora
    container: quay.io/fedora/fedora
    tag: "42"
```

- **`name`** identifies the source and is what an image references. **Names must be unique**;
  `tailor validate` errors on a duplicate `name`.
- **Remote source** → resolved to a `RepoDigest` and recorded in `tailor.lock` (reproducible).
- **Local source** → when the image is local-only, use its `Id` (not lockable) — the escape hatch for
  the how-to's "add extra dependencies" case (build a local image that layers packages onto a distro
  base, list it here).

The **source object** (`container` + optional `tag`/`pull`) is the reusable unit; a catalogue item is
that object plus a `name`. The same source object is what an image may define inline (§2.2).

> **Catalogue convention.** tailor catalogues are **lists whose items each carry a unique `name:`
> field** (uniqueness validated at load) — not maps keyed by name. `toolsDirSources` follows this
> form. `toolsDirSources`, `toolchains.entries`, and `baseImages` all follow this
> list-with-`name` shape for consistency.

### 2.2 `image.yaml` — select a tools dir

An image sets `toolsDir:`, an object with two fields:

- **`source:`** — mirroring `toolchain:` (`ToolchainRef`), **either a string** (the `name` of a
  `toolsDirSources` entry) **or an inline source object** (`container` + optional `tag`/`pull`).
- **`access:`** — how the tools dir is bound into the IC container: **`ro` (default)** or `rw`. Same
  `access:` vocabulary as `runtime.mounts.extraPaths` (mount-isolation design).

```yaml
# image.yaml — by name, default (read-only) access
toolsDir:
  source: acl
```

```yaml
# image.yaml — inline source, writable access
toolsDir:
  source:
    container: quay.io/fedora/fedora
    tag: "42"
    pull: missing
  access: rw
```

Omitted `toolsDir` ⇒ the image needs none and IC runs normally (no default/`true` form — a tools dir
is always explicit). A string `source` absent from `toolsDirSources` is a `tailor validate` error.

Schema: `toolsDir: Option<ToolsDir>` with
`struct ToolsDir { source: ToolsDirSourceRef, #[serde(default)] access: Access }`,
`#[serde(untagged)] enum ToolsDirSourceRef { Id(String), Inline(ToolsDirSource) }` (parallel to
`ToolchainRef`), and `Access { #[default] Ro, Rw }` (the enum shared with the mount-isolation
`access:` field).

---

## 3. What tailor does (per cell whose image sets `toolsDir`)

1. **Resolve** the referenced source (named catalogue entry or inline object) via the shared toolchain
   resolver (pull policy → digest for remote, `Id` for local). This gives a stable content key.
2. **Stage (export) once, cache by digest.** If `<image-cache-dir>/tools-dirs/<digest>/` does not
   already exist, create a throwaway container from the image and **export its flattened filesystem**
   into that dir (via the `ContainerRuntime` port — bollard `export_container` stream → untar), then
   remove the container. The how-to notes the tools dir is **reusable** across runs as long as the
   image is unchanged, so keying the cache on the digest gives correct reuse and cross-cell sharing.
   For **`access: rw`**, tailor does **not** bind the shared cache writable (IC could mutate a dir
   reused by other cells/runs); instead it materializes a **per-cell disposable copy** on the
   `buildDirBase` isolated filesystem (e.g. `<buildDirBase>/<slug>/tools-dir/`, alongside the cell's
   other scratch — a plain copy, or a copy-on-write overlay whose lowerdir is the RO cache) and binds
   that, keeping the canonical digest cache pristine. The copy is disposable and reclaimed with the
   cell's scratch, and inherits the build dir's guard + isolation (never the host root).
3. **Bind it per `access`** into the IC container under `/host`
   (per [container-mount-isolation](./2026-07-09-container-mount-isolation.md)): **read-only by
   default**; writable only when the image sets `access: rw` — and then against the per-cell copy from
   step 2, never the shared cache.
4. **Emit `--tools-dir <staged dir>`** on the IC operations that need it: `customize` (and the signed
   build's raw-intermediate `customize`) and `create`. **Not** on `convert` or `inject-files` (no
   package-manager work). The flag carries the real staged path — never `/`.

New `arg_builder` plumbing: a `tools_dir: Option<PathBuf>` threaded into the customize/create arg
builders, appended as `--tools-dir <path>` when set (mirroring how `--image-cache-dir` is handled).

---

## 4. Preview-feature gating (config-opaque, one read)

`--tools-dir` is an IC **preview feature**: IC rejects it unless the image's IC config lists
`tools-dir` in `previewFeatures` (`ErrToolsDirPreviewRequired` / `PreviewFeatureToolsDir`). tailor
stays config-opaque and does **not** rewrite `previewFeatures`. But — as with the `output-artifacts`
gate ([2026-06-29-output-artifacts-staging.md](./2026-06-29-output-artifacts-staging.md) §4, "narrowed opacity") — tailor
performs a single well-defined **read**: if an image sets `toolsDir:` but its IC config does not opt
into the `tools-dir` preview, `tailor validate` fails with a clear message rather than emitting a
flag IC will reject mid-build. tailor reads the flag; it never authors intent.

---

## 5. Safety — this is the prevention, done right

- **tailor never passes `--tools-dir /`.** The value is always a tailor-owned, digest-keyed cache dir
  under `<image-cache-dir>/tools-dirs/…`, bound **read-only by default**. `access: rw` binds a
  per-cell disposable copy of that dir — still tailor-owned, still never `/`.
- The build/tools-dir **guard** (shared with `buildDirBase`) refuses any tools dir that is `/`, an
  ancestor of `/`, on the same device as `/`, or under a host-root bind — belt-and-suspenders on top
  of "tailor chose the path."
- Because the tools dir is bound **RO by default**, an IC teardown that traverses it fails `EROFS`
  (the read-only overlay behavior confirmed in the incident reproduction) — it cannot delete the
  staged userspace, let alone the host. With `access: rw` the EROFS protection does not apply, but the
  writable target is a **small, disposable, per-cell copy** (guarded as above) — the blast radius is
  that copy, never the host and never the shared cache.
- Combined with [container-mount-isolation](./2026-07-09-container-mount-isolation.md) (no `-v /:/host`),
  the exact conditions of the incident cannot recur: there is no host root in the container, and the
  tools dir is a small cache dir, not `/`.

---

## 6. Reproducibility, lockfile, fingerprint

- **Remote** tools-dir sources are digest-pinned in `tailor.lock` (like toolchains/registry bases), so
  a locked build always stages the same userspace.
- **Local** tools-dir sources use the image `Id` (not lockable) — flagged as non-reproducible, same as
  a local toolchain image.
- The resolved tools-dir digest is a **fingerprint input** for cells that use it (a different tools
  userspace can change package resolution), so `tailor build` re-runs when it changes.
- The staged cache dir is content-addressed by digest; `tailor clean` may prune
  `<image-cache-dir>/tools-dirs/*` like any other cache.

---

## 7. Open questions

- **Extraction mechanism** — export via the `ContainerRuntime` port (bollard `export_container` +
  in-process untar, e.g. the `tar` crate) vs. shelling `docker export | tar`. In-process keeps the
  "Docker-daemon-only host" property and avoids a `tar` binary dependency; confirm bollard's export
  stream is the flattened rootfs (it is `GET /containers/{id}/export`).
- **Cross-arch tools dir** — a tools dir must match the target arch (tdnf/deps are arch-specific).
  Resolve the source per cell arch (pull `linux/<arch>`), and key the cache on `(digest, arch)`.
- **Ownership of the exported tree** — `docker export` yields root-owned files; the staged cache dir
  is reclaimed/normalized sudo-free via the janitor, like other root-owned IC outputs.

## 8. Summary

Add a `toolsDirSources` list to `tailor.yaml`: named container sources (`name` unique), local or
remote. An image sets `toolsDir:` — an object with `source:` (a string naming an entry, or an inline
source object) and `access:` (`ro` default, or `rw`). tailor resolves the container, exports its
filesystem to a digest-keyed cache dir, binds it **read-only by default** (a per-cell disposable copy
when `access: rw`), and passes `--tools-dir <dir>` to `customize`/`create`. It reuses toolchain
resolution + the lockfile, gates on the `tools-dir` preview feature with a single config read, and —
crucially — makes `--tools-dir /` a thing tailor structurally never does. Together with the
mount-isolation design, this closes the incident's root cause while giving ACL (and any other
package-manager-less base) a first-class, reproducible tools-dir.
