//! The pool error type.

use std::fmt;

/// An error returned by a [`Pool`](crate::Pool) operation.
///
/// The type parameter `E` is the manager's own error type
/// ([`Manager::Error`](crate::Manager::Error)). A failure that originates inside
/// the manager — a connection that could not be opened, a transaction that could
/// not be rolled back — is carried unchanged in [`Error::Backend`], so the caller
/// can inspect or match on the underlying cause rather than a stringified copy of
/// it.
///
/// The enum is `#[non_exhaustive]`: future versions may add variants, so a match
/// on it must include a wildcard arm.
///
/// # Examples
///
/// Distinguish a saturated pool from a backend failure:
///
/// ```
/// use pool_mod::Error;
/// use std::convert::Infallible;
///
/// fn describe(err: &Error<Infallible>) -> &'static str {
///     match err {
///         Error::Timeout => "pool is busy, try again",
///         Error::Closed => "pool is shut down",
///         _ => "other",
///     }
/// }
///
/// assert_eq!(describe(&Error::Timeout), "pool is busy, try again");
/// ```
#[non_exhaustive]
#[derive(Debug)]
pub enum Error<E> {
    /// The manager failed to create or recycle a resource. Carries the
    /// manager's own error.
    ///
    /// Inspect the inner error to decide whether the failure is transient (worth
    /// retrying) or permanent.
    Backend(E),

    /// The configured wait elapsed before a resource became available.
    ///
    /// The pool is healthy but saturated. Retry later, raise `max_size`, or hold
    /// borrowed resources for less time.
    Timeout,

    /// The pool has been shut down with [`Pool::close`](crate::Pool::close).
    ///
    /// No further resources will be handed out; this is terminal for the pool.
    Closed,

    /// The pool could not be built because its configuration is invalid.
    ///
    /// The message names the constraint that was violated (for example, a
    /// `max_size` of zero).
    InvalidConfig(&'static str),
}

impl<E: fmt::Display> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Backend(source) => write!(f, "resource manager error: {source}"),
            Error::Timeout => f.write_str("timed out waiting for a resource from the pool"),
            Error::Closed => f.write_str("the pool has been closed"),
            Error::InvalidConfig(reason) => write!(f, "invalid pool configuration: {reason}"),
        }
    }
}

impl<E> std::error::Error for Error<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Backend(source) => Some(source),
            Error::Timeout | Error::Closed | Error::InvalidConfig(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Backend;

    impl fmt::Display for Backend {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("connection refused")
        }
    }

    impl std::error::Error for Backend {}

    #[test]
    fn test_display_backend_includes_source_message() {
        let err = Error::Backend(Backend);
        assert_eq!(
            err.to_string(),
            "resource manager error: connection refused"
        );
    }

    #[test]
    fn test_display_timeout_is_actionable() {
        let err: Error<Backend> = Error::Timeout;
        assert_eq!(
            err.to_string(),
            "timed out waiting for a resource from the pool"
        );
    }

    #[test]
    fn test_display_invalid_config_names_constraint() {
        let err: Error<Backend> = Error::InvalidConfig("max_size must be at least 1");
        assert_eq!(
            err.to_string(),
            "invalid pool configuration: max_size must be at least 1"
        );
    }

    #[test]
    fn test_source_present_only_for_backend() {
        use std::error::Error as _;

        let backend = Error::Backend(Backend);
        assert!(backend.source().is_some());

        let timeout: Error<Backend> = Error::Timeout;
        assert!(timeout.source().is_none());
    }
}
