//! Shared health state types for active health checking.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

// -----------------------------------------------------------------------------
// EndpointHealth
// -----------------------------------------------------------------------------

/// Atomic health state for a single upstream endpoint.
///
/// ```
/// use praxis_core::health::EndpointHealth;
/// use std::sync::atomic::Ordering;
///
/// let ep = EndpointHealth::new();
/// assert!(ep.is_healthy(), "endpoints start healthy");
///
/// ep.mark_unhealthy();
/// assert!(!ep.is_healthy());
///
/// ep.mark_healthy();
/// assert!(ep.is_healthy());
/// ```
pub struct EndpointHealth {
    /// Whether this endpoint is currently considered healthy.
    healthy: AtomicBool,

    /// Consecutive successful probes since last failure.
    consecutive_successes: AtomicU32,

    /// Consecutive failed probes since last success.
    consecutive_failures: AtomicU32,
}

impl EndpointHealth {
    /// Create a new endpoint health state (starts healthy).
    ///
    /// ```
    /// use praxis_core::health::EndpointHealth;
    ///
    /// let ep = EndpointHealth::new();
    /// assert!(ep.is_healthy());
    /// ```
    pub fn new() -> Self {
        Self {
            healthy: AtomicBool::new(true),
            consecutive_successes: AtomicU32::new(0),
            consecutive_failures: AtomicU32::new(0),
        }
    }

