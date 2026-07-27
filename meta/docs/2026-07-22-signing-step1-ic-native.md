# tailor — IC-native deferred signing (execution design)

> **Status:** Ready for review · _2026-07-27_ (rubber-duck pass folded in — see §11 for open
> decisions surfaced: fail-closed chain completeness, verity completeness, fingerprint/caching, the
> cert-contract-vs-port question, and §6 authoring sign-off)
>
> Complete, implementable design for how tailor produces Secure Boot–signed images. tailor drives
> **IC-native deferred signing** — an Image Customizer `output-artifacts` extract pass → a host-side
> sign step → an IC `inject-files` pass. This is exactly tailor's existing three-pass signing model
> (`2026-06-29-signing.md`), so an IC-native signing tool (**`ic-sign`** in this doc) plugs in as a
> **`Signer` backend** with **no change to the executor or the `Signer` port**. Scope of this design:
> a signed `tailor build` produces a signed disk image plus a published enrollment certificate, using
> a self-signed (ephemeral) key.

## 0. Delta from what is checked in

The checked-in signing design (`2026-06-29-signing.md`) and code (`crates/tailor-core/src/signing.rs`
foundation + the `Signer` port in `ports.rs` + the `SigningBackend` enum in
`tailor-config/src/schema.rs`) already give us the 3-pass model, the `Signer`/`SigningPlan` port, and
key-source backends (`local-test-ca` / `keypair` / `azure-key-vault`) — but only as **config +
preflight** (no execution). This design adds four things and changes one principle; **everything else
is reused unchanged**:

1. **An `ic-sign` delegating backend (new).** The checked-in `SigningBackend` variants model a
   **key source**, assuming tailor itself drives signing (loop `openssl`/`sbsign` per artifact). This
   design adds a backend that **delegates the whole host-side sign step** to ic-sign (an IC-native signing tool)
   (a *driver* axis), with `method: ephemeral | service` sub-selecting the tool's key
   source. See §2.1 (two axes) and §2.2 (the tool contract). The built-in backend stays as a peer.
2. **`items` request-set model + the `bootloader`/`verity-hash` rules (new).** The checked-in design
   didn't specify what to extract; this pins the default `[ukis, shim]`, the request-set-vs-sign-set
   split, `bootloader` as opt-in (it hard-errors on a grub-less ESP), and explicit `verity-hash`
   (§5).
3. **IC preview-feature floor + single-binary constraint (new).** States the `output-artifacts` +
   `inject-files` floor and that one IC binary must carry all needed preview features (§8).
4. **`output.artifacts` authoring — the one principle change (§6).** `2026-06-29-signing.md` §3
   made it a **non-goal** for tailor to author `output.artifacts` (the user was to write it), and the
   current executor enforces that (it *errors* if a signed cell has no `output.artifacts`). This
   design **supersedes that**: for a signed build tailor auto-adds the directives to the cell's
   rendered config (pass 1 — no separate extract config). This is the only reversal and is flagged for
   review (gates milestone P2).

**Reused unchanged:** the 3-pass executor (`§5`), the `Signer` trait + `SigningPlan`/`SigningResult`
port granularity, `ca.pem` publication, the no-sudo / tools-dir-isolation invariants, and the
fingerprint/lockfile treatment. No port or executor signature changes — the new backend is a drop-in
`Signer` impl.

## 1. The mechanism

Signing is **deferred** — not baked into the customize run. It slots between two IC invocations using
IC's own preview features `output-artifacts` + `inject-files`:

```
1. IC customize (pass 1)         the user's real config + auto-added output.artifacts:{items, path}; previewFeatures:[output-artifacts,…]; output raw
      → produces the unsigned raw image AND writes the UNSIGNED boot artifacts into <artifacts> + an inject-files.yaml manifest
2. host-side sign (in place)     ic-sign signs the extracted artifacts, keyed by the emitted manifest
3. IC inject-files               inject-files --config-file <artifacts>/inject-files.yaml --image-file <unsigned.raw> --output-image-file <signed.raw> --output-image-format raw
      → then convert raw → the requested disk format
```

