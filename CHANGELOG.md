# Changelog

All notable changes to `pool-mod` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Added

### Changed

### Fixed

### Security

---

## [0.1.0] - 2026-05-27

### Added

- Initial scaffold and repository bootstrap: `Cargo.toml`, dual `LICENSE-APACHE` /
  `LICENSE-MIT`, `README.md`, `CHANGELOG.md`, and the `.dev/` milestone plan.
- Crate root (`src/lib.rs`) with the REPS lint discipline denied at the crate
  level, plus a `VERSION` constant and a smoke test asserting it is populated.
- Canonical `REPS.md` (v0.2.0) as the repository's supreme engineering standard.
- Cross-platform CI (`.github/workflows/ci.yml`) running fmt, clippy, test, and
  doc with `-D warnings` on Linux/macOS/Windows across stable and MSRV 1.75.

### Fixed

- Set the crate to edition 2021 so it builds on the declared MSRV. Edition 2024
  requires Rust 1.85, which made the manifest unbuildable on the 1.75 CI job.
- Added `.gitattributes` (`* text=auto eol=lf`) so the Windows CI runner's
  CRLF checkout no longer fails `cargo fmt --check` against the `Unix`
  `newline_style` pin.
- Stripped the UTF-8 BOM and added trailing newlines to the source files to
  satisfy `rustfmt`.

[Unreleased]: https://github.com/jamesgober/pool-mod/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/jamesgober/pool-mod/releases/tag/v0.1.0