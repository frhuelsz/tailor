# tailor — targeted container mount isolation (drop the full `-v /:/host` bind)

> **Status:** Design · _2026-07-09_
>
> Prevention design following the 2026-07-06 host wipe. Replaces tailor's single "bind the whole
> host root into the IC container" mount with a **minimal set of purpose-scoped binds, read-only by
> default**. The goal is that IC — even if it misbehaves in the exact way that caused the incident
> (a teardown `rm -rf`/`os.RemoveAll` traversing a live mount) — **cannot see or delete anything on
> the host except the few directories a build genuinely needs to write.**

---

## 1. Problem

Today every IC container is launched with the **entire host filesystem** bind-mounted in:

```
docker run --privileged -v /:/host -v /dev:/dev <ic-image> customize \
  --config-file /host/<abs>  --image-file /host/<abs>  --output-image-file /host/<abs> ...
```

`arg_builder` path-translates every host path to `/host/<abs>` (`crates/tailor-exec/src/path_translate.rs`),
and the executor binds `/:/host` + `/dev:/dev` (`arg_builder.rs`, `host_root_bind`/`DEV_BIND`).

This is the **root enabler of the wipe** (`meta/target`/incident analysis): the host root is present
in the container at `/host`, writable, so any IC file operation that escapes its intended scope —
notably the teardown `os.RemoveAll` of a build/chroot dir that still has a live mount under it — can
reach and destroy the whole host. The full bind is a blast radius the size of the machine.

**tailor does not need the whole host.** It needs a handful of specific paths, and most of them are
**inputs it only reads.**

---

## 2. What a build actually touches

Enumerated from the IC arg vectors `arg_builder` emits (customize / convert / inject-files):

| Purpose | Host path source | IC flag | Access |
| --- | --- | --- | --- |
| Merged IC config (working copy) | `<image-dir>/.tailor-render.<slug>.ic.yaml` | `--config-file` | **RO** |
| Relative `files:` / `scripts:` in the config | `<image-dir>/…` | (resolved by IC) | **RO** |
| Local base image | `base.path` (a file) | `--image-file` | **RO** |
| RPM sources | each `rpmSources` entry (dir/file) | `--rpm-source` | **RO** |
| Tools dir (future, §see tools-dir design) | staged tools tree | `--tools-dir` | **RO** |
| Output image | `<output-dir>` | `--output-image-file` | **RW** |
| Image cache (registry bases) | `runtime.imageCacheDir` | `--image-cache-dir` | **RW** |
| Build/scratch | `runtime.buildDirBase/<slug>` | `--build-dir` | **RW** |
| `output.artifacts` staging | `<image-dir>/.tailor-stage.<slug>.<run-id>` | (relocated by tailor) | **RW** |
| Per-cell IC log | `runtime.logDir` | `--log-file` | **RW** |
| Device access (loopback mount of the image) | `/dev` | — | **RW** (see §5) |

Everything above the output rows is an **input tailor only reads**. The writable set is small and
tailor-owned: output dir, cache, build dir, staging, log dir.

---

## 3. Design — minimal, identity, RO-by-default binds

### 3.1 One bind per needed path, at its real path (identity), no `/host`

Replace `-v /:/host` + `/host/<abs>` translation with **identity binds**: each needed host directory
is bound into the container **at the same absolute path**, so the arg vector passes the real path and
**no path translation is needed** (`path_translate::to_container_path` collapses to identity and can
be retired). This mirrors what the janitor already does for its sudo-free chown/rm
(`crates/tailor-exec/src/janitor.rs`, `"{path}:{path}"`).

```
docker run --privileged \
  -v /work/img/gizmo:/work/img/gizmo:ro \          # image dir (config + relative files/scripts)
  -v /work/bases/gizmo.vhdx:/work/bases/gizmo.vhdx:ro \   # base image (if outside the image dir)
  -v /work/rpms:/work/rpms:ro \                    # each rpm-source parent
  -v /data/tailor-build/gizmo_amd64_cosi:/data/tailor-build/gizmo_amd64_cosi:rw \   # build dir (isolated fs)
  -v /work/out:/work/out:rw \                      # output dir
  -v /work/.tailor/cache:/work/.tailor/cache:rw \  # image cache
  -v /dev:/dev \                                   # device access (see §5)
  <ic-image> customize --config-file /work/img/gizmo/.tailor-render.….yaml …
```

Binds are **read-only unless the path is a tailor-owned write target** (output, cache, build,
staging, log). Directories (not individual files) are bound, at their parent when a single file is
needed, to keep the set small.

### 3.2 Computing the bind set

The executor derives the set from the same resolved inputs it already has per cell:

1. Collect `(path, access)` requests: image dir (RO), base file's parent (RO) if outside the image
   dir, each rpm-source parent (RO), tools dir (RO), output dir (RW), cache dir (RW), build dir (RW),
   log dir (RW), staging dir (RW).
2. **Absolutize** every path (already done, `meta` path-resolution memory) — a relative bind source
   is rejected by Docker.
3. **Normalize + dedupe + resolve nesting:** if path B is inside an already-bound path A —
   - same access, or A is RW ⊇ B: drop B (covered).
   - B needs RW but A is RO (e.g. the staging dir inside the RO image dir): keep **both**, with the
     more-specific RW bind **nested** inside the RO one. Docker honors the most-specific bind, so the
     source tree stays RO while the one staging subdir is writable (§3.3).
