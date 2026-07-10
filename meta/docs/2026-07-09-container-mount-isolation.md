# tailor — workspace-scoped container mounts (workspace read-only by default)

> **Status:** Design · _2026-07-09_
>
> Prevention design following the 2026-07-06 host wipe. tailor stops bind-mounting the whole host
> filesystem into the IC container. Instead the container sees, under a single `/host` prefix, **only
> the tailor workspace — mounted read-only by default — plus the specific out-of-workspace inputs a
> build needs and a small set of tailor-owned writable carve-outs**. Everything else on the host is
> simply absent from the container.

---

## 1. Problem

Every IC container today is launched with the entire host filesystem bound in, writable:

```
docker run --privileged -v /:/host -v /dev:/dev <ic-image> customize \
  --config-file /host/<abs> --image-file /host/<abs> --output-image-file /host/<abs> …
```

`arg_builder` translates each host path to `/host/<abs>` (`crates/tailor-exec/src/path_translate.rs`,
`to_container_path`) and binds the host root at `/host` (`arg_builder.rs`, `HOST_ROOT_SOURCE = "/"`,
`host_root_bind`).

That single bind is the structural enabler of the wipe: the real host root is present in the
container at `/host`, **writable**, so an IC operation that escapes its intended scope reaches the
whole machine. In the incident, `--tools-dir /` drove a failed `safechroot` initialization whose
cleanup ran `os.RemoveAll("/")`; because `/host` is a live writable bind *under* `/`, the delete
recursed through it into the real filesystem (root disk **and** every nested data mount). See
`meta/target/ic-tools-dir-host-wipe-footgun.md` for the proven mechanism.

The fix is to stop exposing the machine: bind only the workspace (read-only) and the few paths a
build actually reads or writes.

---

## 2. Model

Path translation is **unchanged** — a host path `P` is still presented to IC as `/host` + `P`
(`to_container_path`; `runtime.mounts.hostRoot` names the prefix, default `/host`). Only the bind set
changes, to four kinds of bind, all mapped into the `/host` namespace:

1. **Workspace root — read-only (default).** The whole tailor workspace (`workspace.root`) is bound
   `ro`. This covers the config, image dirs, config fragments, in-tree `files:`/`scripts:`, local
   bases and rpm sources that live in the tree, and any `../` reference that stays inside the
   workspace — i.e. the large majority of what a build reads, in one bind.
2. **Tailor-owned writable carve-outs.** The small set of paths tailor writes — build dir, image
   cache, output dir, `output.artifacts` staging, per-cell log — bound `rw`. When one is *inside* the
   workspace, it is a more-specific `rw` bind nested inside the `ro` workspace bind (proven viable,
   §4.2).
3. **Out-of-workspace inputs — read-only.** A local base image or rpm source that lives *outside* the
   workspace gets its own targeted `ro` bind.
4. **`extraPaths`** — an explicit escape hatch for anything else a config references outside the
   workspace; `ro` by default, `rw` only when declared (§4.4).

Mapping everything under `/host` (rather than at real paths) keeps the mirror from ever colliding with
the IC container's own root filesystem (`/etc`, `/usr`, `/tmp`, …): a host path `/etc/foo` lands at
`/host/etc/foo`, never shadowing the container's `/etc`.

The blast radius of any IC misbehavior is now bounded to the workspace (read-only) plus the handful of
disposable, tailor-owned writable carve-outs — never the host root.

---

## 3. Mechanics

### 3.1 Workspace root, read-only

```
-v /work/proj:/host/work/proj:ro          # workspace.root (contains configs, image dirs, in-tree inputs)
```

On modern engines (Docker 25+, podman on kernel ≥ 5.12) a `:ro` bind is **recursively read-only**:
nested mounts under the workspace inherit `ro` automatically (verified on Docker 29.6.1 / kernel 7.0 —
a nested tmpfs under the bind was read-only in the container). This is the desired behavior and tailor
relies on it by default.

**If the engine/kernel does not provide recursive-RO**, a filesystem mounted *inside* the workspace
could surface writable in the container despite the `ro` parent. This is a bounded residual — the
worst case is a writable mount *within the workspace tree*, never the host root — and is acceptable
given it strictly minimizes blast radius versus today's whole-host-RW bind. As optional hardening,
tailor may request recursive-RO explicitly
(`--mount type=bind,…,readonly,bind-recursive=readonly,bind-propagation=rprivate`, verified working);
this is not required for the safety guarantee.

### 3.2 Writable carve-outs (nested in the RO workspace)

A more-specific `rw` bind nested inside the `ro` workspace bind is valid and behaves as needed — the
parent stays read-only, only the carve-out is writable, and writes propagate to the host (verified on
Docker 29.6.1: `-v ws:/host/ws:ro -v ws/carve:/host/ws/carve:rw` → root RO, carve-out writable):

```
-v /work/proj:/host/work/proj:ro
-v /work/proj/out:/host/work/proj/out:rw                      # output dir (inside workspace)
-v /work/proj/.tailor/cache:/host/work/proj/.tailor/cache:rw  # image cache (inside workspace)
-v /work/proj/img/gizmo/.tailor-stage.gizmo.<run>:…:rw        # output.artifacts staging (§3.4)
```

Carve-outs that live *outside* the workspace (a `buildDirBase` on a separate disk, an external log
dir) are simply their own top-level `rw` binds under `/host`.

### 3.3 Out-of-workspace inputs (read-only)

