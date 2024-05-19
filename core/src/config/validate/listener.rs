//! Listener validation: presence, count, protocol constraints, and name uniqueness.

use std::collections::HashSet;

use crate::{
    config::{Listener, ProtocolKind},
    errors::ProxyError,
};

// -----------------------------------------------------------------------------
// Validation
// -----------------------------------------------------------------------------

/// Validate listener count, addresses, protocol constraints, and TLS paths.
pub(super) fn validate_listeners(listeners: &[Listener]) -> Result<(), ProxyError> {
    const MAX_LISTENERS: usize = 1_000;

    if listeners.is_empty() {
        return Err(ProxyError::Config("at least one listener required".into()));
    }
    if listeners.len() > MAX_LISTENERS {
        return Err(ProxyError::Config(format!(
            "too many listeners ({}, max {MAX_LISTENERS})",
            listeners.len()
        )));
    }

    for listener in listeners {
        validate_address(&listener.address, &listener.name)?;

        if listener.protocol == ProtocolKind::Tcp && listener.upstream.is_none() {
            return Err(ProxyError::Config(format!(
                "TCP listener '{}' requires an upstream address",
                listener.name
            )));
        }

        if listener.protocol == ProtocolKind::Tcp
            && let Some(ref upstream) = listener.upstream
        {
            validate_tcp_upstream(upstream, &listener.name)?;
        }

        if let Some(tls) = &listener.tls {
            tls.validate()
                .map_err(|e| ProxyError::Config(format!("listener '{}': {e}", listener.name)))?;
        }
    }

    Ok(())
}

/// Validate that a TCP upstream address is a valid socket address.
fn validate_tcp_upstream(addr: &str, listener_name: &str) -> Result<(), ProxyError> {
    use std::net::SocketAddr;

    addr.parse::<SocketAddr>().map_err(|_| {
        ProxyError::Config(format!(
            "TCP listener '{listener_name}': invalid upstream socket address '{addr}'"
        ))
    })?;

    Ok(())
}

// -----------------------------------------------------------------------------
// Address Validation
// -----------------------------------------------------------------------------

/// Verify the listener address parses as a valid `SocketAddr`.
fn validate_address(addr: &str, listener_name: &str) -> Result<(), ProxyError> {
    use std::net::SocketAddr;

    addr.parse::<SocketAddr>()
        .map_err(|_| ProxyError::Config(format!("listener '{listener_name}': invalid socket address '{addr}'")))?;
    Ok(())
}

// -----------------------------------------------------------------------------
// Listener Name Validation
// -----------------------------------------------------------------------------

/// Reject duplicate listener names.
pub(super) fn validate_listener_names(listeners: &[Listener]) -> Result<(), ProxyError> {
    let mut seen = HashSet::new();
    for listener in listeners {
        if !seen.insert(&listener.name) {
            return Err(ProxyError::Config(format!(
                "duplicate listener name '{}'",
                listener.name
            )));
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::validate_listeners;
    use crate::config::Config;

    #[test]
    fn reject_no_listeners() {
        let yaml = r#"
listeners: []
routes:
  - path_prefix: "/"
    cluster: "x"
clusters:
  - name: "x"
    endpoints: ["1.2.3.4:80"]
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("at least one listener"));
    }

    #[test]
    fn reject_invalid_listener_address() {
        let yaml = r#"
listeners:
  - name: web
    address: "not-a-socket-addr"
pipeline:
  - filter: router
    routes:
      - path_prefix: "/"
        cluster: "x"
  - filter: load_balancer
    clusters:
      - name: "x"
        endpoints: ["1.2.3.4:80"]
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("invalid socket address"), "got: {err}");
    }

    #[test]
    fn accept_valid_listener_address() {
        let yaml = r#"
listeners:
  - name: web
    address: "0.0.0.0:8080"
    filter_chains: [main]
filter_chains:
  - name: main
    filters:
      - filter: router
        routes:
          - path_prefix: "/"
            cluster: "x"
      - filter: load_balancer
        clusters:
          - name: "x"
            endpoints: ["1.2.3.4:80"]
"#;
        Config::from_yaml(yaml).unwrap();
    }

    #[test]
    fn tcp_listener_without_upstream_is_rejected() {
        let yaml = r#"
listeners:
  - name: db
    address: "0.0.0.0:5432"
    protocol: tcp
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("requires an upstream address"));
    }

    #[test]
    fn validate_listeners_rejects_empty() {
        let err = validate_listeners(&[]).unwrap_err();
        assert!(err.to_string().contains("at least one listener"));
    }

    #[test]
    fn reject_tls_cert_path_traversal() {
        let yaml = r#"
listeners:
  - name: web
    address: "0.0.0.0:443"
    tls:
      cert_path: "/etc/../../tmp/evil.pem"
      key_path: "/etc/ssl/key.pem"
pipeline:
  - filter: router
    routes:
      - path_prefix: "/"
        cluster: "x"
  - filter: load_balancer
    clusters:
      - name: "x"
        endpoints: ["1.2.3.4:80"]
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("path traversal"), "got: {err}");
    }

    #[test]
    fn reject_tls_key_path_traversal() {
        let yaml = r#"
listeners:
  - name: web
    address: "0.0.0.0:443"
    tls:
      cert_path: "/etc/ssl/cert.pem"
      key_path: "../secret/key.pem"
pipeline:
  - filter: router
    routes:
      - path_prefix: "/"
        cluster: "x"
  - filter: load_balancer
    clusters:
      - name: "x"
        endpoints: ["1.2.3.4:80"]
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("path traversal"), "got: {err}");
    }

    #[test]
    fn reject_duplicate_listener_names() {
        let yaml = r#"
listeners:
  - name: web
    address: "0.0.0.0:8080"
  - name: web
    address: "0.0.0.0:9090"
pipeline:
  - filter: router
    routes:
      - path_prefix: "/"
        cluster: "x"
  - filter: load_balancer
    clusters:
      - name: "x"
        endpoints: ["1.2.3.4:80"]
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("duplicate listener name"));
    }

    #[test]
    fn reject_invalid_tcp_upstream_address() {
        let yaml = r#"
listeners:
  - name: db
    address: "0.0.0.0:5432"
    protocol: tcp
    upstream: "not-a-socket-addr"
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        assert!(
            err.to_string().contains("invalid upstream socket address"),
            "got: {err}"
        );
    }

    #[test]
    fn accept_valid_tcp_upstream_address() {
        let yaml = r#"
listeners:
  - name: db
    address: "0.0.0.0:5432"
    protocol: tcp
    upstream: "10.0.0.1:5432"
"#;
        Config::from_yaml(yaml).unwrap();
    }

    #[test]
    fn reject_too_many_listeners() {
        use crate::config::Listener;

        let listeners: Vec<Listener> = (0..1_001)
            .map(|i| Listener {
                name: format!("l{i}"),
                address: format!("127.0.0.1:{}", 10_000 + i),
                protocol: Default::default(),
                tls: None,
                upstream: None,
                filter_chains: vec![],
                tcp_idle_timeout_ms: None,
                downstream_read_timeout_ms: None,
            })
            .collect();
        let err = validate_listeners(&listeners).unwrap_err();
        assert!(err.to_string().contains("too many listeners"), "got: {err}");
    }
}
