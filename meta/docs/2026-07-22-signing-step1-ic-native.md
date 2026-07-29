# tailor — IC-native deferred signing (execution design)

> **Status:** e2e-proven · _2026-07-29_ · the ephemeral `[ukis]` path built + signed + verified
> end-to-end on a real base (extract → `ic-sign` sign → inject; the output UKI verifies against the
> emitted enrollment cert). Model: **tailor reads the user's `output.artifacts`, it never authors it**
> (see §5/§6). Remaining open decisions in §11 (verity completeness, cert-contract-vs-port,
> fail-closed chain) are refinements, not blockers.
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
preflight** (no execution). This design adds **two** things; **every 2026-06-29 principle and
everything else is reused unchanged**:

1. **An `ic-sign` delegating backend (new).** The checked-in `SigningBackend` variants model a
   **key source**, assuming tailor itself drives signing (loop `openssl`/`sbsign` per artifact). This
   design adds a backend that **delegates the whole host-side sign step** to `ic-sign` (an IC-native
   signing tool) — a *driver* axis — with `method: ephemeral | service` sub-selecting the tool's key
   source. See §2.1 (two axes) and §2.2 (the tool contract). The built-in backend stays as a peer.
2. **IC preview-feature floor + single-binary constraint (new).** States the `output-artifacts` +
   `inject-files` floor and that one IC binary must carry all needed preview features (§8).

**The 2026-06-29 non-goal stands — tailor does NOT author `output.artifacts`.** The user declares the
`output.artifacts` block (items + preview features) in their own IC `config:`; tailor **reads** it
(structural detection + `path` relocation, as `output_artifacts.rs` already does) but never writes the
item list. So there is **no `profile.items` field** and **no auto-authoring** — the user's
`output.artifacts.items` is the single source of truth for what gets extracted, the emitted
`inject-files.yaml` manifest is the source of truth for what gets signed, and both are already covered
by the per-cell fingerprint (the whole IC config is hashed). This keeps config-opacity intact and
avoids a profile-vs-config drift class of bug. See §5 (request set vs sign set) and §6 (why read, not
author).

**Reused unchanged:** the 3-pass executor (`§5`), the `Signer` trait + `SigningPlan`/`SigningResult`
port granularity, `ca.pem` publication, the no-sudo / tools-dir-isolation invariants, and the
fingerprint/lockfile treatment. No port or executor signature changes — the new backend is a drop-in
`Signer` impl. The signing profile is **key-source only** (`backend` + `method` + `bin`, §4).

## 1. The mechanism

Signing is **deferred** — not baked into the customize run. It slots between two IC invocations using
IC's own preview features `output-artifacts` + `inject-files`:

```
1. IC customize (pass 1)         the user's real config (incl. their output.artifacts:{items, path} + previewFeatures:[output-artifacts,inject-files,…]); output raw
      → produces the unsigned raw image AND writes the UNSIGNED boot artifacts into <artifacts> + an inject-files.yaml manifest
2. host-side sign (in place)     ic-sign signs the extracted artifacts, keyed by the emitted manifest
3. IC inject-files               inject-files --config-file <artifacts>/inject-files.yaml --image-file <unsigned.raw> --output-image-file <signed.raw> --output-image-format raw
      → then convert raw → the requested disk format
```

The signable set is the boot chain IC (and tailor) already rebuild — the UKI(s), UKI addons, the boot
loader (systemd-boot / grub), and the dm-verity root hash. **Pass 1 is the full customize** — the
user's actual config (whatever package ops, UKI rebuild, verity re-seal it declares), *including the
`output.artifacts` block the user authored* (§6), so the *same* invocation that produces the unsigned
raw image also extracts the boot artifacts and emits `inject-files.yaml`. tailor relocates that
block's `path` (for sudo-free reclaim) but authors none of it. It therefore runs with whatever
tools-dir that config already needs; there is **no separate extraction-only pass**. This single-pass
identity is essential: the bytes signed in step 2 are exactly the bytes pass 1 baked into
`unsigned.raw`, and step 3 injects the signed replacements back into that same image. The signer
occupies the middle step and signs in place. (This matches the implemented three-pass executor in
`crates/tailor-exec/src/executor.rs`.)

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

