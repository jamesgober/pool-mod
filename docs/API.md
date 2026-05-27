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
reference for the public API: every exported item, what it does, the meaning of
each parameter and return value, and runnable examples for each use case.

At `v0.1.0` the crate is a scaffold. The pooling API — the pool types, the
resource-lifecycle traits, and the error type — is introduced in `v0.2.0`; see
[`.dev/ROADMAP.md`](../.dev/ROADMAP.md) for the milestone plan. The only public
item today is the [`VERSION`](#version) constant, documented below. This page
grows with each milestone so that it always lists the full public surface.

## Table of Contents

- [Installation](#installation)
- [Public API](#public-api)
  - [`VERSION`](#version)
- [Compatibility](#compatibility)

## Installation

Add the crate to your `Cargo.toml`:

```toml
[dependencies]
pool-mod = "0.1"
```

The crate is edition 2021 with a Minimum Supported Rust Version of 1.75. It
compiles on Linux, macOS, and Windows.

## Public API

### `VERSION`

```rust
pub const VERSION: &str;
```

The crate version string, populated from `CARGO_PKG_VERSION` at build time. It
matches the `version` field in the crate's `Cargo.toml` exactly — for this
release, `"0.1.0"`.

Use it to record or report which version of `pool-mod` a binary was built
against, for example in a `--version` banner, a startup log line, or a
diagnostics endpoint.

**Type:** `&'static str` — borrowed for the lifetime of the program; no
allocation.

**Examples**

Read the version directly:

```rust
assert_eq!(pool_mod::VERSION, "0.1.0");
```

Surface it in a startup banner (a real program would use its logging facility
rather than `println!`):

```rust
let banner = format!("pool-mod {}", pool_mod::VERSION);
assert!(banner.starts_with("pool-mod "));
```

Compare against a minimum expected version by parsing the major/minor
components:

```rust
let mut parts = pool_mod::VERSION.split('.');
let major: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
let minor: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
assert!((major, minor) >= (0, 1));
```

## Compatibility

`pool-mod` follows semantic versioning. While the crate is pre-1.0 the public
API may change between minor versions as the pooling surface is built out; each
change is recorded in [`CHANGELOG.md`](../CHANGELOG.md) and in the per-version
release notes under [`docs/release/`](./release). The `1.0.0` release freezes
the API.
