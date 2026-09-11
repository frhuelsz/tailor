# Changelog

All notable changes to this project are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- 1.0.0 stabilization is in progress, including release provenance, compatibility
  guarantees, signing support, and schema hardening.

## [0.8.0] - 2026-09-09

### Added

- Inter-image `inputs:` embedding and interpolation.

## [0.7.0] - 2026-09-09

### Added

- Inter-image dependencies with image-as-base references and build ordering.

## [0.6.0] - 2026-09-09

### Added

- zstd post-build output compression via `compression:`.

## [0.5.0] - 2026-09-09

### Added

- `tailor convert` for workspace-free Image Customizer conversion.
- Extra parameter passthrough for Image Customizer flags.

### Fixed

- RPM source fingerprinting and writable RPM farm handling.
- Same-device build directory defaults for conversion.

## [0.4.0] - 2026-07-20

### Added

- `tailor export` for configs-only exports, including `--check`.
- `--build-dir-base` support for builds.

## [0.3.0] - 2026-07-14

### Added

- Versioned documentation site and release download documentation.

### Fixed

- Cleanup container target binding and native-architecture execution.

## [0.2.0] - 2026-07-10

### Added

- Managed tools directory support.
- Local container image support through pull policy.
- End-to-end signing flow and signing executor support.

### Changed

- Hardened Image Customizer container mounts and host path handling.
- Improved base hashing performance and observability.

## [0.1.0] - 2026-06-23

### Added

- Initial tailor CLI, runtime, documentation, and tests.

[Unreleased]: https://github.com/frhuelsz/tailor/compare/v0.8.0...HEAD
[0.8.0]: https://github.com/frhuelsz/tailor/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/frhuelsz/tailor/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/frhuelsz/tailor/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/frhuelsz/tailor/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/frhuelsz/tailor/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/frhuelsz/tailor/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/frhuelsz/tailor/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/frhuelsz/tailor/releases/tag/v0.1.0
