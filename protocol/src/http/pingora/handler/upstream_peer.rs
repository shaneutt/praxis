//! Upstream peer selection: converts the filter pipeline's [`Upstream`] into a Pingora `HttpPeer`.
//!
//! [`Upstream`]: praxis_core::connectivity::Upstream

use pingora_core::{Result, upstreams::peer::HttpPeer};
use praxis_core::connectivity::Upstream;

use super::super::{context::RequestCtx, convert::apply_connection_options};

// -----------------------------------------------------------------------------
// Execution/Conversion
// -----------------------------------------------------------------------------

/// Convert the pipeline's upstream selection into a Pingora `HttpPeer`.
///
/// On the first call, moves the upstream from `ctx.upstream` into
/// `ctx.upstream_for_retry` and borrows it. On retries, borrows the
/// saved copy directly. No clone is performed.
pub(super) fn execute(ctx: &mut RequestCtx) -> Result<Box<HttpPeer>> {
    if ctx.upstream_for_retry.is_none() {
        ctx.upstream_for_retry = ctx.upstream.take();
    }

    let upstream = ctx.upstream_for_retry.as_ref().ok_or_else(|| {
        pingora_core::Error::explain(
            pingora_core::ErrorType::InternalError,
            "no upstream selected by filter pipeline (is a load_balancer filter configured?)",
        )
    })?;

    build_peer(upstream)
}

/// Parse the upstream address and build an `HttpPeer` with TLS/SNI config.
fn build_peer(upstream: &Upstream) -> Result<Box<HttpPeer>> {
    let addr: std::net::SocketAddr = upstream.address.parse().map_err(|e| {
        pingora_core::Error::explain(
            pingora_core::ErrorType::InternalError,
            format!("invalid upstream address '{}': {e}", upstream.address),
        )
    })?;

    let mut peer = HttpPeer::new(addr, upstream.tls, upstream.sni.clone().unwrap_or_default());
    apply_connection_options(&mut peer, &upstream.connection);
    Ok(Box::new(peer))
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use praxis_core::connectivity::{ConnectionOptions, Upstream};

    use super::*;

    /// Create a test upstream with the given address.
    fn make_upstream(address: &str) -> Upstream {
        Upstream {
            address: Arc::from(address),
            tls: false,
            sni: None,
            connection: ConnectionOptions::default(),
        }
    }

    #[test]
    fn valid_address_builds_peer() {
        assert!(
            build_peer(&make_upstream("127.0.0.1:8080")).is_ok(),
            "valid address should build peer"
        );
    }

    #[test]
    fn build_peer_with_tls_enabled() {
        let upstream = Upstream {
            address: Arc::from("127.0.0.1:8443"),
            tls: true,
            sni: Some("api.example.com".to_owned()),
            connection: ConnectionOptions::default(),
        };
        let peer = build_peer(&upstream).expect("should build TLS peer");
        assert!(!peer.sni.is_empty(), "TLS peer should have a non-empty SNI");
        assert_eq!(peer.sni, "api.example.com", "peer SNI should match configured value");
    }

    #[test]
    fn build_peer_with_tls_and_empty_sni() {
        let upstream = Upstream {
            address: Arc::from("127.0.0.1:8443"),
            tls: true,
            sni: None,
            connection: ConnectionOptions::default(),
        };
        let peer = build_peer(&upstream).expect("should build TLS peer with empty SNI");
        assert_eq!(peer.sni, "", "peer SNI should be empty when upstream sni is None");
    }

    #[test]
    fn build_peer_without_tls() {
        let upstream = Upstream {
            address: Arc::from("127.0.0.1:8080"),
            tls: false,
            sni: None,
            connection: ConnectionOptions::default(),
        };
        let peer = build_peer(&upstream).expect("should build plain peer");
        assert_eq!(peer.sni, "", "plain peer should have empty SNI");
    }

    #[test]
    fn invalid_address_returns_error() {
        assert!(
            build_peer(&make_upstream("not-an-address")).is_err(),
            "invalid address should return error"
        );
    }

    #[test]
    fn missing_port_returns_error() {
        assert!(
            build_peer(&make_upstream("127.0.0.1")).is_err(),
            "address without port should return error"
        );
    }

    #[test]
    fn execute_first_call_moves_upstream_to_retry() {
        let mut ctx = RequestCtx::default();
        ctx.upstream = Some(make_upstream("127.0.0.1:8080"));
        let result = execute(&mut ctx);
        assert!(result.is_ok(), "first execute should succeed");
        assert!(ctx.upstream.is_none(), "upstream should be consumed");
        assert!(ctx.upstream_for_retry.is_some(), "should save for retry");
        assert_eq!(
            &*ctx.upstream_for_retry.as_ref().unwrap().address,
            "127.0.0.1:8080",
            "saved retry address should match original"
        );
    }

    #[test]
    fn execute_retry_reuses_saved_upstream() {
        let mut ctx = RequestCtx::default();
        ctx.upstream = None;
        ctx.upstream_for_retry = Some(make_upstream("127.0.0.1:9090"));
        let result = execute(&mut ctx);
        assert!(result.is_ok(), "retry execute should succeed");
        assert!(
            ctx.upstream_for_retry.is_some(),
            "retry upstream should remain for further retries"
        );
    }

    #[test]
    fn execute_no_upstream_no_retry_returns_error() {
        let mut ctx = RequestCtx::default();
        ctx.upstream = None;
        ctx.upstream_for_retry = None;
        let result = execute(&mut ctx);
        assert!(result.is_err(), "execute with no upstream should return error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no upstream selected"), "unexpected error message: {err}");
    }
}
