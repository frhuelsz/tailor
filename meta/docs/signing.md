# tailor — Signing & `inject-files`

> **Status:** Partially implemented (S1 foundation only) · _last reviewed 2026-06-29_
>
> The `signing:` schema, profile validation, preflight reporting, and `--dry-run` behavior exist in `crates/tailor-config/src/schema.rs`, `crates/tailor-core/src/signing.rs`, and `crates/tailor/src/run.rs`. The actual signed execution pipeline is still absent: signed builds hard-error via `signing_not_implemented`; see `signing-status.md` for the detailed gap analysis.

---

## 1. Current state — what works, what's missing

| Piece | Today |
| ----- | ----- |
| `injectFiles: bool` image field | Parsed (`schema.rs`), validated, and folded into the per-cell **fingerprint** (`fingerprint.rs`) — but **never read by the executor**. |
| IC `output.artifacts` in `config:` | Passed through opaquely like the rest of the IC config (tailor never models it). |
| `inject-files` IC subcommand | Documented (design §7.4) but **not emitted** by `arg_builder.rs`; only `customize`/`convert` are wired. |
| Signing (keys, certs, `pesign`) | **Not implemented.** No signer, no key handling, no CA. |

So a user can *write* `injectFiles: true` and an `output.artifacts` block, and tailor will happily
run a single `customize` pass — producing an image whose boot artifacts are **unsigned**, silently
ignoring the request. The signing pipeline below closes that gap.

> This design **retires the `injectFiles` field** from the user surface: a single `signing:` profile
> (§4) is both the opt-in and the key selection, so a separate boolean gate is redundant. The schema
> field is removed (or kept internal-only); its fingerprint role is replaced by the signer identity
> (§8).

---

## 2. Background — how IC signing actually works

Image Customizer cannot sign boot artifacts itself (signing needs keys the build host owns). It
instead splits a signed build into **two IC passes around a host-side signing step**:

```mermaid
flowchart LR
  C["IC customize<br/>(config has output.artifacts)"] --> X["IC writes:<br/>• unsigned boot artifacts<br/>• inject-files.yaml (manifest)"]
  X --> S["host signs each artifact<br/>(pesign / openssl smime)"]
  S --> I["IC inject-files<br/>(inject-files.yaml)"]
  I --> O["final image with<br/>signed boot artifacts"]
```

1. **`customize`** with `output.artifacts` declared in the IC config. Besides the image, IC extracts
   the **unsigned** boot artifacts (UKIs, `shim`, `systemd-boot`, verity root-hash) to an output
   directory and emits an **`inject-files.yaml`** describing them.
2. **Host-side signing.** For every entry in `inject-files.yaml`, sign the `unsignedSource` →
   `source` in place. Trident's `sign.py` does:
   - **PE binaries** (`vmlinuz*.efi` UKIs, `bootx64.efi` shim, `systemd-bootx64.efi`): `pesign
     --certdir <nssdb> --certificate <leaf> --sign --in <unsigned> --out <signed>`.
   - **verity-hash**: `openssl smime -sign -noattr -binary -outform der` (detached DER signature).
   - Artifact **type** comes from the `inject-files.yaml` entry, or is inferred from the filename.
3. **`inject-files`** IC pass, fed the same `inject-files.yaml`, re-injects the now-signed artifacts
   back into the image at their declared destinations → the final, Secure Boot–bootable image.

### What Trident's builder adds around that

`sign.py` also owns the **key material** for its test images (this is the part Milestone 4 calls
"port `builder/sign.py`"):

- **One CA per build** — `efikeygen -C -S` into an NSS key DB (`generate_ca_certificate`).
- **One leaf signing cert per image/clone** — `efikeygen --signer <CA> [--kernel]`
  (`generate_leaf_certificate`); leaf names are unique per clone so parallel signs don't collide.
- **Publish the CA cert** (`ca_cert.pem`) so it can be enrolled into the firmware db / trusted at
  boot (`publish_ca_certificate`).

