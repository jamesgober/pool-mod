//! Property-based tests for the pool's sizing and lifecycle invariants.
//!
//! The pool API requires the `std` feature, so the whole suite compiles away
//! without it.
#![cfg(feature = "std")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use pool_mod::{Error, Manager, Pool};
use proptest::prelude::*;

/// A manager whose resource never fails to build, counting how many it has
/// created so a property can assert on reuse.
struct Counting {
    created: Arc<AtomicUsize>,
}

impl Manager for Counting {
    type Resource = usize;
    type Error = std::convert::Infallible;

    fn create(&self) -> Result<usize, Self::Error> {
        Ok(self.created.fetch_add(1, Ordering::SeqCst))
    }

    fn recycle(&self, _resource: &mut usize) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn manager() -> (Counting, Arc<AtomicUsize>) {
    let created = Arc::new(AtomicUsize::new(0));
    (
        Counting {
            created: Arc::clone(&created),
        },
        created,
    )
}

proptest! {
    /// A valid configuration builds and pre-warms exactly `min_idle` resources;
    /// an invalid one is rejected with `InvalidConfig`.
    #[test]
    fn build_validates_and_prewarms(max_size in 0usize..32, min_idle in 0usize..32) {
        let (mgr, _created) = manager();
        let result = Pool::builder(mgr).max_size(max_size).min_idle(min_idle).build();

        if max_size == 0 || min_idle > max_size {
            prop_assert!(matches!(result, Err(Error::InvalidConfig(_))));
        } else {
            let pool = result.expect("configuration is valid");
            let status = pool.status();
            prop_assert_eq!(status.idle, min_idle);
            prop_assert_eq!(status.size, min_idle);
            prop_assert_eq!(status.in_use, 0);
        }
    }

    /// The pool never hands out more than `max_size` resources at once, and the
    /// `size == idle + in_use` invariant holds before and after release.
    #[test]
    fn never_exceeds_max_size(max_size in 1usize..32, extra in 0usize..16) {
        let (mgr, _created) = manager();
        let pool = Pool::builder(mgr).max_size(max_size).build().expect("valid");

        // Greedily grab more than the pool can ever hand out.
        let mut guards = Vec::new();
        for _ in 0..(max_size + extra) {
            if let Ok(guard) = pool.try_get() {
                guards.push(guard);
            }
        }

        prop_assert_eq!(guards.len(), max_size);
        let busy = pool.status();
        prop_assert_eq!(busy.size, max_size);
        prop_assert_eq!(busy.in_use, max_size);
        prop_assert_eq!(busy.idle, 0);
        prop_assert_eq!(busy.size, busy.idle + busy.in_use);

        drop(guards);
        let free = pool.status();
        prop_assert_eq!(free.idle, max_size);
        prop_assert_eq!(free.in_use, 0);
        prop_assert_eq!(free.size, free.idle + free.in_use);
    }

    /// Sequential borrow-and-return reuses a single resource, no matter how many
    /// times it repeats — the pool creates exactly one.
    #[test]
    fn sequential_reuse_creates_one(iters in 1usize..64, max_size in 1usize..8) {
        let (mgr, created) = manager();
        let pool = Pool::builder(mgr).max_size(max_size).build().expect("valid");

        for _ in 0..iters {
            let guard = pool.get().expect("a resource is available");
            drop(guard);
        }

        prop_assert_eq!(created.load(Ordering::SeqCst), 1);
        prop_assert_eq!(pool.status().idle, 1);
    }

    /// Closing always drops every idle resource and rejects further checkouts.
    #[test]
    fn close_drops_idle_and_rejects(max_size in 1usize..16, prewarm in 0usize..16) {
        let min_idle = prewarm.min(max_size);
        let (mgr, _created) = manager();
        let pool = Pool::builder(mgr).max_size(max_size).min_idle(min_idle).build().expect("valid");

        pool.close();
        prop_assert!(pool.is_closed());
        prop_assert_eq!(pool.status().idle, 0);
        prop_assert!(matches!(pool.get(), Err(Error::Closed)));
    }
}
