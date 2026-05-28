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

    /// Number of resources to create up front when the pool is built, and the
    /// floor the pool keeps ready. Must not exceed `max_size`.
    pub min_idle: usize,

    /// How long [`Pool::get`](crate::Pool::get) waits for a resource when the
    /// pool is saturated. `None` waits indefinitely.
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
    /// be rejected on their next checkout. `None` (the default) runs no
    /// background thread; expiry is then applied lazily, on borrow.
    ///
    /// The reaper only prunes — it never creates resources — and has no effect
    /// unless `idle_timeout` or `max_lifetime` is also set.
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
