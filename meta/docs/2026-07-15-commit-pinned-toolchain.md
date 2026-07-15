# tailor — commit-pinned toolchain: build IC from a git commit

> **Status:** Proposed · _2026-07-15_
>
> Today a toolchain names a **prebuilt** Image Customizer (IC) container that tailor pulls from a
> registry (or uses locally with `pull: never`). This proposes a second kind of toolchain **source**:
> a **git commit** of the IC repo. tailor clones that commit, builds the `imagecustomizer` binary, and
> assembles a runnable container around it — then consumes the result exactly like a local
> `pull: never` toolchain. The whole build runs **inside Docker**, so tailor's dependency footprint
> stays "just a container engine" (no host Go/make/git).
>
> **Layering (tailor ↔ IC ↔ ACL):** producing the IC container is an *IC* build concern; tailor only
> automates the mechanical clone → compile → assemble → tag pipeline that a user otherwise runs by
> hand. tailor never reasons about what a given IC commit can or cannot do — the commit is just a
> pinned, reproducible input (mirroring how `version` is informational, `2026-06-22-design.md` §8).

## 1. Motivation

Consuming an unreleased or forked IC (e.g. an ACL-pipeline build, or a PR under test) means:

1. clone `azure-linux-image-tools` at some commit,
2. compile the Go `imagecustomizer` binary,
3. wrap it in a container (either the canonical self-contained build, or a binary swap over the
   official image), and
4. wire that local tag into `tailor.yaml` as a `pull: never` toolchain.

Steps 1–3 are today out-of-band shell/`just` glue the user maintains separately. This makes the IC
version a first-class, **commit-pinned, reproducible** toolchain input that tailor owns end to end.

## 2. How IC's container is built upstream (ground truth)

The canonical build is two phases (`toolkit/tools/imagecustomizer/container/` in
`microsoft/azure-linux-image-tools`):

- **binary** — `make` under `toolkit/` compiles the Go binary to `toolkit/out/tools/imagecustomizer`.
  The version string embeds `+<GIT_COMMIT_ID>` (`toolkit/scripts/build_tag_imagecustomizer.mk`).
- **container** — `build-container.sh -t <tag> -a <arch>` stages the binary + licenses + telemetry +
  entrypoint into a context dir, then `docker build`s `imagecustomizer.Dockerfile`
  (`FROM mcr.microsoft.com/azurelinux/base/core:3.0`, `tdnf install` the runtime deps IC shells out
  to — `qemu-img`, `veritysetup`, `grub2`, `createrepo_c`, `systemd-ukify`, …, `COPY usr /usr`,
  entrypoint runs `imagecustomizer "$@"`). The script drops `grub2-pc` on arm64.

A common lighter variant (observed in ACL workflows) is a **binary swap**: `FROM` the *official* IC
container and `COPY` a freshly built binary over `/usr/bin/imagecustomizer`, adding only the deps the
official image lacks (e.g. `systemd-ukify` on arm64). It reuses the official image's whole runtime
environment, so it is much faster, at the cost of tracking an official base tag.

## 3. Config surface

A toolchain entry gains a **source**: the existing `container:` (registry) **XOR** a new `build:`
block. This mirrors `BaseSource`'s untagged-variant style (`schema.rs`).

```yaml
toolchains:
  default: ic-dev
  entries:
    - name: ic-dev
      build:
        git: https://github.com/microsoft/azure-linux-image-tools   # or path: ../ic-checkout
        commit: 3f2a1b0c9d…        # pinned SHA (reproducible); or ref:/branch: for convenience
        strategy: full             # full (default) | binarySwap
        # baseTag: "3.0"           # full: base/core tag · binarySwap: official IC tag to layer on
        # extraPackages: [systemd-ukify]   # extra tdnf packages (binarySwap arm64 needs ukify)
      # No `container:` / `pull:` — a built image is local; pull is implicitly `never`.
```

Rules:

- `container:` and `build:` are mutually exclusive; exactly one is required per entry.
- `build.git` (remote) **XOR** `build.path` (a local IC working tree) as the source.
- With `build:`, `pull` is implicitly `never` (the image is produced locally, not fetched); an
  explicit `pull:` is rejected.
- `commit` accepts a full SHA (preferred, reproducible). `ref:`/`branch:` are conveniences resolved
  to a SHA **at build time** and captured into the lock (§5).

## 4. Mechanics — Docker-only

tailor generates a **multi-stage Dockerfile** + build context and drives `docker build` via bollard.
The clone + checkout + `make` happen **inside the builder stage**, so the host needs only Docker:

