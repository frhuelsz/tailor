# tailor — supporting ACL's Secure Boot signing (design)

> **Status:** Proposed · _2026-07-21_
>
> Investigation + design for making tailor able to reproduce the **Azure Container Linux (ACL)
> template build's image signing**, as a step toward eventually driving that pipeline end-to-end.
> Signing is the one large capability tailor lacks for parity. This doc maps ACL's signing mechanism
> (verified against the ACL build/publish repos), compares it to tailor's existing signing model, and
> proposes how tailor would support the same mechanism — plus a survey of what *else* is missing for
> a full end-to-end run.
>
> Public-repo note: this describes the signing **mechanism** only. Environment-specific identifiers
> (signing-service key codes, service-principal/tenant IDs, key-vault names, internal build IDs and
> repo/script paths) are deliberately omitted.

## 1. Scope — what "end-to-end" can mean for tailor

ACL's template is **built** by an upstream Flatcar/portage-derived build system (kernel, out-of-tree
kernel modules, base rootfs, the base VHD). tailor drives **Image Customizer (IC)**, which *customizes
a prebuilt image* — it does not build the kernel or compile modules. So the realistic e2e surface for
tailor is **customize → sign → publish**, consuming the upstream-built ACL template as its base. Two
consequences fall straight out of that boundary and shape the whole design:

- **Boot-artifact signing (UKI, boot loader, addons) is in reach** — IC regenerates the UKI after a
  kernel swap + verity re-seal, so tailor already owns the very artifact that must be (re-)signed.
- **Out-of-tree kernel-module signing is *not* in reach** for an IC-customize flow — it happens during
  the package/kernel build, upstream of IC (see §2.2).

## 2. How ACL signs today (ground truth)

ACL signing is **two-phase**, plus a separate module-signing concern.

### 2.1 Build-time ephemeral Secure Boot signing

During the template build, after the ESP is populated, a script signs the Secure Boot chain with a
**one-time, self-signed key**:

- **Key/cert:** `openssl req -x509 -newkey rsa:2048 -noenc` — a throwaway codeSigning cert
  (`CA:FALSE`, `extendedKeyUsage=codeSigning`), 1-day validity. The **private key is deleted** right
  after signing; the **public cert is published** for later enrollment.
- **What is signed (`sbsign`, EFI Authenticode):** the **UKI(s)** (`EFI/Linux/*.efi`), the **UKI
  addons** (`*.addon.efi` — firstboot/fips/oem/kdump, both active and template copies), and
  **systemd-boot** (installed as `EFI/BOOT/grub*.efi`). Signing is done **in place on the mounted
  ESP**.
- **What is left alone:** **shim** (`EFI/BOOT/BOOT<ARCH>.EFI`) is Microsoft-signed and untouched.
- **Cert enrollment:** the published cert is enrolled into the test firmware's Secure Boot `db` (OVMF
  for QEMU tests) and into the gallery image's `securityProfile`, so shim accepts these signatures.

This is an **ephemeral, testable** signature: it makes the image boot under Secure Boot in CI/gallery
tests, but it is **not** the customer-facing trust chain — that comes in §2.3.

### 2.2 Out-of-tree kernel-module signing

OOT modules (e.g. ZFS, NVIDIA — relevant to GPU/HPC stacks) are signed so they load under Secure
Boot. This uses the **standard kernel module-signing path** (`USE=modules-sign` in the portage
`linux-mod` eclass → the kernel's `scripts/sign-file` with a `MODULES_SIGN_KEY`/`MODULES_SIGN_HASH`),
with the **ephemeral key** as the module-signing key so the running kernel trusts them. **This happens
at package/kernel-build time, upstream of IC** — it is not an ESP/boot-artifact operation and cannot
be reproduced by customizing a prebuilt image. Flagged here because it is a real future concern if a
GPU/MOFED image ever needs Secure Boot, but it is **outside tailor/IC's model**.

### 2.3 Publish-time re-sign (stable Microsoft cert chain)

At publish time, the producer's ephemeral signatures are **replaced** with a stable Microsoft cert
chain via an **enterprise code-signing service** (remote-service `SigntoolSign`). The flow operates on the
finished **VHD**:

1. **Extract** every in-scope `*.efi` from the VHD's ESP partition (a helper exposes the VHD as a
   block device with `qemu-nbd` and copies out the EFI binaries listed in a config).
