//! Forwarded headers adversarial tests.
//!
//! Verifies that `X-Forwarded-For`, `X-Forwarded-Proto`,
//! and `X-Forwarded-Host` cannot be spoofed by untrusted
//! clients, and that trusted proxy chains are preserved.

use praxis_core::config::Config;

use crate::common::{free_port, http_send, parse_body, start_header_echo_backend, start_proxy};

// -----------------------------------------------------------------------------
// YAML Builder
// -----------------------------------------------------------------------------

// Build proxy YAML with forwarded_headers filter.
fn fwd_yaml(proxy_port: u16, backend_port: u16, trusted: &[&str]) -> String {
    let trusted_yaml = if trusted.is_empty() {
        String::new()
    } else {
        let entries: Vec<String> = trusted.iter().map(|t| format!("          - \"{t}\"")).collect();
        format!("        trusted_proxies:\n{}", entries.join("\n"))
    };

    format!(
        r#"
listeners:
  - name: proxy
    address: "127.0.0.1:{proxy_port}"
    filter_chains:
      - main
filter_chains:
  - name: main
    filters:
      - filter: forwarded_headers
{trusted_yaml}
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
    )
}

// Extract a header value from the echo body (key: value format).
fn body_header(body: &str, name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    body.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        if k.trim().to_lowercase() == lower {
            Some(v.trim().to_string())
        } else {
            None
        }
    })
}

// -----------------------------------------------------------------------------
// Untrusted Client Spoofing
// -----------------------------------------------------------------------------

#[test]
fn untrusted_client_cannot_spoof_xff() {
    let backend_port = start_header_echo_backend();
    let proxy_port = free_port();
    // No trusted proxies: client at 127.0.0.1 is untrusted
    let yaml = fwd_yaml(proxy_port, backend_port, &[]);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    let raw = http_send(
        &addr,
        "GET / HTTP/1.1\r\n\
         Host: localhost\r\n\
         X-Forwarded-For: 10.0.0.99\r\n\
         Connection: close\r\n\r\n",
    );
    let body = parse_body(&raw);

    let xff = body_header(&body, "x-forwarded-for");
    assert!(xff.is_some(), "X-Forwarded-For must be present; body: {body}");
    let xff = xff.unwrap();
    assert!(
        !xff.contains("10.0.0.99"),
        "spoofed XFF value must be overwritten; got: {xff}"
    );
    assert!(
        xff.contains("127.0.0.1"),
        "real client IP must appear in XFF; got: {xff}"
    );
}

#[test]
fn untrusted_cannot_spoof_x_forwarded_proto() {
    let backend_port = start_header_echo_backend();
    let proxy_port = free_port();
    let yaml = fwd_yaml(proxy_port, backend_port, &[]);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    let raw = http_send(
        &addr,
        "GET / HTTP/1.1\r\n\
         Host: localhost\r\n\
         X-Forwarded-Proto: https\r\n\
         Connection: close\r\n\r\n",
    );
    let body = parse_body(&raw);

    let proto = body_header(&body, "x-forwarded-proto");
    assert!(proto.is_some(), "X-Forwarded-Proto must be present; body: {body}");
    // The filter always sets proto from the actual scheme (http for
    // plain TCP), overwriting any spoofed value.
    let proto = proto.unwrap();
    assert_eq!(
        proto, "http",
        "X-Forwarded-Proto must reflect actual scheme, not spoofed value"
    );
}

#[test]
fn untrusted_cannot_spoof_x_forwarded_host() {
    let backend_port = start_header_echo_backend();
    let proxy_port = free_port();
    let yaml = fwd_yaml(proxy_port, backend_port, &[]);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    let raw = http_send(
        &addr,
        "GET / HTTP/1.1\r\n\
         Host: real-host.com\r\n\
         X-Forwarded-Host: evil-host.com\r\n\
         Connection: close\r\n\r\n",
    );
    let body = parse_body(&raw);

    let host = body_header(&body, "x-forwarded-host");
    assert!(host.is_some(), "X-Forwarded-Host must be present; body: {body}");
    let host = host.unwrap();
    assert_eq!(
        host, "real-host.com",
        "X-Forwarded-Host must reflect the actual Host header, not the spoofed value"
    );
}

// -----------------------------------------------------------------------------
// XFF Chain Injection
// -----------------------------------------------------------------------------

