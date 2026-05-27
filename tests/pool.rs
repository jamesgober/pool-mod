//! End-to-end tests for the pool: a realistic manager exercised across threads.
//!
//! The pool API requires the `std` feature, so the whole suite compiles away
//! without it.
#![cfg(feature = "std")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use pool_mod::{Error, Manager, Pool};

/// A stand-in for a real connection: it knows its own id and counts how many
/// times it has been used since it was last recycled.
struct Connection {
    id: usize,
    queries: usize,
}

/// Opens [`Connection`]s and counts how many it has opened, so a test can assert
/// the pool never created more than `max_size`.
struct Connector {
    opened: Arc<AtomicUsize>,
}

impl Manager for Connector {
    type Resource = Connection;
    type Error = std::convert::Infallible;

    fn create(&self) -> Result<Connection, Self::Error> {
        let id = self.opened.fetch_add(1, Ordering::SeqCst);
        Ok(Connection { id, queries: 0 })
    }

    fn recycle(&self, conn: &mut Connection) -> Result<(), Self::Error> {
        conn.queries = 0; // pretend to reset session state
        Ok(())
    }
}

#[test]
fn end_to_end_borrow_use_return() {
    let opened = Arc::new(AtomicUsize::new(0));
    let pool = Pool::builder(Connector {
        opened: Arc::clone(&opened),
    })
    .max_size(2)
    .min_idle(1)
    .build()
    .expect("configuration is valid");

    assert_eq!(opened.load(Ordering::SeqCst), 1); // min_idle pre-created one
    assert_eq!(pool.status().idle, 1);

    {
        let mut conn = pool.get().expect("a connection is available");
        conn.queries += 1;
        assert_eq!(conn.queries, 1);
        assert_eq!(pool.status().in_use, 1);
    }

    // After return + recycle the connection is idle again with reset state.
    let conn = pool.get().expect("the recycled connection is available");
    assert_eq!(conn.queries, 0);
    assert_eq!(opened.load(Ordering::SeqCst), 1); // reused, not reopened
}

#[test]
fn end_to_end_concurrent_never_exceeds_max_size() {
    const MAX: usize = 4;
    const THREADS: usize = 8;
    const ITERS: usize = 250;

    let opened = Arc::new(AtomicUsize::new(0));
    let pool = Pool::builder(Connector {
        opened: Arc::clone(&opened),
    })
    .max_size(MAX)
    .build()
    .expect("configuration is valid");

    let mut handles = Vec::with_capacity(THREADS);
    for _ in 0..THREADS {
        let worker = pool.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..ITERS {
                let mut conn = worker.get().expect("a connection becomes available");
                conn.queries += 1;
                // The pool never opens more than `max_size` connections, so every
                // id stays within range.
                assert!(conn.id < MAX, "connection id {} exceeded max_size", conn.id);
            }
        }));
    }

    for handle in handles {
        handle.join().expect("worker thread did not panic");
    }

    assert!(opened.load(Ordering::SeqCst) <= MAX);
    let status = pool.status();
    assert_eq!(status.in_use, 0);
    assert!(status.size <= MAX);
    assert_eq!(status.size, status.idle + status.in_use);
}

#[test]
fn end_to_end_timeout_when_saturated() {
    let opened = Arc::new(AtomicUsize::new(0));
    let pool = Pool::builder(Connector { opened })
        .max_size(1)
        .build()
        .expect("configuration is valid");

    let _held = pool.get().expect("first checkout succeeds");
    let result = pool.get_timeout(Duration::from_millis(20));
    assert!(matches!(result, Err(Error::Timeout)));
}
