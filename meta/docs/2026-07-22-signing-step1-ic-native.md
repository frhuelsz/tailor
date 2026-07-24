# tailor — IC-native deferred signing (execution design)

> **Status:** Ready for review · _2026-07-23_
>
> Complete, implementable design for how tailor produces Secure Boot–signed images. tailor drives
> **IC-native deferred signing** — an Image Customizer `output-artifacts` extract pass → a host-side
> sign step → an IC `inject-files` pass. This is exactly tailor's existing three-pass signing model
> (`2026-06-29-signing.md`), so the signing tool plugs in as a **`Signer` backend** with **no change
> to the executor or the `Signer` port**. Scope of this design: a signed `tailor build` produces a
> signed disk image plus a published enrollment certificate, using a self-signed (ephemeral) key.

## 1. The mechanism

Signing is **deferred** — not baked into the customize run. It slots between two IC invocations using
IC's own preview features `output-artifacts` + `inject-files`:

```
1. IC customize (extract pass)   config: previewFeatures:[output-artifacts,…] + output.artifacts:{items, path}, output raw
      → writes the UNSIGNED boot artifacts into <artifacts> + an inject-files.yaml manifest   (no package ops → no --tools-dir)
2. host-side sign (in place)     an external signer signs the extracted artifacts, keyed by the emitted manifest
3. IC inject-files               inject-files --config-file <artifacts>/inject-files.yaml --image-file <unsigned.raw> --output-image-file <signed.raw> --output-image-format raw
      → then convert raw → the requested disk format
```

The signable set is the boot chain IC (and tailor) already rebuild — the UKI(s), UKI addons, the boot
loader (systemd-boot / grub), and the dm-verity root hash. The extract pass does **no package
operations**, so it needs no tools-dir. The signer occupies the middle step and signs in place.

**Key sources.** Two modes, orthogonal to the mechanism:
- **ephemeral** — a self-signed certificate generated on the fly, private key destroyed after signing;
  the public certificate is published for Secure Boot `db` enrollment. Dev/test (unique cert per
  build). This design's scope.
- **external service** — a remote/enterprise signing service against a stable certificate chain, for
  production. Modeled as a seam (§7); not built here.

## 2. Reconciliation: the signer is a `Signer` backend; the 3-pass executor is unchanged

The `Signer` port already exists at the right granularity (`crates/tailor-core/src/ports.rs`):

```rust
trait Signer {
    fn preflight(&self) -> Result<(), SignError>;
    fn sign(&self, plan: &SigningPlan) -> Result<SigningResult, SignError>;
}
// SigningPlan { inject_files_yaml, artifacts_dir, leaf_id, ca_cert_dest }
// SigningResult { published_ca_cert: Option<PathBuf> }
```

`SigningPlan` is **artifact-set granular** — it hands the signer the whole `inject-files.yaml` + the
`artifacts_dir`, which is the natural unit for a signer that signs the extracted set in one pass. So a
delegating **external-signer backend is a drop-in `Signer` impl**:

- **`preflight()`** — the signer binary is present (PATH or configured path) and its host dependencies
  are available.
- **`sign(plan)`** — write the signer's config (pointing at `plan.artifacts_dir`, key-source method,
  and a `plan.leaf_id`-scoped output dir), invoke the signer over the artifacts, and return
  `SigningResult { published_ca_cert }` (the enrollment cert, for ephemeral) or `None`.

So the **three-pass executor (`2026-06-29-signing.md` §5) is unchanged**: the signer occupies the
existing host-side sign step; the customize (extract) and `inject-files` passes are as designed. The
built-in `openssl`+`sbsign` backend the 06-29 doc describes remains a **peer `Signer` impl** that
loops over the same `inject-files.yaml` internally. Both satisfy the identical port — the whole point
of the port.

### 2.1 Two axes: driver vs key-source

The existing `SigningBackend` enum (`local-test-ca` / `keypair` / `azure-key-vault`) models a **key
source** on the assumption tailor itself is the signing driver. A delegating external signer adds a
**driver** axis — tailor hands the whole sign step to an external tool that has its own key sources.
The config models this as a new backend whose `method` sub-selects the tool's key source:

| Concept | tailor-driven | delegated-signer |
| --- | --- | --- |
| driver | tailor loops `openssl`/`sbsign` per artifact | delegate to the external signer |
| ephemeral key | `backend: local-test-ca` | `method: ephemeral` |
| BYO key | `backend: keypair` | (tool-dependent) |
| remote/prod | `backend: azure-key-vault` (future) | `method: <external-service>` |

Keep all of them; the delegating backend is one more peer. We do **not** remove the built-in backend —
it preserves environments without the external signer and the S3 pure-Rust north star
(`2026-06-29-signing.md` §11).

## 3. Resolved design decisions

- **Backend split:** add the delegating external-signer as a first-class `Signer` backend; **keep**
  the built-in `openssl`+`sbsign` backend. The external signer is the default where it is the
  sanctioned path; the built-in stays as fallback and for the pure-Rust goal.
- **`items` default `[ukis, shim]`** (universally safe — see §5; `bootloader` is opt-in because it
  hard-errors on a grub-less ESP); the emitted `inject-files.yaml` is the source of truth for what
  gets *signed*. But the request set and sign set are different phases — see §5.
- **Binary sourcing:** preflight the signer on PATH or a configured path, exactly like
  `openssl`/`sbsign`. Acquisition (download + version pin) is environment/pipeline plumbing, not
  tailor's job — keeps tailor environment-agnostic.
- **Key source:** `method: ephemeral | <external-service>`; ephemeral is this design, the external
  service is the documented seam (§7).

## 4. Config surface

Extend `SigningProfile` / `SigningBackend` (`crates/tailor-config/src/schema.rs`):

```yaml
# tailor.yaml
signing:
  default: secureboot-ephemeral
  profiles:
    secureboot-ephemeral:
      backend: external-signer    # NEW delegating driver backend
      method: ephemeral           # ephemeral | <external-service> (§7)
      items: [ukis, shim]         # optional; default. Add `bootloader` for a grub chain (§5). See §5 for the item tokens.
      # bootloader: grub          # optional chain hint → appends `bootloader` (grub-only; §5)
      # publishCaCert: <path>     # default <output_dir>/<slug>.ca_cert.pem
```

```yaml
# image.yaml
signing: secureboot-ephemeral
```

Schema notes:
- the delegating backend is a new `SigningBackend` variant; `method` (enum) is required for it.
  `items` optional (§5). Any external-service sub-config is required only for that method.
- `SigningProfile::validate` gains the new arm (method present; service fields complete when the
  external-service method is chosen). Config-shape validation only; presence/capability probing is the
  build preflight.

## 5. `items`: request set vs sign set

`output.artifacts.items` is an **input** to the extract pass — IC only extracts what you *request*. So:

- **Request set (input, decided before extract):** `profile.items`, default **`[ukis, shim]`** — the
  **universally safe** set (see the `bootloader` rule for why it is not `[ukis, shim, bootloader]`).
  The IC item tokens are: `ukis`, `uki-addons`, `shim`, `bootloader`, `verity-hash`.
  Three rules:
  - **`uki-addons` is auto-included with `ukis`** — listing it explicitly is an error, so the default
    set does not name it.
  - **`verity-hash` is not auto-emitted** — IC extracts the dm-verity root hash **only** when
    `verity-hash` is in `items`. So tailor cannot detect verity from the emitted manifest (the manifest
    only contains what was requested). tailor stays config-opaque (it does not parse the user's
    `config:`), so verity inclusion is **explicit**: `verity-hash` in `profile.items`, or a small
    declared `verity: true` profile flag that tailor expands to append `verity-hash`. Not auto.
  - **`bootloader` is grub-specific and hard-errors without grub — so it is opt-in, not default.**
    The IC `bootloader` item unconditionally copies a **fixed per-arch grub EFI path** off the ESP
    (e.g. `grubaa64.efi` / `grubx64.efi`); it is **not** conditioned on the bootloader actually
    present. On a grub-less ESP that copy fails and IC **aborts the entire `output.artifacts` pass**
    with an artifact-copy error (there is no skip/continue branch as there is for `verity-hash`).
    Confirmed in IC source (`outputArtifacts()` in `artifactsinputoutput.go`). Therefore including
    `bootloader` on a systemd-boot image **hard-fails the build** — it cannot be a silent default.
    tailor stays config-opaque, so `bootloader` is added **explicitly** (`bootloader` in
    `profile.items`) or via a declared `bootloader: grub` chain hint that tailor expands to append it
    — mirroring the `verity: true` pattern. See the coverage note below.
