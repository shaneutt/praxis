// SPDX-License-Identifier: LGPL-3.0-only
// Copyright (c) 2024 Shane Utt

//! Resolved upstream endpoint: address, TLS settings, and connection options.

use std::sync::Arc;

use praxis_tls::ClusterTls;

use super::ConnectionOptions;

// -----------------------------------------------------------------------------
// Upstream
// -----------------------------------------------------------------------------

/// An upstream endpoint to proxy requests to.
///
/// ```
/// use std::sync::Arc;
///
/// use praxis_core::connectivity::{ConnectionOptions, Upstream};
///
/// let upstream = Upstream {
///     address: Arc::from("127.0.0.1:8080"),
///     tls: None,
///     connection: Arc::new(ConnectionOptions::default()),
/// };
///
/// assert_eq!(&*upstream.address, "127.0.0.1:8080");
/// assert!(upstream.tls.is_none());
/// ```
#[derive(Debug, Clone)]
pub struct Upstream {
    /// Address in `host:port` form.
    pub address: Arc<str>,

    /// Connection tuning for this upstream.
    pub connection: Arc<ConnectionOptions>,

    /// TLS settings. `None` means plain TCP.
    pub tls: Option<ClusterTls>,
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_accessible() {
        let u = make_upstream("10.0.0.1:8080", None);
        assert_eq!(&*u.address, "10.0.0.1:8080", "address should be preserved");
        assert!(u.tls.is_none(), "tls should be None");
    }

    #[test]
    fn tls_with_sni() {
        let tls = ClusterTls {
            sni: Some("api.example.com".to_owned()),
            ..ClusterTls::default()
        };
        let u = make_upstream("10.0.0.1:443", Some(tls));
        assert!(u.tls.is_some(), "tls should be present");
        assert_eq!(
            u.tls.as_ref().expect("tls should be present").sni.as_deref(),
            Some("api.example.com"),
            "sni should match configured value"
        );
    }

    #[test]
    fn tls_verify_defaults_to_true() {
        let u = make_upstream("10.0.0.1:443", Some(ClusterTls::default()));
        assert!(
            u.tls.as_ref().expect("tls should be present").verify,
            "verify should default to true"
        );
    }

    #[test]
    fn tls_verify_can_be_disabled() {
        let tls = ClusterTls {
            verify: false,
            ..ClusterTls::default()
        };
        let u = make_upstream("10.0.0.1:443", Some(tls));
        assert!(
            !u.tls.as_ref().expect("tls should be present").verify,
            "verify should be false when explicitly disabled"
        );
    }

    // -------------------------------------------------------------------------
    // Test Utilities
    // -------------------------------------------------------------------------

    /// Build an [`Upstream`] with the given address and optional TLS config.
    fn make_upstream(address: &str, tls: Option<ClusterTls>) -> Upstream {
        Upstream {
            address: Arc::from(address),
            tls,
            connection: Arc::new(ConnectionOptions::default()),
        }
    }
}
