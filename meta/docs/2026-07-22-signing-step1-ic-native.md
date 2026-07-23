# tailor — Secure Boot signing via IC-native deferred signing (external-signer): complete design

> **Status:** Ready for review · _2026-07-23_ (was: Proposed/scoping, 2026-07-22) · design co-driven
> with the `peer-session` myimage/e2e track
>
> Complete, implementable design for tailor Secure Boot signing. tailor drives **IC-native deferred
> signing** — `output-artifacts` → sign → `inject-files` — with the sign step delegated to
> **external-signer** (the org-sanctioned IC-native signer; ephemeral for dev, remote-service for production). This
> is *exactly* tailor's existing three-pass signing model (`2026-06-29-signing.md`), so external-signer
> drops in as a **`Signer` backend** with **no change to the executor or the `Signer` port**. The
> correctness bar is a signed `tailor build myimage <cell>` producing a signed VHD + published `ca.pem`
> via external-signer **ephemeral**, end to end, on the arm64 build VM.
>
> Public-repo note: mechanism only. Internal package feeds/versions, remote-service key codes, managed-identity
> / Key-Vault / cert names, DRI emails, and internal branch/build identifiers are omitted.

## 1. The mechanism (verified against external-signer + IC)

Signing is **deferred** — it slots between two IC invocations using IC's own preview features
`output-artifacts` + `inject-files` (IC ≥ 0.14):

```
1. IC customize (extract pass)   config: previewFeatures:[output-artifacts,…] + output.artifacts:{items, path}, output raw
      → writes the UNSIGNED .efi artifacts into <artifacts> + an inject-files.yaml manifest   (no package ops → no --tools-dir)
2. sign the artifacts in place   external-signer sign-artifacts --build-dir <b> --config-file <sign-config.yaml>
      sign-config.yaml: input.artifactsPath=<artifacts>; signingMethod:{ ephemeral | remote-service }
3. IC inject-files               inject-files --config-file <artifacts>/inject-files.yaml --image-file <unsigned.raw> --output-image-file <signed.raw> --output-image-format raw
      → then qemu-img convert raw → fixed VPC VHD (Azure gallery)
```

