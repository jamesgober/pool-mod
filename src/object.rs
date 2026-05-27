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
/// to the idle set — there is no `release` call to remember, and no way to leak a
/// resource by forgetting one.
///
/// If recycling fails, or the pool has been closed, the resource is dropped
/// instead of pooled and its slot is freed for a replacement.
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
        }
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
        // SAFETY: `resource` is taken exactly once — here, as the guard is
        // destroyed — and is never accessed again afterwards. `ManuallyDrop`
        // suppressed its automatic destructor, so this is the sole owning move
        // and no double drop can occur.
        let resource = unsafe { ManuallyDrop::take(&mut self.resource) };
        self.inner.checkin(resource, self.created_at);
    }
}