The signable set is the boot chain IC (and tailor) already rebuild — the UKI(s), UKI addons, the boot
loader (systemd-boot / grub), and the dm-verity root hash. **Pass 1 is the full customize** — the
user's actual config (whatever package ops, UKI rebuild, verity re-seal it declares), extended with
`output.artifacts` so the *same* invocation that produces the unsigned raw image also extracts the
boot artifacts and emits `inject-files.yaml`. It therefore runs with whatever tools-dir that config
already needs; there is **no separate extraction-only pass**. This single-pass identity is essential:
the bytes signed in step 2 are exactly the bytes pass 1 baked into `unsigned.raw`, and step 3 injects
the signed replacements back into that same image. The signer occupies the middle step and signs in
place. (This matches the implemented three-pass executor in `crates/tailor-exec/src/executor.rs`.)

**Key sources.** Two modes, orthogonal to the mechanism:
- **ephemeral** — a self-signed certificate generated on the fly, private key destroyed after signing;
  the public certificate is published for Secure Boot `db` enrollment. Dev/test (unique cert per
  build). This design's scope.
- **service** — a remote signing service against a stable certificate chain, for
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
`artifacts_dir`, which is the natural unit for a signer that signs the extracted set in one pass. So
the **`ic-sign` backend is a drop-in `Signer` impl**:

- **`preflight()`** — the `ic-sign` binary is present (PATH or configured path) and its host
  dependencies are available.
- **`sign(plan)`** — write `ic-sign`'s config (pointing at `plan.artifacts_dir`, key-source method,
  and a `plan.leaf_id`-scoped output dir), invoke `ic-sign` over the artifacts, and return
  `SigningResult { published_ca_cert }` (the enrollment cert, for ephemeral) or `None`.

So the **three-pass executor (`2026-06-29-signing.md` §5) is unchanged**: the signer occupies the
existing host-side sign step; the customize (extract) and `inject-files` passes are as designed. The
built-in `openssl`+`sbsign` backend the 06-29 doc describes remains a **peer `Signer` impl** that
loops over the same `inject-files.yaml` internally. Both satisfy the identical port — the whole point
of the port.

### 2.1 Two axes: driver vs key-source

The existing `SigningBackend` enum (`local-test-ca` / `keypair` / `azure-key-vault`) models a **key
source** on the assumption tailor itself is the signing driver. The `ic-sign` backend adds a
**driver** axis — tailor hands the whole sign step to an external tool that has its own key sources.
The config models this as a new backend whose `method` sub-selects the tool's key source:

| Concept | tailor-driven | delegated-signer |
| --- | --- | --- |
| driver | tailor loops `openssl`/`sbsign` per artifact | delegate to `ic-sign` |
| ephemeral key | `backend: local-test-ca` | `method: ephemeral` |
| BYO key | `backend: keypair` | (tool-dependent) |
| remote/prod | `backend: azure-key-vault` (future) | `method: service` |

Keep all of them; the delegating backend is one more peer. We do **not** remove the built-in backend —
it preserves environments without `ic-sign` and the S3 pure-Rust north star
(`2026-06-29-signing.md` §11).

### 2.2 The `ic-sign` contract (what the delegating backend integrates against)

The `ic-sign` backend integrates against a small, well-defined tool interface — the contract `ic-sign`
exposes. tailor targets that **contract**, so any signing tool exposing it works; `ic-sign` is the
concrete IC-native tool this design assumes. The contract maps cleanly onto the `Signer` port and onto
IC's `output-artifacts` / `inject-files` shape:

