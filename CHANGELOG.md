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

## [0.2.0] - 2026-05-27

### Added

- `Manager` trait — the resource lifecycle contract: `create`, `recycle`, and an
  optional `validate` health check for validation-on-borrow.
- `Pool<M>` — the thread-safe, cheaply-cloneable pool: `builder`, `new`, `get`,
  `get_timeout`, `status`, `close`, and `is_closed`.
- `Builder<M>` — fluent configuration reached through `Pool::builder`, validated
  on `build`, which pre-creates the `min_idle` resources.
- `PoolConfig` — `max_size`, `min_idle`, `create_timeout`, `idle_timeout`, and
  `max_lifetime`, with documented defaults.
- `Pooled<M>` — the RAII guard that recycles and returns its resource on drop;
  deref-coerces to the resource and is `Send`.
- `Status` — a snapshot of `size`, `idle`, `in_use`, and `max_size`.
- `Error<E>` — `Backend`, `Timeout`, `Closed`, and `InvalidConfig`, generic over
  the manager's own error type, with `Display` and `std::error::Error` impls.
- `prelude` module re-exporting the full public surface.
- Unit tests for every lifecycle path, an end-to-end integration test exercising
  the pool across eight threads, and rustdoc examples on every public item.

### Changed

- Crate root now lays out the module structure (`config`, `error`, `manager`,
  `object`, `pool`, `status`) and re-exports the public types. The pool is gated
  behind the default `std` feature; without it the crate exposes only `VERSION`.

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

[Unreleased]: https://github.com/jamesgober/pool-mod/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/jamesgober/pool-mod/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/pool-mod/releases/tag/v0.1.0