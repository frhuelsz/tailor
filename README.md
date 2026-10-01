<p align="center">
  <img src="docs/resources/logo_small.png" alt="tailor" width="360">
</p>

**Release and documentation home for `tailor`** — a manifest-driven front-end for the Azure Linux
Image Customizer.

> **The source code lives upstream.** tailor is developed in
> [`microsoft/trident`](https://github.com/microsoft/trident) under
> [`tools/tailor`](https://github.com/microsoft/trident/tree/main/tools/tailor). This repository does
> **not** hold the source — it only **builds, signs, and publishes release binaries** and **hosts the
> documentation site**. File issues and PRs against `microsoft/trident`.

📖 **Documentation: [frhuelsz.github.io/tailor](https://frhuelsz.github.io/tailor/)** (synced daily
from upstream).

## Install

### Prebuilt release binary (recommended)

Releases publish static Linux musl binaries for `x86_64` and `aarch64`, each with a `.sha256`
checksum. This snippet fetches the **latest** release:

```bash
set -euo pipefail
target="x86_64-unknown-linux-musl" # or aarch64-unknown-linux-musl
base="https://github.com/frhuelsz/tailor/releases/latest/download"

curl -L -O "${base}/tailor-${target}"
curl -L -O "${base}/tailor-${target}.sha256"
sha256sum -c "tailor-${target}.sha256"
chmod +x "tailor-${target}"
sudo install -m 0755 "tailor-${target}" /usr/local/bin/tailor
tailor --version
```

The binary is fully static: it needs no glibc or OpenSSL on the target. Runtime requirement: access
to a Docker (or Podman) daemon.

### From source

Build from the upstream monorepo:

```bash
cargo install --git https://github.com/microsoft/trident tailor
```

## Verifying a release

Every release binary carries a SHA256 checksum, a **cosign keyless signature**
(`.sig`/`.pem`/`.bundle`), a **CycloneDX SBOM** (`tailor-cyclonedx.json`), and a GitHub
**build-provenance attestation**.

The signature identity is bound to this repo's release workflow. Releases are built on demand / on a
daily schedule (not from a tag push), so the certificate identity is the workflow on `main`:

```bash
set -euo pipefail
repo="frhuelsz/tailor"
target="x86_64-unknown-linux-musl" # or aarch64-unknown-linux-musl
binary="tailor-${target}"
issuer="https://token.actions.githubusercontent.com"
identity="https://github.com/${repo}/.github/workflows/release.yml@refs/heads/main"

sha256sum -c "${binary}.sha256"
cosign verify-blob \
  --certificate "${binary}.pem" \
  --signature "${binary}.sig" \
  --certificate-identity-regexp "${identity}" \
  --certificate-oidc-issuer "${issuer}" \
  "${binary}"
gh attestation verify "${binary}" \
  --repo "${repo}" \
  --cert-identity-regexp "${identity}" \
  --cert-oidc-issuer "${issuer}"
```

## How this repo works

Two GitHub Actions workflows, both daily and manually runnable:

- **[`release.yml`](.github/workflows/release.yml)** — reads the `tools/tailor` version from upstream,
  and if it is newer than the latest `v<version>` release here, builds the musl binaries from the
  upstream source, signs them (cosign), generates the SBOM and provenance attestation, and publishes a
  GitHub Release tagged `v<version>`. Run it manually from the Actions tab to snap a release on demand
  (optionally `force` a rebuild or pick a different upstream `ref`).
- **[`docs.yml`](.github/workflows/docs.yml)** — mirrors `tools/tailor/docs` from upstream and
  publishes the MkDocs Material site to GitHub Pages (versioned via `mike`). The site chrome (nav,
  theme) lives in [`.github/docs/mkdocs.yml`](.github/docs/mkdocs.yml).

The full source history of this repo prior to the move into `microsoft/trident` is preserved on the
[`archive`](https://github.com/frhuelsz/tailor/tree/archive) branch.

## License

tailor is licensed under the [MIT License](LICENSE).
