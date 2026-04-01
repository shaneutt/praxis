use std::collections::HashMap;

use crate::common::{free_port, http_get, start_backend, start_proxy};

/// Least-connections with 3 backends and sequential (non-concurrent)
/// requests. Because each connection completes before the next starts,
/// all backends sit at 0 active connections and the strategy degrades
/// to round-robin-like distribution. Verify all 3 backends are used
/// with roughly equal distribution.
#[test]
fn least_connections() {
    let port_a = start_backend("lc-a");
    let port_b = start_backend("lc-b");
    let port_c = start_backend("lc-c");
    let proxy_port = free_port();
    let config = super::load_example_config(
        "traffic-management/least-connections.yaml",
        proxy_port,
        HashMap::from([
            ("127.0.0.1:3001", port_a),
            ("127.0.0.1:3002", port_b),
            ("127.0.0.1:3003", port_c),
        ]),
    );
    let addr = start_proxy(&config);

    let total = 30u32;
    let mut counts: HashMap<String, u32> = HashMap::new();
    for _ in 0..total {
        let (status, body) = http_get(&addr, "/", None);
        assert_eq!(status, 200);
        *counts.entry(body).or_default() += 1;
    }

    assert_eq!(counts.len(), 3, "least-conn should use all 3 backends");

    // With sequential requests (0 concurrent connections) the
    // distribution should be roughly even. Allow each backend
    // between 7 and 13 of 30 requests.
    for (backend, count) in &counts {
        assert!(
            (7..=13).contains(count),
            "expected ~10 for backend {backend}, got {count}"
        );
    }
}
