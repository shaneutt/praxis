//! YAML configuration parsing, defaults, and validation.

use serde::Deserialize;

mod cluster;
mod condition;
mod filter_chain;
mod listener;
mod loader;
mod pipeline;
mod route;
mod runtime;
mod validate;

pub use cluster::{
    Cluster, ConsistentHashOpts, Endpoint, HealthCheckConfig, HealthCheckType, LoadBalancerStrategy,
    ParameterisedStrategy, SimpleStrategy,
};
pub use condition::{Condition, ConditionMatch, ResponseCondition, ResponseConditionMatch};
pub use filter_chain::FilterChainConfig;
pub use listener::{Listener, ProtocolKind, TlsConfig};
pub use pipeline::FilterEntry;
pub use route::Route;
pub use runtime::RuntimeConfig;

// -----------------------------------------------------------------------------
// Config
// -----------------------------------------------------------------------------

/// Top-level proxy configuration.
///
/// ```
/// use praxis_core::config::Config;
///
/// let config = Config::from_yaml(r#"
/// listeners:
///   - name: web
///     address: "127.0.0.1:8080"
/// routes:
///   - path_prefix: "/"
///     cluster: "web"
/// clusters:
///   - name: "web"
///     endpoints: ["10.0.0.1:8080"]
/// "#).unwrap();
/// assert_eq!(config.listeners[0].address, "127.0.0.1:8080");
/// ```
#[derive(Debug, Clone, Deserialize)]

pub struct Config {
    /// Optional admin listener for health check endpoints (e.g. `/ready`, `/healthy`).
    #[serde(default)]
    pub admin_address: Option<String>,

    /// Cluster definitions referenced by filters.
    #[serde(default)]
    pub clusters: Vec<Cluster>,

    /// Named filter chains.
    #[serde(default)]
    pub filter_chains: Vec<FilterChainConfig>,

    /// Proxy listeners to bind.
    pub listeners: Vec<Listener>,

    /// Legacy filter pipeline entries (executed in order).
    ///
    /// Populated by [`apply_defaults`] from top-level `routes`/`clusters`
    /// when no explicit pipeline or filter chains are configured.
    ///
    /// [`apply_defaults`]: Config::apply_defaults
    #[serde(default)]
    pub pipeline: Vec<FilterEntry>,

    /// Top-level routes.
    #[serde(default)]
    pub routes: Vec<Route>,

    /// Runtime configuration knobs.
    #[serde(default)]
    pub runtime: RuntimeConfig,

    /// Hard ceiling on request body size in bytes. No filter can override this limit.
    #[serde(default)]
    pub max_request_body_bytes: Option<usize>,

    /// Hard ceiling on response body size in bytes. No filter can override this limit.
    #[serde(default)]
    pub max_response_body_bytes: Option<usize>,

    /// Drain time for graceful shutdown.
    #[serde(default = "default_shutdown_timeout_secs")]
    pub shutdown_timeout_secs: u64,
}

// -----------------------------------------------------------------------------
// Defaults
// -----------------------------------------------------------------------------

impl Config {
    /// If no pipeline is configured but legacy routes are present,
    /// generate a default pipeline of [`router`, `load_balancer`].
    pub(crate) fn apply_defaults(&mut self) {
        tracing::debug!("converting legacy routes to pipeline format");
        if self.pipeline.is_empty() && !self.routes.is_empty() {
            self.pipeline = vec![
                pipeline::build_router_entry(&self.routes),
                pipeline::build_lb_entry(&self.clusters),
            ];
        }

        tracing::debug!("converting legacy pipeline to default filter chain");
        if !self.pipeline.is_empty() && self.filter_chains.is_empty() {
            self.filter_chains.push(filter_chain::FilterChainConfig {
                name: "default".to_owned(),
                filters: self.pipeline.clone(),
            });
            for listener in &mut self.listeners {
                if listener.protocol == ProtocolKind::Http && listener.filter_chains.is_empty() {
                    listener.filter_chains.push("default".to_owned());
                }
            }
        }
    }
}

/// Serde default for [`Config::shutdown_timeout_secs`].
fn default_shutdown_timeout_secs() -> u64 {
    30
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        Cluster, Config, Route,
        pipeline::{build_lb_entry, build_router_entry},
    };

    const VALID_YAML: &str = r#"