2. **Byte-equality gate (pre):** the VHD's sha256 is checked against the bundle manifest before any
   mutation (tamper detection); a mismatch is terminal.
3. **Sign** each extracted `*.efi` in place through the code-signing service (`SigntoolSign`, SHA-256
   file digest + page hash + RFC-3161 timestamp), with a **per-environment key code** (dev vs prod use
   different service principals, cert chains, and key codes).
4. **Inject** the signed `*.efi` back into the VHD.
5. **Byte-equality gate (post):** recompute the VHD sha256 and export it; downstream publish validates
   against this exact value.

An **addon-coverage check** mounts the ESP read-only and fails the pipeline if any `*.addon.efi`
exists on the ESP but is missing from the sign scope (so a new addon can never ship with only an
ephemeral signature). The RFC-3161 timestamp makes signing **non-reproducible byte-wise**, which is
why the pipeline signs **once** and pushes identical bytes to every surface.

### 2.4 Verity / UKI relationship

`/usr` is **dm-verity-sealed**; the verity **root hash is carried in the UKI's kernel command line**,
so signing the UKI **transitively secures verity**. The **signable unit is the UKI** (plus boot loader
and addons). This is exactly the artifact IC rebuilds after a kernel swap + verity re-seal — so the
thing that must be re-signed is the thing tailor/IC already regenerates.

## 3. tailor's current signing baseline

tailor already has a signing feature (`2026-06-29-signing.md`, `tailor-sign`): an **IC-mediated,
three-pass** flow — `customize` (with `output.artifacts` in the IC config) → **host-side sign** →
`inject-files` — using host **`openssl`** (per-build CA + per-cell leaf + verity-hash CMS) and
**`sbsign`** (PE/Authenticode). Backends are pluggable (`local-test-ca`, bring-your-own `keypair`,
`azure-key-vault`), the signer is a `Signer` port, and a fail-fast preflight checks tool/key presence
before any build. It publishes the CA cert next to the image and requires **no host `sudo`** (the
janitor normalizes ownership first).

## 4. Gap analysis — tailor today vs ACL's mechanism

| ACL need | tailor today | Gap |
| --- | --- | --- |
| Ephemeral RSA-2048 self-signed codeSigning cert, key deleted, cert published | `local-test-ca` backend already does essentially this (per-build CA + published cert) | **Small** — align cert profile (single self-signed codeSigning leaf, no CA chain) |
| `sbsign` the UKI | `sbsign` PE signing already implemented | **Small** |
| Also sign **systemd-boot** + **UKI addons** (`*.addon.efi`) | tailor signs whatever IC emits as `output.artifacts` | **Medium** — must ensure the boot loader **and ACL's addon layout** are in the signed set (IC's generic artifact extraction may not know ACL addons) |
| Sign artifacts **in place on the ESP** | tailor extracts via IC + re-injects via `inject-files` | **Mechanism difference** — ACL mounts the ESP; tailor relies on IC's artifact/inject-files contract (§5.1) |
| Publish-time **re-sign of the finished VHD** through an enterprise service, replacing the ephemeral sig | Not implemented; `azure-key-vault` backend is named but only as a signer, not a VHD re-sign stage | **Large** — a new "re-sign an existing image" stage + a code-signing-service backend (§5.2) |
| Byte-equality gates (pre/post-resign sha256) | Not modeled | **Medium** |
| OOT kernel-module signing | Not applicable to IC customize | **Out of scope** (§2.2) |
| Enroll cert into firmware `db` / gallery `securityProfile` | Explicit non-goal today ("stop at signed image + published cert") | **Medium** (publish concern, §6) |

