//! The pool itself: [`Pool`] and its [`Builder`].

use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
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
    /// Idle resources. Check-in pushes to the back and checkout pops from the
    /// back (LIFO), so the most recently used resource is reused first and the
    /// front holds the least recently used ones, which are the first to reach
    /// `idle_timeout`.
    idle: VecDeque<Idle<R>>,
    /// Resources the pool currently owns: idle, checked out, or mid-creation.
    total: usize,
    /// Threads blocked in [`PoolInner::reserve`], oldest first. Each waiter
    /// sleeps on its own condition variable, so a waker can signal exactly the
    /// oldest one, and only the oldest waiter may take a resource from inside
    /// the queue. A thread that arrives while the oldest waiter has waited
    /// longer than [`STARVATION_LIMIT`] joins the back instead of taking a
    /// resource ahead of it.
    queue: VecDeque<Arc<Waiter>>,
    closed: bool,
}

/// How long the oldest waiter may wait before newly arriving threads stop
/// taking resources ahead of it.
///
/// Below this, a thread that finds a resource free takes it at once, even when
/// others are waiting: handing every returned resource to a sleeping thread
/// costs a context switch per checkout, which under saturation cut throughput
/// by more than ten times in measurement. Past it, newcomers queue behind the
/// waiters, so no waiter is passed over for longer than about this long plus
/// one resource hold time.
const STARVATION_LIMIT: Duration = Duration::from_millis(1);

/// A thread blocked in [`PoolInner::reserve`].
struct Waiter {
    wake: Condvar,
    since: Instant,
}

impl<R> State<R> {
    /// Whether a checkout could succeed right now: an idle resource is ready or
    /// there is room to create one.
    #[inline]
    fn has_capacity(&self, max_size: usize) -> bool {
        !self.idle.is_empty() || self.total < max_size
    }

    /// The oldest waiter, if any, cloned so it can be signalled after the lock
    /// is released.
    #[inline]
    fn head(&self) -> Option<Arc<Waiter>> {
        self.queue.front().cloned()
    }

    /// Whether a newly arriving thread may take a resource now: nobody is
    /// waiting, or the oldest waiter has not yet waited past
    /// [`STARVATION_LIMIT`].
    #[inline]
    fn newcomer_may_take(&self) -> bool {
        self.queue
            .front()
            .map_or(true, |head| head.since.elapsed() < STARVATION_LIMIT)
    }

    /// Whether `waiter` is the oldest waiter.
    #[inline]
    fn is_head(&self, waiter: &Arc<Waiter>) -> bool {
        self.queue
            .front()
            .is_some_and(|front| Arc::ptr_eq(front, waiter))
    }

    /// Remove `waiter` from the queue. If it was the oldest waiter and a
    /// checkout could now succeed, return the new oldest waiter so the caller
    /// can pass the turn on.
    fn leave(&mut self, waiter: &Arc<Waiter>, max_size: usize) -> Option<Arc<Waiter>> {
        let was_head = self.is_head(waiter);
        self.queue.retain(|queued| !Arc::ptr_eq(queued, waiter));
        if was_head && self.has_capacity(max_size) {
            self.head()
        } else {
            None
        }
    }
}

/// Signal a waiter taken from [`State::head`], if there was one.
#[inline]
fn notify(waiter: Option<Arc<Waiter>>) {
    if let Some(waiter) = waiter {
        waiter.wake.notify_one();
    }
}

/// The decision reached while holding the lock, carried out once it is released.
///
/// Only outcomes that require running user code (and therefore must not hold the
/// lock) escape the locked region; waiting and immediate errors are handled in
/// place.
enum Action<R> {
    Reuse(Idle<R>),
    Create,
}

/// Gives a reserved slot back to the pool when dropped, unless disarmed.
///
/// Every path that reserves a slot (`total += 1`) and then runs user code
/// (`create`, `validate`, `recycle`) holds one of these, so a hook that panics
/// cannot leave the slot counted with no resource behind it.
struct SlotGuard<'a, M: Manager> {
    pool: &'a PoolInner<M>,
    armed: bool,
}

impl<'a, M: Manager> SlotGuard<'a, M> {
    #[inline]
    fn new(pool: &'a PoolInner<M>) -> Self {
        SlotGuard { pool, armed: true }
    }

