use praxis_core::config::Config;

use crate::common::{
    RoutedBackend, free_port, http_get, http_send, parse_body, parse_status, start_header_echo_backend, start_proxy,
};

// -----------------------------------------------------------------------------
// Dynamic Backend & Condition-Gated Routing Tests
// -----------------------------------------------------------------------------

#[test]
fn multi_backend_path_routing() {
    let api_port = RoutedBackend::new().route("/", 200, "api-backend").start();
    let static_port = RoutedBackend::new().route("/", 200, "static-backend").start();
    let default_port = RoutedBackend::new()
        .route_with_headers("/", 200, "default-backend", vec![("X-Source", "default")])
        .start();
    let proxy_port = free_port();

    let yaml = format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
pipeline:
  - filter: router
    routes:
      - path_prefix: "/api/"
        cluster: "api"
      - path_prefix: "/static/"
        cluster: "static"
      - path_prefix: "/"
        cluster: "default"
  - filter: load_balancer
    clusters:
      - name: "api"
        endpoints:
          - "127.0.0.1:{api_port}"
      - name: "static"
        endpoints:
          - "127.0.0.1:{static_port}"
      - name: "default"
        endpoints:
          - "127.0.0.1:{default_port}"
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    let (status, body) = http_get(&addr, "/api/users", None);
    assert_eq!(status, 200);
    assert_eq!(body, "api-backend");

    let (status, body) = http_get(&addr, "/static/style.css", None);
    assert_eq!(status, 200);
    assert_eq!(body, "static-backend");

    let (status, body) = http_get(&addr, "/index.html", None);
    assert_eq!(status, 200);
    assert_eq!(body, "default-backend");
}

