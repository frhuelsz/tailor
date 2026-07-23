# tailor — step 1: sign a customized image via IC-native deferred signing (external-signer)

> **Status:** Proposed (scoping) · _2026-07-22_ · supersedes the mount-based ESP-sbsign scoping
>
> First concrete step toward signed images in tailor. **Revised** to the **IC-native deferred-signing
> flow** — `output-artifacts` → sign → `inject-files` — driven by **external-signer**, the IC-native signer
> that the current myimage SKU work uses. This replaces the earlier "mount the ESP and `sbsign` in
> place" scoping: the IC-native flow needs no image mounting, covers the **dm-verity root hash** as
> well as the EFI chain, and is **productionizable** (remote-service), while being *exactly* tailor's original
> three-pass signing model (`2026-06-29-signing.md` §2). Scope of this step: sign a **single
> customized image (myimage)** with **ephemeral** keys.

## 1. The mechanism (verified against external-signer + IC)

Signing is **deferred** — not baked into the customize run. It slots **between two IC invocations**,
using IC's own preview features `output-artifacts` + `inject-files` (IC ≥ 0.14):

```
1. IC customize (extract pass)   config has previewFeatures:[output-artifacts,…] + output.artifacts:{items:[ukis,shim,bootloader], path}
      customize --config-file <extract.yaml> --image-file <unsigned> --output-image-file <unsigned.raw> --output-image-format raw
      → writes the UNSIGNED .efi artifacts into <artifacts> + an inject-files.yaml manifest   (no --tools-dir; no package ops)

2. sign the artifacts IN PLACE   external-signer sign-artifacts --build-dir <b> --config-file <sign-config.yaml>
      sign-config.yaml: input.artifactsPath: <artifacts>;  signingMethod: { ephemeral | remote-service }

3. IC inject-files                inject-files --config-file <artifacts>/inject-files.yaml --image-file <unsigned.raw> --output-image-file <signed.raw> --output-image-format raw
      → writes the signed artifacts back in;  then qemu-img convert raw → fixed VPC VHD (gallery)
```

