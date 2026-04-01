//! Edge case and meta-tests.

use std::path::Path;

use praxis_core::config::Config;

use super::helpers;

// -----------------------------------------------------------------------------
// Tests - Edge Cases
// -----------------------------------------------------------------------------

#[test]
fn reject_malformed_yaml() {
    let err = Config::from_yaml("not: [valid: yaml: {{").unwrap_err();
    assert!(err.to_string().contains("invalid YAML"), "got: {err}");
}

#[test]
fn reject_empty_yaml() {
    let err = Config::from_yaml("").unwrap_err();
    assert!(err.to_string().contains("invalid YAML"), "got: {err}");
}

// Tests serde deserialization of an unknown enum variant for
// `ProtocolKind`. The `grpc` variant does not exist, so serde
// rejects the input before any custom validation runs.
#[test]
fn reject_unknown_protocol_variant() {
    let yaml = r#"
listeners:
  - name: web
    address: "127.0.0.1:8080"
    protocol: grpc
"#;
    let err = Config::from_yaml(yaml).unwrap_err();
    assert!(err.to_string().contains("invalid YAML"), "got: {err}");
}

#[test]
fn accept_minimal_config() {
    let config = Config::from_yaml(&helpers::minimal_valid_yaml()).unwrap();
    assert_eq!(config.listeners.len(), 1);
    assert_eq!(config.routes.len(), 1);
    assert_eq!(config.clusters.len(), 1);
}

#[test]
fn accept_all_fields_populated() {
    let yaml = r#"
listeners:
  - name: web
    address: "127.0.0.1:8080"
    tls:
      cert_path: "/etc/ssl/cert.pem"
      key_path: "/etc/ssl/key.pem"
    filter_chains: [main]

admin_address: "127.0.0.1:9901"
shutdown_timeout_secs: 60

max_request_body_bytes: 10485760
max_response_body_bytes: 5242880

runtime:
  threads: 4
  work_stealing: false

filter_chains:
  - name: main
    filters:
      - filter: router
        routes:
          - path_prefix: "/api"
            host: "api.example.com"
            headers:
              x-version: "v2"
            cluster: api
          - path_prefix: "/"
            cluster: web
      - filter: load_balancer
        clusters:
          - name: api
            endpoints:
              - address: "10.0.0.1:8080"
                weight: 3
              - address: "10.0.0.2:8080"
                weight: 1
            load_balancer_strategy: least_connections
            connection_timeout_ms: 5000
            total_connection_timeout_ms: 10000
            idle_timeout_ms: 30000
            read_timeout_ms: 10000
            write_timeout_ms: 10000
            upstream_tls: true
            upstream_sni: "api.internal.example.com"
          - name: web
            endpoints: ["10.0.0.3:80"]
            load_balancer_strategy:
              consistent_hash:
                header: "X-User-Id"
"#;
    let config = Config::from_yaml(yaml).unwrap();
    assert_eq!(config.admin_address.as_deref(), Some("127.0.0.1:9901"));
    assert_eq!(config.shutdown_timeout_secs, 60);
    assert_eq!(config.max_request_body_bytes, Some(10_485_760));
    assert_eq!(config.max_response_body_bytes, Some(5_242_880));
    assert_eq!(config.runtime.threads, 4);
    assert!(!config.runtime.work_stealing);

    // TLS config on the listener.
    let tls = config.listeners[0].tls.as_ref().expect("TLS config must be present");
    assert_eq!(tls.cert_path, "/etc/ssl/cert.pem");
    assert_eq!(tls.key_path, "/etc/ssl/key.pem");

    // Cluster endpoint counts from the filter chain.
    let lb_entry = &config.filter_chains[0].filters[1];
    assert_eq!(lb_entry.filter, "load_balancer");
    let clusters = lb_entry
        .config
        .get("clusters")
        .and_then(|v| v.as_sequence())
        .expect("load_balancer must have clusters");
    // "api" cluster has 2 endpoints, "web" has 1.
    let api_endpoints = clusters[0]
        .get("endpoints")
        .and_then(|v| v.as_sequence())
        .expect("api cluster must have endpoints");
    assert_eq!(api_endpoints.len(), 2);
    let web_endpoints = clusters[1]
        .get("endpoints")
        .and_then(|v| v.as_sequence())
        .expect("web cluster must have endpoints");
    assert_eq!(web_endpoints.len(), 1);
}

