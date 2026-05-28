//! The pool itself: [`Pool`] and its [`Builder`].

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::PoolConfig;
use crate::error::Error;
use crate::manager::Manager;
use crate::object::Pooled;
use crate::status::Status;

/// Acquire a mutex guard, recovering the data even if a previous holder panicked.
///
/// The pool only mutates plain counters and a queue while the lock is held, never
/// running user code, so the protected state is always consistent at an unlock
/// point. Honouring poison would convert an unrelated panic elsewhere into a
/// permanent, unrecoverable pool outage, which is the worse failure mode.
#[inline]
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A resource resting in the idle set, tagged with the timestamps the pool uses
/// to enforce `idle_timeout` and `max_lifetime`.
pub(crate) struct Idle<R> {
    pub(crate) resource: R,
    pub(crate) created_at: Instant,
    pub(crate) last_used: Instant,
}

/// The mutable inner state guarded by the pool's mutex.
struct State<R> {
    idle: VecDeque<Idle<R>>,
    /// Resources the pool currently owns: idle, checked out, or mid-creation.
    total: usize,
    closed: bool,
}

/// The decision reached while holding the lock, carried out once it is released.
///
/// Only outcomes that require running user code — and therefore must not hold the
/// lock — escape the locked region; waiting and immediate errors are handled in
/// place.
enum Action<R> {
    Reuse(Idle<R>),
    Create,
}

/// The stop signal shared between the pool and its background reaper thread.
///
/// Held by both sides through an [`Arc`] so it outlives the [`PoolInner`] it
/// guards: when the last pool handle drops, `PoolInner`'s destructor sets the
/// flag and wakes the reaper, which then exits.
struct Shutdown {
    stop: Mutex<bool>,
    wake: Condvar,
}

impl Shutdown {
    fn new() -> Self {
        Shutdown {
            stop: Mutex::new(false),
            wake: Condvar::new(),
        }
    }

    /// Signal the reaper to stop and wake it immediately.
    fn signal(&self) {
        let mut stop = lock(&self.stop);
        *stop = true;
        drop(stop);
        self.wake.notify_all();
    }
}

/// Shared pool state behind an [`Arc`]. Every [`Pool`] handle and every live
/// [`Pooled`] guard holds one of these.
pub(crate) struct PoolInner<M: Manager> {
    pub(crate) manager: M,
    config: PoolConfig,
    state: Mutex<State<M::Resource>>,
    available: Condvar,
    shutdown: Arc<Shutdown>,
}

impl<M: Manager> Drop for PoolInner<M> {
    fn drop(&mut self) {
        // The last handle is gone. Stop the reaper (if one is running) so it does
        // not outlive the pool it maintains.
        self.shutdown.signal();
    }
}

impl<M: Manager> PoolInner<M> {
    /// Take a resource out of the pool, blocking until one is free, a slot opens
    /// for a fresh one, or the deadline passes.
    fn acquire(
        &self,
        deadline: Option<Instant>,
    ) -> Result<(M::Resource, Instant), Error<M::Error>> {
        loop {
            let action = {
                let mut state = lock(&self.state);
                loop {
                    if state.closed {
                        return Err(Error::Closed);
                    }
                    if let Some(idle) = state.idle.pop_front() {
                        break Action::Reuse(idle);
                    }
                    if state.total < self.config.max_size {
                        state.total += 1;
                        break Action::Create;
                    }
                    // Saturated: wait for a check-in or a close, then re-evaluate.
                    match deadline {
                        None => {
                            state = self
                                .available
                                .wait(state)
                                .unwrap_or_else(PoisonError::into_inner);
                        }
                        Some(dl) => {
                            let now = Instant::now();
                            if now >= dl {
                                return Err(Error::Timeout);
                            }
                            let (guard, _) = self
                                .available
                                .wait_timeout(state, dl - now)
                                .unwrap_or_else(PoisonError::into_inner);
                            state = guard;
                        }
                    }
                }
            };

            match action {
                Action::Reuse(idle) => {
                    if let Some(prepared) = self.prepare(idle) {
                        return Ok(prepared);
                    }
                    // The idle resource was stale or invalid and has been dropped;
                    // release its slot and try again from the top.
                    self.release_slot();
                }
                Action::Create => match self.manager.create() {
                    Ok(resource) => return Ok((resource, Instant::now())),
                    Err(source) => {
                        self.release_slot();
                        return Err(Error::Backend(source));
                    }
                },
            }
        }
    }

