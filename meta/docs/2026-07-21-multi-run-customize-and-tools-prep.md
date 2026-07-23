# tailor & multi-run image: producing multi-run image and its derivatives

> **Status:** Proposed · _2026-07-21_ · supersedes the earlier "ACL upstream signing" framing
>
> **Core question (from Paco):** could tailor eventually **produce multi-run image ("multi-run image")** and
> **multi-run image derivatives**? multi-run image is produced **entirely through Image Customizer (IC)** — two chained
> customize runs on a stock ACL VHD — followed by an **ephemeral Secure Boot signing** step. That is
> squarely tailor's domain (drive IC), so this is a realistic target. This doc maps multi-run image's build +
> signing mechanism (verified against the IC-based multi-run image pipeline), then analyses exactly what tailor
> would need to reproduce it, and how much closer *derivatives* are than multi-run image itself.
>
> Public-repo note: mechanism only — internal package lists, repo/gallery/definition names, infra
> identifiers, and internal file paths are omitted.

## 1. How multi-run image is built today (verified)

multi-run image is **not** built by the upstream Flatcar/portage system — that produces the *stock ACL VHD*.
multi-run image is a **second stage that runs IC** to transform that stock VHD:

**Inputs:** the stock ACL VHD (the base), the `imagecustomizer` binary, and an **rpmdb sidecar**
(`rpmdb.sqlite`, emitted by the upstream build).

**A prepared tools-dir.** IC needs a chroot with `tdnf` + `systemd` to resolve packages and rebuild
the UKI. The pipeline bootstraps a container rootfs and then **prepares it in place**: copy
`resolv.conf`, install the RPM GPG key, **rewrite the repo files' `gpgkey` URIs** (IC's relative-URI
schemes aren't parseable by stock tooling) and disable repo-gpgcheck, `rpm --import` the key,
`chroot … tdnf install systemd`, and **seed a schema-valid `tdnf` `history.db`** via
`tdnf-history-util init` inside the chroot. The prepared rootfs is passed as `--tools-dir`.

**Two chained IC customize runs** (each `--tools-dir <prepared>`, run 1's output VHD is run 2's input):

- **Run 1 — seed.** A near-no-op customize whose only effect is `os.additionalFiles` planting the
  **rpmdb** (`/var/lib/rpm/rpmdb.sqlite`) and the **tdnf history.db**. Required because ACL has no
  in-image rpmdb and no initialized tdnf history, so run 2's `tdnf install` would otherwise fail.
  Config carries `previewFeatures: [uki, reinitialize-verity, …]`, `os.uki.mode: create`,
  `storage.reinitializeVerity: all`.
- **Run 2 — install + swap.** Installs the replacement RPMs that supersede the base's embedded
  sysexts plus a runtime package surface, adds a few `additionalFiles`, enables services, and runs
  `scripts.postCustomization` to **drop the now-redundant embedded sysext `.raw` files** and fix up
  repo/gpg for the booted image. With `uki.mode: create` + `reinitializeVerity: all`, IC **rebuilds
  the UKI and re-seals dm-verity**.

**Then: ephemeral Secure Boot signing (mount-based).** IC rebuilds the UKI **unsigned**, so the
pipeline signs it afterwards by operating on the image directly:

1. `qemu-img convert` the run-2 VHD → raw;
2. `losetup --partscan`, locate the **vfat ESP** by filesystem type, mount it;
3. run the **same ephemeral-signing script the upstream ACL pipeline uses**: generate a fresh
   **self-signed RSA-2048 codeSigning cert**, **`sbsign`** the UKI + UKI addons + the boot loader
   **in place** (shim left alone), delete the private key, write the **public cert** out for later
   Secure Boot `db`/gallery enrollment;
4. unmount, `qemu-img convert` raw → **fixed-format VPC VHD** (gallery requirement).

`sbsign` is built from source inline because the build distro lacks it.

**Lifecycle note (downstream, out of scope here):** the ephemeral signature is a *testable* one; at
**publish** time it is replaced by a stable Microsoft cert chain via an enterprise code-signing
service that extracts the `*.efi` from the finished VHD, re-signs them, and re-injects them. That is a
separate publish concern, not part of *producing* multi-run image.

## 2. The signing mechanism, precisely

The signing that makes multi-run image is **mount-the-ESP-and-`sbsign`-in-place**, **not** IC's
`output.artifacts` + `inject-files` flow that tailor's current signer assumes. Key properties:

- **Signable set:** UKI(s) + UKI addons + boot loader (`grub*.efi` = systemd-boot); **shim is
  Microsoft-signed and untouched**.
- **Verity is secured transitively:** the `/usr` dm-verity **root hash rides in the UKI's kernel
  cmdline**, so signing the UKI covers verity. The UKI is the unit that must be re-signed — and it is
  exactly what run 2 regenerates.
- **Ephemeral key**, discarded after use; **public cert published** for firmware-`db`/gallery
  enrollment so the signatures are trusted at boot.

## 3. tailor today vs producing multi-run image — gap analysis