**external-signer** signs, per its manifest: **UKI + UKI addons**, **shim**, **systemd-boot / grub**
(Authenticode), and the **dm-verity root hash** (detached PKCS#7). It has two methods:

- **ephemeral** — self-signed x509 generated on the fly, **private key destroyed** after; the public
  `ca.pem` is captured for Secure Boot `db` enrollment. **Dev/test only** (unique cert per build →
  re-enroll on every image update). Host deps: `pesign`, `certutil` (nss-tools), `openssl`.
- **remote-service** — Microsoft's production signing service, auth via a managed identity against a Key Vault
  OneCert, with per-environment **key codes** and DRI emails. **Required for production/pre-release.**

The signable unit is exactly what IC (and tailor) already rebuild — the UKI + boot chain — so
external-signer slots in as a middle step, and the **extract config carries no package ops** (no tools-dir).

## 2. Why this over the earlier mount-and-sbsign scoping

| | mount-ESP + `sbsign` (old step-1) | IC-native deferred (external-signer) — **chosen** |
| --- | --- | --- |
| Image access | loop-mount the ESP (privileged, image mounting) | none — IC extracts/injects the artifacts |
| Coverage | UKI + addons + boot loader | + **dm-verity root hash**, + shim |
| Production path | ephemeral only (tailor would build its own remote-service) | **ephemeral *and* remote-service** built in |
| Fit with tailor | new mount capability | **already tailor's 3-pass model** (`2026-06-29-signing.md`) |
| Portability | ACL ESP layout assumptions | any IC-built image (artifacts come from IC) |

The mount-and-sbsign path is **dropped** as the primary; it only matters where IC's
`output-artifacts` isn't available.

## 3. Scope of step 1

**In scope:** a tailor **signing stage** that, for a signed cell, runs **extract → sign → inject** as
above with **ephemeral** keys, producing a signed image + the published `ca.pem`. One image (myimage),
one customize pass. **Sudo-free on the host** (the IC passes run in the container as today; the sign
step's host tools run unprivileged).

**Out of scope (deferred):** remote-service method + identity/Key-Vault/key-code plumbing; chained multi-run
customize; tools-dir preparation; producing multi-run image from the stock VHD; `db`/gallery enrollment
automation; Secure Boot boot-test wiring; OOT modules; non-disk outputs.

Context: myimage ships **unsigned** by policy (HPC, Secure Boot off). This step is the tailor
**capability**, validated on myimage as the first consumer — not a policy change for the HPC image.

## 4. Design (this step)

### 4.1 Config surface

Reuse tailor's existing `signing:` profile surface (`2026-06-29-signing.md` §4), adding a backend:

```yaml
# tailor.yaml
signing:
  default: myimage-ephemeral
  profiles:
    myimage-ephemeral:
      backend: external-signer            # IC-native deferred signing via external-signer
      method: ephemeral             # ephemeral (this step) | remote-service (later)
      items: [ukis, shim, bootloader]   # IC output.artifacts items; defaults to this set
      # publishCaCert: <path>       # default <output_dir>/<slug>.ca_cert.pem (from ca.pem)
```

```yaml
# myimage/image.yaml
signing: myimage-ephemeral
```

### 4.2 Execution flow (extends the 3-pass executor)

tailor already contemplates a three-pass signed build (`2026-06-29-signing.md` §5: customize → sign →
`inject-files`). This step makes the **sign** pass call external-signer and drives the config off the
artifacts IC actually emits:

1. **Extract pass** — tailor runs IC `customize` with an **auto-generated extract config**
   (`previewFeatures: [output-artifacts, …]`, `output.artifacts.items = <profile.items>`,
   `output-image-format raw`). Merge tailor's per-cell config with the extract directives.
2. **Sign** — tailor writes a `sign-config.yaml` (`input.artifactsPath = <artifacts>`,
   `signingMethod.ephemeral.publicKeysPath = <out>`), then runs `external-signer sign-artifacts`. Host tools
   (`pesign`/`certutil`/`openssl`) checked in **preflight** (fail fast, like the existing signing
   preflight §5.1).
3. **Inject pass** — tailor runs IC `inject-files` with the emitted `inject-files.yaml`, output raw.
4. **Finalize** — `qemu-img convert` raw → the declared disk format; publish `ca.pem` as
   `<output_dir>/<slug>.ca_cert.pem`.

The IC passes run in the toolchain container as usual; the sign step runs on the host (unprivileged),
so tailor's **no-host-sudo** and janitor-ownership guarantees hold.

### 4.3 Signer sourcing

`external-signer` is an external binary, distributed as a pinned package from an internal feed. tailor
sources it like any external signing tool (preflight-checked on PATH, or a configured path). *Where
the binary comes from and the remote-service identity/key-code/Key-Vault details are environment plumbing,
deferred to the remote-service step.*

## 5. Relationship to tailor's existing signer

tailor's current `2026-06-29-signing.md` design uses host `openssl` + `sbsign` for the middle sign
step. external-signer is a **superset** of that middle step: same `output.artifacts`/`inject-files`
scaffolding, but it also signs the **verity root hash**, handles **shim/bootloader** key-code
routing, and adds the **remote-service** production path. So the cleanest plan is: **keep the three-pass
scaffolding; make external-signer a signer backend** (alongside, or in place of, the raw openssl+sbsign
backend). For the ACL/myimage lineage external-signer is the backend we want; the raw backend can remain for
environments without external-signer.

## 6. Validation (myimage)

1. `tailor build myimage --cell <slug>` with `signing: myimage-ephemeral` → signed VHD + `*.ca_cert.pem`.
2. The signed artifacts verify against the published cert; shim/bootloader/UKI are Authenticode-signed
   and the verity root hash carries a detached signature.
3. (Deferred) enroll `ca.pem` into an OVMF `db` and boot under QEMU Secure Boot — test-wiring work.

## 7. Open questions

1. **Backend split:** ship `backend: external-signer` (method `ephemeral`) as the ACL/myimage signer, and keep
   the raw openssl+sbsign backend for non-external signer environments — or standardize on external-signer?
2. **`items` default:** hard-code `[ukis, shim, bootloader]` (and add `verityHash` when the image is
   verity-sealed) or require the profile to declare it?
3. **Binary sourcing:** preflight a `external-signer` on PATH, or let tailor fetch/pin it (adds a package
   source dependency)?
4. **remote-service (next step):** model `method: remote-service` with its identity/Key-Vault/key-code/DRI config as a
   follow-up; that's the production path and the bigger plumbing.