    /// Apply lifetime, idle-timeout, and validation checks to an idle resource.
    ///
    /// Returns the resource and its original creation time on success; returns
    /// `None` (dropping the resource) when it is too old, has sat idle too long,
    /// or fails validation.
    fn prepare(&self, mut idle: Idle<M::Resource>) -> Option<(M::Resource, Instant)> {
        if self.is_time_expired(&idle, Instant::now()) {
            return None;
        }
        if !self.manager.validate(&mut idle.resource) {
            return None;
        }
        Some((idle.resource, idle.created_at))
    }

    /// Whether an idle resource has outlived `max_lifetime` or `idle_timeout`.
    ///
    /// This is the time-based half of expiry, shared by checkout
    /// ([`prepare`](Self::prepare)) and the background [`reap`](Self::reap); it
    /// deliberately does not call [`Manager::validate`], which is a checkout-time
    /// concern.
    fn is_time_expired(&self, idle: &Idle<M::Resource>, now: Instant) -> bool {
        if let Some(max_lifetime) = self.config.max_lifetime {
            if now.saturating_duration_since(idle.created_at) >= max_lifetime {
                return true;
            }
        }
        if let Some(idle_timeout) = self.config.idle_timeout {
            if now.saturating_duration_since(idle.last_used) >= idle_timeout {
                return true;
            }
        }
        false
    }

    /// Prune idle resources that have outlived their `idle_timeout` or
    /// `max_lifetime`. Called on each tick of the background reaper.
    ///
    /// Expired resources are removed under the lock but dropped outside it, so a
    /// slow destructor does not stall checkouts. The reaper never creates
    /// resources; pruning shrinks the idle set, and on-demand growth refills it.
    fn reap(&self) {
        if self.config.idle_timeout.is_none() && self.config.max_lifetime.is_none() {
            return;
        }
        let now = Instant::now();
        let mut expired = Vec::new();
        {
            let mut state = lock(&self.state);
            if state.closed {
                return;
            }
            let mut kept = VecDeque::with_capacity(state.idle.len());
            while let Some(idle) = state.idle.pop_front() {
                if self.is_time_expired(&idle, now) {
                    expired.push(idle.resource);
                } else {
                    kept.push_back(idle);
                }
            }
            state.total = state.total.saturating_sub(expired.len());
            state.idle = kept;
        }
        let freed = expired.len();
        drop(expired); // destructors run here, outside the lock
        if freed > 0 {
            // Freed slots may unblock threads waiting to create a resource.
            self.available.notify_all();
        }
    }

    /// Give back a reserved slot and wake one waiter so it can claim it.
    fn release_slot(&self) {
        let mut state = lock(&self.state);
        state.total = state.total.saturating_sub(1);
        drop(state);
        self.available.notify_one();
    }

    /// Return a borrowed resource to the pool. Called from [`Pooled`]'s `Drop`.
    pub(crate) fn checkin(&self, mut resource: M::Resource, created_at: Instant) {
        let recycled = self.manager.recycle(&mut resource);
        let mut state = lock(&self.state);
        if state.closed || recycled.is_err() {
            state.total = state.total.saturating_sub(1);
            drop(state);
            self.available.notify_one();
            // `resource` is dropped here, outside the lock.
        } else {
            let last_used = Instant::now();
            state.idle.push_back(Idle {
                resource,
                created_at,
                last_used,
            });
            drop(state);
            self.available.notify_one();
        }
    }
}

/// The body of the background reaper thread.
///
/// Sleeps for `interval` between sweeps, waking early when the shutdown signal
/// fires. It holds only a [`Weak`] reference to the pool, so it never keeps the
/// pool alive; once every handle is dropped (`upgrade` returns `None`) or the
/// stop flag is set, it returns and the thread ends.
fn reaper_loop<M: Manager>(pool: Weak<PoolInner<M>>, shutdown: Arc<Shutdown>, interval: Duration) {
    loop {
        {
            let stop = lock(&shutdown.stop);
            if *stop {
                return;
            }
            let (stop, _timed_out) = shutdown
                .wake
                .wait_timeout(stop, interval)
                .unwrap_or_else(PoisonError::into_inner);
            if *stop {
                return;
            }
        }
        match pool.upgrade() {
            Some(inner) => inner.reap(),
            None => return,
        }
    }
}