    /// Keep the slot: it now belongs to a live resource.
    #[inline]
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl<M: Manager> Drop for SlotGuard<'_, M> {
    fn drop(&mut self) {
        if self.armed {
            self.pool.release_slot();
        }
    }
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
    ///
    /// With `allow_create` false the pool never runs `create`: it hands out an
    /// idle resource or returns [`Error::Timeout`].
    fn acquire(
        &self,
        deadline: Option<Instant>,
        allow_create: bool,
    ) -> Result<(M::Resource, Instant), Error<M::Error>> {
        loop {
            match self.reserve(deadline, allow_create)? {
                Action::Reuse(idle) => {
                    // The idle resource still holds its slot. If validation
                    // rejects it or panics, the guard gives the slot back.
                    let slot = SlotGuard::new(self);
                    if let Some(prepared) = self.prepare(idle) {
                        slot.disarm();
                        return Ok(prepared);
                    }
                    // The idle resource was stale or invalid and has been
                    // dropped; `slot` releases its slot here. Try again.
                }
                Action::Create => {
                    let slot = SlotGuard::new(self);
                    match self.manager.create_with_deadline(deadline) {
                        Ok(resource) => {
                            slot.disarm();
                            return Ok((resource, Instant::now()));
                        }
                        Err(source) => {
                            drop(slot);
                            return Err(Error::Backend(source));
                        }
                    }
                }
            }
        }
    }

