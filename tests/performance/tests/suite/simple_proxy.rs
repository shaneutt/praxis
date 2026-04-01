//! Simple proxy throughput benchmarks.
//!
//! Measures baseline proxy performance with a minimal config:
//! one listener, one catch-all route, one backend. No extra
//! filters beyond the implicit router and load_balancer.

use praxis_core::config::Config;
use praxis_test_utils::{free_port, start_backend, start_proxy};

use crate::helpers::{BenchConfig, assert_performance, report_results, run_get_benchmark};

// -----------------------------------------------------------------------------
// Serial Throughput
// -----------------------------------------------------------------------------

#[test]
fn bench_simple_proxy_serial() {
    let backend_port = start_backend("ok");
    let proxy_port = free_port();
    let yaml = praxis_test_utils::simple_proxy_yaml(proxy_port, backend_port);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);
    let cfg = BenchConfig::new("simple_proxy_serial").total(1000).concurrency(1);

    let result = run_get_benchmark(&cfg, &addr, "/");
    assert_eq!(result.errors, 0, "all requests should succeed");
    report_results(&result);
    // ~10x below typical; catches broken proxy, not perf drift
    assert_performance(&result, 100.0, 500.0);
}

// -----------------------------------------------------------------------------
// Moderate Concurrency
// -----------------------------------------------------------------------------

#[test]
fn bench_simple_proxy_concurrent() {
    let backend_port = start_backend("ok");
    let proxy_port = free_port();
    let yaml = praxis_test_utils::simple_proxy_yaml(proxy_port, backend_port);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    let cfg = BenchConfig::new("simple_proxy_concurrent").total(2000).concurrency(8);
    let result = run_get_benchmark(&cfg, &addr, "/");
    assert_eq!(result.errors, 0, "all requests should succeed");
    report_results(&result);
    // 8 threads should easily exceed 500 rps on any CI runner
    assert_performance(&result, 500.0, 500.0);
}

// -----------------------------------------------------------------------------
// High Concurrency
// -----------------------------------------------------------------------------

#[test]
fn bench_simple_proxy_high_concurrency() {
    let backend_port = start_backend("ok");
    let proxy_port = free_port();
    let yaml = praxis_test_utils::simple_proxy_yaml(proxy_port, backend_port);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);

    let cfg = BenchConfig::new("simple_proxy_high_concurrency")
        .total(4000)
        .concurrency(16);
    let result = run_get_benchmark(&cfg, &addr, "/");
    assert_eq!(result.errors, 0, "all requests should succeed");
    report_results(&result);
    // same floor as c=8; contention shouldn't halve throughput
    assert_performance(&result, 500.0, 500.0);
}