/// A thread-safe pool of reusable resources.
///
/// A `Pool<M>` lends out resources built by a [`Manager`], reclaiming and
/// recycling each one when its [`Pooled`] guard is dropped. It is cheap to clone
/// — every clone is a handle onto the same shared pool — so share it across
/// threads by cloning rather than wrapping it in another `Arc`.
///
/// The pool is runtime-agnostic and carries no async dependency.
/// [`get`](Pool::get) blocks the calling thread until a resource is available; in
/// an async context, acquire on a blocking-friendly executor thread (for example
/// `tokio::task::spawn_blocking`). The returned guard is `Send`, so it can be
/// held across `.await` points.
///
/// # Examples
///
/// ```
/// use pool_mod::{Manager, Pool};
/// use std::convert::Infallible;
///
/// struct Connections;
/// impl Manager for Connections {
///     type Resource = String;
///     type Error = Infallible;
///     fn create(&self) -> Result<String, Infallible> { Ok(String::new()) }
///     fn recycle(&self, c: &mut String) -> Result<(), Infallible> { c.clear(); Ok(()) }
/// }
///
/// let pool = Pool::builder(Connections).max_size(4).build()
///     .expect("configuration is valid");
///
/// let mut conn = pool.get().expect("a connection is available");
/// conn.push_str("SELECT 1");
/// assert_eq!(pool.status().in_use, 1);
/// drop(conn);
/// assert_eq!(pool.status().in_use, 0);
/// ```
pub struct Pool<M: Manager>(Arc<PoolInner<M>>);

impl<M: Manager> Clone for Pool<M> {
    fn clone(&self) -> Self {
        Pool(Arc::clone(&self.0))
    }
}

impl<M: Manager> Pool<M> {
    /// Start building a pool for `manager` with the default configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use pool_mod::{Manager, Pool};
    /// use std::convert::Infallible;
    /// # struct M;
    /// # impl Manager for M {
    /// #   type Resource = u32; type Error = Infallible;
    /// #   fn create(&self) -> Result<u32, Infallible> { Ok(0) }
    /// #   fn recycle(&self, _r: &mut u32) -> Result<(), Infallible> { Ok(()) }
    /// # }
    /// let pool = Pool::builder(M).max_size(8).min_idle(2).build()
    ///     .expect("configuration is valid");
    /// assert_eq!(pool.status().max_size, 8);
    /// ```
    pub fn builder(manager: M) -> Builder<M> {
        Builder::new(manager)
    }

    /// Build a pool for `manager` with the [default configuration](PoolConfig::default).
    ///
    /// A shortcut for `Pool::builder(manager).build()`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Backend`] if pre-creating the initial resources fails.
    /// (With the default `min_idle` of 0, no resources are created up front, so
    /// the default-configured pool only fails to build if you have customized the
    /// configuration through [`Pool::builder`] instead.)
    ///
    /// # Examples
    ///
    /// ```
    /// use pool_mod::{Manager, Pool};
    /// use std::convert::Infallible;
    /// # struct M;
    /// # impl Manager for M {
    /// #   type Resource = u32; type Error = Infallible;
    /// #   fn create(&self) -> Result<u32, Infallible> { Ok(0) }
    /// #   fn recycle(&self, _r: &mut u32) -> Result<(), Infallible> { Ok(()) }
    /// # }
    /// let pool = Pool::new(M).expect("configuration is valid");
    /// assert_eq!(pool.status().max_size, 10); // the default
    /// ```
    pub fn new(manager: M) -> Result<Self, Error<M::Error>> {
        Builder::new(manager).build()
    }

