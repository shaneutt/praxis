use praxis_core::config::Config;
use praxis_filter::{FilterAction, FilterError, HttpFilter, HttpFilterContext};
use praxis_protocol::http::load_http_handler;

use crate::common::{
    build_pipeline, free_port, http_get, http_send, parse_status, registry_with, simple_proxy_yaml, start_backend,
    start_proxy, start_proxy_with_registry, wait_for_tcp,
};

// -----------------------------------------------------------------------------
// Test Filters
// -----------------------------------------------------------------------------

/// A test filter that adds a custom header during the response phase.
struct ResponseHeaderFilter;

#[async_trait::async_trait]
impl HttpFilter for ResponseHeaderFilter {
    fn name(&self) -> &'static str {
        "test_response_header"
    }

    async fn on_request(&self, _ctx: &mut HttpFilterContext<'_>) -> Result<FilterAction, FilterError> {
        Ok(FilterAction::Continue)
    }

    async fn on_response(&self, ctx: &mut HttpFilterContext<'_>) -> Result<FilterAction, FilterError> {
        if let Some(resp) = ctx.response_header.as_mut() {
            resp.headers.insert("X-Praxis-Filtered", "true".parse().unwrap());
        }

        Ok(FilterAction::Continue)
    }
}

// -----------------------------------------------------------------------------
// Integration Tests
// -----------------------------------------------------------------------------

#[test]
fn basic_proxy() {
    let backend_port = start_backend("hello from backend");
    let proxy_port = free_port();
    let config = Config::from_yaml(&simple_proxy_yaml(proxy_port, backend_port)).unwrap();
    let addr = start_proxy(&config);

    let (status, body) = http_get(&addr, "/", None);
    assert_eq!(status, 200);
    assert_eq!(body, "hello from backend");
}

#[test]
fn path_based_routing() {
    let api_port = start_backend("api response");
    let web_port = start_backend("web response");
    let proxy_port = free_port();

    let yaml = format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
routes:
  - path_prefix: "/api/"
    cluster: "api"
  - path_prefix: "/"
    cluster: "web"
clusters:
  - name: "api"
    endpoints:
      - "127.0.0.1:{api_port}"
  - name: "web"
    endpoints:
      - "127.0.0.1:{web_port}"
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    let (status, body) = http_get(&addr, "/api/users", None);
    assert_eq!(status, 200);
    assert_eq!(body, "api response");

    let (status, body) = http_get(&addr, "/index.html", None);
    assert_eq!(status, 200);
    assert_eq!(body, "web response");
}

#[test]
fn no_matching_route_returns_404() {
    let backend_port = start_backend("ok");
    let proxy_port = free_port();

    let yaml = format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
routes:
  - path_prefix: "/api/"
    cluster: "api"
clusters:
  - name: "api"
    endpoints:
      - "127.0.0.1:{backend_port}"
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    let (status, _body) = http_get(&addr, "/other", None);
    assert_eq!(status, 404);
}

#[test]
fn round_robin_distribution() {
    let port_a = start_backend("backend-a");
    let port_b = start_backend("backend-b");
    let port_c = start_backend("backend-c");
    let proxy_port = free_port();

    let yaml = format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
routes:
  - path_prefix: "/"
    cluster: "backends"
clusters:
  - name: "backends"
    endpoints:
      - "127.0.0.1:{port_a}"
      - "127.0.0.1:{port_b}"
      - "127.0.0.1:{port_c}"
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    let mut count_a = 0u32;
    let mut count_b = 0u32;
    let mut count_c = 0u32;
    for _ in 0..15 {
        let (_status, body) = http_get(&addr, "/", None);
        match body.as_str() {
            "backend-a" => count_a += 1,
            "backend-b" => count_b += 1,
            "backend-c" => count_c += 1,
            other => panic!("unexpected backend body: {other}"),
        }
    }

    // Strict round-robin distributes 15 requests across 3
    // backends exactly: 5 each. No scheduling variance applies
    // because round-robin is deterministic.
    assert_eq!(count_a, 5, "expected exactly 5 for backend-a, got {count_a}");
    assert_eq!(count_b, 5, "expected exactly 5 for backend-b, got {count_b}");
    assert_eq!(count_c, 5, "expected exactly 5 for backend-c, got {count_c}");
}