#[test]
fn response_condition_gated_header_filter() {
    let backend_port = RoutedBackend::new()
        .route("/ok", 200, "success")
        .route("/", 404, "not found")
        .start();
    let proxy_port = free_port();

    let yaml = format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
pipeline:
  - filter: router
    routes:
      - path_prefix: "/"
        cluster: "backend"
  - filter: load_balancer
    clusters:
      - name: "backend"
        endpoints:
          - "127.0.0.1:{backend_port}"
  - filter: headers
    response_conditions:
      - when:
          status: [200]
    response_add:
      - name: X-Processed
        value: "true"
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    // 200 response — response condition matches, header added.
    let raw = http_send(
        &addr,
        "GET /ok HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(parse_status(&raw), 200);
    assert!(
        raw.to_lowercase().contains("x-processed: true"),
        "expected x-processed header on 200 response"
    );

    // 404 response — response condition does not match, header skipped.
    let raw = http_send(
        &addr,
        "GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(parse_status(&raw), 404);
    assert!(
        !raw.to_lowercase().contains("x-processed"),
        "x-processed header should NOT appear on 404 response"
    );
}

#[test]
fn response_condition_unless_skips_filter() {
    let backend_port = RoutedBackend::new()
        .route_with_headers("/skip", 200, "skip me", vec![("X-No-Filter", "true")])
        .route("/", 200, "filter me")
        .start();
    let proxy_port = free_port();

    let yaml = format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
pipeline:
  - filter: router
    routes:
      - path_prefix: "/"
        cluster: "backend"
  - filter: load_balancer
    clusters:
      - name: "backend"
        endpoints:
          - "127.0.0.1:{backend_port}"
  - filter: headers
    response_conditions:
      - unless:
          headers:
            x-no-filter: "true"
    response_add:
      - name: X-Enriched
        value: "yes"
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    // Backend does not set x-no-filter — response condition passes, header added.
    let raw = http_send(
        &addr,
        "GET /other HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(parse_status(&raw), 200);
    assert!(
        raw.to_lowercase().contains("x-enriched: yes"),
        "expected x-enriched header when x-no-filter is absent"
    );

    // Backend sets x-no-filter: true — response condition skips the filter.
    let raw = http_send(
        &addr,
        "GET /skip HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(parse_status(&raw), 200);
    assert!(
        !raw.to_lowercase().contains("x-enriched"),
        "x-enriched header should NOT appear when x-no-filter is set"
    );
}

// -----------------------------------------------------------------------------
// Request-phase condition tests
// -----------------------------------------------------------------------------

#[test]
fn request_condition_when_matches_adds_header() {
    let backend_port = start_header_echo_backend();
    let proxy_port = free_port();

    let yaml = format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
pipeline:
  - filter: router
    routes:
      - path_prefix: "/"
        cluster: "backend"
  - filter: headers
    conditions:
      - when:
          path_prefix: "/api/"
    request_add:
      - name: X-Api-Request
        value: "true"
  - filter: load_balancer
    clusters:
      - name: "backend"
        endpoints:
          - "127.0.0.1:{backend_port}"
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    // Path matches /api/ — header should be injected and echoed.
    let raw = http_send(
        &addr,
        "GET /api/users HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(parse_status(&raw), 200);
    let body = parse_body(&raw);
    assert!(
        body.to_lowercase().contains("x-api-request: true"),
        "echo backend should reflect X-Api-Request on /api/ path, got:\n{body}"
    );

    // Path does NOT match /api/ — header should be absent.
    let raw = http_send(
        &addr,
        "GET /index.html HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(parse_status(&raw), 200);
    let body = parse_body(&raw);
    assert!(
        !body.to_lowercase().contains("x-api-request"),
        "X-Api-Request should NOT appear on /index.html path, got:\n{body}"
    );
}

#[test]
fn request_condition_unless_skips_filter() {
    let backend_port = start_header_echo_backend();
    let proxy_port = free_port();

    let yaml = format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
pipeline:
  - filter: router
    routes:
      - path_prefix: "/"
        cluster: "backend"
  - filter: headers
    conditions:
      - unless:
          path_prefix: "/healthz"
    request_add:
      - name: X-Tracked
        value: "true"
  - filter: load_balancer
    clusters:
      - name: "backend"
        endpoints:
          - "127.0.0.1:{backend_port}"
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    // Non-healthz path: condition passes, header added.
    // Echo backend reflects request headers in the body.
    let raw = http_send(
        &addr,
        "GET /api HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(parse_status(&raw), 200);
    let body = parse_body(&raw);
    assert!(
        body.to_lowercase().contains("x-tracked: true"),
        "X-Tracked should be present on non-healthz path, got:\n{body}"
    );

    // Healthz path: unless skips the filter, header absent.
    let raw = http_send(
        &addr,
        "GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(parse_status(&raw), 200);
    let body = parse_body(&raw);
    assert!(
        !body.to_lowercase().contains("x-tracked"),
        "X-Tracked should NOT be present on /healthz path, got:\n{body}"
    );
}

#[test]
fn request_condition_header_verified_via_echo() {
    let backend_port = start_header_echo_backend();
    let proxy_port = free_port();

    let yaml = format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
pipeline:
  - filter: router
    routes:
      - path_prefix: "/"
        cluster: "backend"
  - filter: headers
    conditions:
      - when:
          path_prefix: "/api/"
    request_add:
      - name: X-Api-Tag
        value: "injected"
  - filter: load_balancer
    clusters:
      - name: "backend"
        endpoints:
          - "127.0.0.1:{backend_port}"
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    // Path matches /api/ — header should be injected and echoed.
    let raw = http_send(
        &addr,
        "GET /api/v1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(parse_status(&raw), 200);
    let body = parse_body(&raw);
    assert!(
        body.to_lowercase().contains("x-api-tag: injected"),
        "echo backend should reflect X-Api-Tag on /api/ path, got:\n{body}"
    );

    // Path does NOT match /api/ — header should be absent.
    let raw = http_send(
        &addr,
        "GET /other HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(parse_status(&raw), 200);
    let body = parse_body(&raw);
    assert!(
        !body.to_lowercase().contains("x-api-tag"),
        "X-Api-Tag should NOT appear on /other path, got:\n{body}"
    );
}
