# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.6.0] - 2026-11-25

### Added

- **tui**: wire natural sudo elevation, update CLI schemas and CI gates
- **podman**: harden restic 0700/exit-3 backup, adaptive log sizing, and fish compatibility
- **system**: add hardware-adaptive tuning and declarative retention

### Fixed

- **agent**: eliminate audit/verify secret false positives and add IDE cache audit

## [1.5.0] - 2026-11-25

### Added

- **agent-ws**: standardize docs/agent blueprint and update CLI sim paths
- **install**: auto-install sendme and tmux companion tools on setup

## [1.4.1] - 2026-11-24

### Fixed

- **tui**: preserve translucent spaces and drop DIM/shift flicker in pane transitions

## [1.4.0] - 2026-11-24

### Added

- **cli**: wire agent workspace v2, CoALA memory, 14 harness commands, and exit codes 21-26
- **core**: standardize agent workspace IR, CoALA memory, 14 adapters, and migration
- **templates**: standardize agent workspace blueprints and schemas

### Changed

- **cli**: coalesce TUI live-loop events and pace SSH worker output
- **tui**: eliminate SSH flicker with DEC 2026 sync output, coalescing, and fluid transitions

## [1.3.0] - 2026-11-22

### Added

- **tui**: stream live right-pane output and unify responsive in-TUI UX (U39-U49)

## [1.2.0] - 2026-11-22

### Added

- **tui**: polish U37 unified canvas, SpacingScale, and confirm defaults
- **tui**: implement U34-U36 color-block UI, uniform grid, and confirm defaults
- **tui**: polish U33 KV spacing, mini progress bars, and compact pills
- **cli**: wire debopty agent workspace/cred CLI, global JSON redaction, and schema
- **cli**: add U27 agent-first CLI envelope/schema and wire U23/U24/U31 TUI
- **core**: add VPS agent workspace provisioning and U29 credential hygiene
- **tui**: add live telemetry, inspector, mouse/keybindings, flat slate UI, and help overlay
- **templates**: add embedded VPS agent workspace and credential templates

### Changed

- **governance**: migrate governance to .agents and add U29 credential hygiene

## [1.1.1] - 2026-11-22

### Fixed

- **cli**: default UI backend to auto so ratatui-ui launches on interactive TTY

## [1.1.0] - 2026-11-22

### Fixed

- **install**: harden release fallback, mirror validation, and legacy debopti migration

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

[Unreleased]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.6.0...HEAD
[1.6.0]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.5.0...v1.6.0
[1.5.0]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.4.1...v1.5.0
[1.4.1]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.4.0...v1.4.1
[1.4.0]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.3.0...v1.4.0
[1.3.0]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.1.1...v1.2.0
[1.1.1]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.7...v1.1.0
[1.0.7]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.6...v1.0.7
[1.0.6]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.5...v1.0.6
[1.0.5]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.4...v1.0.5
[1.0.4]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.3...v1.0.4
[1.0.3]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.2...v1.0.3
[1.0.2]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.1...v1.0.2
[1.0.1]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/G3arB0xX/Debian-Optimizer-Script/releases/tag/v1.0.0