#[test]
fn host_based_routing() {
    let api_port = start_backend("api host");
    let default_port = start_backend("default host");
    let proxy_port = free_port();

    let yaml = format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
routes:
  - path_prefix: "/"
    host: "api.example.com"
    cluster: "api"
  - path_prefix: "/"
    cluster: "default"
clusters:
  - name: "api"
    endpoints:
      - "127.0.0.1:{api_port}"
  - name: "default"
    endpoints:
      - "127.0.0.1:{default_port}"
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    let (status, body) = http_get(&addr, "/", Some("api.example.com"));
    assert_eq!(status, 200);
    assert_eq!(body, "api host");

    let (status, body) = http_get(&addr, "/", Some("other.com"));
    assert_eq!(status, 200);
    assert_eq!(body, "default host");
}

#[test]
fn response_filter_executes() {
    let backend_port = start_backend("filtered response");
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
  - filter: test_response_header
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let registry = registry_with("test_response_header", || Box::new(ResponseHeaderFilter));
    let addr = start_proxy_with_registry(&config, &registry);

    let host_header = "localhost";
    let raw = http_send(
        &addr,
        &format!("GET / HTTP/1.1\r\nHost: {host_header}\r\nConnection: close\r\n\r\n"),
    );
    let raw_lower = raw.to_lowercase();
    assert!(
        raw_lower.contains("x-praxis-filtered: true"),
        "response should contain header set by on_response filter, got:\n{raw}"
    );

    let (status, body) = http_get(&addr, "/", None);
    assert_eq!(status, 200);
    assert_eq!(body, "filtered response");
}

#[test]
fn health_endpoints() {
    let backend_port = start_backend("ok");
    let proxy_port = free_port();
    let admin_port = free_port();

    let yaml = format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
admin_address: "127.0.0.1:{admin_port}"
routes:
  - path_prefix: "/"
    cluster: "backend"
clusters:
  - name: "backend"
    endpoints:
      - "127.0.0.1:{backend_port}"
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let pipeline = std::sync::Arc::new(build_pipeline(&config));
    let mut server = praxis_core::server::build_http_server(config.shutdown_timeout_secs, &Default::default());
    for listener in &config.listeners {
        load_http_handler(&mut server, listener, pipeline.clone()).unwrap();
    }
    praxis_protocol::http::pingora::health::add_health_endpoint_to_pingora_server(
        &mut server,
        config.admin_address.as_ref().unwrap(),
    );
    let server = server;
    std::thread::spawn(move || {
        server.run_forever();
    });
    wait_for_tcp(&format!("127.0.0.1:{admin_port}"));

    let admin_addr = format!("127.0.0.1:{admin_port}");
    let (status, body) = http_get(&admin_addr, "/ready", None);
    assert_eq!(status, 200);
    assert!(body.contains("ok"), "body: {body}");

    let (status, body) = http_get(&admin_addr, "/healthy", None);
    assert_eq!(status, 200);
    assert!(body.contains("ok"), "body: {body}");

    let (status, _) = http_get(&admin_addr, "/unknown", None);
    assert_eq!(status, 404);
}

#[test]
fn access_log_filter_processes_request() {
    let backend_port = start_backend("logged response");
    let proxy_port = free_port();

    let yaml = format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
pipeline:
  - filter: access_log
  - filter: router
    routes:
      - path_prefix: "/"
        cluster: "backend"
  - filter: load_balancer
    clusters:
      - name: "backend"
        endpoints:
          - "127.0.0.1:{backend_port}"
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    let (status, body) = http_get(&addr, "/api/test", None);
    assert_eq!(status, 200);
    assert_eq!(body, "logged response");
}

#[test]
fn runtime_config_parsed_from_yaml_and_proxies() {
    let backend_port = start_backend("runtime ok");
    let proxy_port = free_port();

    let yaml = format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
runtime:
  threads: 2
  work_stealing: false
routes:
  - path_prefix: "/"
    cluster: "backend"
clusters:
  - name: "backend"
    endpoints:
      - "127.0.0.1:{backend_port}"
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    assert_eq!(config.runtime.threads, 2);
    assert!(!config.runtime.work_stealing);

    let runtime = praxis_core::RuntimeOptions {
        threads: config.runtime.threads,
        work_stealing: config.runtime.work_stealing,
    };
    let pipeline = std::sync::Arc::new(build_pipeline(&config));
    let mut server = praxis_core::server::build_http_server(config.shutdown_timeout_secs, &runtime);
    for listener in &config.listeners {
        load_http_handler(&mut server, listener, pipeline.clone()).unwrap();
    }
    std::thread::spawn(move || {
        server.run_forever();
    });

    let addr = format!("127.0.0.1:{proxy_port}");
    wait_for_tcp(&addr);

    let (status, body) = http_get(&addr, "/", None);
    assert_eq!(status, 200);
    assert_eq!(body, "runtime ok");
}