| multi-run image ingredient | tailor today | Gap |
| --- | --- | --- |
| Base = stock ACL VHD | `base: { path }` local base | ✅ none |
| The IC config content (previewFeatures, `uki.mode`, `reinitializeVerity`, packages, `additionalFiles`, `postCustomization`) | tailor passes `config:` **opaquely** to IC | ✅ none — any IC config already works |
| rpmdb / history.db sidecar inputs | `additionalFiles` sources (just files) | ✅ (parameterize the sidecar as an input) |
| **Prepared tools-dir** (chroot: install GPG key, rewrite repos, `tdnf install systemd`, seed `history.db`) | tailor exports a container rootfs **as-is** as `--tools-dir` | ⛔ **Gap 1 — tools-dir preparation** (run commands inside the tools-dir before use) |
| **Two chained customize runs** (run 1 → run 2) | tailor does **one** customize per cell (its 3-pass mode is customize→sign→inject, not two customizes) | ⛔ **Gap 2 — chained multi-run customize** |
| **Ephemeral ESP signing** (mount ESP, `sbsign` in place, publish cert) | tailor's signer uses IC `inject-files`, not a mounted ESP | ⛔ **Gap 3 — mount-based ephemeral signing** |
| VHD→raw→fixed-VPC round-trip for the mount+sign | IC handles output format; the raw round-trip is orchestration | ⚠️ minor orchestration |

So **three real capabilities** stand between tailor and *producing multi-run image*: tools-dir preparation,
chained multi-run customize, and mount-based ephemeral signing.

## 4. Derivatives are much closer than multi-run image itself

A **derivative** of multi-run image is "take the multi-run image VHD as base and customize further (and re-sign)." That is
**base + IC customize** — tailor's core competency. In fact **downstream-project's myimage HPC image already
is an (unsigned) multi-run image derivative**: it consumes the multi-run image VHD as its base and customizes it with
tailor today. So:

- **Unsigned multi-run image derivatives: tailor can already do this.** (This is the current downstream-project flow.)
- **Signed multi-run image derivatives:** add only **Gap 3** (mount-based ephemeral re-sign) — the derivative's
  own UKI/verity changes must be re-signed, but it's a single customize + a sign, not the two-run
  seed/swap dance.
- **Producing multi-run image from the stock VHD:** needs **all three gaps** (the seed/install two-run flow +
  prepared tools-dir + signing).

This ordering is the useful part: **signed derivatives are one capability away; full multi-run image production
is three.**

## 5. Design sketch — the three capabilities in tailor's model

- **Gap 2 — chained multi-run customize.** Model an image (or cell) as an **ordered sequence of IC
  customize passes**, each pass's output VHD feeding the next as `--image-file`. tailor's executor
  already runs a multi-pass sequence for signing (customize→sign→inject), so this generalizes that
  into a declared `runs:`/pass list. Derivatives simply append passes. This is the biggest structural
  change and the enabler for multi-run image.
- **Gap 1 — tools-dir preparation.** Extend the tools-dir source with a **prepare step**: commands
  run inside the exported rootfs (a chroot / throwaway container) before it is bound as `--tools-dir`
  — install packages, drop keys, rewrite repo files, seed state. Today tailor exports the container
  fs untouched; this adds a declarative "prepare the tools-dir" hook. (Could also be expressed as
  "the tools-dir is itself produced by an inner build step.")
- **Gap 3 — Secure Boot signing.** Add a signing stage after the final customize. The **preferred**
  mechanism is **IC-native deferred signing** — `output-artifacts` → sign → `inject-files` — driven by
  **external-signer** (ephemeral for dev, remote-service for production), which is exactly tailor's original
  three-pass model (`2026-06-29-signing.md`) and what the current myimage SKU work uses; it signs the
  UKI + shim + bootloader **and the dm-verity root hash**, on any IC-built image, with no image
  mounting. See `2026-07-22-signing-step1-ic-native.md` for the scoped first step. (The older
  ACL-scripts path — loop-mount the ESP and `sbsign` in place — remains a fallback for environments
  where IC's `output-artifacts` isn't available, but is no longer the recommended integration.)

These compose cleanly: a signed multi-run image build is `prepared-tools-dir → run1 → run2 → sign`,
all declared, all IC-driven.

## 6. What stays outside tailor (full-lifecycle context)

- **Upstream:** the stock ACL VHD, the IC binary, and the rpmdb sidecar are produced by the upstream
  Flatcar/portage build. tailor **consumes** these; it does not build the kernel or base rootfs. (OOT
  kernel-module signing also lives here and is not IC-reachable.)
- **Downstream:** publish-time re-sign (stable Microsoft cert chain via an enterprise code-signing
  service), plus gallery/registry/marketplace publish, cert-`db` enrollment, and Secure Boot test
  wiring — all separate from *producing* multi-run image.

Net: tailor's reachable scope is **produce the multi-run image VHD (and derivatives) + ephemeral-sign it**,
consuming the upstream base and stopping before downstream publish.

## 7. Open questions

1. **Chained runs surface:** how should multi-run be declared — an explicit ordered `runs:` list of IC
   configs per image, or a higher-level "stages" concept? How do the matrix/params interact with a
   run sequence (per-run params vs whole-image)?
2. **Tools-dir prep surface:** declarative command list run in the rootfs, a reference to a script, or
   "the tools-dir is the output of another tailor build"? How is it cached/keyed (it's expensive)?
3. **Signing path:** should tailor support **both** the IC-`inject-files` signer (current) **and** the
   mount-based ESP signer (multi-run image), selectable per profile? For multi-run image parity the mount-based path is
   required; is the inject-files path still worth keeping?
4. **Sidecar inputs:** the rpmdb/history seeds are external build artifacts — model them as
   parameterized inputs, or can tailor generate the history seed itself (it already can prepare a
   tools-dir chroot, which is where the seed is made)?
5. **Do we actually want tailor to *produce* multi-run image, or only signed *derivatives*?** The latter is one
   capability (Gap 3); the former is three. The answer scopes the whole effort.