- **Backend split:** add `ic-sign` as a first-class `Signer` backend; **keep** the built-in
  `openssl`+`sbsign` backend. `ic-sign` is the default where it is the sanctioned path; the built-in
  stays as fallback and for the pure-Rust goal.
- **Items live in the user's `output.artifacts`, not the profile.** The user declares
  `config.output.artifacts.items` (the request set); IC emits `inject-files.yaml` (the sign set); the
  signer signs the manifest. tailor authors neither — it reads. So there is **no `profile.items`**
  (§5, §6).
- **Binary sourcing:** preflight the signer on PATH or a configured path, exactly like
  `openssl`/`sbsign`. Acquisition (download + version pin) is environment/pipeline plumbing, not
  tailor's job — keeps tailor environment-agnostic.
- **Key source:** `method: ephemeral | service`; ephemeral is this design, the service method is the
  documented seam (§7).

## 4. Config surface

The signing profile is **key-source only** — `backend` + `method` + `bin` (plus the existing
key-source fields). It does **not** carry `items`: what to extract/sign lives in the user's own IC
`config.output.artifacts` (§5, §6).

```yaml
# tailor.yaml — the signing profile: key source + which tool, nothing about items
signing:
  default: secureboot-ephemeral
  profiles:
    secureboot-ephemeral:
      backend: ic-sign            # delegating driver backend
      method: ephemeral           # ephemeral | service (§7)
      # bin: /opt/ic-sign         # optional; the signing tool. Bare name → PATH; path → workspace-relative. Default `ic-sign`.
      # publishCaCert: ./out/ca.pem   # optional; default <output_dir>/<slug>.ca_cert.pem
```

```yaml
# image.yaml — opt in, and (today) hand-author output.artifacts + the two signing preview features
name: appliance
base:
  path: ./bases/appliance.vhd     # a base that already carries a UKI (mode: passthrough), or use uki.mode: create
  arch: arm64
signing: secureboot-ephemeral     # ← the whole signing opt-in

config:
  previewFeatures:
    - output-artifacts            # signing mechanism (extract pass)
    - inject-files                # signing mechanism (inject pass)
    - uki                         # image-specific: this base's UKI handling
  os:
    uki:
      mode: passthrough           # base already has UKIs — preserve + sign (vs. `create`)
  output:
    artifacts:
      items: [ukis]               # ← the request set: what to extract & sign (user-owned; see §5)
      path: ./out-artifacts       # tailor relocates this to a tailor-owned staging dir anyway
```

Everything under `config:` is the user's IC config, passed through opaquely. tailor only **reads**
`output.artifacts` to (a) confirm a signed cell actually declares it — else it errors — and (b)
relocate its `path` for the sudo-free janitor. `items`/`os.uki.mode`/the non-signing preview features
are the user's to get right for their image (they know its boot chain; tailor is config-opaque). A
signed build additionally needs `runtime.buildDirBase` set (the IC chroot/overlay needs a real host
dir) — see §8.

Schema notes:
- `SigningBackend` gains the `ic-sign` variant; `method` (enum) is **required** for it, `bin`
  optional. Any `service` sub-config is required only for that method.
- `SigningProfile::validate` gains the `ic-sign` arm (require `method`). Config-shape validation only;
  presence/capability probing is the
  build preflight.

## 5. `items`: request set vs sign set

`output.artifacts.items` is an **input** to the extract pass — IC only extracts what you *request*.
**The user authors it** in `config.output.artifacts.items`; tailor reads (never writes) it. Two
phases:

- **Request set (input, in the user's config):** `config.output.artifacts.items`. There is no tailor
  default and no `profile.items` — the person editing the image picks the items, because getting them
  right depends on the image's boot chain, which tailor (config-opaque) cannot see. A safe, common
  starting point is **`[ukis]`** (or `[ukis, shim]`). The IC item tokens are: `ukis`, `uki-addons`,
  `shim`, `bootloader`, `verity-hash`. Rules the author must know:
  - **`uki-addons` is auto-included with `ukis`** — listing it explicitly is an error.
  - **`verity-hash` is not auto-emitted** — IC extracts the dm-verity root hash **only** when
    `verity-hash` is in `items`. **⚠ Correctness caveat:** if the image *is* verity-sealed but the
    author omits `verity-hash`, the root hash is never extracted or signed, yet the build still
    succeeds and looks "signed" — a silent hole (an unverifiable verity chain under Secure Boot). A
    verity-sealed image **must** include `verity-hash`. tailor can't detect verity for them (opaque),
    so this is the author's responsibility; see Open Items for whether tailor should at least warn.
  - **`bootloader` is grub-specific and hard-errors without grub.** The IC `bootloader` item
    unconditionally copies a **fixed per-arch grub EFI path** off the ESP (e.g. `grubaa64.efi` /
    `grubx64.efi`); it is **not** conditioned on the bootloader actually present. On a grub-less ESP
    that copy fails and IC **aborts the entire `output.artifacts` pass** with an artifact-copy error
    (no skip/continue branch, unlike `verity-hash`; confirmed in IC source `outputArtifacts()` in
    `artifactsinputoutput.go`). So include `bootloader` **only** for a grub chain; a systemd-boot
    image must omit it or the build hard-fails.
- **Boot-loader coverage depends on the chain (grub vs systemd-boot):** for a `shim → grub → UKI`
  chain, `[ukis, shim, bootloader]` covers the whole signable EFI chain via IC extraction. For a
  `shim → systemd-boot → UKI` chain (no grub), the extractable set is `[ukis, shim]`, and
  **systemd-boot's own EFI binary is not emitted by any current `output.artifacts` item** — so it
  cannot be signed through the IC-native extract → inject flow today. Signing systemd-boot under
  SB-enforcing therefore needs one of:
  - **(a)** an out-of-band **in-place ESP re-sign** of the systemd-boot EFI after the image is built,
    outside the IC-native inject flow — concretely: mount the image's ESP (loop-mount the built
    image), `sbsign` `systemd-boot*.efi` in place, unmount. This is the mechanism the older
    mount-based signer path provides; or
  - **(b)** a future upstream IC **`systemd-boot` `output.artifacts` item** (does not exist today),
    which would make the systemd-boot chain fully IC-native like the grub chain.
- **Sign set (what actually gets signed):** every entry in the emitted `inject-files.yaml`. The signer
  signs the whole manifest; tailor does not re-derive it. This keeps the sign step config-opaque and
  robust to IC adding artifact kinds.

**Why no `profile.items` / no auto-authoring:** because the request set lives in the user's config, it
is already covered by the per-cell fingerprint (the whole IC config is hashed — `fingerprint.rs`), the
author picks items that are valid for *their* boot chain (no tailor guessing that could hard-fail on
`bootloader`/`shim`), and there is no second copy on the profile to drift out of sync. tailor reading
the config is strictly safer than tailor authoring it. See §6.

**Inject-files CLI/schema (for the signer wiring):** the inject pass is `imagecustomizer inject-files
--build-dir <dir> --config-file <inject-files.yaml> --image-file <base> --output-image-file <out>
--output-image-format <fmt>` (flag is `--config-file`; `--build-dir` required). The manifest is a
top-level `injectFiles:` list (each entry `partition/source/destination/type`) with `previewFeatures:
[inject-files]`. Signing is **in place on `source`** (there is no separate `unsignedSource` field —
the `source`/`unsignedSource` wording in `2026-06-29-signing.md` is outdated and should be corrected
when the signer lands).

