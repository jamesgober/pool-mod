//! # pool-mod
//!
//! Generic object and connection pooling for Rust.
//!
//! `pool-mod` lends out reusable resources — database connections, HTTP clients,
//! worker handles, or anything else expensive to construct — and reclaims each
//! one automatically when the borrow ends. You describe the resource's lifecycle
//! by implementing [`Manager`] (create, validate, recycle); the pool handles
//! sizing, blocking acquisition with timeouts, validation-on-borrow, and
//! idle/lifetime expiry.
//!
//! The pool is **runtime-agnostic**: it carries no async-runtime dependency.
//! [`Pool::get`] blocks the calling thread until a resource is free. In an async
//! context, acquire on a blocking-friendly executor thread (for example
//! `tokio::task::spawn_blocking`); the returned [`Pooled`] guard is `Send`, so it
//! can be held across `.await` points.
//!
//! ## Overview
//!
//! - [`Manager`] — the trait you implement to create, validate, and recycle
//!   resources.
//! - [`Pool`] — the thread-safe, cheaply-cloneable pool handle.
//! - [`Builder`] — fluent configuration, reached through [`Pool::builder`].
//! - [`PoolConfig`] — the underlying limits and lifecycle policy.
//! - [`Pooled`] — the RAII guard that returns its resource to the pool on drop.
//! - [`Status`] — a snapshot of pool occupancy.
//! - [`Error`] — the error type, generic over the manager's own error.
//!
//! ## Example
//!
//! ```
//! use pool_mod::{Manager, Pool};
//! use std::convert::Infallible;
//!
//! // Describe how to make, reset, and (optionally) validate the resource.
//! struct Widgets;
//!
//! impl Manager for Widgets {
//!     type Resource = String;
//!     type Error = Infallible;
//!
//!     fn create(&self) -> Result<String, Infallible> {
//!         Ok(String::with_capacity(1024))
//!     }
//!
//!     fn recycle(&self, buf: &mut String) -> Result<(), Infallible> {
//!         buf.clear(); // reuse the allocation, discard the contents
//!         Ok(())
//!     }
//! }
//!
//! // A pool of at most eight widgets, two kept ready at all times.
//! let pool = Pool::builder(Widgets)
//!     .max_size(8)
//!     .min_idle(2)
//!     .build()
//!     .expect("configuration is valid");
//!
//! // Borrow one; it returns to the pool when `widget` is dropped.
//! let mut widget = pool.get().expect("a widget is available");
//! widget.push_str("hello");
//! assert_eq!(widget.len(), 5);
//! ```
//!
//! ## Feature flags
//!
//! - `std` *(default)* — enables the pool. The pool relies on `std` threading,
//!   timing, and synchronization primitives. With `default-features = false` the
//!   crate is `no_std` and exposes only [`VERSION`].
//!
//! ## License
//!
//! Dual-licensed under Apache-2.0 OR MIT.

#![doc(html_root_url = "https://docs.rs/pool-mod")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(unused_must_use)]
#![deny(unused_results)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::todo)]
#![deny(clippy::unimplemented)]
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]
#![deny(clippy::dbg_macro)]
#![deny(clippy::unreachable)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![deny(clippy::missing_safety_doc)]

#[cfg(feature = "std")]
mod config;
#[cfg(feature = "std")]
mod error;
#[cfg(feature = "std")]
mod manager;
#[cfg(feature = "std")]
mod object;
#[cfg(feature = "std")]
mod pool;
#[cfg(feature = "std")]
mod status;

#[cfg(feature = "std")]
pub use crate::config::PoolConfig;
#[cfg(feature = "std")]
pub use crate::error::Error;
#[cfg(feature = "std")]
pub use crate::manager::Manager;
#[cfg(feature = "std")]
pub use crate::object::Pooled;
#[cfg(feature = "std")]
pub use crate::pool::{Builder, Pool};
#[cfg(feature = "std")]
pub use crate::status::Status;

/// Convenient re-exports: `use pool_mod::prelude::*;`.
///
/// Pulls in every public type needed to build and use a pool.
#[cfg(feature = "std")]
pub mod prelude {
    pub use crate::config::PoolConfig;
    pub use crate::error::Error;
    pub use crate::manager::Manager;
    pub use crate::object::Pooled;
    pub use crate::pool::{Builder, Pool};
    pub use crate::status::Status;
}

/// Crate version string, populated by Cargo at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
