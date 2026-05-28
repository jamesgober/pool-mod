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

## [1.0.0] - 2026-05-27

First stable release. **The public API is frozen** under semantic versioning; no
breaking changes will land within the `1.x` series. No new surface since 0.9.0 —
this release captures benchmark baselines, applies one internal hot-path
optimization, and freezes the API.

### Changed

- Internal: the acquire path now tracks a waiter count under the lock, so
  check-in, slot release, reap, and close signal the condition variable only when
  a thread is actually blocked. The uncontended borrow-and-return path is about
  8% faster as a result (~106 ns → ~98 ns locally). No observable behavior change.

### Performance

- Captured Criterion baselines (Windows x86_64, stable, single-threaded, trivial
  resource): `get` + return ≈ 98 ns, `try_get` + return ≈ 97 ns, `status` ≈ 8.5 ns.
  The steady-state checkout/return path is allocation-free.

---

## [0.9.0] - 2026-05-27

Feature freeze and the pre-1.0 hardening audit. The final feature — an opt-in
background reaper — lands here; the rest is audit and verification.

### Added

- Opt-in background reaper. A new `PoolConfig::reap_interval` (and
  `Builder::reap_interval`) spawns a background thread that prunes idle resources
  past `idle_timeout` / `max_lifetime` rather than waiting for them to be rejected
  on their next checkout. Disabled by default (`None`), so behavior and overhead
  are unchanged unless enabled. The reaper holds only a weak reference and stops
  when the pool is closed or its last handle is dropped.
- `deny.toml` — a `cargo deny` supply-chain policy (advisories, bans, permissive
  licenses, trusted sources).

### Changed

- `prepare` (checkout) and the reaper now share a single time-expiry helper, so
  lazy and eager expiry apply identical `idle_timeout` / `max_lifetime` rules.

### Audit

- Zero runtime dependencies; `cargo audit` reports no advisories across the
  dev-dependency tree; `cargo deny check` passes (advisories, bans, licenses,
  sources).
- No `unwrap` / `expect` / `todo!` / `unimplemented!` / `dbg!` / `print*` in
  shipping code; the single `#[allow]` (test-module `unwrap`) is justified inline.
- `Error` is `#[non_exhaustive]`; every variant is documented and tested.
- Full matrix green on Linux, macOS, and Windows across stable and MSRV 1.75:
  `fmt`, `clippy --all-targets --all-features -D warnings`, `test --all-features`,
  and `doc` with `RUSTDOCFLAGS=-D warnings`.

---

## [0.5.0] - 2026-05-27

### Added

- `Pool::try_get` — a non-blocking checkout that returns `Error::Timeout`
  immediately when no resource is available, equivalent to
  `get_timeout(Duration::ZERO)`.
- Property tests (`proptest`) covering the pool's invariants: configuration
  validation and pre-warming, the `max_size` ceiling, the `size == idle + in_use`
  identity, sequential reuse, and close semantics.
- Integration tests for the lifecycle paths: idle-timeout and max-lifetime
  expiry over real time, recycle-failure discard, close-while-borrowed, and
  waiter wake-up on return.
- Criterion benchmarks for the acquire/return hot path (`benches/pool.rs`):
  steady-state reuse via `get` and `try_get`, plus `status` sampling.

### Changed

- Pinned the dev-dependency tree in `Cargo.lock` to versions that build on the
  MSRV (Rust 1.75): `proptest` 1.4, the `clap` 4.5 line, `tempfile` 3.14,
  `half` 2.4, and the `rand` 0.8 chain. These are dev-only and do not affect
  published consumers.

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

[Unreleased]: https://github.com/jamesgober/pool-mod/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/jamesgober/pool-mod/compare/v0.9.0...v1.0.0
[0.9.0]: https://github.com/jamesgober/pool-mod/compare/v0.5.0...v0.9.0
[0.5.0]: https://github.com/jamesgober/pool-mod/compare/v0.2.0...v0.5.0
[0.2.0]: https://github.com/jamesgober/pool-mod/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/pool-mod/releases/tag/v0.1.0