#[test]
fn connection_timeout_config_parses() {
    let backend_port = start_backend("ok");
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
        connection_timeout_ms: 5000
        idle_timeout_ms: 30000
        read_timeout_ms: 10000
        write_timeout_ms: 10000
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    let (status, body) = http_get(&addr, "/", None);
    assert_eq!(status, 200);
    assert_eq!(body, "ok");
}

#[test]
fn pipeline_style_config_proxies() {
    let backend_port = start_backend("pipeline ok");
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
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    let (status, body) = http_get(&addr, "/", None);
    assert_eq!(status, 200);
    assert_eq!(body, "pipeline ok");
}

#[test]
fn admin_address_none_still_proxies() {
    let backend_port = start_backend("no admin");
    let proxy_port = free_port();
    let config = Config::from_yaml(&simple_proxy_yaml(proxy_port, backend_port)).unwrap();
    assert!(config.admin_address.is_none());
    let addr = start_proxy(&config);

    let (status, body) = http_get(&addr, "/", None);
    assert_eq!(status, 200);
    assert_eq!(body, "no admin");
}

#[test]
fn multiple_listeners() {
    let backend_port = start_backend("multi listener");
    let port_a = free_port();
    let port_b = free_port();
    let port_c = free_port();

    let yaml = format!(
        r#"
listeners:
  - name: listener_a
    address: "127.0.0.1:{port_a}"
  - name: listener_b
    address: "127.0.0.1:{port_b}"
  - name: listener_c
    address: "127.0.0.1:{port_c}"
routes:
  - path_prefix: "/"
    cluster: "backend"
clusters:
  - name: "backend"
    endpoints:
      - "127.0.0.1:{backend_port}"
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    let pipeline = std::sync::Arc::new(build_pipeline(&config));
    let mut server = praxis_core::server::build_http_server(config.shutdown_timeout_secs, &Default::default());
    for listener in &config.listeners {
        load_http_handler(&mut server, listener, pipeline.clone()).unwrap();
    }
    let server = server;
    std::thread::spawn(move || {
        server.run_forever();
    });
    wait_for_tcp(&format!("127.0.0.1:{port_a}"));

    let (status_a, body_a) = http_get(&format!("127.0.0.1:{port_a}"), "/", None);
    assert_eq!(status_a, 200);
    assert_eq!(body_a, "multi listener");

    let (status_b, body_b) = http_get(&format!("127.0.0.1:{port_b}"), "/", None);
    assert_eq!(status_b, 200);
    assert_eq!(body_b, "multi listener");

    let (status_c, body_c) = http_get(&format!("127.0.0.1:{port_c}"), "/", None);
    assert_eq!(status_c, 200);
    assert_eq!(body_c, "multi listener");
}

#[test]
fn per_listener_pipelines() {
    let backend_port = start_backend("ok");
    let port_a = free_port();
    let port_b = free_port();

    let yaml = format!(
        r#"
listeners:
  - name: alpha
    address: "127.0.0.1:{port_a}"
    filter_chains: [shared, chain_alpha]
  - name: beta
    address: "127.0.0.1:{port_b}"
    filter_chains: [shared, chain_beta]
filter_chains:
  - name: shared
    filters:
      - filter: router
        routes:
          - path_prefix: "/"
            cluster: "backend"
      - filter: load_balancer
        clusters:
          - name: "backend"
            endpoints:
              - "127.0.0.1:{backend_port}"
  - name: chain_alpha
    filters:
      - filter: headers
        response_add:
          - name: X-Listener
            value: "alpha"
  - name: chain_beta
    filters:
      - filter: headers
        response_add:
          - name: X-Listener
            value: "beta"
"#
    );

    let config = Config::from_yaml(&yaml).unwrap();
    start_proxy(&config);
    wait_for_tcp(&format!("127.0.0.1:{port_b}"));

    let raw_a = http_send(
        &format!("127.0.0.1:{port_a}"),
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(parse_status(&raw_a), 200);
    assert!(
        raw_a.contains("x-listener: alpha"),
        "listener A should add X-Listener: alpha, got:\n{raw_a}"
    );
    assert!(
        !raw_a.contains("x-listener: beta"),
        "listener A must NOT have beta's header"
    );

    let raw_b = http_send(
        &format!("127.0.0.1:{port_b}"),
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(parse_status(&raw_b), 200);
    assert!(
        raw_b.contains("x-listener: beta"),
        "listener B should add X-Listener: beta, got:\n{raw_b}"
    );
    assert!(
        !raw_b.contains("x-listener: alpha"),
        "listener B must NOT have alpha's header"
    );
}