4. Emit `-v <path>:<path>:<ro|rw>` for each, sorted shortest-first so parents mount before children.

### 3.3 The `output.artifacts` staging carve-out

`output-artifacts-staging.md` §3.2 keeps IC's staging **colocated** in the image dir (so IC resolves
its relative `output.artifacts.path` there). With the image dir now **RO**, IC cannot write the
staging. Resolution: keep the image dir bind **RO**, and add a **nested RW bind** for exactly the
tailor-named staging dir `<image-dir>/.tailor-stage.<slug>.<run-id>` (tailor knows the name before
IC runs). The rest of the source tree remains read-only; only the staging subdir is writable — and
it is reclaimed after the run as today.

### 3.4 Build dir isolation (ties into `buildDirBase`)

The build dir is the one large RW area and the one IC recursively deletes on teardown. It **must** be
an isolated, tailor-owned RW bind on a filesystem that is **not** the host root:

- Resolved from `runtime.buildDirBase/<slug>` (the reconstructed `buildDirBase` feature), on a
  dedicated volume / disk / tmpfs.
- Bound **only as itself** (`-v <build>:<build>:rw`) — never as a child of a host-root bind, because
  there is no host-root bind anymore.
- A **guard refuses to run** if the resolved build dir (or tools dir) is `/`, an ancestor of `/`, on
  the same device as `/`, or — the incident's exact shape — a path that is itself a bind of the host
  root. This is belt-and-suspenders: with targeted binds there is no `/host` to nest under, but the
  guard closes the door explicitly.

---

## 4. Why this prevents the incident

The wipe required the host root to be present, writable, in the container, with a build/mount tree
sitting on it. Under this design:

- **There is no `/host`.** The host root is never bound in. IC cannot address host paths that tailor
  did not explicitly, individually expose.
- **Inputs are read-only.** The base image, the source/image dir, and rpm sources are RO — IC's
  teardown `os.RemoveAll` over any of them fails `EROFS` (exactly as the read-only ACL overlay did in
  the reproduction), so a stray delete cannot destroy inputs or the source tree.
- **The writable set is small and tailor-owned.** The worst a runaway teardown can do is delete
  inside the build dir, staging dir, output dir, cache, or log dir — all disposable, tailor-managed,
  and none of them the host root. The blast radius shrinks from "the whole machine" to "this build's
  scratch."
- **The build dir is isolated + guarded**, so even that RW area cannot be the host root.

---

## 5. `/dev` — the one remaining broad mount

IC loopback-mounts the image (`losetup`) and needs device nodes, so `/dev` access is required for a
real (privileged) build. Options, least-to-most invasive:

- **Keep `-v /dev:/dev`** (default today). `/dev` is device nodes, not the host filesystem tree, so it
  is not a data-loss vector the way `/` is — but it is still broad.
- **Preferred:** rely on the container runtime's own `/dev` (Docker gives the container a minimal
  `/dev`) plus `--device-cgroup-rule` / dynamic `--device` for the specific loop device, so no host
  `/dev` bind at all. Feasibility depends on loop-device allocation inside the container; validate
  against a real ACL build before committing.
- `runtime.mounts.dev` stays as the escape hatch to force the bind on/off.

This is called out as an **open question** (§7) rather than decided, because it needs a live test.

---

## 6. Config & compatibility changes

- **`runtime.mounts.hostRoot` is removed** (there is no host-root bind). `deny_unknown_fields` rejects
  it with a message pointing at this model. `runtime.mounts.dev` stays (§5).
- **`path_translate::to_container_path` is retired** (identity binds). The `RuntimeConfig.host_root`
  field and every `/host/<abs>` construction in `arg_builder` are removed; the arg vectors carry real
  absolute paths.
- The janitor already uses identity binds, so its chown/rm sweep is unaffected (and it, too, should
  bind only the specific managed paths, not `/host`).
- `--dry-run` renders the new, narrower `-v` set — making the isolation visible/inspectable.

---

## 7. Open questions

- **`/dev` without a full bind (§5)** — can IC allocate/attach its loop device with only the
  container's own `/dev` + a device-cgroup rule? Needs a live ACL build to confirm.
- **Overlapping RW-in-RO nesting** — confirm the target runtime (Docker + podman) both honor a
  more-specific RW bind nested inside a RO bind for the staging carve-out (§3.3); if podman differs,
  fall back to relocating staging to a dedicated RW dir and teaching IC's relative resolution via a
  symlink/`--output-artifacts`-path override.
- **Base images / rpm sources on many scattered paths** — a build referencing dozens of distinct host
  dirs yields many binds; fine functionally, but consider a cap / warning.
- **SELinux/`:z`/`:Z` relabeling** — decide whether shared binds need `:z` (they did not under the
  single `/host` bind); RO binds should not be relabeled.

## 8. Summary

Stop h-mounting the machine. Bind only what a cell reads or writes, read-only wherever possible, at
real paths (no `/host`, no translation). Inputs (base, source tree, rpm sources, tools dir) are RO;
the small writable set (build, staging, output, cache, log) is tailor-owned and disposable; the build
dir is isolated and guarded. This removes the structural condition that let an IC teardown delete the
host, and shrinks the blast radius of any future IC misbehavior to a single build's scratch.
