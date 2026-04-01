use std::collections::HashMap;

use crate::common::{Backend, free_port, http_send, parse_header, start_backend, start_proxy};

// --------------------------------------------------------------------------
// Tests - Header Manipulation
// --------------------------------------------------------------------------

#[test]
fn header_manipulation() {
    let backend_port = start_backend("ok");
    let proxy_port = free_port();
    let config = super::load_example_config(
        "transformation/header-manipulation.yaml",
        proxy_port,
        HashMap::from([("127.0.0.1:3000", backend_port)]),
    );
    let addr = start_proxy(&config);
    let raw = http_send(
        &addr,
        "GET / HTTP/1.1\r\n\
         Host: localhost\r\n\
         Connection: close\r\n\r\n",
    );
    assert_eq!(parse_header(&raw, "x-powered-by"), Some("praxis".to_string()),);
    assert_eq!(parse_header(&raw, "x-frame-options"), Some("DENY".to_string()),);
}

#[test]
fn header_response_remove_strips_upstream_header() {
    // The backend sends a "Server" header. The example config's
    // `response_remove` directive should strip it from the
    // response delivered to the client.
    let backend_port = Backend::fixed("ok").header("Server", "upstream-server").start();
    let proxy_port = free_port();
    let config = super::load_example_config(
        "transformation/header-manipulation.yaml",
        proxy_port,
        HashMap::from([("127.0.0.1:3000", backend_port)]),
    );
    let addr = start_proxy(&config);
    let raw = http_send(
        &addr,
        "GET / HTTP/1.1\r\n\
         Host: localhost\r\n\
         Connection: close\r\n\r\n",
    );
    assert!(
        parse_header(&raw, "server").is_none(),
        "Server header should be removed by response_remove; got response:\n{raw}"
    );
}