listeners:
  - name: test
    address: "127.0.0.1:8080"
routes:
  - path_prefix: "/"
    cluster: "backend"
clusters:
  - name: "backend"
    endpoints:
      - "127.0.0.1:3000"
"#;

    #[test]
    fn legacy_config_generates_pipeline() {
        let config = Config::from_yaml(VALID_YAML).unwrap();
        assert_eq!(config.pipeline.len(), 2, "legacy should generate 2-stage pipeline");
        assert_eq!(config.pipeline[0].filter_type, "router", "first entry should be router");
        assert_eq!(
            config.pipeline[1].filter_type, "load_balancer",
            "second should be load_balancer"
        );
    }

    #[test]
    fn build_router_entry_creates_router_filter() {
        let routes = vec![Route {
            path_prefix: "/api/".into(),
            host: None,
            headers: None,
            cluster: Arc::from("api"),
        }];
        let entry = build_router_entry(&routes);
        assert_eq!(entry.filter_type, "router", "entry filter_type should be router");
        let routes_value = entry.config.get("routes").unwrap();
        assert!(routes_value.is_sequence(), "routes config should be a sequence");
    }

    #[test]
    fn build_lb_entry_creates_load_balancer_filter() {
        let clusters = vec![Cluster {
            name: Arc::from("web"),
            endpoints: vec!["10.0.0.1:80".into()],
            connection_timeout_ms: None,
            idle_timeout_ms: None,
            load_balancer_strategy: Default::default(),
            read_timeout_ms: None,
            total_connection_timeout_ms: None,
            upstream_sni: None,
            upstream_tls: false,
            write_timeout_ms: None,
            health_check: None,
        }];
        let entry = build_lb_entry(&clusters);
        assert_eq!(
            entry.filter_type, "load_balancer",
            "entry filter_type should be load_balancer"
        );
        let clusters_value = entry.config.get("clusters").unwrap();
        assert!(clusters_value.is_sequence(), "clusters config should be a sequence");
    }

    #[test]
    fn default_shutdown_timeout_is_30() {
        let yaml = r#"
listeners:
  - name: web
    address: "0.0.0.0:80"
routes:
  - path_prefix: "/"
    cluster: "web"
clusters:
  - name: "web"
    endpoints: ["10.0.0.1:80"]
"#;
        let config = Config::from_yaml(yaml).unwrap();
        assert_eq!(
            config.shutdown_timeout_secs, 30,
            "default shutdown timeout should be 30s"
        );
    }

    #[test]
    fn default_runtime_config() {
        let config = Config::from_yaml(VALID_YAML).unwrap();
        assert_eq!(config.runtime.threads, 0, "default threads should be 0");
        assert!(config.runtime.work_stealing, "default work_stealing should be true");
    }

    #[test]
    fn max_body_bytes_defaults_to_none() {
        let config = Config::from_yaml(VALID_YAML).unwrap();
        assert!(
            config.max_request_body_bytes.is_none(),
            "max_request_body_bytes should default to None"
        );
        assert!(
            config.max_response_body_bytes.is_none(),
            "max_response_body_bytes should default to None"
        );
    }

    #[test]
    fn apply_defaults_creates_filter_chain_from_legacy_pipeline() {
        let yaml = r#"
listeners:
  - name: web
    address: "0.0.0.0:80"
pipeline:
  - filter: router
    routes:
      - path_prefix: "/"
        cluster: "web"
  - filter: load_balancer
    clusters:
      - name: "web"
        endpoints: ["10.0.0.1:80"]
"#;
        let config = Config::from_yaml(yaml).unwrap();
        assert_eq!(config.filter_chains.len(), 1, "should create one default chain");
        assert_eq!(
            config.filter_chains[0].name, "default",
            "auto-chain should be named 'default'"
        );
        assert_eq!(
            config.filter_chains[0].filters.len(),
            2,
            "default chain should have 2 filters"
        );
        assert_eq!(
            config.listeners[0].filter_chains,
            vec!["default"],
            "listener should reference default chain"
        );
    }
}