## 5. Design — how tailor would support the same mechanism

### 5.1 Build-time ephemeral signing (parity)

The crypto is already tailor's (`openssl` + `sbsign`). The real decision is **how tailor gets at the
boot artifacts to sign them**:

- **Option 1 — IC `output.artifacts` + `inject-files` (tailor's current model).** No image mounting;
  IC extracts the unsigned artifacts and re-injects the signed ones. Clean and sudo-free, but it
  requires IC to emit the **full ACL artifact set** — UKI **and** systemd-boot **and** the ACL
  `*.addon.efi` set. tailor must confirm IC's artifact extraction covers addons, or drive the set
  explicitly. **Preferred if IC's contract covers the set.**
- **Option 2 — mount-based ESP signing (ACL's own mechanism).** tailor (or a delegated container)
  exposes the image ESP (loop/`qemu-nbd`), signs the discovered `*.efi` in place, unmounts. Faithful
  to ACL and inherently covers whatever is on the ESP (addons included), but it is a **new
  image-mounting capability** for tailor — privileged, and a departure from "drive IC, never mount the
  image." Best reserved for the re-sign stage (§5.2), which is inherently mount-based anyway.

**Recommendation:** keep build-time ephemeral signing on **Option 1** (extend the existing three-pass
signer so the signed set explicitly includes the boot loader and a declarable addon list), and add an
**`ephemeral` signer profile** that mirrors ACL's cert profile (one self-signed codeSigning cert,
key discarded, cert published for enrollment). This is a thin addition over `local-test-ca`.

### 5.2 Publish-time re-sign (parity)

This is the big new piece and is **architecturally distinct** from the three-pass flow: it operates on
an **already-built VHD**, not during customize. Model it as a **new tailor signing stage / backend**:

- A **`resign` operation** on a finished image: **extract** in-scope `*.efi` from the VHD ESP →
  **sign** each via a pluggable **code-signing-service backend** → **inject** back → **recompute +
  emit sha256**, with the same **pre/post byte-equality gates** and **addon-coverage check** ACL uses.
- The signing-service backend is the pluggable point the existing `Signer` port anticipated (the
  `azure-key-vault` slot generalizes to "an enterprise code-signing service"): it takes a per-env
  identity + key code and signs a PE file, returning signed bytes. tailor supplies the extract/inject/
  gate orchestration; the service does the crypto.
- Because this is mount-based and privileged, it reuses tailor's safe-dir guards and the janitor
  ownership discipline; a natural implementation delegates the ESP extract/inject to a container step
  (mirroring how ACL wraps a helper), keeping tailor's host sudo-free.

This lets tailor produce **both** signatures with parity: the ephemeral build-time signature (testable
Secure Boot) and the stable publish-time re-sign (customer trust chain), or hand the VHD to the
re-sign stage as a standalone operation.

### 5.3 OOT kernel-module signing

**Out of scope for an IC-customize flow** (§2.2) — it is bound to the kernel/package build. If tailor
ever needed it, the only IC-reachable path would be **re-signing prebuilt `.ko`s post-hoc** with a key
the target kernel trusts (kernel `sign-file` against the enrolled cert), as a customize step — a
sizeable separate feature. Document it as a known boundary, not a v1 goal.

### 5.4 Where it fits tailor's model

- Extend the existing **`signing:` profile** with a backend/profile axis: `ephemeral` (§5.1),
  `keypair`/`local-test-ca` (today), and a **code-signing-service** backend usable both as a
  three-pass signer and as the **`resign`** stage (§5.2).
- **Key management:** ephemeral (generate → sign → discard key → publish cert) already matches
  `local-test-ca`; the service backend holds no local key (identity + key code only). Preflight
  fail-fast extends to service reachability/credentials.
- **Verity/UKI/addons compose** exactly as tailor's three-pass already assumes: the UKI is the signed
  unit (verity secured transitively), the boot loader and each addon are individually signed, shim is
  never touched.
- **Reproducibility:** signed outputs are already a non-goal for byte-repro; the RFC-3161 timestamp in
  the service re-sign makes that explicit — hence **sign once, publish the same bytes** (drives the
  byte-equality gates).

## 6. What else is missing for tailor to run this pipeline e2e (beyond signing)

Surveyed from the ACL build/publish pipeline. Signing is the biggest, but not the only, gap:

- **Template build boundary (fundamental).** The base ACL template (kernel, OOT modules, base rootfs)
  is built upstream by the Flatcar/portage system, not by IC. tailor's e2e is **customize → sign →
  publish**, consuming that template as base. OOT-module signing lives on the far side of this line.
- **Bundle produce/consume + manifest.** The pipeline pulls/pushes a large OCI **staging bundle** with
  a `manifest.json` of per-file sha256s. tailor has no bundle model — **this dovetails with the
  export/rendered + manifest work** (`2026-07-16-render-ahead-export.md`); a signed-image bundle +
  manifest is the natural artifact.
- **Byte-equality gates.** Pre/post-resign sha256 validation against the manifest — tailor would need
  to model and emit these (§5.2).
- **Publish surfaces (none exist in tailor):**
  - **MCR / OCI** — push sysexts + VHD as OCI with **COSE Sign1** supply-chain signatures + SBOM
    referrers (via `notation`; a *different* signing concern from Secure Boot — image supply-chain, not
    boot). Likely its own feature or out of scope.
  - **Azure Compute Gallery** — mint an image version, enroll the **UKI cert** into the image's
    Secure Boot `db`, idempotent on VHD sha256.
  - **Marketplace** — VHD blob upload + checksum verify.
- **Secure Boot test wiring.** OVMF `db` enrollment (e.g. `virt-fw-vars`) + a QEMU Secure Boot boot
  test / kola-style validation — tailor has no test-harness concept.
- **Shared version / release bookkeeping** across surfaces and arches.

Net: **signing (both phases) is the headline gap and the enabling one**; a full e2e also needs a
bundle/manifest model, byte-equality gates, and publish/test integrations. The **cert-enrollment +
gallery/marketplace publish** steps are the next-most-substantial after signing.

## 7. Recommendation & phasing

- **Phase 1 — ephemeral build-time parity.** Add the `ephemeral` signer profile (self-signed
  codeSigning cert, key discarded, cert published) and ensure the signed set covers UKI + boot loader
  + a declarable addon list, via the existing three-pass model. Highest value, smallest delta — it
  makes tailor produce a testable Secure Boot image with parity to ACL's build-time phase.
- **Phase 2 — publish-time re-sign stage.** The `resign` operation on a finished VHD (extract → 
  service-sign → inject → sha256 gates + addon-coverage), with a pluggable code-signing-service
  backend. This is the customer-trust-chain half and the larger build.
- **Phase 3 (separate features, likely out of scope near-term):** cert/db enrollment + gallery/
  marketplace publish; OCI/COSE supply-chain signing; Secure Boot test harness; OOT-module re-signing.

## 8. Open questions

1. Does IC's `output.artifacts`/`inject-files` extraction cover **systemd-boot and ACL's `*.addon.efi`
   set**, or must tailor drive the signed set explicitly (deciding Option 1 vs a mount-based signer in
   §5.1)?
2. For the re-sign stage, does tailor **mount the VHD itself** (new privileged capability) or
   **delegate** ESP extract/inject to a container step (keeping host sudo-free)? (Lean: delegate.)
3. Is the enterprise code-signing-service backend in scope for tailor, or should tailor stop at
   "ephemeral-signed image + a `resign`-ready extract/inject bundle" and let the pipeline own the
   service call? (Ties into the export/handoff strategy: tailor could emit the extract/inject +
   byte-equality contract and let a trust-sensitive pipeline run the actual service signing.)
4. Should cert **db/gallery enrollment** ever be tailor's job, or always the publish pipeline's?
   (Today it's an explicit tailor non-goal.)
