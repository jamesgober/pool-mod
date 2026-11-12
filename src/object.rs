//! The [`Pooled`] guard returned by [`Pool::get`](crate::Pool::get).

use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::Instant;

use crate::manager::Manager;
use crate::pool::PoolInner;

/// A borrowed resource that returns itself to the pool when dropped.
///
/// Obtained from [`Pool::get`](crate::Pool::get) and
/// [`Pool::get_timeout`](crate::Pool::get_timeout). A `Pooled<M>` deref-coerces
/// to the underlying resource, so it can be used anywhere a `&M::Resource` or
/// `&mut M::Resource` is expected. When the guard goes out of scope the resource
/// is recycled (via [`Manager::recycle`](crate::Manager::recycle)) and returned
/// to the idle set. There is no `release` call to remember: dropping the guard
/// always gives the resource back. (Passing the guard to [`std::mem::forget`]
/// skips the destructor and keeps its slot taken for the life of the pool.)
///
/// If recycling fails, recycling panics, the guard is dropped while its thread
/// is panicking, or the pool has been closed, the resource is dropped instead of
/// pooled and its slot is freed for a replacement. To take a resource out of the
/// pool on purpose, for example after an I/O error shows it is broken, use
/// [`Pooled::discard`] or [`Pooled::detach`].
///
/// The guard is `Send` whenever the resource is, so it may be held across
/// `.await` points in async code.
///
/// # Examples
///
/// ```
/// use pool_mod::{Manager, Pool};
/// use std::convert::Infallible;
///
/// struct Counters;
/// impl Manager for Counters {
///     type Resource = u64;
///     type Error = Infallible;
///     fn create(&self) -> Result<u64, Infallible> { Ok(0) }
///     fn recycle(&self, _r: &mut u64) -> Result<(), Infallible> { Ok(()) }
/// }
///
/// let pool = Pool::builder(Counters).max_size(2).build()
///     .expect("configuration is valid");
/// {
///     let mut n = pool.get().expect("a resource is available");
///     *n += 1; // DerefMut to the pooled u64
///     assert_eq!(*n, 1);
/// } // `n` is recycled and returned to the pool here
/// assert_eq!(pool.status().idle, 1);
/// ```
#[must_use = "dropping the guard immediately returns the resource to the pool"]
pub struct Pooled<M: Manager> {
    inner: Arc<PoolInner<M>>,
    resource: ManuallyDrop<M::Resource>,
    created_at: Instant,
    /// Set once `resource` has been moved out by `detach` or `discard`.
    taken: bool,
}

impl<M: Manager> Pooled<M> {
    pub(crate) fn new(
        inner: Arc<PoolInner<M>>,
        resource: M::Resource,
        created_at: Instant,
    ) -> Self {
        Pooled {
            inner,
            resource: ManuallyDrop::new(resource),
            created_at,
            taken: false,
        }
    }

    /// Take the resource out of the pool for good.
    ///
    /// The pool forgets the resource and frees its slot, so it can create a
    /// replacement; `recycle` is not called. Use it to hand a connection to code
    /// that will own it from now on.
    ///
    /// This is an associated function (`Pooled::detach(guard)`) rather than a
    /// method, so it cannot shadow a method of the resource reached through
    /// `Deref`.
    ///
    /// # Examples
    ///
    /// ```
    /// use pool_mod::{Manager, Pool, Pooled};
    /// use std::convert::Infallible;
    /// # struct M;
    /// # impl Manager for M {
    /// #   type Resource = String; type Error = Infallible;
    /// #   fn create(&self) -> Result<String, Infallible> { Ok("conn".to_owned()) }
    /// #   fn recycle(&self, _r: &mut String) -> Result<(), Infallible> { Ok(()) }
    /// # }
    /// let pool = Pool::builder(M).max_size(1).build().expect("valid");
    /// let owned: String = Pooled::detach(pool.get().expect("available"));
    /// assert_eq!(owned, "conn");
    /// assert_eq!(pool.status().size, 0); // the slot is free again
    /// ```
    #[must_use = "detach hands the resource to the caller; use `Pooled::discard` to drop it"]
    pub fn detach(mut this: Self) -> M::Resource {
        // SAFETY: `taken` is false here (it is set only below, and `this` is
        // owned), so `resource` has not been moved out. After the take, `taken`
        // is set, and `Drop` checks it and never touches `resource` again, so
        // the value is moved out exactly once and never dropped twice.
        let resource = unsafe { ManuallyDrop::take(&mut this.resource) };
        this.taken = true;
        resource // `this` drops here and frees the slot
    }

    /// Drop the resource instead of returning it to the pool.
    ///
    /// The resource is dropped first and its slot freed afterwards, so the pool
    /// never owns more than `max_size` live resources. `recycle` is not called.
    /// Use it when the caller knows the resource is broken, for example after an
    /// I/O error, so that it is not handed out again.
    ///
    /// # Examples
    ///
    /// ```
    /// use pool_mod::{Manager, Pool, Pooled};
    /// use std::convert::Infallible;
    /// # struct M;
    /// # impl Manager for M {
    /// #   type Resource = u32; type Error = Infallible;
    /// #   fn create(&self) -> Result<u32, Infallible> { Ok(0) }
    /// #   fn recycle(&self, _r: &mut u32) -> Result<(), Infallible> { Ok(()) }
    /// # }
    /// let pool = Pool::builder(M).max_size(1).build().expect("valid");
    /// let conn = pool.get().expect("available");
    /// Pooled::discard(conn);
    /// assert_eq!(pool.status().size, 0);
    /// assert_eq!(pool.status().idle, 0);
    /// ```
    pub fn discard(mut this: Self) {
        // SAFETY: as in `detach`: `taken` is false until set here, and `Drop`
        // never touches `resource` once it is set.
        let resource = unsafe { ManuallyDrop::take(&mut this.resource) };
        this.taken = true;
        drop(resource); // the resource first ...
        drop(this); // ... then the slot
    }
}

impl<M: Manager> Deref for Pooled<M> {
    type Target = M::Resource;

    fn deref(&self) -> &M::Resource {
        &self.resource
    }
}

impl<M: Manager> DerefMut for Pooled<M> {
    fn deref_mut(&mut self) -> &mut M::Resource {
        &mut self.resource
    }
}

impl<M: Manager> Drop for Pooled<M> {
    fn drop(&mut self) {
        if self.taken {
            // `detach` or `discard` already moved the resource out.
            self.inner.release_slot();
            return;
        }
        // SAFETY: `taken` is false, so `resource` has not been moved out by
        // `detach`. It is taken exactly once, here, as the guard is destroyed,
        // and is never accessed again afterwards. `ManuallyDrop` suppressed its
        // automatic destructor, so this is the sole owning move and no double
        // drop can occur.
        let resource = unsafe { ManuallyDrop::take(&mut self.resource) };
        self.inner.checkin(resource, self.created_at);
    }
}
