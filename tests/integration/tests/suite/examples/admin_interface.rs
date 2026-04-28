// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Integration tests for the admin-interface example configuration.

use std::collections::HashMap;

use praxis_test_utils::{free_port, http_get, start_backend, start_proxy, wait_for_tcp};

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[test]
fn admin_interface_config_parses() {
    let backend_port = free_port();
    let admin_port = free_port();
    let config = super::load_example_config(
        "operations/admin-interface.yaml",
        free_port(),
        HashMap::from([("127.0.0.1:3000", backend_port), ("127.0.0.1:9901", admin_port)]),
    );

    assert_eq!(config.listeners.len(), 1, "expected one listener");
    assert!(config.admin.address.is_some(), "admin address should be set");
    assert!(config.admin.verbose, "admin verbose should be true");
}

#[test]
fn admin_interface_serves_health_and_metrics() {
    let backend_port = start_backend("admin-test");
    let proxy_port = free_port();
    let admin_port = free_port();
    let config = super::load_example_config(
        "operations/admin-interface.yaml",
        proxy_port,
        HashMap::from([("127.0.0.1:3000", backend_port), ("127.0.0.1:9901", admin_port)]),
    );

    let _proxy = start_proxy(&config);

    let admin_addr = format!("127.0.0.1:{admin_port}");
    let proxy_addr = format!("127.0.0.1:{proxy_port}");
    wait_for_tcp(&admin_addr);
    wait_for_tcp(&proxy_addr);

    let (healthy_status, _) = http_get(&admin_addr, "/healthy", None);
    assert_eq!(healthy_status, 200, "/healthy should return 200");

    let (ready_status, _) = http_get(&admin_addr, "/ready", None);
    assert_eq!(ready_status, 200, "/ready should return 200");

    let (proxy_status, proxy_body) = http_get(&proxy_addr, "/", None);
    assert_eq!(proxy_status, 200, "proxy should return 200");
    assert_eq!(proxy_body, "admin-test", "proxy should forward to backend");

    let (metrics_status, metrics_body) = http_get(&admin_addr, "/metrics", None);
    assert_eq!(metrics_status, 200, "/metrics should return 200");
    assert!(
        metrics_body.contains("praxis_http_requests_total"),
        "/metrics should contain praxis_http_requests_total: {metrics_body}"
    );
    assert!(
        metrics_body.contains("praxis_http_request_duration_seconds"),
        "/metrics should contain praxis_http_request_duration_seconds: {metrics_body}"
    );
    assert!(
        metrics_body.contains("method=\"GET\""),
        "/metrics should contain method=GET label: {metrics_body}"
    );
    assert!(
        metrics_body.contains("status_class=\"2xx\""),
        "/metrics should contain status_class=2xx label: {metrics_body}"
    );
}