- **builder stage** — `FROM` a golang/build image; `RUN git clone <git> && git -C … checkout
  <commit> && make …` → the binary. `commit` is passed as a `--build-arg` so a new commit busts the
  clone/compile layer cache.
- **runtime stage** —
  - `strategy: full`: reproduce `imagecustomizer.Dockerfile` (`FROM base/core:<baseTag>`, tdnf
    runtime deps + `extraPackages`, `COPY` the built binary + entrypoint, arm64 `grub2-pc` drop).
  - `strategy: binarySwap`: `FROM mcr…/imagecustomizer:<baseTag>`, `COPY` the binary over
    `/usr/bin/imagecustomizer`, `tdnf install <extraPackages>`.

The result is tagged `tailor-ic/<commit-short>` (or a user-supplied `tag`). From there it flows
through the **existing** local-image path unchanged: arch preflight, `--platform linux/<arch>`,
run, janitor cleanup. `local.path` sources build from that working tree as the context (closest to
an IC developer's edit-compile-run loop).

## 5. Caching, reproducibility & the lockfile

- **Content-addressed by commit.** The local tag encodes the resolved SHA. If the image already
  exists locally, skip the build (a `missing`-like check); a `--force` / `--rebuild` flag forces it.
- **Lockfile.** tailor.lock pins the **git commit SHA** (re-fetchable from git) plus the build inputs
  that change the output (`strategy`, `baseTag`, `extraPackages`). A `ref:`/`branch:` source resolves
  to a SHA at build and that SHA is written to the lock. The built image's local `Id` is a build
  **stamp**, not a lock entry — it is not re-fetchable, exactly as local `path:` bases are handled
  (`2026-06-22-design.md` §9; `ports.rs` `ResolvedBase::LocalFile`).
- **Fingerprint.** The toolchain fingerprint component (`2026-06-22-design.md` §8) becomes the commit SHA +
  strategy + baseTag instead of a registry digest, so a commit bump re-triggers dependent cells.
- **Repro caveat (documented):** a source build pins IC's *source*, not its full dependency closure
  (`tdnf` packages float). For byte-reproducibility, also pin `baseTag` by digest. This is a weaker
  guarantee than a digest-pinned official container and must be stated plainly.

## 6. Code touch-points

- `tailor-config` — a `ToolchainSource { Container(...) | Build(...) }` enum; validation of the
  XOR/`pull` rules; `effective_tag` gains a build-derived variant.
- `tailor-exec` — a **new image-builder module**. tailor currently only *runs* containers; building
  an image means adding a `build_image` method to the `ContainerRuntime` trait (`ports.rs:176`);
  bollard supports it. Build progress streams through a **new `build:` log source** (same mechanism
  as the `janitor:` attribution fix — `ic_log.rs`, `LogSource`).
- `tailor-resolve` — a `Build` arm of `resolve_toolchain` that computes the tag, checks local
  presence, resolves a ref→SHA when needed, and drives the build if absent/forced.
- Build scratch (generated Dockerfile + context) lives under the same safe scratch discipline as
  other build dirs and is reclaimed by the janitor. Note this build is **not** privileged and has
  **no** host-root mount, so it is far lower blast-radius than an IC run.

## 7. Risks / edge cases

- **Slow, heavy** (Go compile + `tdnf`). Mitigated by commit-keyed caching + live `build:` progress.
- **Cross-arch.** Source builds realistically want a native-arch host (the same constraint IC runs
  already hit). Default to host arch; honor `--platform` and the arm64 `grub2-pc` fixup. Emulated
  cross-builds are out of scope for phase 1.
- **Build-time network.** Clone + `tdnf` need network; `pull: never` semantics do not map to a
  from-source build. Document a build-network expectation (distinct from run-time pull policy).
- **Private / forked repos.** `build.git` may need credentials. Support a BuildKit `--secret`
  (token/SSH) and the `build.path` local-checkout escape hatch. Never bake secrets into a layer.

## 8. Phasing

- **Phase 1:** `build: { git, commit }`, `strategy: full`, host-arch, commit-derived tag, implicit
  local/`never`, rebuild-if-missing + `--force`, `build:` log source, lock the SHA.
- **Phase 2:** `strategy: binarySwap`, `build.path` local source, `ref:`/`branch:`→SHA lock capture,
  private-repo secrets, richer cross-arch story.

## Open questions

1. Default strategy: **full** (self-contained, matches upstream) vs **binarySwap** (fast, matches the
   ACL binary-swap flow)?
2. Should a `ref:`/`branch:` source be allowed under `--locked`, or must locked builds carry an
   explicit SHA?
