//! Integration tests for resource lifecycle: expiry, recycling, and shutdown.
//!
//! The pool API requires the `std` feature, so the whole suite compiles away
//! without it.
#![cfg(feature = "std")]

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use pool_mod::{Error, Manager, Pool};

#[derive(Debug)]
struct RecycleError;

impl fmt::Display for RecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("recycle failed")
    }
}

impl std::error::Error for RecycleError {}

/// A manager that counts creations and can be told to fail recycling, so the
/// expiry and discard paths can be observed end to end.
struct Steer {
    created: Arc<AtomicUsize>,
    recycle_fails: Arc<AtomicBool>,
}

impl Steer {
    fn new() -> (Self, Arc<AtomicUsize>, Arc<AtomicBool>) {
        let created = Arc::new(AtomicUsize::new(0));
        let recycle_fails = Arc::new(AtomicBool::new(false));
        let steer = Steer {
            created: Arc::clone(&created),
            recycle_fails: Arc::clone(&recycle_fails),
        };
        (steer, created, recycle_fails)
    }
}

impl Manager for Steer {
    type Resource = usize;
    type Error = RecycleError;

    fn create(&self) -> Result<usize, Self::Error> {
        Ok(self.created.fetch_add(1, Ordering::SeqCst))
    }

    fn recycle(&self, _resource: &mut usize) -> Result<(), Self::Error> {
        if self.recycle_fails.load(Ordering::SeqCst) {
            return Err(RecycleError);
        }
        Ok(())
    }
}

#[test]
fn idle_resource_is_reused_before_timeout() {
    let (steer, created, _) = Steer::new();
    let pool = Pool::builder(steer)
        .max_size(2)
        .idle_timeout(Some(Duration::from_secs(3600)))
        .build()
        .expect("configuration is valid");

    drop(pool.get().expect("first checkout"));
    drop(pool.get().expect("second checkout"));

    // Well within the idle window, so the same resource is reused.
    assert_eq!(created.load(Ordering::SeqCst), 1);
}

#[test]
fn idle_resource_is_replaced_after_timeout() {
    let (steer, created, _) = Steer::new();
    let pool = Pool::builder(steer)
        .max_size(2)
        .idle_timeout(Some(Duration::from_millis(40)))
        .build()
        .expect("configuration is valid");

    drop(pool.get().expect("first checkout")); // resource 0 goes idle
    assert_eq!(created.load(Ordering::SeqCst), 1);

    thread::sleep(Duration::from_millis(120)); // let it age past idle_timeout

    // The aged idle resource is discarded on checkout and replaced.
    let fresh = pool.get().expect("a fresh resource");
    assert_eq!(*fresh, 1);
    assert_eq!(created.load(Ordering::SeqCst), 2);
}

#[test]
fn resource_is_replaced_after_max_lifetime() {
    let (steer, created, _) = Steer::new();
    let pool = Pool::builder(steer)
        .max_size(2)
        .max_lifetime(Some(Duration::from_millis(40)))
        .build()
        .expect("configuration is valid");

    drop(pool.get().expect("first checkout"));
    thread::sleep(Duration::from_millis(120));

    let fresh = pool.get().expect("a fresh resource");
    assert_eq!(*fresh, 1);
    assert_eq!(created.load(Ordering::SeqCst), 2);
}

#[test]
fn recycle_failure_discards_resource() {
    let (steer, _created, recycle_fails) = Steer::new();
    let pool = Pool::builder(steer)
        .max_size(2)
        .build()
        .expect("configuration is valid");

    recycle_fails.store(true, Ordering::SeqCst);
    {
        let _resource = pool.get().expect("checkout succeeds");
        assert_eq!(pool.status().size, 1);
    }
    // Recycling failed on return, so the resource was dropped, not pooled.
    assert_eq!(pool.status().size, 0);
    assert_eq!(pool.status().idle, 0);
}

