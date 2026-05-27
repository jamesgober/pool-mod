//! The [`Manager`] trait: how a pool creates, checks, and recycles its resources.

/// Creates and maintains the resources held by a [`Pool`](crate::Pool).
///
/// Implement this for whatever you want to pool — a database connection, an HTTP
/// client, a worker handle, or any other resource that is expensive to build. The
/// pool drives the resource lifecycle through three hooks:
///
/// - [`create`](Manager::create) — build a new resource when the pool needs to
///   grow toward `max_size`.
/// - [`validate`](Manager::validate) — confirm an idle resource is still usable
///   before lending it out (validation-on-borrow).
/// - [`recycle`](Manager::recycle) — reset a returned resource so it can be lent
///   out again.
///
/// One manager is shared across every thread that uses the pool, so it must be
/// `Send + Sync`. The pool never holds its internal lock while calling these
/// methods, so a slow `create` or `validate` does not block other threads from
/// returning resources.
///
/// # Examples
///
/// A manager that pools reusable byte buffers:
///
/// ```
/// use pool_mod::Manager;
/// use std::convert::Infallible;
///
/// struct Buffers {
///     capacity: usize,
/// }
///
/// impl Manager for Buffers {
///     type Resource = Vec<u8>;
///     type Error = Infallible;
///
///     fn create(&self) -> Result<Vec<u8>, Infallible> {
///         Ok(Vec::with_capacity(self.capacity))
///     }
///
///     fn recycle(&self, buf: &mut Vec<u8>) -> Result<(), Infallible> {
///         buf.clear(); // keep the allocation, drop the contents
///         Ok(())
///     }
/// }
/// ```
pub trait Manager: Send + Sync + 'static {
    /// The resource this manager produces and the pool hands out.
    type Resource: Send + 'static;

    /// The error returned when creating or recycling a resource fails.
    ///
    /// Use a domain error type for real backends. When a resource can never fail
    /// to build, [`std::convert::Infallible`] is the honest choice.
    type Error: Send + Sync + 'static;

    /// Build a new resource.
    ///
    /// Called when the pool needs to grow toward `max_size`. Runs without the
    /// pool's internal lock held, so a blocking constructor (opening a socket,
    /// say) does not stall other threads.
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if the resource cannot be created. The pool
    /// surfaces it as [`Error::Backend`](crate::Error::Backend) and releases the
    /// slot it had reserved, so a failed creation does not permanently shrink the
    /// pool.
    fn create(&self) -> Result<Self::Resource, Self::Error>;

    /// Reset a returned resource so it is fit to lend out again.
    ///
    /// Called when a borrowed resource is dropped back into the pool — roll back
    /// an open transaction, clear a buffer, reset a cursor.
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if the resource cannot be made reusable. The pool
    /// then discards it instead of returning it to the idle set, freeing the slot
    /// for a fresh resource.
    fn recycle(&self, resource: &mut Self::Resource) -> Result<(), Self::Error>;

    /// Check that an idle resource is still usable before lending it out.
    ///
    /// Called on checkout for any resource taken from the idle set. Return
    /// `false` to have the pool discard the resource and move on to the next idle
    /// one — or to a freshly created resource if the idle set is exhausted.
    ///
    /// The default accepts every resource. Override it for validation-on-borrow,
    /// such as pinging a connection to confirm the peer has not closed it.
    ///
    /// # Examples
    ///
    /// ```
    /// use pool_mod::Manager;
    /// use std::convert::Infallible;
    ///
    /// struct Sessions;
    ///
    /// impl Manager for Sessions {
    ///     type Resource = (bool, u32); // (alive, id)
    ///     type Error = Infallible;
    ///
    ///     fn create(&self) -> Result<(bool, u32), Infallible> {
    ///         Ok((true, 0))
    ///     }
    ///
    ///     fn recycle(&self, _s: &mut (bool, u32)) -> Result<(), Infallible> {
    ///         Ok(())
    ///     }
    ///
    ///     fn validate(&self, session: &mut (bool, u32)) -> bool {
    ///         session.0 // only lend out sessions still marked alive
    ///     }
    /// }
    /// ```
    fn validate(&self, resource: &mut Self::Resource) -> bool {
        let _ = resource;
        true
    }
}
