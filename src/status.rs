//! A point-in-time snapshot of pool occupancy.

/// A snapshot of how many resources a [`Pool`](crate::Pool) holds.
///
/// Returned by [`Pool::status`](crate::Pool::status). The counts are read under
/// the pool's lock, but in a concurrent program they are stale the instant they
/// are returned — treat them as a gauge for logging and metrics, not as a value
/// to branch on for correctness.
///
/// The invariant `size == idle + in_use` holds for each individual snapshot.
///
/// # Examples
///
/// ```
/// use pool_mod::{Manager, Pool};
/// use std::convert::Infallible;
///
/// struct Nothing;
/// impl Manager for Nothing {
///     type Resource = ();
///     type Error = Infallible;
///     fn create(&self) -> Result<(), Infallible> { Ok(()) }
///     fn recycle(&self, _r: &mut ()) -> Result<(), Infallible> { Ok(()) }
/// }
///
/// let pool = Pool::builder(Nothing).max_size(4).min_idle(2).build()
///     .expect("configuration is valid");
///
/// let status = pool.status();
/// assert_eq!(status.size, 2);
/// assert_eq!(status.idle, 2);
/// assert_eq!(status.in_use, 0);
/// assert_eq!(status.max_size, 4);
/// assert_eq!(status.size, status.idle + status.in_use);
/// ```
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    /// Total resources the pool owns right now: idle, plus checked out, plus any
    /// currently being created.
    pub size: usize,

    /// Resources sitting idle and available for immediate checkout.
    pub idle: usize,

    /// Resources currently lent out to callers (counted as `size - idle`, which
    /// folds in-flight creations into the in-use figure).
    pub in_use: usize,

    /// The configured `max_size` the pool will not grow beyond.
    pub max_size: usize,
}