> Note: `sign.py` shells out with **`sudo cp`** because IC's outputs are root-owned. tailor already
> solves root-owned outputs sudo-free via the **janitor** (design §7.7), so tailor's signer needs
> **no `sudo`** — a concrete improvement over the builder.

---

## 3. Goals & non-goals

**Goals**

- Produce Secure Boot–signed images from a declarative manifest: a single `signing:` profile, no
  bespoke scripts.
- **Minimize external dependencies**: do the trivial parts in pure Rust (cert generation via `rcgen`,
  orchestration, optionally verity-hash CMS) and keep exactly **one** vetted external tool for the
  non-trivial PE/Authenticode signing (`sbsign`), run in a container so the host still needs only a
  Docker daemon (§6).
- **Fail fast**: verify every signing prerequisite — the signer tool/image, BYO key+cert files,
  remote credentials — **once, up front, before any (slow, privileged) IC run**. Never customize N
  cells only to discover at the signing step that `sbsign` or a key is missing.
- Stay **config-opaque** (design §8): tailor must not parse the IC `config:` to discover
  `output.artifacts`. Drive the pipeline off the **artifacts IC actually emits**.
- Keep the **no-`sudo`** guarantee: sign host-side after the janitor normalizes ownership.
- Be **backend-pluggable**: a local test-CA (port of `sign.py`) for CI, with room for real signing
  services (Azure Key Vault / PKCS#11 / remote-service) without touching the orchestration.
- Compose with the **matrix**: per-cell signing, a shared CA per build, leaf-per-cell(/clone).

**Non-goals (for this feature)**

- tailor does **not** model or rewrite `output.artifacts` — the user authors it in their `config:`.
- No firmware/db enrollment, no Azure VM provisioning — tailor stops at "signed image + published
  CA cert".
- No bit-for-bit reproducibility of *signed* outputs (signatures embed certs/timestamps; §9).

---

## 4. Manifest surface

Signing is opt-in **per image via a single field**, `signing:`. There is intentionally **no separate
`injectFiles` flag** — the `inject-files` pass is an implementation detail of "sign this image", so
having both a boolean gate *and* a profile selector would be redundant (and `injectFiles` names the
IC mechanism, not the user's intent). The existing `injectFiles` schema field is therefore retired
from the user surface (it is currently an inert no-op; see §1).

```yaml
# tailor.yaml  (workspace-wide signing profiles)
signing:
  default: test-ca                 # the profile used when an image says `signing: true`
  profiles:
    test-ca:                       # MVP: self-signed CA minted per build (pure-Rust port of sign.py)
      backend: local-test-ca
      publishCaCert: ./artifacts/ca_cert.pem   # where to write the enrollable CA cert
    byo:                           # bring-your-own signing key + cert (no CA generation)
      backend: keypair
      key: ./secrets/db.key        # PEM private key + cert, sourced at build time
      cert: ./secrets/db.crt
    akv:                           # future: remote signing service / HSM
      backend: azure-key-vault
      vault: https://my-vault.vault.azure.net
      certificate: secureboot-db
```

```yaml
# image.yaml
name: appliance
signing: test-ca                   # ← the ONLY opt-in: presence enables the signed pipeline,
                                   #   the value selects a profile (`true` = the workspace default)
config:
  # ... user-authored IC config, INCLUDING their own output.artifacts block ...
  output:
    artifacts:
      items: [ukis, shim, systemd-boot, verity-hash]
      path: ./output-artifacts
```

Notes:

- `signing:` is a profile **id** (string), or `true` to use the workspace `signing.default`.
  **Omitted ⇒ unsigned** — unlike `toolchain:`, the workspace default is *not* auto-applied to every
  image (most images have no boot artifacts to sign), so signing is always an explicit choice.
- The user still authors `output.artifacts` in their own `config:` (it tells IC *what* to extract);
  `signing:` tells tailor *how* to sign. Those are genuinely different concerns, not a duplicate of
  each other — and tailor never reads the `config:` to find `output.artifacts` (§5).
- `backend` is the only field the executor branches on; everything else is backend-specific and
  parsed by the chosen `Signer`. Private key material is **referenced**, never inlined or imaged.

---

## 5. Execution pipeline

The executor (`tailor-exec`) gains a third mode beside `customize`/`convert`. tailor first
**preflights** the signing prerequisites for the whole build (§5.1) and aborts before touching IC if
anything is missing. Then, per cell whose image has a resolved `signing:` profile:

```mermaid
sequenceDiagram
  participant T as tailor (orchestrator)
  participant E as executor (bollard)
  participant J as janitor
  participant G as Signer (port)
  T->>G: preflight(profiles) — once, before any IC run
  G-->>T: ok ✔ / missing tool·key·creds ✘ (fail fast, no build starts)
  T->>E: IC customize (config has output.artifacts)
  E-->>T: image + output-artifacts/ (root-owned) + inject-files.yaml
  T->>J: chown output-artifacts/ to caller (sudo-free)
  T->>G: sign(inject-files.yaml, artifacts dir, profile)
  G-->>T: signed artifacts in place (+ published CA cert)
  T->>E: IC inject-files (inject-files.yaml)
  E-->>T: final signed image
  T->>J: chown final image to caller
```

1. **Customize** exactly as today (the user's `output.artifacts` rides along in the working-copy
   config). Output dir is the cell's artifact dir.
2. **Detect signing work — presence-based, not config parsing.** After customize, if the image has a
   `signing:` profile **and** IC emitted an `inject-files.yaml` (+ artifacts dir), proceed; otherwise:
   - `signing:` set but **no** `inject-files.yaml` ⇒ the IC config declared no `output.artifacts`
     → **hard error** ("`signing:` requested but the IC config produced no `output.artifacts`").
   - no `signing:` ⇒ skip (single-pass, today's behavior). This keeps tailor config-opaque:
     it reacts to IC's *output*, never reads the input `config:`.
3. **Normalize ownership** of the artifacts dir via the janitor so signing runs as the caller.
4. **Sign** via the resolved `Signer` (§6), in place: `unsignedSource` → `source` for every entry in
   `inject-files.yaml`, by artifact type.
5. **inject-files** IC pass: a new arg vector
   `inject-files --build-dir /tmp --image-file <customized> --inject-files-config <inject-files.yaml>
    --output-image-format <fmt> --output-image-file <final>` (exact flags TBD against IC — §10),
   with all host paths translated to the `/host` mount as usual.
6. **Normalize ownership** of the final image; write the build stamp.

`--dry-run` prints all three steps (the two `docker run` invocations and the signing commands)
without executing — so the signed flow is as inspectable as the unsigned one.

### 5.1 Preflight — fail fast before building

An IC `customize` run is slow and privileged; a build set can be many cells. Discovering only at the
signing step that `sbsign` isn't installed, the signer container can't be pulled, a BYO key file is
missing, or a Key Vault credential is absent — *after* customizing N cells — wastes a lot of time and
leaves half-built, root-owned outputs around. So tailor runs a **preflight** check once, **before the
first IC invocation**:

1. Collect the **distinct signing profiles** across the selected cells (a build may mix signed and
   unsigned images; only signed ones contribute).
2. For each, call the backend's `preflight()` (§6) — a **cheap, side-effect-free** capability probe:
   - **signer tool present** — `sbsign` resolvable on `PATH` (host mode), or the **signer image is
     present/pullable** (container mode);
   - **key material resolvable** — for `keypair`, the `key`/`cert` files exist, are readable, and
     parse; for `local-test-ca`, that `rcgen` can mint (always true — pure Rust);
   - **remote reachable** — for `azure-key-vault`/`pkcs11`/`remote-service`, required credentials/modules are
     present and a cheap auth/handshake (or at least config completeness) succeeds.
3. If **anything** is missing, **abort the whole build before any cell is customized**, with an error
   that names every missing prerequisite and the image/profile that needs it (so the user fixes all
   of them in one pass, not one failed build at a time).

This is distinct from the runtime "`signing:` set but IC emitted no `output.artifacts`" error
(pipeline step 2): preflight verifies tailor *can* sign; that check verifies there was *something* to
sign. Preflight is also surfaced **non-fatally** by `tailor validate` and `tailor build --dry-run`
(they *report* missing prerequisites without failing), so the requirements are discoverable without
starting a real build. A pure `tailor build` treats them as a hard gate.

---

## 6. Signer abstraction

A new port in `tailor-core`, implemented in a new `tailor-sign` crate (keeps key/PKI code isolated
and unit-testable, and keeps `tailor-exec` focused on containers):

```rust
// tailor-core::ports
pub trait Signer {
    /// Cheap, side-effect-free check that this backend can sign: tool/image present, key material
    /// resolvable, remote reachable. Called once per build, before any IC run (§5.1).
    async fn preflight(&self) -> Result<(), SignError>;

    /// Sign every entry in inject-files.yaml in place (unsignedSource -> source).
    async fn sign(&self, plan: &SigningPlan) -> Result<SigningResult, SignError>;
}

pub struct SigningPlan {
    pub inject_files_yaml: PathBuf,   // emitted by IC customize
    pub artifacts_dir: PathBuf,       // where the (un)signed artifacts live
    pub leaf_id: String,              // per-cell/clone, for unique leaf keys
}
pub struct SigningResult { pub published_ca_cert: Option<PathBuf> }
```

Each backend implements `preflight()` to assert its own prerequisites — `sbsign`-based backends check
the binary/image; `keypair` stats and parses the `key`/`cert`; remote backends probe credentials —
so the fail-fast gate (§5.1) needs no special-casing in the orchestrator.

**Use pure Rust for the trivial pieces; keep a battle-tested external tool for the one non-trivial
piece (PE signing).** `sign.py` shells out to four tools, but only one does anything hard:

| Operation | `sign.py` uses | Recommendation | Why |
| --------- | -------------- | -------------- | --- |
| **Key + cert generation** (CA, per-image leaf, code-signing EKU) | `efikeygen`, `certutil`, `pk12util` (NSS) | **pure Rust — `rcgen`** | Trivial: just mints an X.509 CA + leaf. Deletes the whole NSS chain; keys live in memory/PEM. |
| **Orchestration / `inject-files.yaml`** | (python) | **pure Rust — `serde_yaml` + std** | Already tailor's wheelhouse. |
| **verity-hash signature** (detached CMS/PKCS#7 DER) | `openssl smime -sign` | **either** — RustCrypto `cms`, or keep `openssl` | Small and clean in Rust, but it *is* signature-format code; not worth agonizing over. |
| **PE/Authenticode signing** (UKIs, shim, systemd-boot `.efi`) | `pesign` | **external — `sbsign`** | Non-trivial + security-critical; no mature pure-Rust drop-in. Keep a vetted tool. |

So the recommended **lean stack** keeps exactly one external signing tool:

> **`rcgen` (certs, pure Rust) + `sbsign` (PE signing, external) + verity-hash via `cms` (pure Rust)
> or `openssl` (external).**

A useful detail: **`sbsign` takes a PEM `--key`/`--cert`** whereas `pesign` wants an NSS db. Pairing
`rcgen`'s PEM output with `sbsign` drops the *entire* `efikeygen`/`certutil`/`pk12util`/NSS/`pesign`
chain — **one** external tool instead of five. The external signer runs **in a container** (host needs
only Docker) or on the host.

> **Fully pure-Rust is possible but optional.** Replacing `sbsign` too means a first-party
> **Authenticode PE writer** — the PE Authenticode hash, an `SpcIndirectDataContent` CMS `SignedData`,
> and the PE attribute-certificate-table embedding. Fully specified (`sbsign`/`osslsigncode` are the
> references) and bounded, but security-critical first-party crypto — so a *later, optional* hardening,
> not a prerequisite. It becomes most attractive for **remote-key backends** (an HSM can't feed
> `sbsign`), where tailor would build the structures and ask the HSM only for the raw signature; the
> `signature::Signer` trait keeps that pluggable. Crypto stays RustCrypto + `ring` (never `aws-lc-rs`),
> so none of this adds a C/system dependency to the binary.

**Backends:**

The three backends are **key-source profiles** (where the signing key comes from); the PE signer is
orthogonal (`sbsign` for local keys, the first-party writer for remote keys, §6):

- **`local-test-ca`** (MVP, CI) — mint a self-signed CA once per build and a leaf per `leaf_id` with
  `rcgen`; sign PE with `sbsign` and verity-hash with `cms`; publish `ca_cert.pem`. Pure-Rust certs;
  **not** a production trust root.
- **`keypair`** (BYO) — load a PEM key + cert and sign with `sbsign`. The "we already have a Secure
  Boot cert" case.
- **`azure-key-vault` / `pkcs11` / `remote-service`** (future) — the key can't be handed to `sbsign`, so tailor
  builds the Authenticode/CMS structures itself (the first-party PE writer, §6) and asks the remote
  signer only for the raw signature; the private key never leaves the HSM/service.

> **Verdict on external deps:** the lean stack is `rcgen` + `sbsign` (one external signing tool, run
> in a container so the host needs only Docker) + verity-hash via `cms`/`openssl`. Pure Rust covers
> every trivial piece *and* the key generation; `sbsign` covers the one non-trivial, security-critical
> piece (PE/Authenticode). A fully pure-Rust signer (first-party Authenticode writer) is an optional
> later step, most useful once remote-key backends arrive.

---

## 7. Matrix, cells & clones

- Signing is **per cell** — only cells whose image has a `signing:` profile and that emit
  `output.artifacts`. A `vm-img` cell with no verity/UKI simply emits no artifacts and is skipped
  (presence-based, §5.2).
- The **CA is per `build` invocation** (one trust root for the run); **leaf certs are per cell, and
  per clone** (`--clones N`) so parallel/independent cells never share a leaf — matching `sign.py`'s
  unique leaf-name scheme.
- Selectors compose unchanged: `tailor build -s variant=root-verity` signs just those cells.

---

## 8. Reproducibility, fingerprint & lockfile

- **Fingerprint.** Add the **signer identity** to the per-cell fingerprint (`FingerprintInputs`):
  the backend id plus a stable key identity (e.g. the leaf/CA **certificate fingerprint** for
  BYO/remote backends). A different signing identity ⇒ a different artifact ⇒ a rebuild. (This
  replaces the retired `injectFiles` bool that the fingerprint hashes today.)
- **`local-test-ca` is intentionally non-reproducible**: it mints fresh keys each build, so signed
  outputs differ run-to-run. That is acceptable and consistent with design §9.3's *bounded*
  reproducibility; `--locked` still pins IC + base digests, just not the freshly-generated keys. Doc
  this loudly; production builds use BYO/remote backends with a fixed cert identity.
- **Lockfile.** No new registry inputs (keys are local/remote, not OCI), so `tailor.lock` is
  unchanged. A remote backend may later record the signing certificate's identity for auditability.

---

## 9. Security considerations

- **No private keys in images.** tailor signs *extracted* artifacts and injects them back; signing
  keys live only in the build environment (file refs or a remote vault) and are never added to the
  IC `config:` or the rootfs.
- **No `sudo`.** The janitor normalizes IC's root-owned artifact dir before signing, so the signer
  runs entirely as the calling user (improving on `sign.py`'s `sudo cp`).
- **Least exposure for BYO keys.** Key/cert paths are read at build time; tailor never copies them
  into outputs or logs their contents. Remote backends keep the private key in the HSM/service.
- **Published CA cert is public** by design (it is the enrollment artifact); only the CA/leaf
  *private* keys are sensitive.
- The signed pipeline keeps IC's existing `--privileged` + `/:/host` blast radius (design §15); it
  adds a `sbsign` signer (host or container) and a little pure-Rust crypto (`rcgen`/`cms`), all
  unprivileged.

---

## 10. Open questions / assumptions to validate

1. **Exact `inject-files` IC arg vector** and the `inject-files.yaml` schema across IC versions
   (`source`/`unsignedSource`/`type`; MIC v1.1+ reuses one path) — validate against IC docs and a
   real run before wiring `arg_builder.rs`.
2. **PE signer choice.** Confirm `sbsign` (PEM key, runs in a container) as the standing PE signer —
   it pairs with `rcgen`'s PEM output and avoids NSS/`pesign`. A first-party pure-Rust Authenticode
   writer (`object`/`goblin` + `cms`) stays optional/later, becoming worthwhile mainly for remote-key
   backends (where `sbsign` can't reach the key); verify either against `sbsign` output on a real
   UKI/shim. Also decide verity-hash: RustCrypto `cms` vs. keeping `openssl`.
3. **Output-artifacts directory location** relative to the working-copy config (IC resolves
   `output.artifacts.path` relative to the config file, like other paths — §7.6) and its
   translation into `/host`.
4. **Verity-hash signing** detail parity with `sign.py` (`openssl smime` detached DER) and any other
   artifact types newer IC versions emit.
5. **CA lifetime** — confirm per-build (not per-cell) CA is the right granularity for multi-image
   workspaces; consider a `signing.profiles.*.caCert`/`caKey` to reuse a stable CA across builds.

---

## 11. Milestones (refines design.md §17 M4)

```mermaid
graph LR
  S1["S1: pipeline + lean stack<br/>(rcgen + sbsign-in-container)"] --> S2["S2: remote backends<br/>(Key Vault / PKCS#11 / remote-service)"]
  S2 --> S3["S3 (optional): pure-Rust<br/>Authenticode writer (drop sbsign)"]
```

- **S1 — pipeline + the lean stack.** Three-pass executor mode (`customize` → sign → `inject-files`),
  **fail-fast preflight (§5.1)**, presence-based detection, janitor ownership, `--dry-run`, and the
  fingerprint change. Certs in pure Rust (`rcgen`), PE signing via `sbsign` **in a container**
  (Docker-only, no host tools), verity-hash via `cms`. Backends `keypair` (BYO) + `local-test-ca`;
  one real signed E2E cell as the correctness bar.
- **S2 — remote backends.** `azure-key-vault` / `pkcs11` / `remote-service` behind the same `Signer` trait. As
  `sbsign` can't reach a remote key, this is where the first-party Authenticode/CMS structure-building
  lands for PE artifacts (tailor builds, the HSM/service signs).
- **S3 (optional) — fully pure-Rust signer.** A first-party Authenticode PE writer to drop `sbsign`
  entirely, for a zero-external-signing-tool binary. Only if the maintenance of security-critical
  first-party crypto is judged worth it over a vetted tool.

---

## 12. Summary

tailor can't sign today — `injectFiles` is an inert placeholder. This design adds a **config-opaque,
no-`sudo`, dependency-lean, backend-pluggable** signed pipeline: tailor runs IC `customize`, reacts
to the `inject-files.yaml`/artifacts IC emits, signs them through a `Signer` port — the trivial parts
in pure Rust (cert generation via `rcgen`, verity-hash via `cms`) and the one non-trivial part
(PE/Authenticode) via `sbsign` in a container, so the host needs only Docker — then runs IC
`inject-files` to produce the final Secure Boot–signed image. It's all driven by a **single `signing:`
profile** (the redundant `injectFiles` flag is retired), with the user's `output.artifacts` left
untouched in their IC config. A fully pure-Rust signer (first-party Authenticode writer) remains an
optional later step.
