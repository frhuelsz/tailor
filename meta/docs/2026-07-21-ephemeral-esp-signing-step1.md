# tailor — step 1: sign a customized image with multi-run image's ephemeral ESP strategy

> **Status:** Proposed (scoping) · _2026-07-21_
>
> First concrete step toward tailor producing multi-run image and derivatives
> (`2026-07-21-multi-run-image-production.md`). Scope is deliberately **just one capability** — **Gap 3,
> mount-based ephemeral Secure Boot signing** — applied to a **single customized image (myimage)**. This
> is the "signed derivative" case: one normal tailor customize, then re-sign the boot chain exactly
> the way multi-run image does. Everything else (chained multi-run customize, tools-dir preparation, producing
> multi-run image from the stock VHD, publish-time remote-service re-sign, `db`/gallery enrollment, OOT modules) is
> **out of scope for this step**.
>
> Context: downstream-project's myimage ships **unsigned** by policy (HPC, Secure Boot off). This step is
> about giving tailor the **capability** and validating it on myimage as the first consumer — not a
> policy change for the HPC image.

## 1. What "the same strategy as multi-run image" means (recap)

multi-run image signs **after** IC produces the (unsigned) image, by operating on the image directly
(`2026-07-21-multi-run-image-production.md` §2):

1. convert the output VHD → raw;
2. `losetup --partscan`, find the **vfat ESP**, mount it;
3. generate a **fresh self-signed RSA-2048 codeSigning cert** (openssl), **`sbsign`** the boot chain
   **in place**, **discard the private key**, write the **public cert** out;
4. unmount, convert raw → the final image format.

Signed set: **UKI(s) + UKI addons + boot loader**; **shim is left alone**. Verity is secured
transitively (its root hash rides in the UKI cmdline). This is **not** IC's `output.artifacts` /
`inject-files` flow — it is a post-build ESP mount-and-sign.

## 2. Scope of step 1

**In scope:**

- A new tailor **signing backend** that, **after the customize pass**, mounts the output image's ESP
  and `sbsign`s a declared artifact set with a **throwaway ephemeral cert**, then publishes the
  public cert next to the image.
- Applied to **one image, one customize pass** (myimage) producing a disk-image output (VHD/vhd-fixed).
- **Sudo-free on the host** (§4).

**Out of scope (deferred):** chained multi-run customize (Gap 2); tools-dir preparation (Gap 1);
producing multi-run image from the stock ACL VHD; publish-time stable re-sign (remote-service); firmware-`db`/gallery
cert enrollment; Secure Boot boot-test wiring beyond a signature-validity check; OOT module signing;
`cosi`/`iso` outputs.

## 3. Design (this step only)

### 3.1 Config surface

Reuse the existing `signing:` profile surface with a **new backend**:

```yaml
# tailor.yaml
signing:
  default: acl-ephemeral
  profiles:
    acl-ephemeral:
      backend: espSbsign             # NEW: mount ESP + sbsign in place (post-customize)
      key: ephemeral                 # ephemeral (throwaway, default, multi-run image parity)
      #   OR a persistent bring-your-own key for a durable signature:
      # key:
      #   keypair: { key: <path/ref>, cert: <path/ref> }
      # espArtifacts:                # optional; defaults to the ACL ESP layout below
      #   - "EFI/Linux/*.efi"        # UKI(s)
      #   - "acl/uki-addons/*.efi"   # UKI addons
      #   - "EFI/BOOT/grub*.efi"     # boot loader (systemd-boot)
      # publishCaCert: <path>        # default <output_dir>/<slug>.ca_cert.pem
```

```yaml
# myimage/image.yaml
signing: acl-ephemeral
```

- **`backend: espSbsign`** selects the mount-and-sbsign path (distinct from tailor's existing
  IC-`inject-files` signer). The *method* (mount ESP + `sbsign`) is the same regardless of key source.
- **`key`** is the **key source**, orthogonal to the method:
  - **`ephemeral`** (default) — generate a throwaway self-signed RSA-2048 codeSigning cert, sign,
    **discard the private key**, publish only the public cert. This is exactly what multi-run image does: the
    build-time signature is **testability-only** and is replaced downstream by a stable cert chain at
    publish, so the key is deliberately thrown away.
  - **`keypair`** — a **persistent bring-your-own** key + cert, for a **durable** signature when there
    is **no downstream re-sign** (e.g. a standalone signed image that must be trusted as-is). Reuses
    tailor's existing `keypair` key handling. *Ships after `ephemeral`; the config door is open.*
- **`espArtifacts`** is a glob list resolved **against the ESP root**; defaults to the ACL layout.
  **Shim (`EFI/BOOT/BOOT*.EFI`) is always excluded.**
- **`publishCaCert`** defaults to the per-cell `<output_dir>/<slug>.ca_cert.pem` (tailor's existing
  cert-publish convention). With `keypair`, "publish the cert" means publishing the BYO public cert.

### 3.2 Execution flow

tailor forces the customize pass to emit **raw** for a signed cell (`--output-image-format raw`), so
the ESP is directly loop-mountable with **no pre-conversion**. A **sign stage** runs a **single
privileged helper container** (§4) that signs the raw in place; a **single** format conversion then
produces each declared disk-image output:

