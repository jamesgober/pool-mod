//! Pool configuration.

use std::time::Duration;

/// Tunable limits and lifecycle policy for a [`Pool`](crate::Pool).
///
/// Construct one through [`Pool::builder`](crate::Pool::builder) for the fluent
/// path, or assemble it directly — every field is public, so a configuration can
/// equally be deserialized from a settings file. Validation happens when the pool
/// is built, not when the struct is created; see
/// [`Builder::build`](crate::Builder::build).
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use pool_mod::PoolConfig;
///
/// // Start from the defaults and adjust only what matters.
/// let cfg = PoolConfig {
///     max_size: 16,
///     min_idle: 2,
///     idle_timeout: Some(Duration::from_secs(300)),
///     ..PoolConfig::default()
/// };
///
/// assert_eq!(cfg.max_size, 16);
/// assert_eq!(cfg.create_timeout, Some(Duration::from_secs(30))); // inherited default
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolConfig {
    /// Hard upper bound on the number of resources the pool owns at once — idle
    /// plus checked out. Must be at least 1.
    pub max_size: usize,

    /// Number of idle resources to create up front when the pool is built. When
    /// `reap_interval` is set, the reaper also refills the idle set to this
    /// floor on each pass; without it, the idle set is refilled only on demand.
    /// Must not exceed `max_size`.
    pub min_idle: usize,

    /// How long [`Pool::get`](crate::Pool::get) waits for a free resource or
    /// slot when the pool is saturated. `None` waits indefinitely, as does a
    /// value too large to add to the current time.
    ///
    /// Despite the name, this bounds waiting only: it does not interrupt a
    /// [`Manager::create`](crate::Manager::create) that has started. The
    /// deadline is passed to
    /// [`Manager::create_with_deadline`](crate::Manager::create_with_deadline)
    /// so a manager can bound its own connect time.
    pub create_timeout: Option<Duration>,

    /// Discard and replace an idle resource that has gone unused for at least
    /// this long, checked the next time it is borrowed. `None` disables idle
    /// expiry.
    pub idle_timeout: Option<Duration>,

    /// Discard and replace a resource older than this, regardless of use,
    /// checked the next time it is borrowed. `None` disables lifetime expiry.
    pub max_lifetime: Option<Duration>,

    /// How often a background thread should prune idle resources that have
    /// outlived `idle_timeout` or `max_lifetime`, rather than waiting for them to
    /// be rejected on their next checkout, and refill the idle set to
    /// `min_idle`. `None` (the default) runs no background thread; expiry is
    /// then applied lazily, on borrow, and the idle set is refilled only on
    /// demand.
    ///
    /// The refill runs [`Manager::create`](crate::Manager::create) on the
    /// reaper thread and never grows the pool past `max_size`. A failed or
    /// panicking `create` ends that pass; the next pass tries again.
    pub reap_interval: Option<Duration>,
}

impl Default for PoolConfig {
    /// A pool of up to ten resources, created on demand, that waits up to thirty
    /// seconds for a free resource, never expires idle or aged resources, and
    /// runs no background reaper.
    fn default() -> Self {
        PoolConfig {
            max_size: 10,
            min_idle: 0,
            create_timeout: Some(Duration::from_secs(30)),
            idle_timeout: None,
            max_lifetime: None,
            reap_interval: None,
        }
    }
}