#[test]
fn default_shutdown_timeout_is_30() {
    let config = Config::from_yaml(&helpers::minimal_valid_yaml()).unwrap();
    assert_eq!(config.shutdown_timeout_secs, 30);
}

#[test]
fn accept_custom_shutdown_timeout() {
    let yaml = r#"
listeners:
  - name: web
    address: "127.0.0.1:8080"
shutdown_timeout_secs: 120
routes:
  - path_prefix: "/"
    cluster: b
clusters:
  - name: b
    endpoints: ["1.2.3.4:80"]
"#;
    let config = Config::from_yaml(yaml).unwrap();
    assert_eq!(config.shutdown_timeout_secs, 120);
}

#[test]
fn body_byte_limits_default_to_none() {
    let config = Config::from_yaml(&helpers::minimal_valid_yaml()).unwrap();
    assert!(config.max_request_body_bytes.is_none());
    assert!(config.max_response_body_bytes.is_none());
}

#[test]
fn accept_runtime_config_defaults() {
    let config = Config::from_yaml(&helpers::minimal_valid_yaml()).unwrap();
    assert_eq!(config.runtime.threads, 0);
    assert!(config.runtime.work_stealing);
    assert!(config.runtime.log_overrides.is_empty());
}

#[test]
fn accept_conditions() {
    let yaml = r#"
listeners:
  - name: web
    address: "127.0.0.1:8080"
    filter_chains: [main]
filter_chains:
  - name: main
    filters:
      - filter: router
        routes:
          - path_prefix: "/"
            cluster: backend
      - filter: headers
        request_add:
          - name: "X-Source"
            value: "gateway"
        conditions:
          - when:
              path_prefix: "/api"
          - unless:
              methods: ["OPTIONS"]
      - filter: headers
        response_set:
          - name: "Cache-Control"
            value: "no-store"
        response_conditions:
          - when:
              status: [200]
      - filter: load_balancer
        clusters:
          - name: backend
            endpoints: ["127.0.0.1:3000"]
"#;
    let config = Config::from_yaml(yaml).unwrap();
    let filters = &config.filter_chains[0].filters;

    // The headers filter with request conditions.
    assert_eq!(filters[1].conditions.len(), 2);

    // The headers filter with response conditions.
    assert_eq!(filters[2].response_conditions.len(), 1);
}

#[test]
fn reject_condition_both_when_and_unless() {
    let yaml = r#"
listeners:
  - name: web
    address: "127.0.0.1:8080"
    filter_chains: [main]
filter_chains:
  - name: main
    filters:
      - filter: headers
        request_add:
          - name: "X-Source"
            value: "gateway"
        conditions:
          - when:
              path_prefix: "/api"
            unless:
              methods: ["GET"]
      - filter: router
        routes:
          - path_prefix: "/"
            cluster: backend
      - filter: load_balancer
        clusters:
          - name: backend
            endpoints: ["127.0.0.1:3000"]
"#;
    let err = Config::from_yaml(yaml).unwrap_err();
    assert!(err.to_string().contains("exactly one"), "got: {err}");
}

#[test]
fn reject_oversized_yaml() {
    let huge = "x".repeat(5 * 1024 * 1024);
    let err = Config::from_yaml(&huge).unwrap_err();
    assert!(err.to_string().contains("too large"), "got: {err}");
}

#[test]
fn accept_config_from_example_files() {
    // Find the workspace root by looking for Cargo.toml alongside
    // the examples/ directory.
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("could not find workspace root");
    let examples_dir = workspace_root.join("examples/configs");
    assert!(
        examples_dir.exists(),
        "examples/configs directory not found at {}",
        examples_dir.display()
    );

    let mut count = 0;
    visit_yaml_files(&examples_dir, &mut count);

    assert!(count > 0, "no example YAML files found in {}", examples_dir.display());
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

/// Recursively visit all .yaml files in a directory tree.
fn visit_yaml_files(dir: &Path, count: &mut usize) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| {
        panic!("failed to read {}: {e}", dir.display());
    });

    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            visit_yaml_files(&path, count);
        } else if path.extension().is_some_and(|e| e == "yaml") {
            let content = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!("failed to read {}: {e}", path.display());
            });
            Config::from_yaml(&content).unwrap_or_else(|e| {
                panic!("example config {} failed validation: {e}", path.display());
            });
            *count += 1;
        }
    }
}