    /// The locked half of [`acquire`](Self::acquire): pop an idle resource or
    /// reserve a slot, waiting in arrival order while the pool is saturated.
    fn reserve(
        &self,
        deadline: Option<Instant>,
        allow_create: bool,
    ) -> Result<Action<M::Resource>, Error<M::Error>> {
        let max_size = self.config.max_size;
        let mut state = lock(&self.state);
        // This thread's place in the wait queue, once it has had to wait.
        let mut me: Option<Arc<Waiter>> = None;
        loop {
            if state.closed {
                if let Some(waiter) = &me {
                    let _ = state.leave(waiter, max_size);
                }
                return Err(Error::Closed);
            }
            let my_turn = match &me {
                None => state.newcomer_may_take(),
                Some(waiter) => state.is_head(waiter),
            };
            if my_turn {
                let action = if let Some(idle) = state.idle.pop_back() {
                    Some(Action::Reuse(idle))
                } else if allow_create && state.total < max_size {
                    state.total += 1;
                    Some(Action::Create)
                } else {
                    None
                };
                if let Some(action) = action {
                    if me.is_some() {
                        let _ = state.queue.pop_front();
                    }
                    // Wake the oldest waiter if it could also be served.
                    let next = if state.has_capacity(max_size) {
                        state.head()
                    } else {
                        None
                    };
                    drop(state);
                    notify(next);
                    return Ok(action);
                }
            }

            // Saturated, or a starved waiter is ahead of us: wait for our turn.
            let wait_for = match deadline {
                None => None,
                Some(dl) => {
                    let now = Instant::now();
                    if now >= dl {
                        let next = match &me {
                            Some(waiter) => state.leave(waiter, max_size),
                            None => None,
                        };
                        drop(state);
                        notify(next);
                        return Err(Error::Timeout);
                    }
                    Some(dl - now)
                }
            };
            let waiter = me.get_or_insert_with(|| {
                let waiter = Arc::new(Waiter {
                    wake: Condvar::new(),
                    since: Instant::now(),
                });
                state.queue.push_back(Arc::clone(&waiter));
                waiter
            });
            state = match wait_for {
                None => waiter
                    .wake
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
                Some(timeout) => {
                    waiter
                        .wake
                        .wait_timeout(state, timeout)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
    }

    /// Apply lifetime, idle-timeout, and validation checks to an idle resource.
    ///
    /// Returns the resource and its original creation time on success; returns
    /// `None` (dropping the resource) when it is too old, has sat idle too long,
    /// or fails validation.
    fn prepare(&self, mut idle: Idle<M::Resource>) -> Option<(M::Resource, Instant)> {
        if self.has_expiry() && self.is_time_expired(&idle, Instant::now()) {
            return None;
        }
        if !self.manager.validate(&mut idle.resource) {
            return None;
        }
        Some((idle.resource, idle.created_at))
    }

    /// Whether any time-based expiry policy is configured.
    #[inline]
    fn has_expiry(&self) -> bool {
        self.config.idle_timeout.is_some() || self.config.max_lifetime.is_some()
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

    /// One pass of the background reaper: prune idle resources that have
    /// outlived their `idle_timeout` or `max_lifetime`, then create resources
    /// until the idle set is back at `min_idle` (never past `max_size`).
    ///
    /// Expired resources are removed under the lock but dropped outside it, and
    /// new ones are created outside it, so neither a slow destructor nor a slow
    /// `create` stalls checkouts.
    fn reap(&self) {
        let now = Instant::now();
        let mut expired = Vec::new();
        let refill;
        let next;
        {
            let mut state = lock(&self.state);
            if state.closed {
                return;
            }
            if self.has_expiry() {
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
            // Reserve the slots for the refill now, in the same critical
            // section, so the pool never looks emptier than it is about to be.
            let below_floor = self.config.min_idle.saturating_sub(state.idle.len());
            let room = self.config.max_size.saturating_sub(state.total);
            refill = below_floor.min(room);
            state.total += refill;
            next = if expired.is_empty() {
                None
            } else {
                state.head()
            };
        }
        drop(expired); // destructors run here, outside the lock
        notify(next); // freed slots may unblock a thread waiting to create
        for created in 0..refill {
            let slot = SlotGuard::new(self);
            match self.manager.create_with_deadline(None) {
                Ok(resource) => {
                    let now = Instant::now();
                    let mut state = lock(&self.state);
                    if state.closed {
                        drop(state);
                        drop(resource);
                        // `slot` releases the slot.
                        self.release_slots(refill - created - 1);
                        return;
                    }
                    state.idle.push_back(Idle {
                        resource,
                        created_at: now,
                        last_used: now,
                    });
                    slot.disarm();
                    let next = state.head();
                    drop(state);
                    notify(next);
                }
                Err(_) => {
                    // Give back this slot and the ones not yet used; on-demand
                    // creation will surface the error to a caller.
                    drop(slot);
                    self.release_slots(refill - created - 1);
                    return;
                }
            }
        }
    }

    /// Give back a reserved slot and wake the oldest waiter, if any, so it can
    /// claim it. Also used by [`Pooled::detach`] and [`Pooled::discard`].
    pub(crate) fn release_slot(&self) {
        self.release_slots(1);
    }

    /// Give back `count` reserved slots and wake the oldest waiter, if any.
    fn release_slots(&self, count: usize) {
        if count == 0 {
            return;
        }
        let mut state = lock(&self.state);
        state.total = state.total.saturating_sub(count);
        let next = state.head();
        drop(state);
        notify(next);
    }

    /// Return a borrowed resource to the pool. Called from [`Pooled`]'s `Drop`.
    pub(crate) fn checkin(&self, resource: M::Resource, created_at: Instant) {
        // Declared before `resource` so the resource is dropped first and the
        // slot released after it, on every path including a panic in `recycle`.
        let slot = SlotGuard::new(self);
        let mut resource = resource;
        if thread::panicking() {
            // The borrower is unwinding, so the resource may be half way through
            // an operation. Never pool it, and do not call `recycle`: a second
            // panic during unwinding would abort the process.
            drop(resource);
            return;
        }
        if self.manager.recycle(&mut resource).is_err() {
            drop(resource);
            return;
        }
        let mut state = lock(&self.state);
        if state.closed {
            drop(state);
            drop(resource); // outside the lock; `slot` then releases the slot
            return;
        }
        state.idle.push_back(Idle {
            resource,
            created_at,
            last_used: Instant::now(),
        });
        slot.disarm(); // the idle entry now holds the slot
        let next = state.head();
        drop(state);
        notify(next);
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
            // A panicking `create` during the refill must not end the reaper.
            // The slot guards have already restored the pool's counts, so the
            // pool is consistent and the next pass can carry on.
            Some(inner) => {
                let _ = panic::catch_unwind(AssertUnwindSafe(|| inner.reap()));
            }
            None => return,
        }
    }
}

/// The instant `timeout` from now, or `None` (no deadline) when that instant
/// cannot be represented, as with [`Duration::MAX`].
#[inline]
fn deadline_after(timeout: Duration) -> Option<Instant> {
    Instant::now().checked_add(timeout)
}

/// A thread-safe pool of reusable resources.
///
/// A `Pool<M>` lends out resources built by a [`Manager`], reclaiming and
/// recycling each one when its [`Pooled`] guard is dropped. It is cheap to clone
/// — every clone is a handle onto the same shared pool — so share it across
/// threads by cloning rather than wrapping it in another `Arc`.
///
/// Threads that have to wait for a resource are served in arrival order, and a
/// returned resource is offered to the thread that has waited longest. A thread
/// that arrives and finds a resource free may still take it first, which keeps
/// throughput high under contention, but only while the oldest waiter has
/// waited less than about a millisecond; after that, newcomers queue behind the
/// waiters, so no waiter is passed over indefinitely.
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
    /// resource is returned or the timeout elapses. Threads that have to wait are
    /// served in arrival order (see [`Pool`] for the fairness rule).
    ///
    /// The timeout bounds the wait for a free resource or slot. It does not bound
    /// [`Manager::validate`] or [`Manager::create`] once one of them has started;
    /// the deadline is passed to [`Manager::create_with_deadline`] so a manager
    /// can bound its own connect time. A timeout too large to represent as an
    /// [`Instant`] (such as [`Duration::MAX`]) waits indefinitely.
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
        let deadline = self.0.config.create_timeout.and_then(deadline_after);
        self.acquire(deadline, true)
    }

    /// Borrow a resource, waiting at most `timeout` for a free resource or slot,
    /// regardless of the configured
    /// [`create_timeout`](PoolConfig::create_timeout).
    ///
    /// A `timeout` of [`Duration::ZERO`] never waits for another thread: it returns
    /// [`Error::Timeout`] at once if no resource can be handed out immediately. As
    /// with [`get`](Pool::get), the timeout does not bound a `validate` or
    /// `create` call that has already started, and a timeout too large to
    /// represent waits indefinitely.
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
        self.acquire(deadline_after(timeout), true)
    }

    /// Borrow a resource without waiting for another thread to return one.
    ///
    /// Returns a resource if one can be handed out immediately (an idle resource
    /// is ready, or the pool has room to create one) and otherwise returns
    /// [`Error::Timeout`] at once. Equivalent to
    /// [`get_timeout(Duration::ZERO)`](Pool::get_timeout).
    ///
    /// When the pool has room, this runs [`Manager::create`] on the calling
    /// thread, which takes as long as the manager takes. To never run `create`,
    /// use [`try_get_idle`](Pool::try_get_idle).
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
        self.acquire(Some(Instant::now()), true)
    }

