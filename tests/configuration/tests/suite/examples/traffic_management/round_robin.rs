// SPDX-License-Identifier: MIT
// Copyright (c) 2024 Shane Utt

//! Round-robin load balancing example tests.

use std::collections::HashMap;

use praxis_test_utils::{free_port, http_get, start_backend_with_shutdown, start_proxy};

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[test]
fn round_robin() {
    let backend_a = start_backend_with_shutdown("a");
    let backend_b = start_backend_with_shutdown("b");
    let backend_c = start_backend_with_shutdown("c");
    let proxy_port = free_port();
    let config = crate::example_utils::load_example_config(
        "traffic-management/round-robin.yaml",
        proxy_port,
        HashMap::from([
            ("127.0.0.1:3001", backend_a.port()),
            ("127.0.0.1:3002", backend_b.port()),
            ("127.0.0.1:3003", backend_c.port()),
        ]),
    );
    let proxy = start_proxy(&config);

    let total = 30u32;
    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut sequence: Vec<String> = Vec::with_capacity(total as usize);
    for _ in 0..total {
        let (status, body) = http_get(proxy.addr(), "/", None);
        assert_eq!(status, 200, "round-robin request should return 200");
        *counts.entry(body.clone()).or_default() += 1;
        sequence.push(body);
    }

    assert_eq!(counts.len(), 3, "round robin should hit all 3 backends");

    for (backend, count) in &counts {
        assert_eq!(*count, 10, "expected exactly 10 for backend {backend}, got {count}");
    }

    let cycle: Vec<&str> = sequence[..3].iter().map(|s| s.as_str()).collect();
    for chunk in sequence.chunks(3).skip(1) {
        let got: Vec<&str> = chunk.iter().map(|s| s.as_str()).collect();
        assert_eq!(got, cycle, "round-robin should repeat the same cycle order");
    }
}
