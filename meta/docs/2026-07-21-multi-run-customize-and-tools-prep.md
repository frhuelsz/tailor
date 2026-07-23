# tailor: chained multi-run customize + tools-dir preparation

> **Status:** Proposed · _2026-07-21_
>
> Some target images can't be produced by a single Image Customizer (IC) `customize` run. They need a
> **pipeline** of IC passes plus a **prepared package-manager tools-dir**, and (if Secure Boot is
> required) a signing stage. tailor already drives IC and already runs a multi-pass sequence for
> signing, so these are natural extensions of its model. This doc scopes the three capabilities,
> orders them by cost, and sketches how each fits tailor's config surface.
>
> Scope: **tailor capabilities only** — driving IC. Everything a base image's upstream build produces
> (kernel, rootfs, package DBs, sidecar artifacts) is an *input* tailor consumes, not something tailor
> builds; downstream publish/enrollment is out of scope.

## 1. Motivation

A single `customize` run is enough for most images: `base + config → image`. But a class of targets
needs more:

- **A prepared tools-dir.** Sealed / minimal / verity-based images ship no in-image package manager,
  so IC needs an external `--tools-dir` chroot with `tdnf`/`dnf` **and its state initialized**
  (package DB present, package-manager history seeded, signing keys imported, repo files rewritten to
  URIs the tooling can resolve). Exporting a container rootfs *as-is* isn't enough — it must be
  **prepared** first.
- **Chained customize runs.** Producing such an image can require **two or more** customize passes in
  sequence (e.g. a first pass that plants required state files, a second that installs packages and
  rebuilds the UKI + re-seals dm-verity), where each pass's output feeds the next as its input image.
- **Secure Boot signing.** IC rebuilds the UKI *unsigned*, so a signed image needs a signing stage
  after the final customize (covered by `2026-06-29-signing.md` / `2026-07-22-signing-step1-ic-native.md`).

## 2. Gap analysis vs tailor today

| Ingredient | tailor today | Gap |
| --- | --- | --- |
| Base = a local/remote VHD | `base: { path }` / catalogue slot | ✅ none |
| IC config content (previewFeatures, `uki.mode`, `reinitializeVerity`, packages, `additionalFiles`, `postCustomization`) | passed to IC **opaquely** via `config:` | ✅ none — any IC config already works |
| Extra input files (package-DB / history sidecars) | `additionalFiles` sources | ✅ (parameterize as inputs) |
| **Prepared tools-dir** (chroot: import keys, rewrite repos, install packages, seed package-manager state) | tailor exports a container rootfs **as-is** as `--tools-dir` | ⛔ **Gap 1 — tools-dir preparation** |
| **Chained customize runs** (run 1 → run 2 …) | one customize per cell (the 3-pass mode is customize→sign→inject, not N customizes) | ⛔ **Gap 2 — chained multi-run customize** |
| **Secure Boot signing** | designed; see signing docs | ⚠️ in progress separately |

So two new structural capabilities — **tools-dir preparation** and **chained multi-run customize** —
plus the already-designed signing stage.

## 3. Cost ordering

The capabilities compose, and they layer by cost:

- **Unsigned single-run customize:** tailor does this today.
- **Unsigned derivative of a prebuilt image:** "take an already-produced VHD as base and customize
  further" is just `base + customize` — tailor does this today too.
- **Signed derivative:** add only the **signing** stage to a single customize.
- **Full multi-run production from a stock base:** needs **all three** — prepared tools-dir + chained
  runs + signing.

The useful takeaway: signed derivatives are one capability away; full multi-run production is three.

## 4. Design sketch

- **Gap 2 — chained multi-run customize.** Model an image (or cell) as an **ordered sequence of IC
  customize passes**, each pass's output image feeding the next as `--image-file`. tailor's executor
  already runs a multi-pass sequence for signing, so this generalizes that into a declared pass list
  (`runs:` / stages). A derivative simply appends passes. This is the biggest structural change.
- **Gap 1 — tools-dir preparation.** Extend a tools-dir source with a **prepare step**: commands run
  inside the exported rootfs (a chroot / throwaway container) before it is bound as `--tools-dir` —
  install packages, import keys, rewrite repo files, seed package-manager state. Today tailor exports
  the container fs untouched; this adds a declarative "prepare the tools-dir" hook. (Equivalently: the
  tools-dir could be *produced by an inner build step*.) Must be cached/keyed since preparation is
  expensive.
- **Signing.** After the final customize, run the signing stage from the signing design — the
  IC-native deferred flow (`output-artifacts` → sign → `inject-files`) via a pluggable `Signer`
  backend. See `2026-07-22-signing-step1-ic-native.md`.

These compose cleanly: a signed multi-run build is `prepared-tools-dir → run1 → run2 → sign`, all
declared, all IC-driven.

## 5. Open questions

1. **Chained-runs surface:** an explicit ordered `runs:` list of IC configs per image, or a
   higher-level "stages" concept? How do matrix/params interact with a run sequence (per-run params
   vs whole-image)?
2. **Tools-dir prep surface:** a declarative command list run in the rootfs, a script reference, or
   "the tools-dir is the output of another tailor build"? How is it cached/keyed (it's expensive)?
3. **Sidecar inputs:** model required external state files (package DB / history seeds) as
   parameterized inputs, or can tailor generate them itself while preparing the tools-dir chroot
   (which is where such state is normally initialized)?
4. **Scope:** do we want tailor to produce a full multi-run image from a stock base (three
   capabilities), or only signed single-run derivatives (one)? The answer scopes the whole effort.
