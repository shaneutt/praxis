// SPDX-License-Identifier: MIT
// Copyright (c) 2024 Shane Utt

//! TLS configuration and listener grouping utilities for TCP protocol.

use std::collections::HashMap;

use pingora_core::services::listening::Service;
use praxis_core::{
    ProxyError,
    config::{Config, ProtocolKind},
};
use tracing::info;

use super::proxy::PingoraTcpProxy;

// -----------------------------------------------------------------------------
// Types
// -----------------------------------------------------------------------------

/// Grouping key: `(upstream, idle_timeout_ms, max_duration_secs)`.
pub(super) type TcpGroupKey = (Option<String>, Option<u64>, Option<u64>);

// -----------------------------------------------------------------------------
// Grouping
// -----------------------------------------------------------------------------

/// Group TCP listeners by `(upstream, idle_timeout, max_duration)`.
pub(super) fn group_tcp_listeners(config: &Config) -> HashMap<TcpGroupKey, Vec<&praxis_core::config::Listener>> {
    let mut groups: HashMap<TcpGroupKey, Vec<&praxis_core::config::Listener>> = HashMap::new();
    for listener in &config.listeners {
        if listener.protocol != ProtocolKind::Tcp {
            continue;
        }
        let key = (
            listener.upstream.clone(),
            listener.tcp_idle_timeout_ms,
            listener.tcp_max_duration_secs,
        );
        groups.entry(key).or_default().push(listener);
    }
    groups
}

// -----------------------------------------------------------------------------
// Listener Registration
// -----------------------------------------------------------------------------

/// Add TCP or TLS listeners to a service.
pub(super) fn register_tcp_listeners(
    service: &mut Service<PingoraTcpProxy>,
    listeners: &[&praxis_core::config::Listener],
    upstream: Option<&str>,
) -> Result<(), ProxyError> {
    let display_upstream = upstream.unwrap_or("filter-routed");
    for listener in listeners {
        if let Some(ref tls) = listener.tls {
            let tls_settings = build_tcp_tls_settings(tls, &listener.address)?;
            service.add_tls_with_settings(&listener.address, None, tls_settings);
        } else {
            service.add_tcp(&listener.address);
        }
        info!(
            name = %listener.name,
            address = %listener.address,
            upstream = %display_upstream,
            "TCP listener registered"
        );
    }
    Ok(())
}

// -----------------------------------------------------------------------------
// TLS
// -----------------------------------------------------------------------------

/// Build [`TlsSettings`] for a TCP listener.
///
/// When `hot_reload` is enabled, uses a [`ReloadableCertResolver`]
/// and spawns a [`CertWatcher`] background task. Otherwise builds
/// a static [`ServerConfig`] via [`build_server_config`].
///
/// [`TlsSettings`]: pingora_core::listeners::tls::TlsSettings
/// [`ServerConfig`]: rustls::ServerConfig
/// [`build_server_config`]: praxis_tls::setup::build_server_config
/// [`ReloadableCertResolver`]: praxis_tls::reload::ReloadableCertResolver
/// [`CertWatcher`]: praxis_tls::watcher::CertWatcher
fn build_tcp_tls_settings(
    tls: &praxis_tls::ListenerTls,
    address: &str,
) -> Result<pingora_core::listeners::tls::TlsSettings, ProxyError> {
    if tls.is_hot_reload() {
        tracing::debug!(address, "building TLS ServerConfig with hot-reload (TCP)");
        let (server_config, swap_handle) = praxis_tls::setup::build_reloadable_server_config(tls)
            .map_err(|e| ProxyError::Config(format!("TLS hot-reload for {address}: {e}")))?;

        let pair =
            tls.certificates.first().cloned().ok_or_else(|| {
                ProxyError::Config(format!("TLS hot-reload for {address}: no certificate configured"))
            })?;
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        praxis_tls::watcher::CertWatcher::spawn(swap_handle, pair, shutdown_rx);

        return pingora_core::listeners::tls::TlsSettings::with_server_config(server_config)
            .map_err(|e| ProxyError::Config(format!("TLS for {address}: {e}")));
    }

    tracing::debug!(address, "building TLS ServerConfig (TCP)");
    let server_config = praxis_tls::setup::build_server_config(tls)
        .map_err(|e| ProxyError::Config(format!("TLS for {address}: {e}")))?;
    pingora_core::listeners::tls::TlsSettings::with_server_config(server_config)
        .map_err(|e| ProxyError::Config(format!("TLS for {address}: {e}")))
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, reason = "tests")]
mod tests {
    use praxis_core::config::{AdminConfig, BodyLimitsConfig, Config, InsecureOptions, RuntimeConfig};

    use super::*;