A local base or rpm source outside the workspace is bound `ro` at its own path:

```
-v /data/bases/gizmo.vhdx:/host/data/bases/gizmo.vhdx:ro
-v /srv/rpms:/host/srv/rpms:ro
```

Inputs *inside* the workspace need no extra bind — they are covered by §3.1.

### 3.4 `runtime.mounts.extraPaths` — declaring additional paths

For anything a config references that is outside the workspace and not auto-derived, the tailor config
declares it. **Read-only by default; writable only when `access: rw` is set:**

```yaml
runtime:
  mounts:
    extraPaths:
      - path: /opt/shared-scripts        # referenced from a config, outside the workspace
        # access defaults to ro
      - path: /data/scratch
        access: rw                       # writable only because explicitly requested
```

Each entry becomes a `-v <path>:/host<path>:<ro|rw>` bind. **Relative paths are allowed**, resolved
against the **workspace root** — consistent with the sibling `runtime.imageCacheDir`, which
absolutizes against `workspace.root` (`resolve_image_cache_dir(…, &workspace.root)`). Absolute paths
are used as-is; **cwd is never the anchor** (a relative bind source is rejected by the engine). tailor
absolutizes each entry before emitting its bind. Writable extra paths pass through the guard (§3.5).

### 3.5 Build dir isolation + fail-closed guard

The build dir is the one large RW area and the one IC recursively deletes. It must be an isolated,
tailor-owned RW path on a filesystem that is **not** the host root, resolved from
`runtime.buildDirBase/<slug>` (the reconstructed `buildDirBase` feature) and bound only as itself.

Before launching, a **guard canonicalizes** the resolved build dir, any `access: rw` carve-out or
extra path, and any future tools dir, and **refuses to run** if any is `/`, an ancestor of `/`, on the
**same device** as the running root filesystem, or otherwise resolves to (part of) the host root.
tailor **never** constructs `--tools-dir /`. With no whole-host bind this is belt-and-suspenders, but
it closes the door explicitly and protects user-supplied `rw` paths.

### 3.6 Relative-to-config resolution

IC resolves a config's relative `files:`/`scripts:` entries against the config file's directory. Under
this model that works unchanged: the config lives at `/host/<image-dir>/…`; in-directory relatives
resolve within the workspace bind, and `../` references resolve correctly as paths because `/host`
mirrors the real layout — covered by the workspace bind while they stay in the tree, and by
`extraPaths` when they escape it.

---

## 4. `/dev`

IC loopback-mounts the image (`losetup`) and needs device nodes, so `/dev` access is required for a
real privileged build. tailor keeps `-v /dev:/dev` (the current default) for compatibility. `/dev` is
device nodes, not the host filesystem tree, so it is not a data-loss vector the way `/` is.
`runtime.mounts.dev` remains the on/off escape hatch.

---

## 5. Config & compatibility

`runtime.mounts`:

```yaml
runtime:
  mounts:
    hostRoot: /host        # the /host namespace prefix (default /host); paths are mapped under it
    dev: true              # bind /dev (default true)
    extraPaths:            # additional out-of-workspace paths (RO unless access: rw)
      - path: /opt/shared-scripts
      - path: /data/scratch
        access: rw
```

- `hostRoot` and `dev` are retained. `hostRoot` no longer means "bind the whole host here" — it is the
  prefix of the mapped namespace.
- `extraPaths` is new: a list of `{ path, access: ro|rw }`, `access` defaulting to `ro`.
- No whole-host bind is emitted; `host_root_bind` (the `/:/host` volume) is removed. `arg_builder`
  instead emits the computed bind set (workspace RO + carve-outs + out-of-workspace inputs + extras).
  `to_container_path` and the `/host` prefix are unchanged.
- The janitor's own chown/rm binds are unaffected (they already bind specific tailor-owned paths).
- `--dry-run` renders the full computed `-v` set, making the exposed surface inspectable.

---

## 6. Why this prevents the incident

- **The host root is never in the container.** Only the workspace (RO) and explicitly mapped paths
  exist under `/host`. A stray `os.RemoveAll` — even `RemoveAll("/")` inside the container — cannot
  reach a host path tailor did not map.
- **The workspace and all inputs are read-only.** A stray delete over them fails `EROFS` (as the
  read-only overlay did in the reproduction), so configs, the source tree, bases, and rpm sources
  survive.
- **The writable set is small, tailor-owned, disposable.** The worst a runaway can do is delete inside
  the build, cache, output, staging, or log dir — none of them the host root. Blast radius drops from
  "the whole machine" to "this build's scratch."
- **The build dir (and any RW path) is isolated and guarded**, so no writable target can be the host
  root or on its filesystem.

---

## 7. Open questions

- **podman parity** — verify podman honors (a) recursive read-only on a `:ro` bind and (b) a
  more-specific `rw` bind nested inside a `ro` bind, matching the Docker behavior verified here. If it
  differs, request recursive-RO explicitly and/or relocate the staging carve-out.
- **Recursive-RO on old engines** — on pre-25 Docker / kernel < 5.12, a nested mount under the
  workspace may surface writable; decide whether to detect and warn, or accept the bounded residual.
- **Many scattered out-of-workspace inputs** — a build referencing many external dirs yields many
  binds; functionally fine, but consider a cap / warning.
- **SELinux `:z` / `:Z` relabeling** — decide whether shared binds need relabeling under this model;
  RO input binds should not be relabeled.