    /// Borrow an idle resource without waiting and without creating one.
    ///
    /// Returns an idle resource (after the usual expiry and
    /// [`validate`](Manager::validate) checks) if one is ready and no other thread
    /// is already waiting, and otherwise returns [`Error::Timeout`] at once. It
    /// never calls [`Manager::create`], so it never waits on a backend connect;
    /// the only user code it runs is `validate`.
    ///
    /// # Errors
    ///
    /// - [`Error::Timeout`] if no idle resource is ready.
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
    /// let pool = Pool::builder(M).max_size(2).build().expect("valid");
    /// // Nothing has been created yet, and this call does not create anything.
    /// assert!(matches!(pool.try_get_idle(), Err(Error::Timeout)));
    ///
    /// drop(pool.get().expect("room to create one"));
    /// assert!(pool.try_get_idle().is_ok()); // the returned resource is idle now
    /// ```
    pub fn try_get_idle(&self) -> Result<Pooled<M>, Error<M::Error>> {
        self.acquire(Some(Instant::now()), false)
    }

    fn acquire(
        &self,
        deadline: Option<Instant>,
        allow_create: bool,
    ) -> Result<Pooled<M>, Error<M::Error>> {
        let (resource, created_at) = self.0.acquire(deadline, allow_create)?;
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
        let waiters: Vec<Arc<Waiter>> = state.queue.iter().cloned().collect();
        drop(state);
        // Wake every blocked acquirer so it observes the close.
        for waiter in waiters {
            waiter.wake.notify_one();
        }
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

    /// Set how many idle resources to create up front. When a
    /// [`reap_interval`](Builder::reap_interval) is set, the reaper also refills
    /// the idle set to this floor on each pass.
    pub fn min_idle(mut self, min_idle: usize) -> Self {
        self.config.min_idle = min_idle;
        self
    }

    /// Set how long [`Pool::get`] waits for a free resource or slot when the pool
    /// is saturated. `None` waits indefinitely. This bounds waiting only, not
    /// [`Manager::create`]; see [`Manager::create_with_deadline`].
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

    /// Set how often a background thread prunes expired idle resources and
    /// refills the idle set to `min_idle`. `None` (the default) runs no
    /// background thread: expiry is applied lazily on borrow and the idle set is
    /// refilled only on demand.
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
                queue: VecDeque::new(),
                closed: false,
            }),
            shutdown: Arc::new(Shutdown::new()),
        }));

        for _ in 0..pool.0.config.min_idle {
            match pool.0.manager.create_with_deadline(None) {
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
        let p = pool(|b| b.max_size(4).idle_timeout(Some(Duration::ZERO)));
        let a = p.get().unwrap();
        let b = p.get().unwrap();
        drop((a, b));
        assert_eq!(p.status().idle, 2);
        p.0.reap();
        assert_eq!(p.status().idle, 0);
        assert_eq!(p.status().size, 0);
    }

    #[test]
    fn test_reap_refills_to_min_idle() {
        // Every idle resource is expired at once, so the pass prunes both and
        // then creates two fresh ones to restore the floor.
        let p = pool(|b| b.max_size(4).min_idle(2).idle_timeout(Some(Duration::ZERO)));
        assert_eq!(p.0.manager.created.load(Ordering::SeqCst), 2);
        p.0.reap();
        assert_eq!(p.status().idle, 2);
        assert_eq!(p.status().size, 2);
        assert_eq!(p.0.manager.created.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn test_reap_refill_respects_max_size() {
        let p = pool(|b| b.max_size(2).min_idle(2));
        let a = p.get().unwrap();
        let b = p.get().unwrap();
        p.0.reap(); // both slots are checked out, so there is no room to refill
        assert_eq!(p.status().size, 2);
        assert_eq!(p.0.manager.created.load(Ordering::SeqCst), 2);
        drop((a, b));
    }

    #[test]
    fn test_reap_refill_stops_on_create_error() {
        let p = pool(|b| b.max_size(4).min_idle(2).idle_timeout(Some(Duration::ZERO)));
        p.0.manager.create_fails.store(true, Ordering::SeqCst);
        p.0.reap();
        let status = p.status();
        assert_eq!(status.idle, 0);
        assert_eq!(status.size, 0); // the reserved slots were all given back
    }

    /// Spin until `n` threads are queued in `reserve`.
    fn wait_for_queue(p: &Pool<Steerable>, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while lock(&p.0.state).queue.len() < n {
            assert!(Instant::now() < deadline, "waiters never queued");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn test_waiters_are_served_in_arrival_order() {
        let p = pool(|b| b.max_size(1).create_timeout(None));
        let held = p.get().unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut handles = Vec::new();
        for id in 0..3 {
            let (p2, order2) = (p.clone(), Arc::clone(&order));
            handles.push(thread::spawn(move || {
                let guard = p2.get().unwrap();
                lock(&order2).push(id);
                thread::sleep(Duration::from_millis(5));
                drop(guard);
            }));
            wait_for_queue(&p, id + 1);
        }
        drop(held);
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(*lock(&order), vec![0, 1, 2]);
    }

    #[test]
    fn test_newcomer_does_not_take_a_resource_ahead_of_a_starved_waiter() {
        let p = pool(|b| b.max_size(1).create_timeout(None));
        let held = p.get().unwrap();
        let p2 = p.clone();
        let waiter = thread::spawn(move || drop(p2.get().unwrap()));
        wait_for_queue(&p, 1);
        thread::sleep(STARVATION_LIMIT * 5);
        // The resource goes back while a thread has waited past the starvation
        // limit. A newcomer that asks right away must not take it first.
        drop(held);
        assert!(matches!(p.try_get(), Err(Error::Timeout)));
        waiter.join().unwrap();
        assert!(p.try_get().is_ok());
    }

    #[test]
    fn test_timed_out_head_passes_the_turn_on() {
        let p = pool(|b| b.max_size(1).create_timeout(None));
        let held = p.get().unwrap();
        let p2 = p.clone();
        let short = thread::spawn(move || p2.get_timeout(Duration::from_millis(50)).is_err());
        wait_for_queue(&p, 1);
        let p3 = p.clone();
        let long = thread::spawn(move || drop(p3.get().unwrap()));
        wait_for_queue(&p, 2);
        assert!(short.join().unwrap(), "the first waiter times out");
        drop(held);
        long.join().unwrap(); // the second waiter is still served
        assert_eq!(lock(&p.0.state).queue.len(), 0);
    }

    #[test]
    fn test_close_wakes_every_waiter() {
        let p = pool(|b| b.max_size(1).create_timeout(None));
        let _held = p.get().unwrap();
        let handles: Vec<_> = (0..3)
            .map(|_| {
                let p2 = p.clone();
                thread::spawn(move || matches!(p2.get(), Err(Error::Closed)))
            })
            .collect();
        wait_for_queue(&p, 3);
        p.close();
        for handle in handles {
            assert!(handle.join().unwrap());
        }
        assert_eq!(lock(&p.0.state).queue.len(), 0);
    }

    #[test]
    fn test_checkout_is_lifo() {
        let p = pool(|b| b.max_size(4).min_idle(4));
        // The most recently returned resource is handed out first.
        let first = *p.get().unwrap();
        let again = *p.get().unwrap();
        assert_eq!(first, again);
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
