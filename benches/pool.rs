//! Microbenchmarks for the pool's acquire/return hot path.
//!
//! Run with `cargo bench`. The benchmarks isolate the steady-state cost of
//! borrowing and returning a resource (the path that runs on every checkout),
//! separate from resource construction.

use std::convert::Infallible;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use pool_mod::{Manager, Pool};

/// A manager whose resource is trivial to build, so a benchmark measures the
/// pool machinery rather than the cost of `create`.
struct Trivial;

impl Manager for Trivial {
    type Resource = u64;
    type Error = Infallible;

    fn create(&self) -> Result<u64, Infallible> {
        Ok(0)
    }

    fn recycle(&self, _resource: &mut u64) -> Result<(), Infallible> {
        Ok(())
    }
}

/// Steady-state borrow-and-return against a pre-warmed pool: every iteration
/// reuses an idle resource and recycles it on drop. This is the hot path.
fn bench_get_return_reuse(c: &mut Criterion) {
    let pool = Pool::builder(Trivial)
        .max_size(16)
        .min_idle(16)
        .build()
        .expect("configuration is valid");

    c.bench_function("get_return_reuse", |b| {
        b.iter(|| {
            let guard = pool.get().expect("a resource is available");
            black_box(*guard);
        });
    });
}

/// Non-blocking checkout against a pre-warmed pool.
fn bench_try_get_reuse(c: &mut Criterion) {
    let pool = Pool::builder(Trivial)
        .max_size(16)
        .min_idle(16)
        .build()
        .expect("configuration is valid");

    c.bench_function("try_get_return_reuse", |b| {
        b.iter(|| {
            let guard = pool.try_get().expect("a resource is available");
            black_box(*guard);
        });
    });
}

/// Cost of sampling pool occupancy.
fn bench_status(c: &mut Criterion) {
    let pool = Pool::builder(Trivial)
        .max_size(16)
        .min_idle(8)
        .build()
        .expect("configuration is valid");

    c.bench_function("status", |b| {
        b.iter(|| black_box(pool.status()));
    });
}

criterion_group!(
    benches,
    bench_get_return_reuse,
    bench_try_get_reuse,
    bench_status
);
criterion_main!(benches);
