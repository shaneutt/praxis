use praxis_core::config::Config;

use crate::common::{free_port, http_get, http_send, parse_status, simple_proxy_yaml, start_backend, start_proxy};

// -----------------------------------------------------------------------------
// Retry behavior
// -----------------------------------------------------------------------------

#[test]
fn get_to_dead_backend_returns_502() {
    // Use a port with nothing listening on it
    let dead_port = free_port();
    let proxy_port = free_port();
    let yaml = simple_proxy_yaml(proxy_port, dead_port);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);
    let raw = http_send(&addr, "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    let status = parse_status(&raw);
    assert_eq!(status, 502, "expected 502 for dead backend, got: {raw}");
}

#[test]
fn post_to_dead_backend_returns_502() {
    let dead_port = free_port();
    let proxy_port = free_port();
    let yaml = simple_proxy_yaml(proxy_port, dead_port);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);
    let raw = http_send(
        &addr,
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
    );
    let status = parse_status(&raw);
    assert_eq!(status, 502, "expected 502 for dead backend, got: {raw}");
}

#[test]
fn retry_succeeds_when_backend_is_up() {
    let backend_port = start_backend("retry-ok");
    let proxy_port = free_port();
    let yaml = simple_proxy_yaml(proxy_port, backend_port);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);
    let (status, body) = http_get(&addr, "/", None);
    assert_eq!(status, 200);
    assert_eq!(body, "retry-ok");
}