1. **Invocation — one "sign these artifacts" command.** `ic-sign` exposes a single non-interactive
   subcommand (conceptually `sign-artifacts`) that accepts:
   - a **work/build dir** (scratch),
   - a **signer-config file** (see (4)),
   - the **artifacts path** — the directory IC's extract pass populated, and
   - for the ephemeral method, an **output path for the public key(s)/cert**.
2. **Discovery — `ic-sign` reads IC's emitted manifest, not tailor's opinion.** It signs the boot
   artifacts present in the artifacts dir as described by the `inject-files.yaml` IC emitted (UKI(s),
   UKI addons, shim, the boot loader, and the dm-verity root hash when extracted). tailor does not
   enumerate them; the manifest is the source of truth (§5, sign set).
3. **In-place signing.** `ic-sign` writes signatures **back into the artifacts dir in place**, so the
   subsequent IC `inject-files` pass injects the now-signed bytes into the image. No image mounting.
   Signature formats by artifact kind: **Authenticode** (PE) for UKI / UKI-addons / shim / boot
   loader; **detached PKCS#7** for the dm-verity root hash.
4. **Key source selected in the signer-config file — at least two methods:**
   - **ephemeral** — generate a self-signed cert on the fly, sign, destroy the private key, and emit
     the **public cert** to the requested output path (for Secure Boot `db` enrollment). Config is
     just the public-key output location.
   - **service** (§7) — sign against a stable certificate chain held in a secret store;
     config carries an identity, the key/cert references, and per-artifact-kind key selectors. Emits
     no enrollment cert.
5. **Exit contract.** Non-zero exit on any signing failure; the signed artifacts and (ephemeral) the
   public cert are the only outputs tailor consumes.

**How the `Signer` port maps onto this contract** (the delegating impl in `tailor-sign`):

| `Signer` / `SigningPlan` | ic-sign backend behavior |
| --- | --- |
| `preflight()` | tool binary resolvable (PATH/configured) + its host deps present; config-shape valid |
| `sign(plan)` | render a signer-config file (method = `ephemeral`/external; ephemeral public-key out = a `plan.leaf_id`-scoped dir), then invoke the tool's `sign-artifacts` with `--artifacts-path plan.artifacts_dir` and the build dir |
| `plan.inject_files_yaml` | the manifest the tool reads to know what to sign (2) — tailor passes/points at it, stays opaque to its contents |
| `plan.artifacts_dir` | the tool's artifacts path (signed in place) |
| `plan.leaf_id` | scopes the per-cell signer-config + ephemeral key output so parallel cells never share a key |
| `plan.ca_cert_dest` / `SigningResult.published_ca_cert` | ephemeral: the emitted public cert, copied to `<output_dir>/<slug>.ca_cert.pem`; service: `None` |

