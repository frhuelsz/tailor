# tailor — non-containerized run: driving a bare `imagecustomizer` binary

> **Status:** Proposed · _2026-07-15_
>
> tailor drives IC exclusively as a container today (bollard `create_and_run`, `--privileged`,
> `-v /:/host` translated paths, a sudo-free cleanup janitor). This proposes an **opt-in second
> execution mode** that runs a **bare `imagecustomizer` binary** directly as a host process — no
> container, no image. The motivating case is tailor running **inside an already-isolated,
> already-root, ephemeral environment** (a CI job container, a disposable build VM) where
> nested/rootless Docker (DinD) is painful or unavailable and the container boundary adds overhead
> with little marginal safety.
>
> **This mode is strictly less safe than the container mode** and is documented as such. The
> 2026-07-06 wipe is the reason tailor is container-first; process mode removes the boundary that
> (imperfectly) backstops it, so running it requires an explicit, deliberately-unwieldy per-build
> CLI flag and hardened guards (§4). It is **implied by the toolchain** (a `binary:` entry), not a
> separate runtime switch.

## 1. What the container gives us — and what a bare binary loses

| Concern | Container mode (today) | Process mode (proposed) |
| --- | --- | --- |
| **Path translation** | Host paths rewritten under `/host` (`path_translate::to_container_path`, `host_root=/host`) | Native paths — identity translation (`host_root=/`) |
| **Privilege** | `--privileged` inside a namespace; host user stays unprivileged | IC runs as **host root directly**; tailor must already be root |
| **Runtime deps** | Bundled in the IC image (`qemu-img`, `veritysetup`, `grub2`, `createrepo_c`, `systemd-ukify`, …) | Must all be present on the **host** PATH |
| **`/dev` / loop** | `-v /dev:/dev` into the namespace | Native `/dev`, available to root |
| **Cleanup of root-owned scratch** | Sudo-free **janitor** container reuses the IC image | tailor is already root → direct `chown`/`rm` **on the host** |
| **Blast radius** | Container boundary (a backstop, not a guarantee) | **None** — IC's own build-dir teardown `os.RemoveAll`s real host paths |

The crucial line is the last two: in process mode, tailor's own cleanup and IC's internal teardown
both run as root against real host paths with **no boundary**. That is precisely the class of the
2026-07-06 incident, so process mode leans entirely on tailor's directory guards (§4).

## 2. Why the existing abstractions make this feasible

- **Path translation already parametrizes on `host_root`.** `to_container_path(p, "/")` is the
  identity map, so the arg-builder produces native paths simply by setting `host_root = /`
  (`path_translate.rs:5`). No per-flag rework.
- **Execution is behind ports.** IC invocation goes through `tailor-core`'s port traits
  (`Executor` / `ContainerRuntime`, `ports.rs`). Process mode is a **new implementation** of the run
  path — `exec` the binary via `tokio::process::Command` instead of bollard `create_and_run` — while
  the arg vector, three-pass signing flow, and RPM farm are reused.
- **Log re-emission already has a source enum.** `LogSource` (`ic_log.rs`) tags IC output; process
  mode reuses `LogSource::ImageCustomizer` for the child process's stdout/stderr.

## 3. Config surface

Process mode is **not** a separate runtime switch — it is **implied by the toolchain**. A toolchain
entry that provides a bare `binary:` (instead of a `container:`/`build:` source) *is* a process-mode
toolchain; any cell whose selected toolchain is a `binary:` one runs as a host process. There is
**no `runtime.exec` key** — the toolchain choice already carries the mode, so a separate switch would
be redundant.

```yaml
toolchains:
  default: ic-native
  entries:
    - name: ic-native
      binary: /usr/local/bin/imagecustomizer   # bare IC binary on the host → process mode
      # `binary:` is mutually exclusive with `container:` / `build:`
```

Interactions:

- A cell is a **process-mode cell** iff its resolved toolchain has a `binary:` source. Container-mode
  and process-mode cells can coexist in one workspace (different images/toolchains); each cell runs
  in its own mode.
- Container-only runtime knobs (`privileged`, `mounts.hostRoot`, `mounts.dev`, `janitorImage`) do
  not apply to a process-mode cell (a container-mode cell in the same run still honors them).
- `buildDirBase` is still honored for process-mode cells (and more important than ever — §4).

## 4. Safety model (the heart of this design)

Process mode must be **fail-closed** and loud:

