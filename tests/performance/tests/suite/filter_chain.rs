//! Filter chain depth benchmarks.
//!
//! Measures how pipeline length affects throughput. Each test
//! builds a chain with `request_id`, N `headers` filters,
//! `router`, and `load_balancer`, then drives GET traffic
//! through it.

use praxis_core::config::Config;
use praxis_test_utils::{free_port, start_backend, start_proxy};

use crate::helpers::{BenchConfig, assert_performance, report_results, run_get_benchmark};

// -----------------------------------------------------------------------------
// YAML Generator
// -----------------------------------------------------------------------------

// Build a filter_chains YAML with `num_header_filters` headers
// filters between request_id and router+lb.
fn multi_filter_yaml(proxy_port: u16, backend_port: u16, num_header_filters: usize) -> String {
    let mut headers_block = String::new();
    for i in 0..num_header_filters {
        headers_block.push_str(&format!(
            r#"      - filter: headers
        request_add:
          - name: "X-Bench-{i}"
            value: "value-{i}"
"#
        ));
    }

    format!(
        r#"listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
    filter_chains:
      - bench

filter_chains:
  - name: bench
    filters:
      - filter: request_id
{headers_block}      - filter: router
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

// -----------------------------------------------------------------------------
// 4 Filters (request_id + 1 headers + router + lb)
// -----------------------------------------------------------------------------

#[test]
fn bench_pipeline_4_filters() {
    let backend_port = start_backend("ok");
    let proxy_port = free_port();
    let yaml = multi_filter_yaml(proxy_port, backend_port, 1);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);
    let cfg = BenchConfig::new("pipeline_4_filters (rid + 1h + router + lb)");
    let result = run_get_benchmark(&cfg, &addr, "/");
    assert_eq!(result.errors, 0, "all requests should succeed");
    report_results(&result);
    // 4 filters should not noticeably degrade from baseline
    assert_performance(&result, 500.0, 500.0);
}

// -----------------------------------------------------------------------------
// 8 Filters (request_id + 5 headers + router + lb)
// -----------------------------------------------------------------------------

#[test]
fn bench_pipeline_8_filters() {
    let backend_port = start_backend("ok");
    let proxy_port = free_port();
    let yaml = multi_filter_yaml(proxy_port, backend_port, 5);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);
    let cfg = BenchConfig::new("pipeline_8_filters (rid + 5h + router + lb)");
    let result = run_get_benchmark(&cfg, &addr, "/");
    assert_eq!(result.errors, 0, "all requests should succeed");
    report_results(&result);
    // 5 extra header filters add per-request work; allow ~40% drop
    assert_performance(&result, 300.0, 500.0);
}

// -----------------------------------------------------------------------------
// 15 Filters (request_id + 12 headers + router + lb)
// -----------------------------------------------------------------------------

#[test]
fn bench_pipeline_15_filters() {
    let backend_port = start_backend("ok");
    let proxy_port = free_port();
    let yaml = multi_filter_yaml(proxy_port, backend_port, 12);
    let config = Config::from_yaml(&yaml).unwrap();
    let addr = start_proxy(&config);
    let cfg = BenchConfig::new("pipeline_15_filters (rid + 12h + router + lb)");
    let result = run_get_benchmark(&cfg, &addr, "/");
    assert_eq!(result.errors, 0, "all requests should succeed");
    report_results(&result);
    // 12 header filters; still shouldn't drop below 200 rps
    assert_performance(&result, 200.0, 500.0);
}