```
customize (existing; output forced to RAW when signing) → <slug>.raw
        │
        ▼   sign stage (backend=espSbsign)   [ helper container, --privileged, /dev ]
    1. losetup --find --partscan <slug>.raw; locate the vfat ESP by FSTYPE
    2. key source:
         ephemeral → openssl req -x509 -newkey rsa:2048 -noenc …   (generated in-container)
         keypair   → use the bring-your-own key + cert
    3. for each espArtifacts match (minus shim): sbsign --key --cert --output <in place>
    4. umount + losetup -d
    5. publish the PUBLIC cert → <output_dir>/<slug>.ca_cert.pem
       (with `ephemeral`, the private key never leaves the container and is discarded)
        │
        ▼   format stage
    6. qemu-img convert <slug>.raw → each declared disk output
       (e.g. vhd-fixed: -O vpc -o subformat=fixed,force_size; no-op if the output IS raw)
        │
        ▼
  janitor normalizes ownership (existing) → signed <slug>.<fmt>(s) + <slug>.ca_cert.pem
```

**Why raw-first:** `losetup` needs a raw image anyway (a fixed-VHD footer / dynamic-VHD layout is not
directly loop-mountable), so emitting raw from IC removes the redundant `format → raw → format`
round-trip multi-run image does — one conversion instead of two, which matters on a ~30 GB image. It also means
signing happens **once** on the raw and every declared disk output is a straight `qemu-img convert` of
the **same signed bytes** — byte-identical signed content across output formats.

### 3.3 The helper container (sudo-free)

Mounting a loop device needs privilege. tailor's principle is **no host `sudo`**, so the mount +
sbsign run inside a **privileged container** (the same `--privileged` + `/dev` shape tailor already
uses for IC, and the same janitor pattern for ownership). The container needs `qemu-img`,
`util-linux` (`losetup`/`mount`), `openssl`, and `sbsign`. Make the image **configurable** like the
janitor image (a `signerImage` with a sane default); a first cut can layer `sbsign` onto the IC
base. This keeps every privileged operation in a throwaway container and the host `sudo`-free.

### 3.4 Preflight

Fail fast, before the (slow) customize:

- the `signerImage` resolves/pulls;
- the profile is valid and, if `espArtifacts` is defaulted, the image is a **disk image** (not
  `cosi`/`iso`);
- (in-container, early) the ESP is present and looks like a UKI image (`EFI/Linux` exists) — else a
  clear error, never a silent no-op.

### 3.5 Reuse vs new

- **Reuse:** the `Signer` port, tailor's openssl cert-generation and cert-publish convention, the
  privileged-container + janitor ownership machinery, and the per-cell/output plumbing.
- **New:** the `espSbsign` backend (raw-first: force raw from IC, mount ESP, in-place sbsign, then a
  single format conversion) and the `signerImage` config. This backend does **not** use IC passes at
  all — it post-processes the final image, which is the key structural difference from the existing
  three-pass (inject-files) signer.

## 4. Validation plan (myimage)

1. `tailor build` myimage with `signing: acl-ephemeral` → signed VHD + `myimage_*.ca_cert.pem`.
2. **Signature check:** `sbverify --cert <published cert>` each signed artifact succeeds; shim
   unchanged; unsigned-before / signed-after diff is only the expected `.efi`.
3. (Stretch, likely deferred) enroll the cert into an OVMF `db` and boot the VHD under QEMU Secure
   Boot — this belongs to the later test-wiring work, not step 1.

## 5. Non-goals restated

Not this step: multi-run customize, tools-dir prep, multi-run image-from-stock, remote-service publish re-sign, cert
enrollment, full Secure Boot boot-test harness, OOT modules, `cosi`/`iso` signing. Those are tracked
in `2026-07-21-multi-run-image-production.md`.

## 6. Open questions (to confirm before implementing)

**Resolved (this round):**

- **Raw-first (note #1):** tailor forces IC to emit **raw** for a signed cell, signs it, then a
  **single** `qemu-img convert` produces each declared disk output — no redundant round-trip (§3.2).
- **Throwaway key (note #2):** `key: ephemeral` (throwaway) is faithful to multi-run image and is the step-1
  default; a `keypair` (BYO persistent key) source is provided for the durable-signature case where
  there is no downstream re-sign (§3.1).

**Still open:**

1. **Helper container:** layer `sbsign` onto the IC base as the default `signerImage`, or ship/point
   at a dedicated minimal signer image? (Lean: configurable, default = IC base + `sbsign`.)
2. **Artifact-set default:** hard-code the ACL ESP layout as the default `espArtifacts` (with
   override), or require the profile to declare it explicitly? (Lean: ACL-layout default + override,
   since myimage is an ACL derivative.)
3. **Output formats:** restrict step 1 to disk-image outputs (VHD/vhd-fixed/raw) and error clearly on
   `cosi`/`iso`? (Lean: yes.)
4. **Cert reuse across a matrix/clones:** one ephemeral cert per cell (simple, matches "leaf per
   cell"), or one per build shared across cells? (Lean: per cell — no cross-cell coupling, mirrors
   multi-run image's per-image ephemeral cert.)
5. **Ship `keypair` in step 1 or defer it?** The backend is identical; only the key source differs.
   Ship `ephemeral` first (multi-run image parity) and add `keypair` immediately after, or build both together?
   (Lean: `ephemeral` first, `keypair` fast-follow — unless myimage needs a durable signature now.)
