<h1 align="center">
    <img width="99" alt="Rust logo" src="https://raw.githubusercontent.com/jamesgober/rust-collection/72baabd71f00e14aa9184efcb16fa3deddda3a0a/assets/rust-logo.svg">
    <br>
    <strong>pool-mod</strong>
    <br>
    <sup><sub>GENERIC OBJECT AND CONNECTION POOLING</sub></sup>
</h1>

<p align="center">
    <a href="https://crates.io/crates/pool-mod"><img alt="crates.io" src="https://img.shields.io/crates/v/pool-mod.svg"></a>
    <a href="https://crates.io/crates/pool-mod" alt="Download pool-mod"><img alt="Crates.io Downloads" src="https://img.shields.io/crates/d/pool-mod?color=%230099ff"></a>
    <a href="https://docs.rs/pool-mod"><img alt="docs.rs" src="https://docs.rs/pool-mod/badge.svg"></a>
    <a href="https://github.com/jamesgober/pool-mod/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/jamesgober/pool-mod/actions/workflows/ci.yml/badge.svg"></a>
    <a href="https://github.com/rust-lang/rfcs/blob/master/text/2495-min-rust-version.md" title="MSRV"><img alt="MSRV" src="https://img.shields.io/badge/MSRV-1.75%2B-blue"></a>
</p>

<p align="center">Async-safe min/max sizing, idle timeouts, max-lifetime enforcement, validation-on-borrow, health-check callbacks. Runtime-agnostic.</p>

<br>

<div align="left">
    <p>
        <strong>pool-mod</strong> is a generic pool for any resource that is expensive to build — database connections, HTTP clients, worker handles, parsers, large buffers. You describe the resource's lifecycle once by implementing a single trait; the pool takes care of <b>sizing</b>, <b>blocking acquisition with timeouts</b>, <b>validation-on-borrow</b>, and <b>idle / lifetime expiry</b>.
    </p>
    <p>
        The pool is <b>runtime-agnostic</b>: it pulls in <em>zero dependencies</em> and carries no async runtime. The borrow guard returned on checkout is <code>Send</code>, so it works in synchronous code directly and in async code by acquiring on a blocking-friendly executor thread (such as <code>tokio::task::spawn_blocking</code>).
    </p>
</div>

<hr>
<br>

## Features