    /// Returns `true` if the endpoint is considered healthy.
    ///
    /// ```
    /// use praxis_core::health::EndpointHealth;
    ///
    /// let ep = EndpointHealth::new();
    /// assert!(ep.is_healthy());
    /// ```
    #[inline]
    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    /// Mark the endpoint as healthy and reset failure counter.
    ///
    /// ```
    /// use praxis_core::health::EndpointHealth;
    ///
    /// let ep = EndpointHealth::new();
    /// ep.mark_unhealthy();
    /// ep.mark_healthy();
    /// assert!(ep.is_healthy());
    /// ```
    pub fn mark_healthy(&self) {
        self.healthy.store(true, Ordering::Relaxed);
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    /// Mark the endpoint as unhealthy and reset success counter.
    ///
    /// ```
    /// use praxis_core::health::EndpointHealth;
    ///
    /// let ep = EndpointHealth::new();
    /// ep.mark_unhealthy();
    /// assert!(!ep.is_healthy());
    /// ```
    pub fn mark_unhealthy(&self) {
        self.healthy.store(false, Ordering::Relaxed);
        self.consecutive_successes.store(0, Ordering::Relaxed);
    }

    /// Record a successful probe and return whether the endpoint
    /// transitioned from unhealthy to healthy.
    ///
    /// ```
    /// use praxis_core::health::EndpointHealth;
    ///
    /// let ep = EndpointHealth::new();
    /// ep.mark_unhealthy();
    /// assert!(!ep.record_success(2), "need 2 successes to recover");
    /// assert!(ep.record_success(2), "second success should transition");
    /// assert!(ep.is_healthy());
    /// ```
    pub fn record_success(&self, healthy_threshold: u32) -> bool {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        let count = self.consecutive_successes.fetch_add(1, Ordering::Relaxed) + 1;
        if count >= healthy_threshold && !self.is_healthy() {
            self.healthy.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// Record a failed probe and return whether the endpoint
    /// transitioned from healthy to unhealthy.
    ///
    /// ```
    /// use praxis_core::health::EndpointHealth;
    ///
    /// let ep = EndpointHealth::new();
    /// assert!(!ep.record_failure(3), "need 3 failures to mark down");
    /// assert!(!ep.record_failure(3), "still need one more");
    /// assert!(ep.record_failure(3), "third failure should transition");
    /// assert!(!ep.is_healthy());
    /// ```
    pub fn record_failure(&self, unhealthy_threshold: u32) -> bool {
        self.consecutive_successes.store(0, Ordering::Relaxed);
        let count = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        if count >= unhealthy_threshold && self.is_healthy() {
            self.healthy.store(false, Ordering::Relaxed);
            return true;
        }
        false
    }
}

impl Default for EndpointHealth {
    fn default() -> Self {
        Self::new()
    }
}

// Allow sending across threads (atomics are inherently thread-safe).
// SAFETY: All fields are atomics; no raw pointers or non-Send types.
// `unsafe_code` is denied at crate level so we rely on the compiler
// auto-deriving Send+Sync for atomic types, which it already does.

// -----------------------------------------------------------------------------
// ClusterHealthState
// -----------------------------------------------------------------------------

/// Shared health state for all endpoints in a cluster.
///
/// The `Vec` is indexed in the same order as the cluster's
/// endpoint list.
///
/// ```
/// use praxis_core::health::{ClusterHealthState, EndpointHealth};
/// use std::sync::Arc;
///
/// let state: ClusterHealthState = Arc::new(vec![
///     EndpointHealth::new(),
///     EndpointHealth::new(),
/// ]);
/// assert!(state[0].is_healthy());
/// assert!(state[1].is_healthy());
/// ```
pub type ClusterHealthState = Arc<Vec<EndpointHealth>>;

// -----------------------------------------------------------------------------
// HealthRegistry
// -----------------------------------------------------------------------------

/// Maps cluster names to their endpoint health state.
///
/// Shared between the background health check runner
/// (writer) and the load balancer (reader).
///
/// ```
/// use praxis_core::health::{HealthRegistry, EndpointHealth};
/// use std::{collections::HashMap, sync::Arc};
///
/// let mut map = HashMap::new();
/// map.insert(
///     Arc::from("backend"),
///     Arc::new(vec![EndpointHealth::new()]),
/// );
/// let registry: HealthRegistry = Arc::new(map);
/// assert!(registry["backend"][0].is_healthy());
/// ```
pub type HealthRegistry = Arc<HashMap<Arc<str>, ClusterHealthState>>;

/// Build a [`HealthRegistry`] from the cluster config.
///
/// Only clusters with a `health_check` config get entries.
/// Endpoints without health checks are not tracked (always
/// considered healthy by the load balancer).
///
/// ```
/// use praxis_core::{config::Cluster, health::build_health_registry};
///
/// let clusters: Vec<Cluster> = vec![];
/// let registry = build_health_registry(&clusters);
/// assert!(registry.is_empty());
/// ```
pub fn build_health_registry(clusters: &[crate::config::Cluster]) -> HealthRegistry {
    let mut map = HashMap::new();
    for cluster in clusters {
        if cluster.health_check.is_some() {
            let state: Vec<EndpointHealth> = cluster.endpoints.iter().map(|_| EndpointHealth::new()).collect();
            map.insert(cluster.name.clone(), Arc::new(state));
        }
    }
    Arc::new(map)
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_starts_healthy() {
        let ep = EndpointHealth::new();
        assert!(ep.is_healthy(), "new endpoint should be healthy");
    }

    #[test]
    fn mark_unhealthy_then_healthy() {
        let ep = EndpointHealth::new();
        ep.mark_unhealthy();
        assert!(!ep.is_healthy(), "should be unhealthy after mark_unhealthy");
        ep.mark_healthy();
        assert!(ep.is_healthy(), "should be healthy after mark_healthy");
    }

    #[test]
    fn record_failure_transitions_at_threshold() {
        let ep = EndpointHealth::new();
        assert!(!ep.record_failure(3), "failure 1/3 should not transition");
        assert!(!ep.record_failure(3), "failure 2/3 should not transition");
        assert!(ep.record_failure(3), "failure 3/3 should transition to unhealthy");
        assert!(!ep.is_healthy(), "should be unhealthy after threshold failures");
    }

    #[test]
    fn record_success_transitions_at_threshold() {
        let ep = EndpointHealth::new();
        ep.mark_unhealthy();
        assert!(!ep.record_success(2), "success 1/2 should not transition");
        assert!(ep.record_success(2), "success 2/2 should transition to healthy");
        assert!(ep.is_healthy(), "should be healthy after threshold successes");
    }

    #[test]
    fn failure_resets_success_counter() {
        let ep = EndpointHealth::new();
        ep.mark_unhealthy();
        ep.record_success(3);
        ep.record_failure(1);
        assert!(!ep.record_success(3), "success counter should have reset");
    }

    #[test]
    fn success_resets_failure_counter() {
        let ep = EndpointHealth::new();
        ep.record_failure(3);
        ep.record_failure(3);
        ep.record_success(1);
        assert!(!ep.record_failure(3), "failure counter should have reset");
    }

    #[test]
    fn already_healthy_success_no_transition() {
        let ep = EndpointHealth::new();
        assert!(!ep.record_success(1), "already-healthy should not report transition");
    }

    #[test]
    fn already_unhealthy_failure_no_transition() {
        let ep = EndpointHealth::new();
        ep.mark_unhealthy();
        assert!(!ep.record_failure(1), "already-unhealthy should not report transition");
    }

    #[test]
    fn default_is_healthy() {
        let ep = EndpointHealth::default();
        assert!(ep.is_healthy(), "default should be healthy");
    }

    #[test]
    fn build_registry_only_includes_health_checked_clusters() {
        let clusters = vec![
            crate::config::Cluster {
                health_check: Some(crate::config::HealthCheckConfig {
                    check_type: crate::config::HealthCheckType::Http,
                    path: "/".to_owned(),
                    expected_status: 200,
                    interval_ms: 5000,
                    timeout_ms: 2000,
                    healthy_threshold: 2,
                    unhealthy_threshold: 3,
                }),
                ..crate::config::Cluster::with_defaults("checked", vec!["10.0.0.1:80".into(), "10.0.0.2:80".into()])
            },
            crate::config::Cluster::with_defaults("unchecked", vec!["10.0.0.3:80".into()]),
        ];

        let registry = build_health_registry(&clusters);
        assert!(
            registry.contains_key("checked"),
            "checked cluster should be in registry"
        );
        assert!(
            !registry.contains_key("unchecked"),
            "unchecked cluster should not be in registry"
        );
        assert_eq!(registry["checked"].len(), 2, "checked cluster should have 2 endpoints");
    }

    #[test]
    fn build_registry_empty_clusters() {
        let registry = build_health_registry(&[]);
        assert!(registry.is_empty(), "empty clusters should produce empty registry");
    }
}