external-signer signs, per the manifest IC emits: **UKI + UKI addons**, **shim**, **systemd-boot / grub**
(Authenticode), and the **dm-verity root hash** (detached PKCS#7). Two methods:

- **ephemeral** — self-signed x509 generated on the fly, **private key destroyed** after; public
  `ca.pem` captured for Secure Boot `db` enrollment. **Dev/test only** (unique cert per build). Host
  deps: `pesign`, `certutil` (nss-tools), `openssl`.
- **remote-service** — Microsoft production signing service; managed-identity auth against a Key-Vault OneCert,
  per-environment key codes. **Required for production/pre-release.**

The signable unit is exactly what IC (and tailor) already rebuild — the UKI + boot chain — so the
extract pass needs no packages (no tools-dir), and external-signer occupies the sign pass unchanged.

## 2. Reconciliation: external-signer is a `Signer` backend; the 3-pass executor is unchanged

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
`artifacts_dir`. That is precisely external-signer's unit of work, so **`External-signerSigner` is a drop-in
`Signer` impl**:

- **`preflight()`** — `external-signer` present (PATH or configured path); for `ephemeral`, the host deps
  `pesign`/`certutil`/`openssl`; for `remote-service`, identity/Key-Vault/key-code/DRI config completeness.
- **`sign(plan)`** — write a `sign-config.yaml` (`input.artifactsPath = plan.artifacts_dir`;
  `signingMethod: { ephemeral | remote-service }`; ephemeral `publicKeysPath` under a `plan.leaf_id`-scoped dir),
  run `external-signer sign-artifacts --build-dir <b> --config-file <sign-config.yaml>`, then return
  `SigningResult { published_ca_cert: Some(<ca.pem copied to plan.ca_cert_dest>) }` (ephemeral) or
  `None` (remote-service).

So the **three-pass executor (`2026-06-29-signing.md` §5) is unchanged**: external-signer occupies the
existing "host-side sign step"; the customize (extract) and `inject-files` passes are as designed. The
raw `openssl`+`sbsign` backend the 06-29 doc describes remains a **peer `Signer` impl** that loops
over the same `inject-files.yaml` internally. Both satisfy the identical port — this is the whole
point of the port.

### 2.1 Two axes: driver vs key-source

The existing `SigningBackend` enum (`local-test-ca` / `keypair` / `azure-key-vault`) models a
**key source** on the assumption tailor itself is the signing driver. external-signer adds a **driver**
axis (tailor delegates the whole sign step to an external tool that has *its own* key sources). The
config models this as a new backend `external-signer` whose `method` sub-selects external-signer's key source:

| Concept | tailor-driven | external-signer-driven |
| --- | --- | --- |
| driver | tailor loops `openssl`/`sbsign` per artifact | delegate to `external-signer sign-artifacts` |
| ephemeral key | `backend: local-test-ca` | `backend: external-signer, method: ephemeral` |
| BYO key | `backend: keypair` | (external-signer has no BYO-file method today) |
| remote/prod | `backend: azure-key-vault` (future) | `backend: external-signer, method: remote-service` |

Keep all of them; `external-signer` is the ACL/myimage default (§4). We do **not** delete the raw backend —
it preserves non-external signer environments and the S3 pure-Rust north star (`2026-06-29-signing.md` §11).

## 3. Resolved open questions (OQ1–OQ4)

- **OQ1 — backend split: add `external-signer` as a first-class `Signer` backend; keep the raw
  `openssl`+`sbsign` backend.** external-signer is the default for ACL/myimage (org-sanctioned, matches the
  tailor-less ship-today path); the raw backend stays for non-external signer envs and the pure-Rust goal.
  *(Agreed with downstream.)*
- **OQ2 — `items`: profile declares `items`, default `[ukis, shim, bootloader]`; the emitted
  `inject-files.yaml` is the source of truth for what gets *signed*.** But see §5 — the request set
  (`output.artifacts.items`, an **input** to the extract pass) and the sign set (the emitted manifest)
  are different phases; verityHash must be *requested* up front, not only detected after. *(Refines
  downstream's OQ2.)*
- **OQ3 — binary sourcing: preflight `external-signer` on PATH or a configured path, exactly like
  `openssl`/`sbsign`.** Acquisition (feed download + version pin) is **pipeline plumbing**, not
  tailor's job — keeps tailor environment-agnostic and free of a package-source dependency.
  *(Agreed.)*
- **OQ4 — remote-service: `method: ephemeral | remote-service` under `external-signer`; ephemeral is the complete step-1,
  remote-service is the documented seam (§7).** HPC ships unsigned/SB-off, so remote-service isn't on the myimage
  validation critical path; ephemeral is the correctness bar. *(Agreed.)*

## 4. Config surface

Extend `SigningProfile` / `SigningBackend` (`crates/tailor-config/src/schema.rs`):

```yaml
# tailor.yaml
signing:
  default: myimage-ephemeral
  profiles:
    myimage-ephemeral:
      backend: external-signer         # NEW driver backend (kebab-case, matching existing tokens)
      method: ephemeral           # ephemeral (step 1) | remote-service (§7)
      items: [ukis, shim, bootloader]   # optional; default this set (+ verityHash when verity-sealed, §5)
      # publishCaCert: <path>     # default <output_dir>/<slug>.ca_cert.pem
      # --- remote-service only (§7): ---
      # remote-service: { clientId, keyVaultName, certificateName, keyCodes: {uki, shim, bootloader, verityHash}, driEmails: [...] }
```

```yaml
# myimage/image.yaml
signing: myimage-ephemeral
```

Schema notes:
- `backend: external-signer` is a new `SigningBackend` variant; `method` (enum `ephemeral|remote-service`) is
  required for it. `items` optional (§5). `remote-service:` sub-object required iff `method: remote-service`.
- `SigningProfile::validate` gains the `external-signer` arm (method present; remote-service fields complete when
  `method: remote-service`). This is config-shape validation only; presence/capability probing is the build
  preflight.

## 5. `items`: request set vs sign set (the one subtlety to get right)

`output.artifacts.items` is an **input** to the extract pass — IC only extracts what you *request*.
So:

- **Request set (input, decided before extract):** `profile.items`, defaulting to
  `[ukis, shim, bootloader]`. **verityHash must be added to the request set when the image is
  verity-sealed** — tailor cannot "detect it from the emitted manifest" because the manifest only
  contains what was requested. tailor stays **config-opaque** (`2026-06-29-signing.md` §8): it does
  **not** parse the user's `config:` to discover verity. So verity inclusion is either (a) explicit in
  `profile.items`, or (b) driven by a small declared signal (e.g. a `verity: true` profile flag).
  **Open item to verify with the `ic` agent:** does IC's `output-artifacts` emit the verity root-hash
  artifact automatically for a verity-sealed image, or only when `verityHash`/`verity` is in `items`?
  The answer picks (a) vs (b). Until confirmed, default the request set to `[ukis, shim, bootloader]`
  and let the profile add `verityHash` explicitly.
- **Sign set (what actually gets signed):** every entry in the emitted `inject-files.yaml`. external-signer
  signs the whole manifest; tailor does not re-derive it. This keeps the sign step config-opaque and
  robust to IC adding artifact kinds.

## 6. output.artifacts authorship — a deliberate, documented change from 06-29

`2026-06-29-signing.md` §3 lists as a **non-goal**: *"tailor does not model or rewrite
`output.artifacts` — the user authors it in their `config:`."* This design **supersedes that specific
non-goal**: for a signed build, **tailor auto-authors the `output.artifacts` extract directives** (and
`previewFeatures: [output-artifacts, …]`, raw output) for the extract pass, derived mechanically from
`profile.items`. Rationale: requiring every user to hand-write IC preview scaffolding to get a signed
image defeats "declarative signing"; the directives are purely mechanical and fully determined by the
profile. tailor still never parses or rewrites the user's *functional* `config:` — it only **adds** the
extract directives for the dedicated extract pass. (The final image is produced by the `inject-files`
pass over the user's real customized image, unchanged.) This is the one intentional principle change,
called out so reviewers see it.

**Collision case (user also hand-authored `output.artifacts`):** because tailor generates a **dedicated
extract config** for the extract pass (rather than editing the user's config in place), the user's own
`output.artifacts`, if any, does not apply to the extract pass — tailor's generated directives are
authoritative there. To avoid silent surprise, the recommended behavior is: if the user's `config:`
already contains an `output.artifacts` block on a **signed** cell, tailor **errors** with a clear
message ("remove `output.artifacts`; tailor authors it for signed builds") rather than silently
overriding or attempting a merge. (Merging is rejected — reconciling two artifact-item sets is exactly
the config-modeling tailor avoids.)

## 7. remote-service seam (next milestone, not step 1)

`method: remote-service` reuses the same `External-signerSigner`, differing only in the `sign-config.yaml`
`signingMethod` block and `preflight()`:

- **Config:** the `remote-service:` sub-object (managed-identity client id, Key-Vault name, cert name, per-item
  key codes, DRI emails). All environment-specific — **not** committed to a public workspace; supplied
  via the pipeline/environment.
- **Auth:** `az login` as the managed identity before signing (pipeline step); `preflight()` checks
  config completeness, not live auth.
- **No `ca.pem`:** remote-service signs against a stable Microsoft chain, so `SigningResult.published_ca_cert`
  is `None` (no per-build enrollment cert).
- **Non-repro:** remote-service embeds an RFC-3161 timestamp → signed bytes aren't reproducible (sign once,
  reuse the bytes downstream). Consistent with `2026-06-29-signing.md` §9.

Ship after the ephemeral e2e is green.

## 8. Invariants & environment floor

- **IC version floor + toolchain provenance.** `output-artifacts` + `inject-files` need **IC ≥ 0.14**;
  the signing extract pass does no package ops, so it needs only that floor. **Important (myimage):** the
  myimage SKU configs use ACL-specific IC surface — an `acl:` config block and related preview features
  (ACL repartitioning / verity re-init / OEM-id) — which **stock IC rejects**. So the toolchain
  container must be built from the **custom IC branch that carries that ACL support**, *not* the
  generic internal-registry IC drop. This reframes the floor question into a **single-binary risk**: the *same*
  custom IC branch that adds ACL repartitioning must **also** carry `output-artifacts` + `inject-files`
  (≥ 0.14), because one binary must both **repartition and sign**. If the ACL branch predates 0.14 or
  lacks those preview features, build-IC and sign-IC diverge — a real design risk to resolve up front
  (open item #2).
- **Signer identity in the fingerprint.** Per `2026-06-29-signing.md` §8, the signer identity feeds the
  per-cell fingerprint. For this backend that identity is **`backend` + `method`** (e.g.
  `external-signer/ephemeral`). Note the **ephemeral** method is intentionally **non-reproducible** (a fresh
  cert per build), so signed *bytes* are not stamped for reproducibility; the fingerprint tracks the
  *signing configuration*, not the signature bytes (consistent with the §9 non-repro stance).
- **No host sudo:** the janitor normalizes IC's root-owned staging **before** the host sign step, so
  `external-signer` runs unprivileged (`2026-06-29-signing.md` §7.7 / §9). The IC passes run in the
  toolchain container as usual.
- **tools-dir / build-dir isolation:** unchanged — `buildDirBase` off `/`, tools-dir isolation so IC
  cleanup can't reach host root (the wipe class of bug).
- **ca.pem publication:** to `<output_dir>/<slug>.ca_cert.pem` (never into the swept staging dir), the
  enrollment artifact for the (deferred) Secure Boot boot test.
- **Pinned signer for provenance.** Since org-trust is the whole premise, the pipeline must **pin a
  specific `external-signer` version** (acquired from its internal feed) rather than tracking latest —
  provenance/reproducibility of the signing tool itself. tailor only preflights presence (OQ3); the
  pin is a pipeline responsibility, but the design calls it out as required for a trusted build.

## 9. Correctness bar — myimage ephemeral e2e

"Fully works" =:

1. `tailor build myimage --cell <slug>` with `signing: myimage-ephemeral` runs extract → `external-signer
   sign-artifacts` (ephemeral) → `inject-files` → fixed-VPC VHD, and emits `<slug>.ca_cert.pem`.
2. The signed artifacts verify against the published cert: UKI/shim/bootloader are Authenticode-signed;
   the verity root hash carries a detached signature (when in the item set).
3. Runs on the arm64 build VM, host-sudo-free, with the IC binary from the **custom ACL IC branch**
   (which must carry both repartitioning and `output-artifacts`/`inject-files` — §8).
4. (Deferred, test-wiring) enroll `ca.pem` into an OVMF `db` and boot under QEMU Secure Boot.

## 10. Implementation plan / milestones

- **P1 — config + backend surface.** Add `external-signer` to `SigningBackend` + `method`/`items`/`remote-service`
  fields to `SigningProfile`; extend `validate` and the `preflight_profile` capability checks
  (`external-signer` + ephemeral host deps). *(config + preflight; no execution — does not touch the §6
  principle, so cleared to start ahead of Paco's review.)*
- **P2 — extract-pass authoring.** Auto-generate the `output.artifacts` extract config from
  `profile.items` (§6), wired into the three-pass executor's first pass (raw output). **HOLD until
  Paco blesses the §6 non-goal supersession** — this is the reversal he needs to sign off.
- **P3 — `External-signerSigner`.** Implement the `Signer` impl in `tailor-sign` (write `sign-config.yaml`,
  run `external-signer sign-artifacts`, publish `ca.pem`); register it for `backend: external-signer`.
- **P4 — myimage ephemeral e2e** (the §9 bar) on the arm64 VM; iterate to green with `peer-session`.
- **P5 — remote-service seam** (§7) as a follow-up milestone.

## 11. Open items (to close before/while implementing)

1. **verityHash extraction (§5):** does IC auto-emit the verity root-hash artifact, or must it be in
   `output.artifacts.items`? → picks the `items` default behavior. *(verify with `ic`)*
2. **Toolchain IC branch carries both repartitioning *and* signing (§8):** myimage needs the **custom
   IC branch** with ACL support (`acl:` config block + repartitioning/verity/OEM preview features) —
   stock/internal-registry IC rejects those. Confirm that **same** branch also carries `output-artifacts` +
   `inject-files` (≥ 0.14); if not, build-IC and sign-IC diverge (single-binary risk). *(owner:
   peer-session + ic)*
3. **external-signer arm64 host deps:** confirm `pesign`/`certutil`/`openssl` are present (or installable)
   on the arm64 build VM for ephemeral.
4. **`items` token names:** confirm IC's exact `output.artifacts.items` tokens (`ukis`, `shim`,
   `bootloader`, and the verity token) match external-signer's expectations end to end.
