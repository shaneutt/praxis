use std::collections::HashMap;

use crate::common::{free_port, http_send, parse_body, parse_header, parse_status, start_backend, start_proxy};

#[test]
fn access_logging() {
    let backend_port = start_backend("logged");
    let proxy_port = free_port();
    let config = super::load_example_config(
        "observability/access-logging.yaml",
        proxy_port,
        HashMap::from([("127.0.0.1:3000", backend_port)]),
    );
    let addr = start_proxy(&config);

    // Basic request: status 200, body matches backend.
    let raw = http_send(&addr, "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    assert_eq!(parse_status(&raw), 200);
    assert_eq!(parse_body(&raw), "logged");

    // With X-Request-Id: proxy should echo it back.
    let raw = http_send(
        &addr,
        "GET / HTTP/1.1\r\n\
         Host: localhost\r\n\
         X-Request-Id: trace-abc\r\n\
         Connection: close\r\n\r\n",
    );
    assert_eq!(parse_status(&raw), 200);
    assert_eq!(parse_header(&raw, "x-request-id"), Some("trace-abc".to_string()),);
}
