//! Regression tests for the 1.1.0 review fixes: panicking hooks, timeout
//! overflow, the deadline passed to `create`, the `min_idle` refill, LIFO reuse,
//! and taking a resource out of the pool on purpose.

// Justification: in test code a failed setup or checkout has no meaningful
// recovery; unwinding with a clear panic is the correct outcome.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use pool_mod::{Error, Manager, Pool, Pooled};

/// A manager whose hooks can be told to panic, and which counts its calls.
#[derive(Default)]
struct Flaky {
    panic_create: AtomicBool,
    panic_validate: AtomicBool,
    panic_recycle: AtomicBool,
    created: AtomicUsize,
    recycled: AtomicUsize,
    slow_create: AtomicBool,
    deadlines: Mutex<Vec<Option<Instant>>>,
}

/// The manager handed to the pool; the test keeps its own handle on the flags.
struct M(Arc<Flaky>);

impl Manager for M {
    type Resource = usize;
    type Error = Infallible;

    fn create(&self) -> Result<usize, Infallible> {
        if self.0.panic_create.load(Ordering::SeqCst) {
            panic!("create panicked");
        }
        if self.0.slow_create.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(500));
        }
        Ok(self.0.created.fetch_add(1, Ordering::SeqCst))
    }

    fn create_with_deadline(&self, deadline: Option<Instant>) -> Result<usize, Infallible> {
        self.0.deadlines.lock().unwrap().push(deadline);
        self.create()
    }

    fn recycle(&self, _resource: &mut usize) -> Result<(), Infallible> {
        let _ = self.0.recycled.fetch_add(1, Ordering::SeqCst);
        if self.0.panic_recycle.load(Ordering::SeqCst) {
            panic!("recycle panicked");
        }
        Ok(())
    }

    fn validate(&self, _resource: &mut usize) -> bool {
        if self.0.panic_validate.load(Ordering::SeqCst) {
            panic!("validate panicked");
        }
        true
    }
}

const MAX: usize = 2;
/// More panics than the pool has slots: before the fix, any `MAX` of them left
/// the pool unable to hand out anything.
const PANICS: usize = 3 * MAX;

fn pool(flaky: &Arc<Flaky>) -> Pool<M> {
    Pool::builder(M(Arc::clone(flaky)))
        .max_size(MAX)
        .create_timeout(Some(Duration::from_millis(200)))
        .build()
        .unwrap()
}

/// After the panics, the pool must still hand out `MAX` resources at once.
fn assert_fully_usable(pool: &Pool<M>) {
    let status = pool.status();
    assert_eq!(status.in_use, 0, "no slot may stay reserved: {status:?}");
    let held: Vec<_> = (0..MAX)
        .map(|_| pool.get().expect("pool still hands out resources"))
        .collect();
    assert_eq!(pool.status().in_use, MAX);
    drop(held);
}

#[test]
fn panicking_create_does_not_leak_slots() {
    let flaky = Arc::new(Flaky::default());
    let pool = pool(&flaky);
    flaky.panic_create.store(true, Ordering::SeqCst);
    for _ in 0..PANICS {
        assert!(catch_unwind(AssertUnwindSafe(|| pool.get())).is_err());
    }
    flaky.panic_create.store(false, Ordering::SeqCst);
    assert_eq!(pool.status().size, 0);
    assert_fully_usable(&pool);
}

#[test]
fn panicking_validate_does_not_leak_slots() {
    let flaky = Arc::new(Flaky::default());
    let pool = pool(&flaky);
    for _ in 0..PANICS {
        // Put one resource in the idle set, then make its validation panic.
        drop(pool.get().unwrap());
        flaky.panic_validate.store(true, Ordering::SeqCst);
        assert!(catch_unwind(AssertUnwindSafe(|| pool.get())).is_err());
        flaky.panic_validate.store(false, Ordering::SeqCst);
    }
    assert_eq!(pool.status().size, 0, "the rejected resource was dropped");
    assert_fully_usable(&pool);
}

#[test]
fn panicking_recycle_does_not_leak_slots() {
    let flaky = Arc::new(Flaky::default());
    let pool = pool(&flaky);
    for _ in 0..PANICS {
        let guard = pool.get().unwrap();
        flaky.panic_recycle.store(true, Ordering::SeqCst);
        assert!(catch_unwind(AssertUnwindSafe(move || drop(guard))).is_err());
        flaky.panic_recycle.store(false, Ordering::SeqCst);
    }
    assert_eq!(pool.status().size, 0, "the unrecycled resource was dropped");
    assert_fully_usable(&pool);
}

#[test]
fn guard_dropped_while_panicking_is_discarded_without_recycle() {
    let flaky = Arc::new(Flaky::default());
    let pool = pool(&flaky);
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _guard = pool.get().unwrap();
        panic!("the borrower panicked mid-operation");
    }));
    assert!(result.is_err());
    assert_eq!(flaky.recycled.load(Ordering::SeqCst), 0);
    let status = pool.status();
    assert_eq!((status.size, status.idle), (0, 0));
    assert_fully_usable(&pool);
}

