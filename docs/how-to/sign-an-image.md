# Sign an image

tailor can produce **Secure Boot–signed** images by orchestrating Image Customizer's
`output.artifacts` → host-side signing → `inject-files` flow. You declare *how* to sign with a
`signing:` profile; you keep authoring *what* to extract (`output.artifacts`) in your own IC `config:`.

> **Status — foundation only.** The `signing:` configuration, profile resolution, and the
> **fail-fast preflight** are implemented and enforced. The signing *execution* (certificate minting,
> PE signing, the `inject-files` pass) is a later milestone (`meta/docs/2026-06-29-signing.md` §11). Until it
> lands, a signed `tailor build` runs the preflight and then **stops with a clear error** rather than
> produce a silently-unsigned image. `tailor validate` and `tailor build --dry-run` report signing
> readiness without failing.

## 1. Declare signing profiles

Add a `signing:` block to `tailor.yaml`. A profile names a key-source `backend` plus its settings:

```yaml
# tailor.yaml
signing:
  default: test-ca            # profile used when an image says `signing: true`
  profiles:
    test-ca:                  # self-signed CA minted per build (CI / local; not a production root)
      backend: local-test-ca
      publishCaCert: ./artifacts/ca_cert.pem
    byo:                      # bring your own Secure Boot key + cert
      backend: keypair
      key: ./secrets/db.key   # PEM private key (referenced, never imaged)
      cert: ./secrets/db.crt  # PEM certificate
    akv:                      # remote signing service (future)
      backend: azure-key-vault
      vault: https://my-vault.vault.azure.net
      certificate: secureboot-db
```

| Backend | Required fields | Use |
| --- | --- | --- |
| `local-test-ca` | none | MVP / CI. Pure-Rust self-signed CA + leaf minted per build. Not a production trust root. |
| `keypair` | `key`, `cert` | Bring your own Secure Boot key + certificate (PEM). |
| `azure-key-vault` | `vault`, `certificate` | Remote signing (future milestone). |

## 2. Opt an image in

Set `signing:` on the image — `true` for the workspace default profile, or a profile id. The image
still authors its own `output.artifacts` (that is what tells IC which boot artifacts to extract):

```yaml
# image.yaml
name: appliance
signing: true            # or `signing: byo`
config:
  output:
    artifacts:
      items: [ukis, shim, systemd-boot, verity-hash]
      path: ./output-artifacts
```

Omit `signing:` (or set `signing: false`) for an unsigned image — unlike `toolchain:`, the workspace
default is **not** auto-applied, so signing is always an explicit choice.

## 3. Check readiness (fail fast)

Before any build, tailor verifies every signing prerequisite — once, up front — so a signed build
never customizes N cells only to discover a key is missing. Report readiness without building:

```bash
tailor validate appliance
# ✓ appliance                    2 cell(s) valid
# ✓ signing profile `byo` ready (image(s): appliance)
```

If a prerequisite is missing, `validate` warns; a real `build` aborts before touching IC, naming
every unmet prerequisite and the images that need it:

```text
$ tailor build appliance
error: signing preflight failed — fix every prerequisite below, then rebuild:
  - profile `byo` (needed by: appliance): cannot read `key` `./secrets/db.key`: No such file or directory
```

What the preflight checks per backend:

- **`local-test-ca`** — always ready (keys are minted in pure Rust at sign time).
- **`keypair`** — the `key` and `cert` files exist, are readable, and are PEM.
- **`azure-key-vault`** — configuration completeness (a live credential probe arrives with the remote
  backend milestone).

## 4. Dry-run

`tailor build --dry-run` never contacts an engine and reports the signing plan:

```bash
tailor build --dry-run appliance
# … the customize invocation …
# ✓ signing profile `byo` ready (image(s): appliance)
# note: signing execution is not yet implemented; this dry-run shows the unsigned customize invocation.
```

## Notes

- Private key material is always **referenced** by path, never inlined into the manifest or written
  into an image.
- `local-test-ca` mints fresh keys each build, so its signed outputs are intentionally not
  reproducible; use `keypair` (a fixed cert identity) for reproducible production builds.
- The legacy `injectFiles` boolean is an inert placeholder superseded by `signing:`; do not use it.