## 6. output.artifacts: read, don't author (the 06-29 non-goal stands)

`2026-06-29-signing.md` §3 lists as a **non-goal**: *"tailor does not model or rewrite
`output.artifacts` — the user authors it in their `config:`."* **This design keeps that non-goal.**
An earlier revision proposed reversing it (tailor auto-authoring the `output.artifacts` block from a
`profile.items` field); that is **rejected** in favour of **read, don't author**:

- **The user declares `output.artifacts`** (items + `path`) in their own IC `config:`, alongside the
  other IC directives they already write (`os.uki`, non-signing `previewFeatures`, storage, …).
- **tailor reads it, never writes the item list.** It performs exactly the structural touch it does
  today (`output_artifacts.rs`): (a) `uses_output_artifacts()` confirms a **signed** cell actually
  declares an `output.artifacts` block — and **errors** if not (*"image requests `signing:` but its IC
  config declares no `output.artifacts`"*, already implemented in `executor.rs`); and (b) it relocates
  the block's `path` to a tailor-owned staging dir so IC's scratch is reclaimable sudo-free. It does
  **not** add, remove, or rewrite the `items`.

**Why read beats author** (the reasons the reversal was dropped):
1. **No item-guessing that can hard-fail.** The item set's validity depends on the image's boot chain
   (`bootloader` hard-errors on a grub-less ESP; a verity image must include `verity-hash`; §5).
   tailor is config-opaque and cannot see the chain — so only the image author can pick a set that
   won't break the build. Authoring from a profile default would shift that burden onto someone who
   can't see the image.
2. **No drift.** A `profile.items` field would be a second copy of the request set that could diverge
   from the user's actual `output.artifacts` — an ambiguity (*which wins?*) that read-only eliminates.
3. **Fingerprint already covers it.** Because the request set lives in the user's IC config, and the
   whole config is hashed into the per-cell fingerprint (`fingerprint.rs`), changing `items` already
   rebuilds the cell — no special-casing needed.
4. **Opacity preserved.** tailor still does only the narrow structural read/relocate it already does;
   it adds no `items` and models nothing semantically.

**Cost:** a signed image's config carries a few mechanical lines — the `output.artifacts` block and
the `output-artifacts` + `inject-files` preview features (see §4's `image.yaml`). That is a small,
explicit price for keeping signing opaque and correct-by-construction. (If it ever proves worth
automating, a *future* opt-in could have tailor append just the two signing preview features — never
the `items` — but that is out of scope here.)

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
  include the resolved **signer identity** so a signing-config change reliably rebuilds the cell —
  `backend` + `method` (+ `bin`/pinned version, and for `service` the stable key/cert identity). The
  **request set is already covered**: `items` live in the user's `output.artifacts`, and the whole IC
  config is hashed (`fingerprint.rs` hashes `canonical_config(ic_config)`), so changing `items`
  already rebuilds — no separate item hashing needed (a direct benefit of read-don't-author, §6).
  **Current-state gap:** `fingerprint.rs` hashes an `inject` bool but **not** the signer identity yet —
  extend it before signed caching is trustworthy. The **ephemeral** method is intentionally
  **non-reproducible** (fresh cert per build), so its fingerprint tracks the *signing configuration*,
  not the signature bytes; because a cached, differently-signed image would then read as up-to-date,
  ephemeral signed cells must treat the image **and** its emitted `ca_cert.pem` as one bundle (both
  must exist to be up-to-date), or disable caching / add a per-invocation nonce if "fresh cert every
  build" is a hard requirement. See Open Items.
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

- **P1 — config + backend surface. ✅ DONE.** `SigningBackend::IcSign` + `SigningMethod{ephemeral,
  service}` + `method`/`bin` on `SigningProfile`; `validate` requires `method`; `preflight` checks the
  `bin` (missing signer host deps warn, not fail — a delegating backend only requires its `bin`).
  **No `items`/`verity`/`bootloader` on the profile** (read-don't-author, §6).
- **P2 — output.artifacts authoring. ✂ DROPPED.** Superseded by read-don't-author (§6): the user
  authors `output.artifacts`; the executor already requires it and tailor reads/relocates it. No
  auto-authoring work. (A future opt-in to append only the two *signing preview features* could be
  revisited, but it never authors `items`.)
- **P3 — the `ic-sign` `Signer` impl. ✅ DONE.** In `tailor-sign`: renders `sign-config.yaml`, runs
  `<bin> sign-artifacts …`, publishes the emitted enrollment cert; registered for the backend.
- **P4 — the ephemeral end-to-end. ✅ DONE (the §9 bar).** Verified on a real base: extract → sign →
  inject produced a signed image + `ca_cert.pem`; the output UKI is Authenticode-signed and verifies
  against the emitted cert (and fails against a wrong cert — a real positive).
- **P5 — service seam** (§7) as a follow-up.

## 11. Open items

- **~~`bootloader` on a grub-less ESP — skip or error?~~ RESOLVED (2026-07-24):** IC's `bootloader`
  item **hard-errors** (does not skip) on a grub-less ESP — `outputArtifacts()` unconditionally copies
  a fixed per-arch grub EFI path and aborts the whole pass if it is absent (confirmed in IC source).
  Folded into §5 as author guidance: include `bootloader` only for a grub chain.
- **~~Host deps~~ RESOLVED:** the ephemeral signer's host deps (`openssl`/`pesign`/`certutil`) are the
  signer's own; tailor's preflight now **warns** (not fails) when they're absent, so a containerized
  signer preflights clean (only a missing `bin` hard-fails).
- **[decision] Fail-closed boot-chain completeness (§9.0).** Should a signed build require a declared
  boot chain (`grub` → require `bootloader`; `systemd-boot` → flag that systemd-boot's EFI isn't signed
  by this flow; unknown → reject)? Today a build can complete with part of the chain unsigned yet be
  labeled "signed." tailor can't detect the chain (opaque), so any enforcement would be author-declared
  (e.g. a profile/image `chain:` hint) — recommend fail-closed if we add it.
- **[decision] Verity completeness (§5).** A verity-sealed image whose author omits `verity-hash` ships
  an unsigned root hash but still "succeeds." tailor can't detect verity (opaque). Open question: should
  tailor at least **warn** on a signed build whose `output.artifacts.items` omits `verity-hash`
  (cheap, structural — it already reads the block), accepting some false positives, or leave it wholly
  to the author? Recommend a soft warn.
- **[decision] Ephemeral caching (§8).** Extend `fingerprint.rs` to hash the signer identity
  (`backend`/`method`/`bin`), and define the up-to-date rule for ephemeral cells (image+cert bundle, or
  nonce/no-cache) so a cached, differently-signed image isn't wrongly skipped. (The request set is
  already hashed via the user config — read-don't-author, §6.)
- **[decision] `ic-sign` cert contract vs the port.** §2.2 mentions "public key(s)/cert" (plural) and a
  `leaf_id`-scoped output dir, but `SigningPlan.ca_cert_dest`/`SigningResult.published_ca_cert` are a
  single path. The e2e emitted exactly **one** enrollment cert per cell (single path is fine, and
  "unique cert per build" = per cell); if multiple certs or build-level artifacts are ever needed, the
  port must return a set/dir. Also: the emitted cert is a self-signed leaf, so "CA cert" naming may be
  a misnomer.
- **[robustness] `ic-sign` atomicity & fail-closed signing.** Non-zero exit isn't enough for an
  in-place multi-file op: prefer the tool hard-fail on unrecognized manifest entries, sign into a
  shadow tree + verify every entry + atomically replace, and publish the enrollment cert only after all
  artifacts verify. A post-sign verification step before `inject-files` would make this robust.
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