#[test]
fn duration_max_timeouts_do_not_panic() {
    let flaky = Arc::new(Flaky::default());
    let pool = pool(&flaky);
    drop(pool.get_timeout(Duration::MAX).expect("room to create one"));

    let unbounded = Pool::builder(M(Arc::clone(&flaky)))
        .max_size(1)
        .create_timeout(Some(Duration::MAX))
        .build()
        .unwrap();
    drop(unbounded.get().expect("room to create one"));
    // An unrepresentable deadline is treated as no deadline at all.
    assert_eq!(flaky.deadlines.lock().unwrap().last(), Some(&None));
}

#[test]
fn create_receives_the_callers_deadline() {
    let flaky = Arc::new(Flaky::default());
    let pool = Pool::builder(M(Arc::clone(&flaky)))
        .max_size(8)
        .min_idle(1)
        .create_timeout(None)
        .build()
        .unwrap();
    let _idle = pool.get().unwrap(); // takes the pre-created resource

    let before = Instant::now();
    let _a = pool.get_timeout(Duration::from_secs(60)).unwrap();
    let _b = pool.get().unwrap();
    let _c = pool.try_get().unwrap();
    let after = Instant::now();

    let deadlines = flaky.deadlines.lock().unwrap().clone();
    assert_eq!(deadlines.len(), 4);
    assert_eq!(
        deadlines[0], None,
        "build-time pre-creation has no deadline"
    );
    let timed = deadlines[1].expect("get_timeout passes its deadline");
    assert!(timed >= before + Duration::from_secs(60));
    assert!(timed <= after + Duration::from_secs(60));
    assert_eq!(
        deadlines[2], None,
        "create_timeout of None means no deadline"
    );
    let immediate = deadlines[3].expect("try_get passes the moment of the call");
    assert!(immediate >= before && immediate <= after);
}

#[test]
fn try_get_idle_never_creates() {
    let flaky = Arc::new(Flaky::default());
    flaky.slow_create.store(true, Ordering::SeqCst);
    let pool = pool(&flaky);

    let started = Instant::now();
    assert!(matches!(pool.try_get_idle(), Err(Error::Timeout)));
    assert!(started.elapsed() < Duration::from_millis(400));
    assert_eq!(flaky.created.load(Ordering::SeqCst), 0);

    drop(pool.get().unwrap()); // one slow create, then the resource is idle
    let reused = pool.try_get_idle().expect("an idle resource is ready");
    assert_eq!(*reused, 0);
    assert_eq!(flaky.created.load(Ordering::SeqCst), 1);
    drop(reused);

    pool.close();
    assert!(matches!(pool.try_get_idle(), Err(Error::Closed)));
}

#[test]
fn reaper_keeps_min_idle_ready() {
    let flaky = Arc::new(Flaky::default());
    let pool = Pool::builder(M(Arc::clone(&flaky)))
        .max_size(4)
        .min_idle(2)
        .idle_timeout(Some(Duration::from_millis(50)))
        .reap_interval(Some(Duration::from_millis(20)))
        .build()
        .unwrap();
    // Wait until the first two resources have expired and been replaced.
    let deadline = Instant::now() + Duration::from_secs(20);
    while flaky.created.load(Ordering::SeqCst) < 4 {
        assert!(Instant::now() < deadline, "the reaper never refilled");
        thread::sleep(Duration::from_millis(10));
    }
    // The refill reserves its slots in the same pass as the prune, so the
    // floor holds whenever the refilled resources have been created.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = pool.status();
        assert!(status.size <= 4);
        if status.idle == 2 {
            break;
        }
        assert!(Instant::now() < deadline, "idle stayed below min_idle");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn lifo_reuse_lets_surplus_resources_idle_out() {
    let flaky = Arc::new(Flaky::default());
    let pool = Pool::builder(M(Arc::clone(&flaky)))
        .max_size(4)
        .min_idle(4)
        .build()
        .unwrap();
    // Serial, light load: one resource at a time.
    let mut seen = BTreeSet::new();
    for _ in 0..20 {
        let guard = pool.get().unwrap();
        let _ = seen.insert(*guard);
    }
    assert_eq!(seen.len(), 1, "serial checkouts reuse the same resource");
}

#[test]
fn detach_and_discard_free_the_slot() {
    let flaky = Arc::new(Flaky::default());
    let pool = pool(&flaky);

    let owned = Pooled::detach(pool.get().unwrap());
    assert_eq!(owned, 0);
    assert_eq!(pool.status().size, 0);

    Pooled::discard(pool.get().unwrap());
    let status = pool.status();
    assert_eq!((status.size, status.idle), (0, 0));
    assert_eq!(flaky.recycled.load(Ordering::SeqCst), 0);
    assert_fully_usable(&pool);
}
