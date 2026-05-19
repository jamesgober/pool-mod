<h1 align="center">
    <strong>pool-mod</strong>
    <br>
    <sup><sub>GENERIC OBJECT AND CONNECTION POOLING</sub></sup>
</h1>

<p align="center">
    <a href="https://crates.io/crates/pool-mod"><img alt="crates.io" src="https://img.shields.io/crates/v/pool-mod.svg"></a>
    <a href="https://docs.rs/pool-mod"><img alt="docs.rs" src="https://docs.rs/pool-mod/badge.svg"></a>
    <a href="https://github.com/jamesgober/pool-mod/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/jamesgober/pool-mod/actions/workflows/ci.yml/badge.svg"></a>
    <a href="#license"><img alt="license" src="https://img.shields.io/badge/license-Apache--2.0%20OR%20MIT-blue.svg"></a>
</p>

<p align="center">Async-safe min/max sizing, idle timeouts, max-lifetime enforcement, validation-on-borrow, health-check callbacks. Runtime-agnostic.</p>

---

## Status

**Active development.** Scaffolded and on the path to 1.0. See [.dev/ROADMAP.md](.dev/ROADMAP.md) for milestone tracking.

The public API is not yet stable. Pin specific versions; expect changes pre-1.0.

---

## What it does

Generic object and connection pooling. Async-safe with min/max sizing, idle timeouts, max-lifetime enforcement, validation-on-borrow, and health-check callbacks. Works for database connections, HTTP clients, worker threads, or any expensive resource.

---

## Quick start

```toml
[dependencies]
pool-mod = "0.1"
```

---

## Standards

- **REPS** governs every decision. See [REPS.md](REPS.md).
- **MSRV:** Rust 1.75.
- **Edition:** 2024.
- **Cross-platform:** Linux, macOS, Windows.

---

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

---

<sub>Copyright &copy; 2026 <strong>James Gober</strong>. All rights reserved.</sub>