1. **Explicit per-build opt-in via a deliberately-unwieldy flag.** Selecting a `binary:` toolchain is
   not enough to *run* it. Any build that would execute a process-mode cell requires the CLI flag
   **`--allow-ic-host-process-mode`** — intentionally long and annoying so it is never muscle-memory
   or casually copy-pasted. It is a **run-time consent, never a config key**: it lives only on the
   command line, so a checked-in `tailor.yaml` can never silently enable host execution.
   - **Without the flag:** every matched cell whose toolchain is process-mode is **skipped with a
     prominent warning** (naming the cell and its toolchain), and the build **continues** for all
     container-mode cells. This is a skip, not a hard error — a mixed workspace still builds its
     container cells cleanly; process-mode cells are simply not run.
   - **With the flag:** process-mode cells run, subject to every guard below.
2. **Reuse the directory guards unconditionally.** The same fail-closed guards that protect the
   container path (`guard::ensure_safe_build_dir`, `ensure_safe_rw_target`,
   `ensure_safe_removal_parent`) gate every path tailor creates, writes, or removes — now enforcing
   an **in-process** `rm`/`chown` that has no container backstop. `--build-dir` / `--tools-dir` may
   never be `/`, an ancestor of `/`, or on the same filesystem as `/`.
3. **No janitor, but the same reclaim discipline.** tailor is already root, so cleanup is a direct
   `chown`/`rm` — restricted to **tailor-owned scratch under `buildDirBase`** and routed through a
   `cleanup:` log source. Removal still binds the *named target* under a guarded parent (the same
   invariant behind the janitor parent-bind fix), never a bare recursive delete of a broad root.
4. **Root preflight.** Fail at build start if `geteuid() != 0` (IC needs root); tailor **never
   self-escalates** — it refuses rather than invoking `sudo`.
5. **Dependency preflight.** Extend the existing fail-fast preflight (today: `openssl`/`sbsign` for
   signing) to check IC's full runtime dependency set on PATH — `qemu-img`, `veritysetup`, `grub2`,
   `mkfs.*`, `createrepo_c`, `systemd-ukify`, … — **before** any slow work, since the host no longer
   inherits them from the IC image.
6. **Recommended envelope, documented prominently.** Process mode is intended for disposable,
   already-isolated environments (CI container, throwaway VM). The docs must state that on a
   developer's real machine the container mode is the correct choice, and that process mode trades
   the last isolation layer for speed/simplicity.

## 5. Code touch-points

- `tailor-config` — a `binary:` toolchain source (mutually exclusive with `container:`/`build:`);
  **no `runtime.exec` key**. Validation only ensures `binary:` is not combined with container fields.
- CLI (`tailor` binary) — the `--allow-ic-host-process-mode` flag; the target/cell selection layer
  detects process-mode cells and, when the flag is absent, filters them out with a warning.
- `tailor-core` — the run path stays behind the port trait; the composition root picks the
  container vs process executor **per cell** based on the resolved toolchain's source.
- `tailor-exec` — a new `process_runner` module: `tokio::process::Command`, native stdout/stderr
  streamed through `ic_log`, native (`host_root=/`) arg building, in-process guarded cleanup
  replacing the janitor. Root + dependency preflight.
- Reuse unchanged: arg vector (`arg_builder`), three-pass signing, RPM farm, output/artifact
  staging.

## 6. Reproducibility / lock

A bare binary has no registry digest. Pin what is re-fetchable and stamp what is not (mirroring local
`path:` bases): if the binary comes from the commit-build feature, the lock carries the **commit
SHA**; a hand-provided `binary:` path is stamped by `(size, hash)` only, not locked. The build stamp
records the binary's fingerprint so incremental rebuilds detect a changed binary.

## 7. Risks / non-goals

- **Reduced isolation is the whole point and the whole risk.** This must never be presented as
  equivalent to container mode.
- **Host-dependency drift.** A host missing/mismatched on IC's runtime deps yields obscure IC
  failures; the dependency preflight (§4.5) converts those into a clear up-front error.
- **Non-goal:** rootless/user-namespace execution of IC — IC needs real root for loop/mount/chroot.
- **Non-goal:** making process mode the default or a fallback. It is an explicit, deliberate choice.

## Open questions

1. When the flag is absent and *every* selected cell is process-mode (so nothing is left to build),
   is that success-with-warnings or a distinct non-zero "nothing ran" exit? (Leaning: non-zero, so a
   misconfigured CI job fails loudly rather than silently doing nothing.)
2. Should the skip-with-warning be promotable to a hard error for strict callers (e.g. under
   `--locked` or an explicit `--strict`), for pipelines that would rather fail than silently drop
   targets?
