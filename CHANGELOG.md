# Changelog

All notable changes to `pool-mod` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

---

## [1.1.0] - 2026-11-12

Fixes from an independent review. No breaking API change and no MSRV change.
Some runtime behavior changes; each one is listed under Changed.

### Added

- `Pool::try_get_idle`: hand out an idle resource or return `Error::Timeout` at
  once. It never calls `create`, so it never waits on a backend connect.
- `Manager::create_with_deadline(&self, Option<Instant>)`, a provided method
  that defaults to `create()`. The pool now always creates through it and passes
  the caller's deadline, so a manager can bound its own connect time.
- `Pooled::discard(guard)` drops a resource known to be broken instead of
  returning it to the pool. `Pooled::detach(guard)` takes the resource out of
  the pool for good. Both free the slot. They are associated functions, so they
  cannot shadow a method of the resource.

### Changed

- With `reap_interval` set, the reaper now refills the idle set to `min_idle` on
  each pass (never past `max_size`). Before, `min_idle` resources were created
  once at build time and never replaced, although the docs said the pool kept
  them ready.
- Idle resources are reused last in, first out. The most recently returned
  resource is handed out first, so under light load the surplus resources reach
  `idle_timeout` and expire, as configured. Before, checkout rotated through
  every idle resource and kept all of them warm.
- Waiting threads are served in arrival order, and a returned resource is
  offered to the one that has waited longest. A newly arriving thread may still
  take a free resource first, but only while the oldest waiter has waited less
  than about a millisecond; after that, newcomers queue behind the waiters.
  Before, a newcomer could take a returned resource ahead of a waiter for as
  long as the pool stayed saturated, so an early waiter could time out while
  later arrivals succeeded. (A strict hand-off to the oldest waiter was measured
  and cut saturated throughput by more than ten times, so newcomers keep the
  short head start.)
- A guard dropped while its thread is panicking is discarded without calling
  `recycle`, since the resource may have been left half way through an
  operation, and a second panic during unwinding would abort the process.
- Docs: the timeouts bound the wait for a free slot, not `create` or `validate`
  once started; `try_get` never waits for another thread but may run `create`;
  `mem::forget` on a guard keeps its slot taken; `Error::Backend` carries the
  manager's error unchanged into `Display`.
- Checkout skips reading the clock when neither `idle_timeout` nor
  `max_lifetime` is set.

### Fixed

- A panic in `create`, `validate` or `recycle` leaked the slot reserved for the
  resource. After `max_size` such panics, every `get` returned `Timeout` for the
  life of the pool. Each hook now runs under a guard that gives the slot back on
  unwind.
- `get_timeout(Duration::MAX)`, and `get` with `create_timeout` set to
  `Duration::MAX`, panicked on `Instant` overflow. A timeout too large to
  represent now waits indefinitely.
- A panicking `create` during the reaper's refill no longer ends the reaper
  thread.

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
  `LICENSE-MIT`, `README.md`, and `CHANGELOG.md`.
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

[Unreleased]: https://github.com/jamesgober/pool-mod/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/jamesgober/pool-mod/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/jamesgober/pool-mod/compare/v0.9.0...v1.0.0
[0.9.0]: https://github.com/jamesgober/pool-mod/compare/v0.5.0...v0.9.0
[0.5.0]: https://github.com/jamesgober/pool-mod/compare/v0.2.0...v0.5.0
[0.2.0]: https://github.com/jamesgober/pool-mod/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/pool-mod/releases/tag/v0.1.0