- **Boot-loader coverage depends on the chain (grub vs systemd-boot):** for a `shim → grub → UKI`
  chain, adding `bootloader` (→ `[ukis, shim, bootloader]`) covers the whole signable EFI chain via IC
  extraction. For a `shim → systemd-boot → UKI` chain (no grub), the safe set is the default
  **`[ukis, shim]`**, and **systemd-boot's own EFI binary is not emitted by any current
  `output.artifacts` item** — so it cannot be signed through the IC-native extract → inject flow
  today. Signing systemd-boot under SB-enforcing therefore needs one of:
  - **(a)** an out-of-band **in-place ESP re-sign** of the systemd-boot EFI after the image is built,
    outside the IC-native inject flow — concretely: mount the image's ESP (loop-mount the built
    image), `sbsign` `systemd-boot*.efi` in place, unmount. This is the mechanism the older
    mount-based signer path provides; or
  - **(b)** a future upstream IC **`systemd-boot` `output.artifacts` item** (does not exist today),
    which would make the systemd-boot chain fully IC-native like the grub chain.

  **Default resolution:** because `bootloader` hard-errors on a grub-less ESP (confirmed, above),
  `[ukis, shim, bootloader]` is **not** a safe universal default. The default is **`[ukis, shim]`**;
  a **grub** target opts `bootloader` in (explicitly, or via `bootloader: grub`). Since tailor is
  config-opaque it cannot auto-detect the chain, so it does not silently add `bootloader`; a build
  that probes the ESP to auto-select per chain is a possible future enhancement, but the safe,
  opaque-preserving default is `[ukis, shim]` + explicit opt-in.
- **Sign set (what actually gets signed):** every entry in the emitted `inject-files.yaml`. The signer
  signs the whole manifest; tailor does not re-derive it. This keeps the sign step config-opaque and
  robust to IC adding artifact kinds.

**Inject-files CLI/schema (for the signer wiring):** the inject pass is `imagecustomizer inject-files
--build-dir <dir> --config-file <inject-files.yaml> --image-file <base> --output-image-file <out>
--output-image-format <fmt>` (flag is `--config-file`; `--build-dir` required). The manifest is a
top-level `injectFiles:` list (each entry `partition/source/destination/type`) with `previewFeatures:
[inject-files]`. Signing is **in place on `source`** (there is no separate `unsignedSource` field —
the `source`/`unsignedSource` wording in `2026-06-29-signing.md` is outdated and should be corrected
when the signer lands).

## 6. output.artifacts authorship — a deliberate change from 06-29

`2026-06-29-signing.md` §3 lists as a **non-goal**: *"tailor does not model or rewrite
`output.artifacts` — the user authors it in their `config:`."* This design **supersedes that specific
non-goal**: for a signed build, tailor **auto-authors** the `output.artifacts` extract directives (and
`previewFeatures: [output-artifacts, …]`, raw output) for the dedicated extract pass, derived
mechanically from `profile.items`. Rationale: requiring every user to hand-write IC preview
scaffolding to get a signed image defeats declarative signing; the directives are purely mechanical
and fully determined by the profile. tailor still never parses or rewrites the user's *functional*
`config:` — it only **adds** the extract directives for the extract pass, and produces the final image
via the `inject-files` pass over the user's real customized image, unchanged.