#[test]
fn close_while_borrowed_discards_on_return() {
    let (steer, _created, _) = Steer::new();
    let pool = Pool::builder(steer)
        .max_size(2)
        .build()
        .expect("configuration is valid");

    let borrowed = pool.get().expect("checkout succeeds");
    assert_eq!(pool.status().in_use, 1);

    pool.close();
    assert!(pool.is_closed());
    // The borrowed resource still exists until its guard drops.
    assert_eq!(pool.status().size, 1);

    drop(borrowed);
    // On return into a closed pool, the resource is discarded.
    assert_eq!(pool.status().size, 0);
    assert!(matches!(pool.get(), Err(Error::Closed)));
}

#[test]
fn background_reaper_prunes_expired_idle() {
    let (steer, _created, _) = Steer::new();
    let pool = Pool::builder(steer)
        .max_size(4)
        .idle_timeout(Some(Duration::from_millis(15)))
        .reap_interval(Some(Duration::from_millis(15)))
        .build()
        .expect("configuration is valid");

    // Park two resources in the idle set.
    let a = pool.get().expect("first");
    let b = pool.get().expect("second");
    drop((a, b));
    assert_eq!(pool.status().idle, 2);

    // Give the reaper several ticks to notice they have aged out.
    thread::sleep(Duration::from_millis(200));

    let status = pool.status();
    assert_eq!(
        status.idle, 0,
        "reaper should have pruned the aged idle resources"
    );
    assert_eq!(status.size, 0);
}

#[test]
fn lazy_expiry_keeps_idle_until_borrowed_without_reaper() {
    let (steer, created, _) = Steer::new();
    let pool = Pool::builder(steer)
        .max_size(2)
        .idle_timeout(Some(Duration::from_millis(15)))
        // No reap_interval: the reaper is disabled, expiry is lazy.
        .build()
        .expect("configuration is valid");

    drop(pool.get().expect("first checkout"));
    assert_eq!(pool.status().idle, 1);

    thread::sleep(Duration::from_millis(200));

    // With no reaper, the aged resource lingers in the idle set...
    assert_eq!(pool.status().idle, 1);
    assert_eq!(created.load(Ordering::SeqCst), 1);

    // ...and is only discarded and replaced when it is next borrowed.
    let fresh = pool.get().expect("a fresh resource");
    assert_eq!(*fresh, 1);
    assert_eq!(created.load(Ordering::SeqCst), 2);
}

#[test]
fn reaper_stops_after_close() {
    let (steer, _created, _) = Steer::new();
    let pool = Pool::builder(steer)
        .max_size(2)
        .idle_timeout(Some(Duration::from_millis(15)))
        .reap_interval(Some(Duration::from_millis(15)))
        .build()
        .expect("configuration is valid");

    pool.close();
    assert!(pool.is_closed());
    // Closing signals the reaper to stop; nothing should hang or panic.
    thread::sleep(Duration::from_millis(50));
    assert!(matches!(pool.get(), Err(Error::Closed)));
}

#[test]
fn reaper_exits_when_pool_is_dropped() {
    let (steer, _created, _) = Steer::new();
    let pool = Pool::builder(steer)
        .max_size(2)
        .idle_timeout(Some(Duration::from_millis(15)))
        .reap_interval(Some(Duration::from_millis(15)))
        .build()
        .expect("configuration is valid");

    // Dropping the only handle stops the reaper via the shutdown signal. The test
    // completing without hanging is the assertion.
    drop(pool);
    thread::sleep(Duration::from_millis(50));
}

#[test]
fn waiter_is_woken_when_a_resource_returns() {
    let (steer, _created, _) = Steer::new();
    let pool = Pool::builder(steer)
        .max_size(1)
        .build()
        .expect("configuration is valid");

    let held = pool.get().expect("first checkout takes the only slot");

    let waiter = {
        let pool = pool.clone();
        thread::spawn(move || {
            // Blocks until the main thread returns the resource.
            pool.get_timeout(Duration::from_secs(5)).map(|guard| *guard)
        })
    };

    thread::sleep(Duration::from_millis(50));
    drop(held); // wake the waiter

    let got = waiter.join().expect("waiter thread did not panic");
    assert!(got.is_ok());
}
