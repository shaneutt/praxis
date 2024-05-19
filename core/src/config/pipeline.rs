//! Filter entry: a single filter step with optional conditions.

use serde::Deserialize;

use super::{Cluster, Condition, ResponseCondition, Route};

// -----------------------------------------------------------------------------
// FilterEntry
// -----------------------------------------------------------------------------

/// A single filter in the pipeline.
///
/// ```
/// use praxis_core::config::FilterEntry;
///
/// let entry: FilterEntry = serde_yaml::from_str(r#"
/// filter: router
/// routes:
///   - path_prefix: "/"
///     cluster: web
/// "#).unwrap();
/// assert_eq!(entry.filter_type, "router");
/// assert!(entry.conditions.is_empty());
/// ```
#[derive(Debug, Clone, Deserialize)]

pub struct FilterEntry {
    /// Filter type name (e.g. `"router"`, `"load_balancer"`, or a custom name).
    #[serde(rename = "filter")]
    pub filter_type: String,

    /// Ordered conditions that gate whether this filter runs on requests.
    /// Empty means the filter always runs.
    #[serde(default)]
    pub conditions: Vec<Condition>,

    /// Ordered conditions that gate whether this filter runs on responses.
    /// Evaluated against the upstream response (status, headers).
    /// Empty means the filter always runs on responses.
    #[serde(default)]
    pub response_conditions: Vec<ResponseCondition>,

    /// Arbitrary YAML config passed to the filter's factory function.
    #[serde(flatten)]
    pub config: serde_yaml::Value,
}

// -----------------------------------------------------------------------------
// Pipeline Defaults
// -----------------------------------------------------------------------------

/// Build a `router` filter entry from shorthand top-level routes.
pub(crate) fn build_router_entry(routes: &[Route]) -> FilterEntry {
    let routes_value = serde_yaml::to_value(routes).unwrap_or(serde_yaml::Value::Sequence(vec![]));
    let mut config = serde_yaml::Mapping::new();

    config.insert("routes".into(), routes_value);

    FilterEntry {
        filter_type: "router".to_owned(),
        conditions: vec![],
        response_conditions: vec![],
        config: serde_yaml::Value::Mapping(config),
    }
}

/// Build a `load_balancer` filter entry from shorthand top-level clusters.
pub(crate) fn build_lb_entry(clusters: &[Cluster]) -> FilterEntry {
    let clusters_value = serde_yaml::to_value(clusters).unwrap_or(serde_yaml::Value::Sequence(vec![]));
    let mut config = serde_yaml::Mapping::new();

    config.insert("clusters".into(), clusters_value);

    FilterEntry {
        filter_type: "load_balancer".to_owned(),
        conditions: vec![],
        response_conditions: vec![],
        config: serde_yaml::Value::Mapping(config),
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_filter_entry() {
        let yaml = r#"
filter: router
routes:
  - path_prefix: "/"
    cluster: "web"
"#;
        let entry: FilterEntry = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(entry.filter_type, "router", "filter_type mismatch");
        assert!(entry.config.get("routes").is_some(), "routes config should be present");
    }

    #[test]
    fn parse_filter_entry_custom_filter() {
        let yaml = r#"
filter: rate_limiter
requests_per_second: 100
"#;
        let entry: FilterEntry = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(entry.filter_type, "rate_limiter", "filter_type mismatch");
        let rps = entry.config.get("requests_per_second").unwrap();
        assert_eq!(rps.as_u64(), Some(100), "requests_per_second should be 100");
    }

    #[test]
    fn parse_filter_entry_with_conditions() {
        let yaml = r#"
filter: headers
conditions:
  - when:
      path_prefix: "/api"
  - unless:
      methods: ["OPTIONS"]
request_add:
  - ["X-Api-Version", "v2"]
"#;
        let entry: FilterEntry = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(entry.filter_type, "headers", "filter_type mismatch");
        assert_eq!(entry.conditions.len(), 2, "should have 2 conditions");
    }

    #[test]
    fn parse_filter_entry_without_conditions() {
        let yaml = r#"
filter: router
routes: []
"#;
        let entry: FilterEntry = serde_yaml::from_str(yaml).unwrap();
        assert!(entry.conditions.is_empty(), "conditions should be empty when omitted");
        assert!(
            entry.response_conditions.is_empty(),
            "response_conditions should be empty when omitted"
        );
    }

    #[test]
    fn parse_filter_entry_with_response_conditions() {
        let yaml = r#"
filter: headers
response_conditions:
  - when:
      status: [200, 201]
  - unless:
      headers:
        x-skip: "true"
response_add:
  - name: X-Processed
    value: "true"
"#;
        let entry: FilterEntry = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(entry.filter_type, "headers", "filter_type mismatch");
        assert!(entry.conditions.is_empty(), "request conditions should be empty");
        assert_eq!(entry.response_conditions.len(), 2, "should have 2 response conditions");
    }
}