    /// Borrow a resource, waiting up to the configured
    /// [`create_timeout`](PoolConfig::create_timeout) if the pool is saturated.
    ///
    /// Reuses an idle resource when one is available (after validation), grows the
    /// pool toward `max_size` when it is not, and otherwise blocks until a
    /// resource is returned or the timeout elapses.
    ///
    /// # Errors
    ///
    /// - [`Error::Backend`] if the manager fails to create a resource.
    /// - [`Error::Timeout`] if the pool stays saturated past `create_timeout`.
    /// - [`Error::Closed`] if the pool has been closed.
    ///
    /// # Examples
    ///
    /// ```
    /// use pool_mod::{Manager, Pool};
    /// use std::convert::Infallible;
    /// # struct M;
    /// # impl Manager for M {
    /// #   type Resource = u32; type Error = Infallible;
    /// #   fn create(&self) -> Result<u32, Infallible> { Ok(7) }
    /// #   fn recycle(&self, _r: &mut u32) -> Result<(), Infallible> { Ok(()) }
    /// # }
    /// let pool = Pool::builder(M).max_size(2).build().expect("valid");
    /// let resource = pool.get().expect("available");
    /// assert_eq!(*resource, 7);
    /// ```
    pub fn get(&self) -> Result<Pooled<M>, Error<M::Error>> {
        let deadline = self
            .0
            .config
            .create_timeout
            .map(|timeout| Instant::now() + timeout);
        self.acquire(deadline)
    }

    /// Borrow a resource, waiting at most `timeout` regardless of the configured
    /// [`create_timeout`](PoolConfig::create_timeout).
    ///
    /// A `timeout` of [`Duration::ZERO`] makes this a non-blocking try: it returns
    /// [`Error::Timeout`] at once if no resource can be handed out immediately.
    ///
    /// # Errors
    ///
    /// - [`Error::Backend`] if the manager fails to create a resource.
    /// - [`Error::Timeout`] if no resource becomes available within `timeout`.
    /// - [`Error::Closed`] if the pool has been closed.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use pool_mod::{Error, Manager, Pool};
    /// use std::convert::Infallible;
    /// # struct M;
    /// # impl Manager for M {
    /// #   type Resource = u32; type Error = Infallible;
    /// #   fn create(&self) -> Result<u32, Infallible> { Ok(0) }
    /// #   fn recycle(&self, _r: &mut u32) -> Result<(), Infallible> { Ok(()) }
    /// # }
    /// let pool = Pool::builder(M).max_size(1).build().expect("valid");
    /// let held = pool.get().expect("first checkout");
    /// // The single slot is taken, so an immediate retry times out.
    /// assert!(matches!(pool.get_timeout(Duration::ZERO), Err(Error::Timeout)));
    /// ```
    pub fn get_timeout(&self, timeout: Duration) -> Result<Pooled<M>, Error<M::Error>> {
        self.acquire(Some(Instant::now() + timeout))
    }

    /// Borrow a resource without ever blocking.
    ///
    /// Returns a resource if one can be handed out immediately — an idle resource
    /// is ready, or the pool has room to create one — and otherwise returns
    /// [`Error::Timeout`] at once. Equivalent to
    /// [`get_timeout(Duration::ZERO)`](Pool::get_timeout).
    ///
    /// # Errors
    ///
    /// - [`Error::Backend`] if the manager fails to create a resource.
    /// - [`Error::Timeout`] if no resource is immediately available.
    /// - [`Error::Closed`] if the pool has been closed.
    ///
    /// # Examples
    ///
    /// ```
    /// use pool_mod::{Error, Manager, Pool};
    /// use std::convert::Infallible;
    /// # struct M;
    /// # impl Manager for M {
    /// #   type Resource = u32; type Error = Infallible;
    /// #   fn create(&self) -> Result<u32, Infallible> { Ok(0) }
    /// #   fn recycle(&self, _r: &mut u32) -> Result<(), Infallible> { Ok(()) }
    /// # }
    /// let pool = Pool::builder(M).max_size(1).build().expect("valid");
    /// let first = pool.try_get().expect("room to create one");
    /// // The only slot is taken, so the next try fails immediately.
    /// assert!(matches!(pool.try_get(), Err(Error::Timeout)));
    /// ```
    pub fn try_get(&self) -> Result<Pooled<M>, Error<M::Error>> {
        self.acquire(Some(Instant::now()))
    }