**Collision case:** because tailor generates a **dedicated** extract config (rather than editing the
user's config in place), a user's own `output.artifacts` does not apply to the extract pass. To avoid
silent surprise, if the user's `config:` already contains an `output.artifacts` block on a **signed**
cell, tailor **errors** ("remove `output.artifacts`; tailor authors it for signed builds") rather than
silently overriding or merging. This is the one intentional principle change, flagged for review.

## 7. External-service seam (next milestone)

An external production signing service reuses the same delegating `Signer`, differing only in the
signer config's key-source block and `preflight()`:
- **Config:** an environment-specific service sub-object (identity, key/cert references, per-item key
  codes). Supplied via the environment, **never committed to a workspace**.
- **No enrollment cert:** a stable production chain means `SigningResult.published_ca_cert` is `None`.
- **Non-reproducible:** production signatures typically embed a timestamp, so signed bytes are not
  reproducible (sign once, reuse the bytes). Consistent with `2026-06-29-signing.md` §9.

Ship after the ephemeral path is green.

## 8. Invariants & environment floor

- **IC version floor:** the design needs an IC that provides the `output-artifacts` + `inject-files`
  preview features (the signing extract pass does no package ops, so it needs only those). The
  toolchain container tailor drives must provide them. Note the single-binary constraint: when the
  same IC binary is also relied on for other preview features, that one binary must carry **all** of
  them (there is no per-pass binary selection).
- **Signer identity in the fingerprint:** per `2026-06-29-signing.md` §8, the signer identity feeds
  the per-cell fingerprint — here `backend` + `method`. The **ephemeral** method is intentionally
  **non-reproducible** (fresh cert per build), so the fingerprint tracks the *signing configuration*,
  not the signature bytes.
- **No host sudo:** the janitor normalizes IC's root-owned staging **before** the host sign step, so
  the signer runs unprivileged (`2026-06-29-signing.md` §7.7 / §9). The IC passes run in the toolchain
  container as usual.
- **tools-dir / build-dir isolation:** unchanged — `buildDirBase` off `/`, tools-dir isolation so IC
  cleanup can't reach host root (the wipe class of bug).
- **ca.pem publication:** to `<output_dir>/<slug>.ca_cert.pem` (never into the swept staging dir), the
  enrollment artifact for the (deferred) Secure Boot boot test.
- **Pinned signer:** where reproducibility/provenance matter, the environment should pin a specific
  signer version rather than track latest. tailor only preflights presence; the pin is an environment
  responsibility.

## 9. Correctness bar

"Fully works" =:
1. `tailor build <image> --cell <slug>` with an ephemeral signing profile runs extract → sign →
   `inject-files` → the requested disk format, and emits `<slug>.ca_cert.pem`.
2. The signed artifacts verify against the published cert: UKI/shim (and `bootloader` for a grub
   chain) are Authenticode-signed; the verity root hash carries a detached signature (when in the item
   set).
3. Host-sudo-free, with a toolchain IC that provides `output-artifacts`/`inject-files`.
4. (Deferred, test-wiring) enroll the cert into a firmware `db` and boot under Secure Boot.

## 10. Implementation plan / milestones

- **P1 — config + backend surface.** Add the delegating backend to `SigningBackend` +
  `method`/`items` fields to `SigningProfile`; extend `validate` and the `preflight_profile` capability
  checks (the signer + its host deps). *(config + preflight; no execution — does not touch the §6
  principle, cleared to start ahead of the principle sign-off.)*
- **P2 — extract-pass authoring.** Auto-generate the `output.artifacts` extract config from
  `profile.items` (§6), wired into the three-pass executor's first pass (raw output). **HOLD until the
  §6 non-goal supersession is signed off** — the one reversal that needs review.
- **P3 — the delegating `Signer` impl** in `tailor-sign` (write the signer config, run the signer,
  publish `ca.pem`); register it for the new backend.
- **P4 — the ephemeral end-to-end** (the §9 bar).
- **P5 — external-service seam** (§7) as a follow-up.

## 11. Open items

- **~~`bootloader` on a grub-less ESP — skip or error?~~ RESOLVED (2026-07-24):** IC's `bootloader`
  item **hard-errors** (does not skip) on a grub-less ESP — `outputArtifacts()` unconditionally copies
  a fixed per-arch grub EFI path and aborts the whole pass if it is absent (confirmed in IC source).
  Resolution folded into §5: the default is **`[ukis, shim]`** and `bootloader` is **opt-in** (explicit
  or via a `bootloader: grub` hint); it is never silently defaulted.
- **Host deps:** confirm the ephemeral signer's host dependencies are present (or installable) on the
  host running tailor.
