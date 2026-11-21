# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.0.7] - 2026-11-21

### Added

- **tests**: add real systemd Podman E2E test and harden add_fw_rule

## [1.0.6] - 2026-11-21

### Fixed

- **scripts**: resolve all strict shellcheck warnings across core and app modules

## [1.0.5] - 2026-11-21

### Fixed

- **templates**: remove untracked empty docker dir from INDEX

## [1.0.4] - 2026-11-21

### Changed

- **governance**: rewrite agent-only docs and add L0 infra

## [1.0.3] - 2026-11-21

### Changed

- Maintenance release.

## [1.0.2] - 2026-11-21

### Changed

- Maintenance release.

## [1.0.1] - 2026-11-21

### Fixed

- **xray**: split local declaration and assignment for shellcheck SC2155

## [1.0.0] - 2026-11-21

### Fixed

- **xray**: use read -rp to satisfy shellcheck SC2162

## 26.11.21 (slrwqpmzsqkv)

- Build: slrwqpmzsqkv
- feat(governance,release,mirrors): agent governance, release CLI/CI, CN mirrors & xray beta


## 26.08.17 (yvzuyloxmluo)

- Build: yvzuyloxmluo
- fix: keep VERSION parseable by YYYY.MM.DD.N updaters
## 26.08.16 (luutwynsyqkq)

- Build: luutwynsyqkq
- feat: use YY.MM.DD calendar versions and distinguish same-day releases by Build
## 2026.08.16.1

- Build: lzuyltvsxsxn
- feat: adopt calendar versions with Build id and guarded sanitized releases
## 2026.08.15.1

- Build: uxvkolop
- Introduce calendar versions with a Build id for traceability.

[Unreleased]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.7...HEAD
[1.0.7]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.6...v1.0.7
[1.0.6]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.5...v1.0.6
[1.0.5]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.4...v1.0.5
[1.0.4]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.3...v1.0.4
[1.0.3]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.2...v1.0.3
[1.0.2]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.1...v1.0.2
[1.0.1]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/G3arB0xX/Debian-Optimizer-Script/releases/tag/v1.0.0
