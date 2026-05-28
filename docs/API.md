<h1 align="center">
    <img width="99" alt="Rust logo" src="https://raw.githubusercontent.com/jamesgober/rust-collection/72baabd71f00e14aa9184efcb16fa3deddda3a0a/assets/rust-logo.svg">
    <br>
    <b>pool-mod</b>
    <br>
    <sub><sup>API REFERENCE</sup></sub>
</h1>
<div align="center">
    <sup>
        <a href="../README.md" title="Project Home"><b>HOME</b></a>
        <span>&nbsp;│&nbsp;</span>
        <span>API</span>
    </sup>
</div>

<br>

`pool-mod` is a generic object and connection pool. This document is the complete
reference for the public API as of `v0.9.0`: every exported item, what it does,
the meaning of each parameter and return value, the error semantics, and runnable
examples for each use case.

The whole pooling surface lives behind the default `std` feature. With
`default-features = false` the crate is `no_std` and exposes only
[`VERSION`](#version).

## Table of Contents

- [Installation](#installation)
- [Quick Start](#quick-start)
- [Public API](#public-api)
  - [`Manager`](#manager)
  - [`Pool`](#pool)
    - [`Pool::builder`](#poolbuilder)
    - [`Pool::new`](#poolnew)
    - [`Pool::get`](#poolget)
    - [`Pool::get_timeout`](#poolget_timeout)
    - [`Pool::try_get`](#pooltry_get)
    - [`Pool::status`](#poolstatus)
    - [`Pool::close`](#poolclose)
    - [`Pool::is_closed`](#poolis_closed)
  - [`Builder`](#builder)
  - [`PoolConfig`](#poolconfig)
  - [`Pooled`](#pooled)
  - [`Status`](#status)
  - [`Error`](#error)
  - [`prelude`](#prelude)
  - [`VERSION`](#version)
- [Patterns](#patterns)
  - [Pooling database connections](#pooling-database-connections)
  - [Validation-on-borrow](#validation-on-borrow)
  - [Sharing a pool across threads](#sharing-a-pool-across-threads)
  - [Using the pool from async code](#using-the-pool-from-async-code)
- [Feature Flags](#feature-flags)
- [Compatibility](#compatibility)

## Installation

```toml
[dependencies]
pool-mod = "0.9"
```

The crate is edition 2021 with a Minimum Supported Rust Version of 1.75, and
builds on Linux, macOS, and Windows.

## Quick Start

Implement [`Manager`](#manager) for your resource, build a [`Pool`](#pool), and
borrow from it. The borrow returns to the pool automatically when its guard is
dropped.

```rust
use pool_mod::{Manager, Pool};
use std::convert::Infallible;

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
        buf.clear();
        Ok(())
    }
}

let pool = Pool::builder(Buffers { capacity: 4096 })
    .max_size(16)
    .min_idle(4)
    .build()
    .expect("configuration is valid");

let mut buf = pool.get().expect("a buffer is available");
buf.extend_from_slice(b"payload");
assert_eq!(buf.len(), 7);
```

## Public API

### `Manager`

```rust
pub trait Manager: Send + Sync + 'static {
    type Resource: Send + 'static;
    type Error: Send + Sync + 'static;

    fn create(&self) -> Result<Self::Resource, Self::Error>;
    fn recycle(&self, resource: &mut Self::Resource) -> Result<(), Self::Error>;
    fn validate(&self, resource: &mut Self::Resource) -> bool { /* default: true */ }
}
```

The contract you implement to teach the pool how to manage a resource. One
manager instance is shared across every thread that uses the pool, which is why
it must be `Send + Sync`. The pool never holds its internal lock while calling
these methods, so a slow `create` or `validate` does not block other threads from
returning resources.

**Associated types**

- `Resource` — the value the pool stores and hands out. Must be `Send + 'static`
  so it can move between threads and live in the pool.
- `Error` — the error returned by `create` and `recycle`. Must be
  `Send + Sync + 'static`. Use a domain error type for real backends; use
  [`std::convert::Infallible`] when construction cannot fail.

**Methods**

| Method     | Signature                                              | Called when                                  |
|------------|--------------------------------------------------------|----------------------------------------------|
| `create`   | `fn create(&self) -> Result<Resource, Error>`          | the pool needs to grow toward `max_size`     |
| `recycle`  | `fn recycle(&self, &mut Resource) -> Result<(), Error>`| a borrowed resource is returned              |
| `validate` | `fn validate(&self, &mut Resource) -> bool`            | an idle resource is checked out (default `true`) |

`create` errors surface to the caller as [`Error::Backend`](#error) and the
reserved slot is released. `recycle` errors cause the resource to be discarded
(not pooled) and its slot freed. `validate` returning `false` discards the
resource and the pool tries the next idle one, or creates a fresh resource.

**Examples**

A minimal manager whose resource never fails to build:

```rust
use pool_mod::Manager;
use std::convert::Infallible;

struct Counters;

impl Manager for Counters {
    type Resource = u64;
    type Error = Infallible;

    fn create(&self) -> Result<u64, Infallible> {
        Ok(0)
    }

    fn recycle(&self, n: &mut u64) -> Result<(), Infallible> {
        *n = 0;
        Ok(())
    }
}
```

A manager with a real error type and a fallible constructor:

```rust
use pool_mod::Manager;
use std::fmt;

#[derive(Debug)]
struct ConnectError(String);

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "connect failed: {}", self.0)
    }
}
impl std::error::Error for ConnectError {}

struct Db {
    url: String,
}

impl Manager for Db {
    type Resource = String; // stand-in for a real connection handle
    type Error = ConnectError;

    fn create(&self) -> Result<String, ConnectError> {
        if self.url.is_empty() {
            return Err(ConnectError("empty url".into()));
        }
        Ok(format!("connection to {}", self.url))
    }

    fn recycle(&self, _conn: &mut String) -> Result<(), ConnectError> {
        Ok(()) // e.g. roll back any open transaction
    }
}
```

### `Pool`

```rust
pub struct Pool<M: Manager> { /* private */ }

impl<M: Manager> Clone for Pool<M> { /* shares one pool */ }
```

A thread-safe pool of reusable resources built by a [`Manager`](#manager).
`Pool<M>` is `Send + Sync` and cheap to clone — every clone is another handle onto
the same shared pool, so share it across threads by cloning rather than wrapping
it in an `Arc`.

The pool is runtime-agnostic and has no async dependency; see
[Using the pool from async code](#using-the-pool-from-async-code).

#### `Pool::builder`

```rust
pub fn builder(manager: M) -> Builder<M>
```

Start configuring a pool. Returns a [`Builder`](#builder) seeded with the default
[`PoolConfig`](#poolconfig).

- `manager` — the [`Manager`](#manager) that will create and maintain the
  resources.

```rust
use pool_mod::{Manager, Pool};
# use std::convert::Infallible;
# struct M;
# impl Manager for M {
#   type Resource = (); type Error = Infallible;
#   fn create(&self) -> Result<(), Infallible> { Ok(()) }
#   fn recycle(&self, _r: &mut ()) -> Result<(), Infallible> { Ok(()) }
# }
let pool = Pool::builder(M).max_size(8).build().expect("valid");
assert_eq!(pool.status().max_size, 8);
```

#### `Pool::new`

```rust
pub fn new(manager: M) -> Result<Pool<M>, Error<M::Error>>
```

Build a pool with the default configuration — a shortcut for
`Pool::builder(manager).build()`.

- `manager` — the [`Manager`](#manager) for the pool.

**Errors:** [`Error::Backend`](#error) if pre-creating the initial resources
fails. With the default `min_idle` of 0, nothing is created up front, so the
default-configured pool builds infallibly.

```rust
use pool_mod::{Manager, Pool};
# use std::convert::Infallible;
# struct M;
# impl Manager for M {
#   type Resource = (); type Error = Infallible;
#   fn create(&self) -> Result<(), Infallible> { Ok(()) }
#   fn recycle(&self, _r: &mut ()) -> Result<(), Infallible> { Ok(()) }
# }
let pool = Pool::new(M).expect("default config is valid");
assert_eq!(pool.status().max_size, 10); // the default
```

#### `Pool::get`

```rust
pub fn get(&self) -> Result<Pooled<M>, Error<M::Error>>
```

Borrow a resource, waiting up to the configured
[`create_timeout`](#poolconfig) if the pool is saturated. Reuses a validated idle
resource when one is available, grows the pool toward `max_size` when none is,
and otherwise blocks until a resource is returned or the timeout elapses. Returns
a [`Pooled`](#pooled) guard that returns the resource to the pool on drop.

**Errors:**

- [`Error::Backend`](#error) — the manager failed to create a resource.
- [`Error::Timeout`](#error) — the pool stayed saturated past `create_timeout`.
- [`Error::Closed`](#error) — the pool has been closed.

```rust
use pool_mod::{Manager, Pool};
# use std::convert::Infallible;
# struct M;
# impl Manager for M {
#   type Resource = u32; type Error = Infallible;
#   fn create(&self) -> Result<u32, Infallible> { Ok(7) }
#   fn recycle(&self, _r: &mut u32) -> Result<(), Infallible> { Ok(()) }
# }
let pool = Pool::builder(M).max_size(2).build().expect("valid");
let resource = pool.get().expect("available");
assert_eq!(*resource, 7);
```

#### `Pool::get_timeout`

```rust
pub fn get_timeout(&self, timeout: Duration) -> Result<Pooled<M>, Error<M::Error>>
```

Borrow a resource, waiting at most `timeout` regardless of the configured
`create_timeout`.

- `timeout` — the maximum time to wait. [`Duration::ZERO`] makes a non-blocking
  try that returns [`Error::Timeout`](#error) immediately if no resource can be
  handed out at once.

**Errors:** identical to [`Pool::get`](#poolget), with the timeout taken from the
argument.

```rust
use std::time::Duration;
use pool_mod::{Error, Manager, Pool};
# use std::convert::Infallible;
# struct M;
# impl Manager for M {
#   type Resource = u32; type Error = Infallible;
#   fn create(&self) -> Result<u32, Infallible> { Ok(0) }
#   fn recycle(&self, _r: &mut u32) -> Result<(), Infallible> { Ok(()) }
# }
let pool = Pool::builder(M).max_size(1).build().expect("valid");
let held = pool.get().expect("first checkout");
// The single slot is taken, so an immediate retry times out.
assert!(matches!(pool.get_timeout(Duration::ZERO), Err(Error::Timeout)));
```

#### `Pool::try_get`

```rust
pub fn try_get(&self) -> Result<Pooled<M>, Error<M::Error>>
```

Borrow a resource without ever blocking. Returns a resource if one can be handed
out immediately — an idle resource is ready, or the pool has room to create one —
and otherwise returns [`Error::Timeout`](#error) at once. Equivalent to
[`get_timeout(Duration::ZERO)`](#poolget_timeout).

**Errors:** identical to [`Pool::get`](#poolget); `Timeout` is returned the
instant no resource is available rather than after a wait.

```rust
use pool_mod::{Error, Manager, Pool};
# use std::convert::Infallible;
# struct M;
# impl Manager for M {
#   type Resource = u32; type Error = Infallible;
#   fn create(&self) -> Result<u32, Infallible> { Ok(0) }
#   fn recycle(&self, _r: &mut u32) -> Result<(), Infallible> { Ok(()) }
# }
let pool = Pool::builder(M).max_size(1).build().expect("valid");
let first = pool.try_get().expect("room to create one");
// The only slot is taken, so the next try fails immediately.
assert!(matches!(pool.try_get(), Err(Error::Timeout)));
```

#### `Pool::status`

```rust
pub fn status(&self) -> Status
```

Take a [`Status`](#status) snapshot of current occupancy. The counts are read
under the pool's lock but are immediately stale in a concurrent program; use them
for logging and metrics, not for correctness decisions.

```rust
use pool_mod::{Manager, Pool};
# use std::convert::Infallible;
# struct M;
# impl Manager for M {
#   type Resource = (); type Error = Infallible;
#   fn create(&self) -> Result<(), Infallible> { Ok(()) }
#   fn recycle(&self, _r: &mut ()) -> Result<(), Infallible> { Ok(()) }
# }
let pool = Pool::builder(M).max_size(4).min_idle(2).build().expect("valid");
let status = pool.status();
assert_eq!(status.idle, 2);
assert_eq!(status.in_use, 0);
assert_eq!(status.size, 2);
assert_eq!(status.max_size, 4);
```

#### `Pool::close`

```rust
pub fn close(&self)
```

Close the pool: discard every idle resource and reject all future checkouts with
[`Error::Closed`](#error). Resources currently checked out are unaffected and are
simply dropped — not returned to the idle set — when their guards fall. Closing is
idempotent. Idle resources are dropped outside the pool's lock, so a slow resource
destructor does not block other threads.

```rust
use pool_mod::{Error, Manager, Pool};
# use std::convert::Infallible;
# struct M;
# impl Manager for M {
#   type Resource = (); type Error = Infallible;
#   fn create(&self) -> Result<(), Infallible> { Ok(()) }
#   fn recycle(&self, _r: &mut ()) -> Result<(), Infallible> { Ok(()) }
# }
let pool = Pool::builder(M).max_size(2).min_idle(2).build().expect("valid");
pool.close();
assert!(pool.is_closed());
assert!(matches!(pool.get(), Err(Error::Closed)));
```

#### `Pool::is_closed`

```rust
pub fn is_closed(&self) -> bool
```

Report whether the pool has been [closed](#poolclose).

```rust
use pool_mod::{Manager, Pool};
# use std::convert::Infallible;
# struct M;
# impl Manager for M {
#   type Resource = (); type Error = Infallible;
#   fn create(&self) -> Result<(), Infallible> { Ok(()) }
#   fn recycle(&self, _r: &mut ()) -> Result<(), Infallible> { Ok(()) }
# }
let pool = Pool::new(M).expect("valid");
assert!(!pool.is_closed());
pool.close();
assert!(pool.is_closed());
```

### `Builder`

```rust
pub struct Builder<M: Manager> { /* private */ }
```

A fluent builder for a [`Pool`](#pool), created by [`Pool::builder`](#poolbuilder).
Each setter consumes and returns the builder, so calls chain. Every setter mirrors
a field of [`PoolConfig`](#poolconfig).

| Setter            | Argument           | Effect                                          |
|-------------------|--------------------|-------------------------------------------------|
| `max_size`        | `usize`            | Upper bound on owned resources.                 |
| `min_idle`        | `usize`            | Resources created up front and kept ready.      |
| `create_timeout`  | `Option<Duration>` | `get` wait bound; `None` waits forever.         |
| `idle_timeout`    | `Option<Duration>` | Idle-expiry window; `None` disables it.         |
| `max_lifetime`    | `Option<Duration>` | Lifetime cap; `None` disables it.               |
| `reap_interval`   | `Option<Duration>` | Background prune cadence; `None` disables the reaper. |
| `config`          | `PoolConfig`       | Replace the whole configuration at once.        |

`build` validates the configuration and pre-creates the `min_idle` resources:

```rust
pub fn build(self) -> Result<Pool<M>, Error<M::Error>>
```

**Errors:**

- [`Error::InvalidConfig`](#error) if `max_size` is zero or `min_idle` exceeds
  `max_size`.
- [`Error::Backend`](#error) if creating one of the `min_idle` resources fails;
  any already-created resources are dropped before returning.

**Examples**

Tune several settings and pre-warm the pool:

```rust
use std::time::Duration;
use pool_mod::{Manager, Pool};
# use std::convert::Infallible;
# struct M;
# impl Manager for M {
#   type Resource = u32; type Error = Infallible;
#   fn create(&self) -> Result<u32, Infallible> { Ok(0) }
#   fn recycle(&self, _r: &mut u32) -> Result<(), Infallible> { Ok(()) }
# }
let pool = Pool::builder(M)
    .max_size(32)
    .min_idle(4)
    .idle_timeout(Some(Duration::from_secs(600)))
    .max_lifetime(Some(Duration::from_secs(3600)))
    .build()
    .expect("configuration is valid");
assert_eq!(pool.status().idle, 4);
```

An invalid configuration is rejected at build time:

```rust
use pool_mod::{Error, Manager, Pool};
# use std::convert::Infallible;
# struct M;
# impl Manager for M {
#   type Resource = (); type Error = Infallible;
#   fn create(&self) -> Result<(), Infallible> { Ok(()) }
#   fn recycle(&self, _r: &mut ()) -> Result<(), Infallible> { Ok(()) }
# }
let result = Pool::builder(M).max_size(0).build();
assert!(matches!(result, Err(Error::InvalidConfig(_))));
```

### `PoolConfig`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolConfig {
    pub max_size: usize,
    pub min_idle: usize,
    pub create_timeout: Option<Duration>,
    pub idle_timeout: Option<Duration>,
    pub max_lifetime: Option<Duration>,
    pub reap_interval: Option<Duration>,
}
```

The limits and lifecycle policy for a pool. Build one through the
[`Builder`](#builder), or construct it directly — every field is public, so it can
be deserialized from a settings file and handed to
[`Builder::config`](#builder). Validation happens at build time, not on
construction.

**Fields**

| Field            | Default | Meaning                                                          |
|------------------|---------|------------------------------------------------------------------|
| `max_size`       | `10`    | Hard cap on owned resources (idle + checked out). Must be ≥ 1.   |
| `min_idle`       | `0`     | Resources created up front and kept ready. Must be ≤ `max_size`. |
| `create_timeout` | `30s`   | `get` wait bound when saturated. `None` waits indefinitely.      |
| `idle_timeout`   | `None`  | Replace a resource unused this long, checked on next borrow.     |
| `max_lifetime`   | `None`  | Replace a resource older than this, checked on next borrow.      |
| `reap_interval`  | `None`  | Background prune cadence; `None` applies expiry lazily on borrow. |

`PoolConfig::default()` returns the values in the table above.

When `reap_interval` is `Some`, the pool spawns a background thread that prunes
idle resources past `idle_timeout` / `max_lifetime` on that cadence, instead of
waiting for them to be rejected at their next checkout. The reaper only prunes —
it never creates resources — and has no effect unless `idle_timeout` or
`max_lifetime` is also set. It holds a weak reference to the pool and stops when
the pool is closed or its last handle is dropped. If the OS cannot spawn the
thread, the pool falls back to lazy, checkout-time expiry.

**Examples**

Start from the defaults and override a couple of fields:

```rust
use std::time::Duration;
use pool_mod::PoolConfig;

let cfg = PoolConfig {
    max_size: 16,
    min_idle: 2,
    idle_timeout: Some(Duration::from_secs(300)),
    ..PoolConfig::default()
};
assert_eq!(cfg.max_size, 16);
assert_eq!(cfg.create_timeout, Some(Duration::from_secs(30))); // inherited
```

Hand a pre-built configuration to the builder:

```rust
use pool_mod::{Manager, Pool, PoolConfig};
# use std::convert::Infallible;
# struct M;
# impl Manager for M {
#   type Resource = (); type Error = Infallible;
#   fn create(&self) -> Result<(), Infallible> { Ok(()) }
#   fn recycle(&self, _r: &mut ()) -> Result<(), Infallible> { Ok(()) }
# }
let cfg = PoolConfig { max_size: 4, ..PoolConfig::default() };
let pool = Pool::builder(M).config(cfg).build().expect("valid");
assert_eq!(pool.status().max_size, 4);
```

### `Pooled`

```rust
pub struct Pooled<M: Manager> { /* private */ }

impl<M: Manager> Deref for Pooled<M> { type Target = M::Resource; }
impl<M: Manager> DerefMut for Pooled<M> { }
impl<M: Manager> Drop for Pooled<M> { }
```

The RAII guard returned by [`Pool::get`](#poolget) and
[`Pool::get_timeout`](#poolget_timeout). It deref-coerces to the underlying
resource, so it can be used anywhere a `&M::Resource` or `&mut M::Resource` is
expected. When the guard is dropped, the resource is recycled (via
[`Manager::recycle`](#manager)) and returned to the idle set — there is no
`release` call to remember, and forgetting a guard cannot leak a resource.

If recycling fails, or the pool has been closed, the resource is dropped instead
of pooled and its slot is freed. The guard is `Send` whenever the resource is, so
it may be held across `.await` points.

**Examples**

Mutate the resource through `DerefMut`, then let it return on scope exit:

```rust
use pool_mod::{Manager, Pool};
# use std::convert::Infallible;
struct Counters;
impl Manager for Counters {
    type Resource = u64;
    type Error = Infallible;
    fn create(&self) -> Result<u64, Infallible> { Ok(0) }
    fn recycle(&self, _r: &mut u64) -> Result<(), Infallible> { Ok(()) }
}

let pool = Pool::builder(Counters).max_size(2).build().expect("valid");
{
    let mut n = pool.get().expect("available");
    *n += 41;
    assert_eq!(*n, 41);
} // recycled and returned here
assert_eq!(pool.status().idle, 1);
```

Call a method on the pooled resource directly through deref coercion:

```rust
use pool_mod::{Manager, Pool};
# use std::convert::Infallible;
struct Strings;
impl Manager for Strings {
    type Resource = String;
    type Error = Infallible;
    fn create(&self) -> Result<String, Infallible> { Ok(String::new()) }
    fn recycle(&self, s: &mut String) -> Result<(), Infallible> { s.clear(); Ok(()) }
}

let pool = Pool::builder(Strings).max_size(1).build().expect("valid");
let mut s = pool.get().expect("available");
s.push_str("hello");          // String::push_str via DerefMut
assert_eq!(s.len(), 5);       // String::len via Deref
```

### `Status`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub size: usize,
    pub idle: usize,
    pub in_use: usize,
    pub max_size: usize,
}
```

A point-in-time snapshot from [`Pool::status`](#poolstatus). The invariant
`size == idle + in_use` holds for any single snapshot.

**Fields**

- `size` — total resources owned now (idle + checked out + any being created).
- `idle` — resources available for immediate checkout.
- `in_use` — resources lent out (computed as `size - idle`, so in-flight
  creations count as in-use).
- `max_size` — the configured cap the pool will not exceed.

**Example**

```rust
use pool_mod::{Manager, Pool};
# use std::convert::Infallible;
# struct M;
# impl Manager for M {
#   type Resource = (); type Error = Infallible;
#   fn create(&self) -> Result<(), Infallible> { Ok(()) }
#   fn recycle(&self, _r: &mut ()) -> Result<(), Infallible> { Ok(()) }
# }
let pool = Pool::builder(M).max_size(3).build().expect("valid");
let a = pool.get().expect("first");
let b = pool.get().expect("second");
let status = pool.status();
assert_eq!(status.in_use, 2);
assert_eq!(status.size, 2);
assert_eq!(status.size, status.idle + status.in_use);
drop((a, b));
```

### `Error`

```rust
#[non_exhaustive]
#[derive(Debug)]
pub enum Error<E> {
    Backend(E),
    Timeout,
    Closed,
    InvalidConfig(&'static str),
}
```

The error type for pool operations, generic over the manager's own error type
`E` ([`Manager::Error`](#manager)). It implements [`Display`] and, when `E`
implements [`std::error::Error`], implements `std::error::Error` too — with
`source()` returning the inner error for the `Backend` variant. The enum is
`#[non_exhaustive]`, so a match on it must include a wildcard arm.

**Variants**

| Variant              | Meaning                                                       | Typical response                          |
|----------------------|---------------------------------------------------------------|-------------------------------------------|
| `Backend(E)`         | The manager failed to create or recycle a resource.           | Inspect `E`; retry if transient.          |
| `Timeout`            | The configured wait elapsed before a resource was free.       | Retry later, or raise `max_size`.         |
| `Closed`             | The pool has been [closed](#poolclose).                       | Stop using the pool; it is terminal.      |
| `InvalidConfig(msg)` | The configuration was invalid at [`build`](#builder) time.    | Fix the named constraint.                 |

**Examples**

Branch on the failure mode:

```rust
use pool_mod::Error;
use std::convert::Infallible;

fn describe(err: &Error<Infallible>) -> &'static str {
    match err {
        Error::Timeout => "pool busy, try again",
        Error::Closed => "pool shut down",
        Error::Backend(_) => "backend failure",
        _ => "other",
    }
}

assert_eq!(describe(&Error::Timeout), "pool busy, try again");
```

Recover the underlying cause through `source()`:

```rust
use std::error::Error as _;
use pool_mod::Error;
use std::fmt;

#[derive(Debug)]
struct Boom;
impl fmt::Display for Boom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("boom") }
}
impl std::error::Error for Boom {}

let err = Error::Backend(Boom);
assert_eq!(err.to_string(), "resource manager error: boom");
assert!(err.source().is_some());
```

### `prelude`

```rust
pub mod prelude { /* re-exports */ }
```

Brings the full public surface into scope in one line:

```rust
use pool_mod::prelude::*;
// Manager, Pool, Builder, PoolConfig, Pooled, Status, Error are all in scope.
```

### `VERSION`

```rust
pub const VERSION: &str;
```

The crate version string, populated from `CARGO_PKG_VERSION` at build time —
`"0.9.0"` for this release. Available even under `no_std`. Useful in a `--version`
banner, a startup log line, or a diagnostics endpoint.

```rust
assert_eq!(pool_mod::VERSION, "0.9.0");
```

## Patterns

### Pooling database connections

A `Manager` whose `recycle` resets per-connection state, and whose `validate`
checks the connection is still healthy before lending it out:

```rust
use pool_mod::{Manager, Pool};
use std::fmt;

#[derive(Debug)]
struct DbError(&'static str);
impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.0) }
}
impl std::error::Error for DbError {}

struct Conn {
    healthy: bool,
    in_transaction: bool,
}

struct Postgres;

impl Manager for Postgres {
    type Resource = Conn;
    type Error = DbError;

    fn create(&self) -> Result<Conn, DbError> {
        Ok(Conn { healthy: true, in_transaction: false })
    }

    fn recycle(&self, conn: &mut Conn) -> Result<(), DbError> {
        if conn.in_transaction {
            conn.in_transaction = false; // ROLLBACK
        }
        Ok(())
    }

    fn validate(&self, conn: &mut Conn) -> bool {
        conn.healthy // e.g. a cheap `SELECT 1`
    }
}

let pool = Pool::builder(Postgres).max_size(8).min_idle(2).build()
    .expect("configuration is valid");
let conn = pool.get().expect("a healthy connection");
assert!(conn.healthy);
```

### Validation-on-borrow

`validate` runs on every checkout of an idle resource. Returning `false` discards
it and the pool transparently supplies a replacement, so callers never see a dead
resource:

```rust
use pool_mod::{Manager, Pool};
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};

struct Sessions {
    accept: AtomicBool,
}

impl Manager for Sessions {
    type Resource = u32;
    type Error = Infallible;
    fn create(&self) -> Result<u32, Infallible> { Ok(1) }
    fn recycle(&self, _s: &mut u32) -> Result<(), Infallible> { Ok(()) }
    fn validate(&self, _s: &mut u32) -> bool { self.accept.load(Ordering::SeqCst) }
}

let pool = Pool::builder(Sessions { accept: AtomicBool::new(true) })
    .max_size(4)
    .build()
    .expect("valid");
let s = pool.get().expect("a valid session");
assert_eq!(*s, 1);
```

### Sharing a pool across threads

`Pool` is `Send + Sync`; clone it to hand a handle to each worker. All clones
share the same resources and limits:

```rust
use pool_mod::{Manager, Pool};
use std::convert::Infallible;
use std::thread;

struct M;
impl Manager for M {
    type Resource = usize;
    type Error = Infallible;
    fn create(&self) -> Result<usize, Infallible> { Ok(0) }
    fn recycle(&self, _r: &mut usize) -> Result<(), Infallible> { Ok(()) }
}

let pool = Pool::builder(M).max_size(4).build().expect("valid");
let mut handles = Vec::new();
for _ in 0..4 {
    let worker = pool.clone();
    handles.push(thread::spawn(move || {
        let mut r = worker.get().expect("available");
        *r += 1;
    }));
}
for h in handles {
    h.join().expect("no panic");
}
assert_eq!(pool.status().in_use, 0);
```

### Using the pool from async code

`Pool::get` blocks the calling thread. Under an async runtime, acquire on a
blocking-friendly thread so the executor is not stalled. The returned guard is
`Send`, so it can be held across `.await`:

```rust,ignore
let pool = pool.clone();
let mut conn = tokio::task::spawn_blocking(move || pool.get()).await??;
// `conn` may be used across awaits here.
```

A native non-blocking async API is planned for a future release.

## Feature Flags

| Feature | Default | Effect                                                                 |
|---------|---------|------------------------------------------------------------------------|
| `std`   | yes     | Enables the pool. Without it the crate is `no_std` and exposes only [`VERSION`](#version). |

The pool relies on `std` threading, timing, and synchronization primitives, so
the entire pooling API requires the `std` feature. Feature flags are additive.

## Compatibility

`pool-mod` follows semantic versioning. While the crate is pre-1.0 the public API
may change between minor versions as the remaining roadmap items land (a
background idle reaper, a native async API). Every change is recorded in
[`CHANGELOG.md`](../CHANGELOG.md) and in the per-version notes under
[`docs/release/`](./release). The `1.0.0` release freezes the API.