    fn acquire(&self, deadline: Option<Instant>) -> Result<Pooled<M>, Error<M::Error>> {
        let (resource, created_at) = self.0.acquire(deadline)?;
        Ok(Pooled::new(Arc::clone(&self.0), resource, created_at))
    }

    /// Take a snapshot of the pool's current occupancy.
    ///
    /// # Examples
    ///
    /// ```
    /// use pool_mod::{Manager, Pool};
    /// use std::convert::Infallible;
    /// # struct M;
    /// # impl Manager for M {
    /// #   type Resource = (); type Error = Infallible;
    /// #   fn create(&self) -> Result<(), Infallible> { Ok(()) }
    /// #   fn recycle(&self, _r: &mut ()) -> Result<(), Infallible> { Ok(()) }
    /// # }
    /// let pool = Pool::builder(M).max_size(4).min_idle(1).build().expect("valid");
    /// let status = pool.status();
    /// assert_eq!(status.idle, 1);
    /// assert_eq!(status.max_size, 4);
    /// ```
    pub fn status(&self) -> Status {
        let state = lock(&self.0.state);
        let idle = state.idle.len();
        let size = state.total;
        Status {
            size,
            idle,
            in_use: size.saturating_sub(idle),
            max_size: self.0.config.max_size,
        }
    }

    /// Close the pool: discard every idle resource and reject all future
    /// checkouts with [`Error::Closed`].
    ///
    /// Resources currently checked out are unaffected and are simply dropped
    /// (not returned to the idle set) when their guards fall. Closing is
    /// idempotent. Idle resources are dropped outside the pool's lock, so a slow
    /// resource destructor does not block other threads.
    ///
    /// # Examples
    ///
    /// ```
    /// use pool_mod::{Error, Manager, Pool};
    /// use std::convert::Infallible;
    /// # struct M;
    /// # impl Manager for M {
    /// #   type Resource = (); type Error = Infallible;
    /// #   fn create(&self) -> Result<(), Infallible> { Ok(()) }
    /// #   fn recycle(&self, _r: &mut ()) -> Result<(), Infallible> { Ok(()) }
    /// # }
    /// let pool = Pool::builder(M).max_size(2).min_idle(2).build().expect("valid");
    /// pool.close();
    /// assert!(pool.is_closed());
    /// assert!(matches!(pool.get(), Err(Error::Closed)));
    /// ```
    pub fn close(&self) {
        let mut state = lock(&self.0.state);
        let drained = std::mem::take(&mut state.idle);
        state.total = state.total.saturating_sub(drained.len());
        state.closed = true;
        drop(state);
        self.0.available.notify_all();
        self.0.shutdown.signal(); // stop the reaper; a closed pool needs no upkeep
        drop(drained); // resource destructors run here, outside the lock
    }

    /// Report whether the pool has been [closed](Pool::close).
    #[must_use]
    pub fn is_closed(&self) -> bool {
        lock(&self.0.state).closed
    }
}

/// A fluent builder for a [`Pool`].
///
/// Created by [`Pool::builder`]. Each setter consumes and returns the builder, so
/// calls chain; [`build`](Builder::build) validates the configuration and
/// pre-creates the `min_idle` resources.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use pool_mod::{Manager, Pool};
/// use std::convert::Infallible;
/// # struct M;
/// # impl Manager for M {
/// #   type Resource = u32; type Error = Infallible;
/// #   fn create(&self) -> Result<u32, Infallible> { Ok(0) }
/// #   fn recycle(&self, _r: &mut u32) -> Result<(), Infallible> { Ok(()) }
/// # }
/// let pool = Pool::builder(M)
///     .max_size(32)
///     .min_idle(4)
///     .idle_timeout(Some(Duration::from_secs(600)))
///     .max_lifetime(Some(Duration::from_secs(3600)))
///     .build()
///     .expect("configuration is valid");
/// assert_eq!(pool.status().idle, 4);
/// ```
#[must_use = "a Builder does nothing until `.build()` is called"]
pub struct Builder<M: Manager> {
    manager: M,
    config: PoolConfig,
}

impl<M: Manager> Builder<M> {
    /// Create a builder for `manager` seeded with the default configuration.
    pub fn new(manager: M) -> Self {
        Builder {
            manager,
            config: PoolConfig::default(),
        }
    }

