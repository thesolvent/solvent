//! Telemetry shim. Instrument via these macros (`use crate::obs::{info, warn, error, debug};`),
//! never `tracing::` directly, so telemetry is optional and the host owns the subscriber. Redaction
//! is mandatory on key paths: wrap secrets in [`Redacted`] and annotate key-touching fns with
//! `#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(<allow-list>)))]`.

use core::fmt;

#[cfg(feature = "tracing")]
pub use tracing::{debug, error, info, warn};

/// No-op stand-in for the tracing macros when the `tracing` feature is off.
#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! __obs_noop {
    ($($t:tt)*) => {{}};
}
#[cfg(not(feature = "tracing"))]
pub use crate::__obs_noop as debug;
#[cfg(not(feature = "tracing"))]
pub use crate::__obs_noop as error;
#[cfg(not(feature = "tracing"))]
pub use crate::__obs_noop as info;
#[cfg(not(feature = "tracing"))]
pub use crate::__obs_noop as warn;

/// Wraps a secret so it can live in a struct or log site without leaking: `Debug`/`Display` print
/// `<redacted>`, never the value. Read the secret only through the explicit, greppable [`expose`].
///
/// [`expose`]: Redacted::expose
#[derive(Clone)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    pub fn new(secret: T) -> Self {
        Self(secret)
    }

    /// Explicit access to the wrapped secret. The only way in — easy to grep, hard to do by accident.
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacted_never_leaks_the_secret() {
        let s = Redacted::new("super-secret-key");
        assert_eq!(format!("{s:?}"), "<redacted>");
        assert_eq!(format!("{s}"), "<redacted>");
        assert!(!format!("{s:?} {s}").contains("super-secret-key"));
        assert_eq!(*s.expose(), "super-secret-key");
    }
}