Because the contract is exactly "sign the IC-extracted artifact set in place, keyed by the emitted
manifest, with an ephemeral or a remote key source," it slots into the existing host-side sign step of
the 3-pass executor with **no port change** — `ic-sign`'s process invocation replaces the built-in
`openssl`/`sbsign` loop, nothing else moves. `ic-sign` exposes exactly this contract; because tailor
integrates against the contract (not `ic-sign`'s internals), any other tool exposing the same
interface would drop in unchanged.

## 3. Resolved design decisions

- **Backend split:** add the `ic-sign` as a first-class `Signer` backend; **keep**
  the built-in `openssl`+`sbsign` backend. `ic-sign` is the default where it is the
  sanctioned path; the built-in stays as fallback and for the pure-Rust goal.
- **`items` default `[ukis, shim]`** (universally safe — see §5; `bootloader` is opt-in because it
  hard-errors on a grub-less ESP); the emitted `inject-files.yaml` is the source of truth for what
  gets *signed*. But the request set and sign set are different phases — see §5.
- **Binary sourcing:** preflight the signer on PATH or a configured path, exactly like
  `openssl`/`sbsign`. Acquisition (download + version pin) is environment/pipeline plumbing, not
  tailor's job — keeps tailor environment-agnostic.
- **Key source:** `method: ephemeral | service`; ephemeral is this design, the
  service method is the documented seam (§7).

## 4. Config surface

Extend `SigningProfile` / `SigningBackend` (`crates/tailor-config/src/schema.rs`):

```yaml
# tailor.yaml
signing:
  default: secureboot-ephemeral
  profiles:
    secureboot-ephemeral:
      backend: ic-sign    # NEW delegating driver backend
      method: ephemeral           # ephemeral | service (§7)
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
  `items` optional (§5). Any service sub-config is required only for that method.
- `SigningProfile::validate` gains the new arm (method present; service fields complete when the
  service method is chosen). Config-shape validation only; presence/capability probing is the
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
    only contains what was requested). tailor stays **config-opaque** — meaning it does no *typed
    semantic* modeling of the user's IC config (it does, today, structurally inspect and rewrite the
    generic YAML tree: `output_artifacts.rs` already detects an `output.artifacts` block and relocates
    its path). So verity inclusion is **explicit**: `verity-hash` in `profile.items`, or a small
    declared `verity: true` profile flag that tailor expands to append `verity-hash`. Not auto.
    **⚠ Correctness caveat (open item):** if the image *is* verity-sealed but the profile omits
    `verity-hash`, the root hash is never extracted or signed, yet the build still succeeds and is
    presented as "signed" — a silent hole (an unverifiable verity chain under Secure Boot). A signed
    profile should therefore require an **explicit** verity decision (`verity: true|false`) rather
    than defaulting to absent. See Open Items.
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
non-goal**: for a signed build, tailor **auto-adds** the `output.artifacts` directives (and
`previewFeatures: [output-artifacts, …]`, raw output) to the cell's **rendered config** (the same
working copy pass 1 runs), derived mechanically from `profile.items`. Rationale: requiring every user
to hand-write IC preview scaffolding to get a signed image defeats declarative signing; the directives
are purely mechanical and fully determined by the profile. There is **no separate extract config** —
tailor injects the directives into the one full customize pass (§1). It does no *typed semantic*
modeling of the user's config; it only structurally **adds** the `output.artifacts` mapping (the same
class of structural rewrite `output_artifacts.rs` already performs), leaving every functional
directive the user wrote intact.

> **Current-state note:** the implemented executor does **not** yet auto-add these directives — it
> **requires** the user to author `output.artifacts` and errors otherwise
> (`executor.rs`: *"image requests `signing:` but its IC config declares no `output.artifacts`"*).
> This section proposes reversing that. That reversal is milestone P2 and is the one change gated on
> the author's sign-off.

**Collision case:** if the user's rendered `config:` already contains an `output.artifacts` block on a
**signed** cell, tailor's auto-added directives would conflict with it. tailor detects this with the
existing structural check (`output_artifacts::uses_output_artifacts()`) and **errors** ("remove
`output.artifacts`; tailor authors it for signed builds") rather than silently overriding or merging.
(Detection is a narrow structural inspection of the generic YAML — consistent with "no typed semantic
modeling," not with "never inspects.") This is the one intentional principle change, flagged for
review.

## 7. Service seam (next milestone)

An `ic-sign` production run against a remote signing service reuses the same `ic-sign`
backend, differing only in `ic-sign`'s key-source block (`method: service`) and
`preflight()`:
- **Config:** an environment-specific service sub-object (identity, key/cert references, per-item key
  codes). Supplied via the environment, **never committed to a workspace**.
- **No enrollment cert:** a stable production chain means `SigningResult.published_ca_cert` is `None`.
- **Non-reproducible:** production signatures typically embed a timestamp, so signed bytes are not
  reproducible (sign once, reuse the bytes). Consistent with `2026-06-29-signing.md` §9.

Ship after the ephemeral path is green.

## 8. Invariants & environment floor

- **IC version floor:** the design needs an IC that provides the `output-artifacts` + `inject-files`
  preview features. Pass 1 is the full customize (§1), so the toolchain container must additionally
  provide whatever the user's config needs (package ops, tools-dir, etc.) — the signing feature does
  not *reduce* the toolchain requirement, it only *adds* the two preview features. Note the
  single-binary constraint: when the same IC binary is also relied on for other preview features, that
  one binary must carry **all** of them (there is no per-pass binary selection).
- **Signer identity in the fingerprint:** the per-cell fingerprint (`2026-06-29-signing.md` §8) must
  include the full resolved signing identity so a config change reliably rebuilds the cell. That is
  `backend` + `method` + the **normalized request set** (`items` after `verity`/`bootloader` hint
  expansion) + the chain/verity decision + (for `service`) the stable key/cert identity + the pinned
  signer version. **Current-state gap:** `fingerprint.rs` today hashes an `inject` bool but **not** the
  signing profile at all — this must be extended before signed caching is trustworthy. The
  **ephemeral** method is intentionally **non-reproducible** (fresh cert per build), so its fingerprint
  tracks the *signing configuration*, not the signature bytes; because a cached, differently-signed
  image would then read as up-to-date, ephemeral signed cells must treat the image **and** its emitted
  `ca_cert.pem` as one bundle (both must exist to be up-to-date), or disable caching / add a
  per-invocation nonce if "fresh cert every build" is a hard requirement. See Open Items.
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
0. **The complete executable trust chain is signed** for the target's boot chain (§5) — not merely
   "IC did not error." A **grub** target must include `bootloader`; a **systemd-boot** target has no
   IC-extractable boot-loader item, so systemd-boot's own EFI is *not* signed by this flow (§5(a)) —
   such a target must not be presented as a fully Secure Boot–signed image without the out-of-band ESP
   re-sign. tailor should **fail closed** on an unresolved/unsupported chain rather than emit a
   partially-signed image (see Open Items).
1. `tailor build <image> --cell <slug>` with an ephemeral signing profile runs pass 1 → sign →
   `inject-files` → the requested disk format, and emits `<slug>.ca_cert.pem`.
2. The signed artifacts verify against the published cert: UKI/shim (and `bootloader` for a grub
   chain) are Authenticode-signed; the verity root hash carries a detached signature (when the image
   is verity-sealed and `verity-hash` is in the item set — see the verity caveat in §5).
3. Host-sudo-free, with a toolchain IC that provides `output-artifacts`/`inject-files`.
4. (Deferred, test-wiring) enroll the cert into a firmware `db` and boot under Secure Boot.

## 10. Implementation plan / milestones

- **P1 — config + backend surface.** Add the `ic-sign` backend to `SigningBackend` +
  `method`/`items` fields to `SigningProfile`; extend `validate` and the `preflight_profile` capability
  checks (the signer + its host deps). *(config + preflight; no execution.)* **Coupling caveat:** the
  `items`/`verity`/`bootloader`-hint shape P1 stabilizes only makes sense if §6 (tailor owns
  `output.artifacts` authoring) is accepted — if §6 is rejected, these fields are redundant with the
  user's hand-authored block. Resolve §6 and the chain/verity semantics **before** freezing the P1
  schema, even though P1 writes no execution code.
- **P2 — output.artifacts authoring.** Auto-add the `output.artifacts` directives to the cell's
  rendered config from `profile.items` (§6), wired into pass 1 of the three-pass executor. **HOLD until
  the §6 non-goal supersession is signed off** — the one reversal that needs review. Until then the
  executor's current "user must author `output.artifacts`" behavior stands.
- **P3 — the `ic-sign` `Signer` impl** in `tailor-sign` (write the signer config, run `ic-sign`,
  publish `ca.pem`); register it for the new backend. Unit-testable against manifest fixtures without
  P2; a real end-to-end needs P2 (or hand-authored `output.artifacts` test configs).
- **P4 — the ephemeral end-to-end** (the §9 bar).
- **P5 — service seam** (§7) as a follow-up.

## 11. Open items

- **~~`bootloader` on a grub-less ESP — skip or error?~~ RESOLVED (2026-07-24):** IC's `bootloader`
  item **hard-errors** (does not skip) on a grub-less ESP — `outputArtifacts()` unconditionally copies
  a fixed per-arch grub EFI path and aborts the whole pass if it is absent (confirmed in IC source).
  Resolution folded into §5: the default is **`[ukis, shim]`** and `bootloader` is **opt-in** (explicit
  or via a `bootloader: grub` hint); it is never silently defaulted.
- **Host deps:** confirm the ephemeral signer's host dependencies are present (or installable) on the
  host running tailor.
- **[decision] Fail-closed boot-chain completeness (§9.0).** Should a signed profile require a declared
  boot chain (`grub` → require `bootloader`; `systemd-boot` → reject the IC-native flow until the
  out-of-band ESP re-sign exists; unknown → reject)? Today `[ukis, shim]` can complete on a
  systemd-boot image with systemd-boot left unsigned yet be labeled "signed." Recommend fail-closed.
- **[decision] Verity completeness (§5).** Require an explicit `verity: true|false` on signed profiles
  (no implicit "absent") so a verity-sealed image can't silently ship an unsigned root hash. If we
  won't require it, document the sharp edge loudly.
- **[decision] Fingerprint contents + ephemeral caching (§8).** Extend `fingerprint.rs` to hash the
  resolved signing identity (backend/method/normalized items/chain/verity/cert identity/signer
  version), and define the up-to-date rule for ephemeral cells (image+cert bundle, or nonce/no-cache).
  Without this, signed cells can be wrongly skipped.
- **[decision] `ic-sign` cert contract vs the port.** §2.2 mentions "public key(s)/cert" (plural) and a
  `leaf_id`-scoped output dir, but `SigningPlan.ca_cert_dest`/`SigningResult.published_ca_cert` are a
  single path. Confirm ephemeral emits exactly **one** enrollment cert per cell (→ single path is
  fine, and "unique cert per build" means per cell); if multiple certs or build-level artifacts are
  ever needed, the port must return a set/dir — which would break the "no port change" headline. Also:
  the emitted cert is a self-signed leaf, so "CA cert" naming may be a misnomer.
- **[decision] Conflict semantics for `items` vs `bootloader: grub` / `verity: true`.** Two ways to
  request the same token. Define: hint expansion is idempotent with an explicit `items` entry;
  contradictions (`verity: false` + `verity-hash` in `items`) are validation errors; fingerprint uses
  the normalized set.
- **[robustness] `ic-sign` atomicity & fail-closed signing.** Non-zero exit isn't enough for an
  in-place multi-file op: require it to hard-fail on unrecognized manifest entries, sign into a shadow
  tree + verify every entry + atomically replace, and publish the enrollment cert only after all
  artifacts verify. Add a post-sign verification step before `inject-files`. Define partial-failure
  cleanup.
- **[robustness] Manifest/path integrity.** The emitted `inject-files.yaml` is trusted by the
  privileged `inject-files` pass; canonicalize each `source`, require it to stay under `artifacts_dir`
  (reject `..`/symlink escapes), and verify the manifest is unchanged between sign and inject.
- **[robustness] Ephemeral key crash-cleanup & work-dir isolation.** "Destroy the private key after
  signing" must cover crash/SIGKILL: mode-0700 run-unique scratch, orphan cleanup, no private material
  in the public-cert dir, and a run-unique (not just `leaf_id`) work path. State whether concurrent
  builds of the same cell are supported.
- **[cleanup] Correct the stale port doc-comment** in `ports.rs` (`Signer::sign` still says
  "`unsignedSource` → `source`"; §5 establishes there is no `unsignedSource` — sign in place on
  `source`).