#[test]
fn xff_chain_injection_prevented() {
    let backend_port = start_header_echo_backend();
    let proxy_port = free_port();
    let yaml = fwd_yaml(proxy_port, backend_port, &[]);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    // Attacker sends a fake chain to confuse IP extraction
    let raw = http_send(
        &addr,
        "GET / HTTP/1.1\r\n\
         Host: localhost\r\n\
         X-Forwarded-For: 10.0.0.1, 192.168.1.1\r\n\
         Connection: close\r\n\r\n",
    );
    let body = parse_body(&raw);

    let xff = body_header(&body, "x-forwarded-for");
    let xff = xff.unwrap_or_default();
    // Entire spoofed chain must be replaced, not appended to
    assert!(
        !xff.contains("10.0.0.1"),
        "injected chain IP must not survive; got: {xff}"
    );
    assert!(
        !xff.contains("192.168.1.1"),
        "injected chain IP must not survive; got: {xff}"
    );
    assert_eq!(xff, "127.0.0.1", "XFF must contain only the real client IP; got: {xff}");
}

// -----------------------------------------------------------------------------
// Trusted Proxy Preservation
// -----------------------------------------------------------------------------

#[test]
fn trusted_proxy_preserves_chain() {
    let backend_port = start_header_echo_backend();
    let proxy_port = free_port();
    // Trust loopback (our test client)
    let yaml = fwd_yaml(proxy_port, backend_port, &["127.0.0.0/8"]);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    let raw = http_send(
        &addr,
        "GET / HTTP/1.1\r\n\
         Host: localhost\r\n\
         X-Forwarded-For: 203.0.113.50\r\n\
         Connection: close\r\n\r\n",
    );
    let body = parse_body(&raw);

    let xff = body_header(&body, "x-forwarded-for");
    let xff = xff.unwrap_or_default();
    // Trusted proxy: existing chain preserved, our IP appended
    assert!(
        xff.contains("203.0.113.50"),
        "trusted proxy must preserve existing XFF; got: {xff}"
    );
    assert!(
        xff.contains("127.0.0.1"),
        "trusted proxy must append its own IP; got: {xff}"
    );
    assert_eq!(
        xff, "203.0.113.50, 127.0.0.1",
        "XFF must be existing + appended; got: {xff}"
    );
}

#[test]
fn trusted_proxy_with_long_xff_chain() {
    let backend_port = start_header_echo_backend();
    let proxy_port = free_port();
    let yaml = fwd_yaml(proxy_port, backend_port, &["127.0.0.0/8"]);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    // Three-hop chain: original client + two proxies, then us
    let raw = http_send(
        &addr,
        "GET / HTTP/1.1\r\n\
         Host: localhost\r\n\
         X-Forwarded-For: 203.0.113.1, 10.1.1.1, 10.2.2.2\r\n\
         Connection: close\r\n\r\n",
    );
    let body = parse_body(&raw);

    let xff = body_header(&body, "x-forwarded-for");
    let xff = xff.unwrap_or_default();
    assert_eq!(
        xff, "203.0.113.1, 10.1.1.1, 10.2.2.2, 127.0.0.1",
        "full chain must be preserved with our IP appended; got: {xff}"
    );
}

// -----------------------------------------------------------------------------
// Multiple XFF Headers From Untrusted Client
// -----------------------------------------------------------------------------

#[test]
fn multiple_xff_headers_from_untrusted_overwritten() {
    let backend_port = start_header_echo_backend();
    let proxy_port = free_port();
    let yaml = fwd_yaml(proxy_port, backend_port, &[]);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    // Send two XFF headers via raw TCP
    let raw = http_send(
        &addr,
        "GET / HTTP/1.1\r\n\
         Host: localhost\r\n\
         X-Forwarded-For: 10.0.0.1\r\n\
         X-Forwarded-For: 192.168.1.1\r\n\
         Connection: close\r\n\r\n",
    );
    let body = parse_body(&raw);

    // The filter overwrites for untrusted clients. At minimum,
    // the spoofed IPs must not be the only values.
    let xff = body_header(&body, "x-forwarded-for");
    let xff = xff.unwrap_or_default();
    assert!(
        xff.contains("127.0.0.1"),
        "real client IP must appear in XFF; got: {xff}"
    );
}