- **Generic over any resource** — pool connections, clients, threads, buffers, or anything else through one [`Manager`](./docs/API.md#manager) trait.
- **Min / max sizing** — `min_idle` resources are created up front and kept ready; the pool grows on demand up to `max_size` and never beyond it.
- **Blocking acquisition with timeouts** — [`get`](./docs/API.md#poolget) waits up to a configured `create_timeout`; [`get_timeout`](./docs/API.md#poolget_timeout) overrides it per call, and [`try_get`](./docs/API.md#pooltry_get) never blocks.
- **Validation-on-borrow** — an optional `validate` hook (a health-check callback) runs on checkout; a resource that fails is discarded and replaced transparently.
- **Idle & max-lifetime expiry** — stale resources are dropped and replaced, bounded by `idle_timeout` and `max_lifetime`. Applied lazily on checkout by default, or eagerly by an opt-in background reaper (`reap_interval`).
- **RAII return** — the [`Pooled`](./docs/API.md#pooled) guard recycles and returns its resource automatically on drop. There is no `release` to forget and no way to leak a resource.
- **Thread-safe and cheap to share** — [`Pool`](./docs/API.md#pool) is `Send + Sync` and clones into another handle onto the same pool.
- **Runtime-agnostic, zero-dependency** — no async runtime, no third-party crates.
- **`no_std`-aware** — the crate root compiles without `std`; the pool itself is behind the default `std` feature.

<br>
<hr>
<br>

## Installation

```toml
[dependencies]
pool-mod = "0.9"
```

MSRV is Rust 1.75. The crate is edition 2021 and builds on Linux, macOS, and Windows.

<br>

## Quick Start

Implement [`Manager`](./docs/API.md#manager) for your resource, then build a pool and borrow from it:

```rust
use pool_mod::{Manager, Pool};
use std::convert::Infallible;

// Describe how to create, reset, and (optionally) validate the resource.
struct Buffers {
    capacity: usize,
}

impl Manager for Buffers {
    type Resource = Vec<u8>;
    type Error = Infallible;

    fn create(&self) -> Result<Vec<u8>, Infallible> {
        Ok(Vec::with_capacity(self.capacity))
    }

    fn recycle(&self, buf: &mut Vec<u8>) -> Result<(), Infallible> {
        buf.clear(); // reuse the allocation, drop the contents
        Ok(())
    }
}

let pool = Pool::builder(Buffers { capacity: 4096 })
    .max_size(16)
    .min_idle(4)
    .build()
    .expect("configuration is valid");

// Borrow a buffer; it returns to the pool when `buf` is dropped.
let mut buf = pool.get().expect("a buffer is available");
buf.extend_from_slice(b"payload");
assert_eq!(buf.len(), 7);
```

<br>

## How It Works

A pool owns up to `max_size` resources. Each checkout:

1. **Reuses** an idle resource if one is available — after applying `max_lifetime`, `idle_timeout`, and the `validate` health check. A resource that is too old, too stale, or invalid is dropped and the pool moves on.
2. **Creates** a new resource if none is idle and the pool has not reached `max_size`.
3. **Waits** if the pool is saturated, until a resource is returned or the timeout elapses.

When a [`Pooled`](./docs/API.md#pooled) guard is dropped, its resource is passed to `recycle` and returned to the idle set. If recycling fails — or the pool has been closed — the resource is dropped instead and its slot is freed for a replacement.

Resource construction, validation, and recycling all run **without the pool's internal lock held**, so a slow `create` (opening a socket, say) never blocks other threads from returning resources. The lock guards only a small queue and a couple of counters.

<br>

## Configuration

Configure through the [`Builder`](./docs/API.md#builder), or build a [`PoolConfig`](./docs/API.md#poolconfig) directly (for example, from a settings file).

| Setting          | Default | Meaning                                                            |
|------------------|---------|--------------------------------------------------------------------|
| `max_size`       | `10`    | Upper bound on resources owned at once (idle + checked out).       |
| `min_idle`       | `0`     | Resources created up front and kept ready. Must be ≤ `max_size`.   |
| `create_timeout` | `30s`   | How long `get` waits when saturated. `None` waits indefinitely.    |
| `idle_timeout`   | `None`  | Replace a resource unused for this long, checked on next borrow.   |
| `max_lifetime`   | `None`  | Replace a resource older than this, checked on next borrow.        |
| `reap_interval`  | `None`  | Background prune cadence. `None` applies expiry lazily on borrow.  |

```rust
use std::time::Duration;
use pool_mod::{Manager, Pool};
# use std::convert::Infallible;
# struct M;
# impl Manager for M {
#   type Resource = (); type Error = Infallible;
#   fn create(&self) -> Result<(), Infallible> { Ok(()) }
#   fn recycle(&self, _r: &mut ()) -> Result<(), Infallible> { Ok(()) }
# }

let pool = Pool::builder(M)
    .max_size(32)
    .min_idle(4)
    .create_timeout(Some(Duration::from_secs(5)))
    .idle_timeout(Some(Duration::from_secs(600)))
    .max_lifetime(Some(Duration::from_secs(3600)))
    .build()
    .expect("configuration is valid");
# let _ = pool;
```

<br>

## Using It From Async

The pool has no async dependency and `get` blocks the calling thread. In an async
context, acquire on a blocking-friendly executor thread; the returned guard is
`Send`, so it may be held across `.await` points:

```rust,ignore
let pool = pool.clone();
let mut conn = tokio::task::spawn_blocking(move || pool.get()).await??;
// `conn` is usable across awaits here.
```

A native non-blocking async acquisition API is on the roadmap (see below).

<br>

## API Overview

For the complete reference — every public item, its parameters, return values, error semantics, and runnable examples — see [`docs/API.md`](./docs/API.md).

- [`Manager`](./docs/API.md#manager) — the trait you implement: `create`, `recycle`, and the optional `validate` health check.
- [`Pool`](./docs/API.md#pool) — `builder` / `new`, `get` / `get_timeout` / `try_get`, `status`, `close`, `is_closed`.
- [`Builder`](./docs/API.md#builder) — fluent configuration.
- [`PoolConfig`](./docs/API.md#poolconfig) — limits and lifecycle policy.
- [`Pooled`](./docs/API.md#pooled) — the RAII guard, deref-coercing to your resource.
- [`Status`](./docs/API.md#status) — `size`, `idle`, `in_use`, `max_size`.
- [`Error`](./docs/API.md#error) — `Backend`, `Timeout`, `Closed`, `InvalidConfig`.

<br>

## Cross-Platform Support

- Linux (x86_64, aarch64)
- macOS (x86_64, Apple Silicon)
- Windows (x86_64)

CI runs formatting, lints, tests, and rustdoc with `-D warnings` on all three
operating systems, across both the stable toolchain and the MSRV (1.75).

<br>

## Testing

The suite covers every lifecycle path with unit tests, an eight-thread
concurrency test, `proptest` properties for the pool's invariants (the `max_size`
ceiling, the `size == idle + in_use` identity, reuse, and close semantics), and
doctests on every public item.

```bash
# Full suite (unit, integration, property, and doctests)
cargo test --all-features

# Microbenchmarks for the acquire/return hot path
cargo bench

# Lints and formatting, as enforced in CI
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

<br>

## Roadmap

`pool-mod` is fast-tracking to a stable 1.0. Property tests and hot-path
benchmarks landed in v0.5.0; the opt-in background reaper and the pre-1.0
hardening audit (zero-dependency, `cargo audit` / `cargo deny` clean, full
cross-platform matrix on stable and MSRV) landed in v0.9.0. Remaining before 1.0:
tracked benchmark baselines and the final API freeze. See
[`.dev/ROADMAP.md`](./.dev/ROADMAP.md).

<br>
<hr>

## Standards

- **REPS** governs every decision. See [REPS.md](REPS.md).
- **MSRV:** Rust 1.75.
- **Edition:** 2021.
- **Cross-platform:** Linux, macOS, Windows.

---

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

<!-- FOOT COPYRIGHT
################################################# -->
<div align="center">
  <h2></h2>
  <sup>COPYRIGHT <small>&copy;</small> 2025 <strong>JAMES GOBER.</strong></sup>
</div>