    #[test]
    fn group_tcp_listeners_groups_by_upstream_and_timeout() {
        let config = Config::from_yaml(
            r#"
listeners:
  - name: db1
    address: "0.0.0.0:5432"
    protocol: tcp
    upstream: "10.0.0.1:5432"
  - name: db2
    address: "0.0.0.0:5433"
    protocol: tcp
    upstream: "10.0.0.1:5432"
"#,
        )
        .unwrap();
        let groups = group_tcp_listeners(&config);
        assert_eq!(groups.len(), 1, "same upstream + timeout should produce one group");
        let key = (Some("10.0.0.1:5432".to_owned()), Some(300_000), None);
        assert_eq!(groups[&key].len(), 2, "both listeners should be in the same group");
    }

    #[test]
    fn group_tcp_listeners_separates_different_upstreams() {
        let config = Config::from_yaml(
            r#"
listeners:
  - name: db
    address: "0.0.0.0:5432"
    protocol: tcp
    upstream: "10.0.0.1:5432"
  - name: cache
    address: "0.0.0.0:6379"
    protocol: tcp
    upstream: "10.0.0.2:6379"
"#,
        )
        .unwrap();
        let groups = group_tcp_listeners(&config);
        assert_eq!(groups.len(), 2, "different upstreams should produce separate groups");
    }

    #[test]
    fn group_tcp_listeners_separates_different_timeouts() {
        let config = Config::from_yaml(
            r#"
listeners:
  - name: a
    address: "0.0.0.0:5432"
    protocol: tcp
    upstream: "10.0.0.1:5432"
  - name: b
    address: "0.0.0.0:5433"
    protocol: tcp
    upstream: "10.0.0.1:5432"
    tcp_idle_timeout_ms: 30000
"#,
        )
        .unwrap();
        let groups = group_tcp_listeners(&config);
        assert_eq!(
            groups.len(),
            2,
            "same upstream but different timeouts should produce separate groups"
        );
    }

    #[test]
    fn group_tcp_listeners_skips_http_listeners() {
        let config = Config::from_yaml(
            r#"
listeners:
  - name: web
    address: "0.0.0.0:8080"
    filter_chains: [main]
  - name: db
    address: "0.0.0.0:5432"
    protocol: tcp
    upstream: "10.0.0.1:5432"
filter_chains:
  - name: main
    filters:
      - filter: router
        routes:
          - path_prefix: "/"
            cluster: default
      - filter: load_balancer
        clusters:
          - name: default
            endpoints: ["127.0.0.1:9090"]
"#,
        )
        .unwrap();
        let groups = group_tcp_listeners(&config);
        assert_eq!(groups.len(), 1, "HTTP listeners should be excluded");
        let key = (Some("10.0.0.1:5432".to_owned()), Some(300_000), None);
        assert!(groups.contains_key(&key), "only TCP listener should be grouped");
    }

    #[test]
    fn group_tcp_listeners_includes_tcp_without_upstream() {
        let config = config_with_tcp_no_upstream();
        let groups = group_tcp_listeners(&config);
        assert_eq!(
            groups.len(),
            1,
            "TCP listener without upstream should be grouped with None key"
        );
        let key = (None, None, None);
        assert!(groups.contains_key(&key), "group key should have None upstream");
    }

    #[test]
    fn group_tcp_listeners_http_only_yields_empty() {
        let config = Config::from_yaml(
            r#"
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
            cluster: default
      - filter: load_balancer
        clusters:
          - name: default
            endpoints: ["127.0.0.1:9090"]
"#,
        )
        .unwrap();
        let groups = group_tcp_listeners(&config);
        assert!(
            groups.is_empty(),
            "config with only HTTP listeners should yield empty groups"
        );
    }

    // -------------------------------------------------------------------------
    // Test Utilities
    // -------------------------------------------------------------------------

    /// Build a Config with a TCP listener lacking an upstream address.
    ///
    /// This bypasses `Config::from_yaml` validation which rejects TCP
    /// listeners without an upstream.
    fn config_with_tcp_no_upstream() -> Config {
        use praxis_core::config::{Listener, ProtocolKind};
        Config {
            admin: AdminConfig::default(),
            body_limits: BodyLimitsConfig::default(),
            clusters: vec![],
            filter_chains: vec![],
            insecure_options: InsecureOptions::default(),
            listeners: vec![Listener {
                name: "orphan".to_owned(),
                address: "0.0.0.0:9999".to_owned(),
                protocol: ProtocolKind::Tcp,
                tls: None,
                upstream: None,
                filter_chains: vec![],
                tcp_idle_timeout_ms: None,
                downstream_read_timeout_ms: None,
                tcp_max_duration_secs: None,
            }],
            runtime: RuntimeConfig::default(),
            shutdown_timeout_secs: 10,
        }
    }
}