    /// Set the maximum number of resources the pool may own at once.
    pub fn max_size(mut self, max_size: usize) -> Self {
        self.config.max_size = max_size;
        self
    }

    /// Set how many resources to create up front and keep ready.
    pub fn min_idle(mut self, min_idle: usize) -> Self {
        self.config.min_idle = min_idle;
        self
    }

    /// Set how long [`Pool::get`] waits when the pool is saturated. `None` waits
    /// indefinitely.
    pub fn create_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.config.create_timeout = timeout;
        self
    }

    /// Set the idle-expiry window. `None` disables idle expiry.
    pub fn idle_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.config.idle_timeout = timeout;
        self
    }

    /// Set the maximum resource lifetime. `None` disables lifetime expiry.
    pub fn max_lifetime(mut self, lifetime: Option<Duration>) -> Self {
        self.config.max_lifetime = lifetime;
        self
    }

    /// Set how often a background thread prunes expired idle resources. `None`
    /// (the default) runs no background thread, applying expiry lazily on borrow.
    ///
    /// Has no effect unless `idle_timeout` or `max_lifetime` is also set.
    pub fn reap_interval(mut self, interval: Option<Duration>) -> Self {
        self.config.reap_interval = interval;
        self
    }

    /// Replace the entire configuration with `config`.
    ///
    /// Useful when the configuration is loaded from a file rather than assembled
    /// setter by setter.
    pub fn config(mut self, config: PoolConfig) -> Self {
        self.config = config;
        self
    }

    /// Validate the configuration, build the pool, and pre-create `min_idle`
    /// resources.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidConfig`] if `max_size` is zero or `min_idle` exceeds
    ///   `max_size`.
    /// - [`Error::Backend`] if creating one of the `min_idle` resources fails;
    ///   any already-created resources are dropped before returning.
    ///
    /// # Examples
    ///
    /// ```
    /// use pool_mod::{Error, Manager, Pool};
    /// use std::convert::Infallible;
    /// # struct M;
    /// # impl Manager for M {
    /// #   type Resource = (); type Error = Infallible;
    /// #   fn create(&self) -> Result<(), Infallible> { Ok(()) }
    /// #   fn recycle(&self, _r: &mut ()) -> Result<(), Infallible> { Ok(()) }
    /// # }
    /// let invalid = Pool::builder(M).max_size(0).build();
    /// assert!(matches!(invalid, Err(Error::InvalidConfig(_))));
    /// ```
    pub fn build(self) -> Result<Pool<M>, Error<M::Error>> {
        if self.config.max_size == 0 {
            return Err(Error::InvalidConfig("max_size must be at least 1"));
        }
        if self.config.min_idle > self.config.max_size {
            return Err(Error::InvalidConfig("min_idle must not exceed max_size"));
        }

        let pool = Pool(Arc::new(PoolInner {
            manager: self.manager,
            config: self.config,
            state: Mutex::new(State {
                idle: VecDeque::with_capacity(self.config.max_size),
                total: 0,
                closed: false,
            }),
            available: Condvar::new(),
            shutdown: Arc::new(Shutdown::new()),
        }));

        for _ in 0..pool.0.config.min_idle {
            match pool.0.manager.create() {
                Ok(resource) => {
                    let now = Instant::now();
                    let mut state = lock(&pool.0.state);
                    state.idle.push_back(Idle {
                        resource,
                        created_at: now,
                        last_used: now,
                    });
                    state.total += 1;
                }
                Err(source) => return Err(Error::Backend(source)),
            }
        }

        pool.spawn_reaper();

        Ok(pool)
    }
}

impl<M: Manager> Pool<M> {
    /// Start the background reaper if `reap_interval` is configured.
    ///
    /// The reaper holds only a [`Weak`] handle, so it never keeps the pool alive;
    /// it stops when every handle is dropped or the pool is closed. If the OS
    /// refuses a new thread, the pool keeps working with expiry applied lazily on
    /// checkout — the reaper is an optimization, not a correctness requirement.
    fn spawn_reaper(&self) {
        let Some(interval) = self.0.config.reap_interval else {
            return;
        };
        let pool = Arc::downgrade(&self.0);
        let shutdown = Arc::clone(&self.0.shutdown);
        match thread::Builder::new()
            .name("pool-mod-reaper".to_owned())
            .spawn(move || reaper_loop(pool, shutdown, interval))
        {
            // Detach the handle; the reaper self-terminates via the shutdown
            // signal and its `Weak` reference.
            Ok(handle) => drop(handle),
            // Spawn failed: fall back to lazy, checkout-time expiry.
            Err(_) => self.0.shutdown.signal(),
        }
    }
}

