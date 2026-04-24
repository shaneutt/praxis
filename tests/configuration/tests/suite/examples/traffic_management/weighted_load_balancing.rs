// SPDX-License-Identifier: MIT
// Copyright (c) 2024 Shane Utt

//! Weighted load balancing example tests.

use std::collections::HashMap;

use praxis_test_utils::{free_port, http_get, start_backend_with_shutdown, start_proxy};

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[test]
fn weighted_load_balancing() {
    let backend_light = start_backend_with_shutdown("light");
    let backend_heavy = start_backend_with_shutdown("heavy");
    let proxy_port = free_port();
    let config = crate::example_utils::load_example_config(
        "traffic-management/weighted-load-balancing.yaml",
        proxy_port,
        HashMap::from([("127.0.0.1:3001", backend_light.port()), ("127.0.0.1:3002", backend_heavy.port())]),
    );
    let proxy = start_proxy(&config);

    let total = 200u32;
    let mut light_count = 0u32;
    let mut heavy_count = 0u32;
    for _ in 0..total {
        let (status, body) = http_get(proxy.addr(), "/", None);
        assert_eq!(status, 200, "weighted LB request should return 200");
        match body.as_str() {
            "light" => light_count += 1,
            "heavy" => heavy_count += 1,
            other => panic!("unexpected body: {other}"),
        }
    }

    assert_eq!(light_count + heavy_count, total, "all requests should reach a backend");

    assert!(
        (30..=70).contains(&light_count),
        "expected ~50 light (weight=1/4), got {light_count}"
    );
    assert!(
        (130..=170).contains(&heavy_count),
        "expected ~150 heavy (weight=3/4), got {heavy_count}"
    );

    let ratio = heavy_count as f64 / light_count as f64;
    assert!((2.0..=4.0).contains(&ratio), "expected ratio ~3.0, got {ratio}");
}
