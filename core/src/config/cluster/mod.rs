//! Upstream cluster definitions: endpoints, load-balancing strategies, and timeouts.

mod endpoint;
mod health_check;
mod load_balancer_strategy;

use std::sync::Arc;

pub use endpoint::Endpoint;
pub use health_check::{HealthCheckConfig, HealthCheckType};
pub use load_balancer_strategy::{ConsistentHashOpts, LoadBalancerStrategy, ParameterisedStrategy, SimpleStrategy};
use serde::{Deserialize, Serialize};

// -----------------------------------------------------------------------------
// Cluster
// -----------------------------------------------------------------------------

/// A named group of upstream endpoints.
///
/// ```
/// # use praxis_core::config::Cluster;
/// let yaml = r#"
/// name: "backend"
/// endpoints: ["10.0.0.1:8080"]
/// connection_timeout_ms: 5000
/// idle_timeout_ms: 30000
/// "#;
/// let cluster: Cluster = serde_yaml::from_str(yaml).unwrap();
/// assert_eq!(cluster.connection_timeout_ms, Some(5000));
/// assert_eq!(cluster.idle_timeout_ms, Some(30000));
/// assert!(cluster.read_timeout_ms.is_none());
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]

pub struct Cluster {
    /// Unique name for the cluster.
    pub name: Arc<str>,

    /// TCP connection timeout in milliseconds.
    #[serde(default)]
    pub connection_timeout_ms: Option<u64>,

    /// List of endpoints for the cluster. Each entry is either a plain
    /// `"host:port"` string or a `{ address, weight }` object.
    pub endpoints: Vec<Endpoint>,

    /// Total connection timeout in milliseconds (TCP + TLS)
    #[serde(default)]
    pub total_connection_timeout_ms: Option<u64>,

    /// Idle connection timeout in milliseconds.
    #[serde(default)]
    pub idle_timeout_ms: Option<u64>,

    /// Load-balancing algorithm for this cluster. Defaults to `round_robin`.
    #[serde(default)]
    pub load_balancer_strategy: LoadBalancerStrategy,

    /// Read timeout in milliseconds.
    #[serde(default)]
    pub read_timeout_ms: Option<u64>,

    /// SNI hostname to present when opening TLS connections to upstream
    /// endpoints. Defaults to the `Host` request header when not set.
    #[serde(default)]
    pub upstream_sni: Option<String>,

    /// Connect to upstream endpoints over TLS. Defaults to `false` (plain
    /// HTTP).
    #[serde(default)]
    pub upstream_tls: bool,

    /// Write timeout in milliseconds.
    #[serde(default)]
    pub write_timeout_ms: Option<u64>,

    /// Active health check configuration for this cluster.
    #[serde(default)]
    pub health_check: Option<HealthCheckConfig>,
}

impl Cluster {
    /// Build a cluster with only a name and endpoints; all other
    /// fields use their defaults (no timeouts, no TLS, no health
    /// check, `round_robin` strategy).
    ///
    /// ```
    /// use praxis_core::config::Cluster;
    ///
    /// let c = Cluster {
    ///     upstream_tls: true,
    ///     ..Cluster::with_defaults("backend", vec!["10.0.0.1:443".into()])
    /// };
    /// assert_eq!(&*c.name, "backend");
    /// assert!(c.upstream_tls);
    /// ```
    pub fn with_defaults(name: &str, endpoints: Vec<Endpoint>) -> Self {
        Self {
            name: Arc::from(name),
            endpoints,
            connection_timeout_ms: None,
            idle_timeout_ms: None,
            load_balancer_strategy: Default::default(),
            read_timeout_ms: None,
            total_connection_timeout_ms: None,
            upstream_sni: None,
            upstream_tls: false,
            write_timeout_ms: None,
            health_check: None,
        }
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cluster_minimal() {
        let yaml = r#"
name: "backend"
endpoints: ["10.0.0.1:8080"]
"#;
        let cluster: Cluster = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(&*cluster.name, "backend", "cluster name mismatch");
        assert_eq!(
            cluster.endpoints[0].address(),
            "10.0.0.1:8080",
            "endpoint address mismatch"
        );
        assert_eq!(cluster.endpoints[0].weight(), 1, "default weight should be 1");
        assert_eq!(
            cluster.load_balancer_strategy,
            LoadBalancerStrategy::default(),
            "strategy should default"
        );
        assert!(
            cluster.connection_timeout_ms.is_none(),
            "connection_timeout should default to None"
        );
    }

    #[test]
    fn parse_cluster_with_weights() {
        let yaml = r#"
name: "backend"
endpoints:
  - "10.0.0.1:8080"
  - address: "10.0.0.2:8080"
    weight: 3
"#;
        let cluster: Cluster = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cluster.endpoints.len(), 2, "should parse two endpoints");
        assert_eq!(cluster.endpoints[0].weight(), 1, "simple endpoint weight should be 1");
        assert_eq!(cluster.endpoints[1].weight(), 3, "weighted endpoint weight should be 3");
    }

    #[test]
    fn parse_cluster_with_timeouts() {
        let yaml = r#"
name: "backend"
endpoints: ["10.0.0.1:8080"]
connection_timeout_ms: 5000
idle_timeout_ms: 30000
read_timeout_ms: 10000
write_timeout_ms: 10000
"#;
        let cluster: Cluster = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            cluster.connection_timeout_ms,
            Some(5000),
            "connection_timeout_ms mismatch"
        );
        assert_eq!(cluster.idle_timeout_ms, Some(30000), "idle_timeout_ms mismatch");
        assert_eq!(cluster.read_timeout_ms, Some(10000), "read_timeout_ms mismatch");
        assert_eq!(cluster.write_timeout_ms, Some(10000), "write_timeout_ms mismatch");
    }

    #[test]
    fn cluster_roundtrips_via_serde() {
        let cluster = Cluster {
            connection_timeout_ms: Some(1000),
            ..Cluster::with_defaults("web", vec!["10.0.0.1:80".into()])
        };
        let value = serde_yaml::to_value(&cluster).unwrap();
        let back: Cluster = serde_yaml::from_value(value).unwrap();
        assert_eq!(back.name, cluster.name, "name should roundtrip");
        assert_eq!(back.endpoints, cluster.endpoints, "endpoints should roundtrip");
        assert_eq!(
            back.connection_timeout_ms, cluster.connection_timeout_ms,
            "timeout should roundtrip"
        );
    }

    #[test]
    fn upstream_tls_and_sni_parse_correctly() {
        let yaml = r#"
name: "backend"
endpoints: ["10.0.0.1:443"]
upstream_tls: true
upstream_sni: "api.example.com"
"#;
        let cluster: Cluster = serde_yaml::from_str(yaml).unwrap();
        assert!(cluster.upstream_tls, "upstream_tls should be true");
        assert_eq!(
            cluster.upstream_sni.as_deref(),
            Some("api.example.com"),
            "upstream_sni mismatch"
        );
    }
}