#[cfg(test)]
// Justification: in test code a failed setup or checkout has no meaningful
// recovery — unwinding with a clear panic is the correct outcome. REPS permits
// `unwrap`/`expect` in test modules for exactly this reason.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Debug, PartialEq, Eq)]
    struct TestError(&'static str);

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }

    impl std::error::Error for TestError {}

    /// A manager whose behaviour is steerable through atomics so individual
    /// lifecycle paths can be exercised deterministically.
    struct Steerable {
        created: AtomicUsize,
        recycled: AtomicUsize,
        validated: AtomicUsize,
        create_fails: AtomicBool,
        recycle_fails: AtomicBool,
        valid: AtomicBool,
    }

    impl Steerable {
        fn new() -> Self {
            Steerable {
                created: AtomicUsize::new(0),
                recycled: AtomicUsize::new(0),
                validated: AtomicUsize::new(0),
                create_fails: AtomicBool::new(false),
                recycle_fails: AtomicBool::new(false),
                valid: AtomicBool::new(true),
            }
        }
    }

    impl Manager for Steerable {
        type Resource = usize;
        type Error = TestError;

        fn create(&self) -> Result<usize, TestError> {
            if self.create_fails.load(Ordering::SeqCst) {
                return Err(TestError("create failed"));
            }
            Ok(self.created.fetch_add(1, Ordering::SeqCst))
        }

        fn recycle(&self, _resource: &mut usize) -> Result<(), TestError> {
            let _ = self.recycled.fetch_add(1, Ordering::SeqCst);
            if self.recycle_fails.load(Ordering::SeqCst) {
                return Err(TestError("recycle failed"));
            }
            Ok(())
        }

        fn validate(&self, _resource: &mut usize) -> bool {
            let _ = self.validated.fetch_add(1, Ordering::SeqCst);
            self.valid.load(Ordering::SeqCst)
        }
    }

    fn pool(builder: impl FnOnce(Builder<Steerable>) -> Builder<Steerable>) -> Pool<Steerable> {
        builder(Pool::builder(Steerable::new())).build().unwrap()
    }

    #[test]
    fn test_build_min_idle_precreates_resources() {
        let p = pool(|b| b.max_size(4).min_idle(2));
        assert_eq!(p.0.manager.created.load(Ordering::SeqCst), 2);
        let status = p.status();
        assert_eq!(status.idle, 2);
        assert_eq!(status.size, 2);
        assert_eq!(status.in_use, 0);
    }

    #[test]
    fn test_get_then_drop_reuses_same_resource() {
        let p = pool(|b| b.max_size(4));
        {
            let first = p.get().unwrap();
            assert_eq!(*first, 0);
        }
        let second = p.get().unwrap();
        assert_eq!(*second, 0); // same resource id, reused
        assert_eq!(p.0.manager.created.load(Ordering::SeqCst), 1);
        assert_eq!(p.0.manager.recycled.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_in_use_tracks_outstanding_guards() {
        let p = pool(|b| b.max_size(2));
        let a = p.get().unwrap();
        let b = p.get().unwrap();
        assert_eq!(p.status().in_use, 2);
        assert_eq!(p.status().idle, 0);
        drop(a);
        drop(b);
        assert_eq!(p.status().in_use, 0);
        assert_eq!(p.status().idle, 2);
    }

    #[test]
    fn test_saturated_pool_times_out() {
        let p = pool(|b| b.max_size(1));
        let _held = p.get().unwrap();
        let result = p.get_timeout(Duration::ZERO);
        assert!(matches!(result, Err(Error::Timeout)));
    }

    #[test]
    fn test_invalid_resource_is_discarded_and_replaced() {
        let p = pool(|b| b.max_size(4).min_idle(1));
        assert_eq!(p.0.manager.created.load(Ordering::SeqCst), 1);
        p.0.manager.valid.store(false, Ordering::SeqCst);

        // The single idle resource fails validation, so it is dropped and a fresh
        // one is created. (The fresh resource is not itself re-validated.)
        let resource = p.get().unwrap();
        assert_eq!(*resource, 1);
        assert_eq!(p.0.manager.created.load(Ordering::SeqCst), 2);
        assert!(p.0.manager.validated.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn test_max_lifetime_forces_replacement() {
        let p = pool(|b| b.max_size(4).min_idle(1).max_lifetime(Some(Duration::ZERO)));
        // Zero lifetime means any idle resource is always too old on checkout.
        let resource = p.get().unwrap();
        assert_eq!(*resource, 1);
        assert_eq!(p.0.manager.created.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_idle_timeout_forces_replacement() {
        let p = pool(|b| b.max_size(4).min_idle(1).idle_timeout(Some(Duration::ZERO)));
        let resource = p.get().unwrap();
        assert_eq!(*resource, 1);
        assert_eq!(p.0.manager.created.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_recycle_failure_drops_resource() {
        let p = pool(|b| b.max_size(2));
        p.0.manager.recycle_fails.store(true, Ordering::SeqCst);
        {
            let _resource = p.get().unwrap();
            assert_eq!(p.status().size, 1);
        }
        // Recycle failed on return, so the resource was discarded, not pooled.
        assert_eq!(p.status().size, 0);
        assert_eq!(p.status().idle, 0);
    }

    #[test]
    fn test_create_failure_surfaces_and_frees_slot() {
        let p = pool(|b| b.max_size(2));
        p.0.manager.create_fails.store(true, Ordering::SeqCst);
        let result = p.get();
        assert!(matches!(
            result,
            Err(Error::Backend(TestError("create failed")))
        ));
        // The reserved slot was released, so the pool did not shrink.
        assert_eq!(p.status().size, 0);
    }

    #[test]
    fn test_closed_pool_rejects_checkout() {
        let p = pool(|b| b.max_size(2).min_idle(1));
        p.close();
        assert!(p.is_closed());
        assert!(matches!(p.get(), Err(Error::Closed)));
        assert_eq!(p.status().idle, 0); // idle resources were dropped on close
    }

    #[test]
    fn test_close_is_idempotent() {
        let p = pool(|b| b.max_size(2).min_idle(2));
        p.close();
        p.close();
        assert!(p.is_closed());
    }

    #[test]
    fn test_build_rejects_zero_max_size() {
        let result = Pool::builder(Steerable::new()).max_size(0).build();
        assert!(matches!(result, Err(Error::InvalidConfig(_))));
    }

    #[test]
    fn test_build_rejects_min_idle_above_max_size() {
        let result = Pool::builder(Steerable::new())
            .max_size(2)
            .min_idle(3)
            .build();
        assert!(matches!(result, Err(Error::InvalidConfig(_))));
    }

    #[test]
    fn test_try_get_does_not_block_when_saturated() {
        let p = pool(|b| b.max_size(1));
        let _held = p.try_get().unwrap();
        assert!(matches!(p.try_get(), Err(Error::Timeout)));
    }

    #[test]
    fn test_reap_prunes_time_expired_idle() {
        // A zero idle_timeout makes every idle resource immediately expired, so
        // `reap` is deterministic without a background thread or sleeps.
        let p = pool(|b| b.max_size(4).min_idle(2).idle_timeout(Some(Duration::ZERO)));
        assert_eq!(p.status().idle, 2);
        p.0.reap();
        assert_eq!(p.status().idle, 0);
        assert_eq!(p.status().size, 0);
    }

    #[test]
    fn test_reap_is_noop_without_expiry_policy() {
        let p = pool(|b| b.max_size(4).min_idle(2));
        p.0.reap();
        assert_eq!(p.status().idle, 2); // no idle_timeout/max_lifetime: nothing to prune
    }

    #[test]
    fn test_clone_shares_one_pool() {
        let p = pool(|b| b.max_size(1));
        let clone = p.clone();
        let _held = p.get().unwrap();
        // The clone sees the same exhausted pool.
        assert!(matches!(
            clone.get_timeout(Duration::ZERO),
            Err(Error::Timeout)
        ));
    }
}
