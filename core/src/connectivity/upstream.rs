//! Resolved upstream endpoint: address, TLS settings, and connection options.

use std::sync::Arc;

use super::ConnectionOptions;

// -----------------------------------------------------------------------------
// Upstream
// -----------------------------------------------------------------------------

/// An upstream endpoint to proxy requests to.
///
/// ```
/// use std::sync::Arc;
/// use praxis_core::connectivity::{ConnectionOptions, Upstream};
///
/// let upstream = Upstream {
///     address: Arc::from("127.0.0.1:8080"),
///     tls: false,
///     sni: None,
///     connection: ConnectionOptions::default(),
/// };
///
/// assert_eq!(&*upstream.address, "127.0.0.1:8080");
/// assert!(!upstream.tls);
/// assert!(upstream.sni.is_none());
/// ```
#[derive(Debug, Clone)]

pub struct Upstream {
    /// Address in `host:port` form.
    pub address: Arc<str>,

    /// Connection tuning for this upstream.
    pub connection: ConnectionOptions,

    /// SNI hostname for TLS. `None` means no explicit SNI.
    pub sni: Option<String>,

    /// Whether to use TLS to this upstream.
    pub tls: bool,
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_upstream(address: &str, tls: bool, sni: Option<&str>) -> Upstream {
        Upstream {
            address: Arc::from(address),
            tls,
            sni: sni.map(String::from),
            connection: ConnectionOptions::default(),
        }
    }

    #[test]
    fn fields_are_accessible() {
        let u = make_upstream("10.0.0.1:8080", false, None);
        assert_eq!(&*u.address, "10.0.0.1:8080", "address should be preserved");
        assert!(!u.tls, "tls should be false");
        assert!(u.sni.is_none(), "sni should be None");
    }

    #[test]
    fn tls_with_sni() {
        let u = make_upstream("10.0.0.1:443", true, Some("api.example.com"));
        assert!(u.tls, "tls should be true");
        assert_eq!(
            u.sni.as_deref(),
            Some("api.example.com"),
            "sni should match configured value"
        );
    }
}
