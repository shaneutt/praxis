// SPDX-License-Identifier: MIT
// Copyright (c) 2024 Shane Utt

//! Benchmark result types and comparison logic.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::BenchmarkError;

// -----------------------------------------------------------------------------
// Metric Types
// -----------------------------------------------------------------------------

/// Latency metrics from a benchmark run.
///
/// ```
/// use benchmarks::result::LatencyMetrics;
///
/// let m = LatencyMetrics::zeroed();
/// assert_eq!(m.p99, 0.0);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyMetrics {
    /// Minimum observed latency in seconds.
    pub min: f64,

    /// Maximum observed latency in seconds.
    pub max: f64,

    /// Mean latency in seconds.
    pub mean: f64,

    /// 50th percentile (median) latency in seconds.
    pub p50: f64,

    /// 90th percentile latency in seconds.
    pub p90: f64,

    /// 95th percentile latency in seconds.
    pub p95: f64,

    /// 99th percentile latency in seconds.
    pub p99: f64,

    /// 99.9th percentile latency in seconds.
    pub p99_9: f64,
}

impl LatencyMetrics {
    /// Create zeroed latency metrics (some tools don't report all).
    pub fn zeroed() -> Self {
        Self {
            min: 0.0,
            max: 0.0,
            mean: 0.0,
            p50: 0.0,
            p90: 0.0,
            p95: 0.0,
            p99: 0.0,
            p99_9: 0.0,
        }
    }
}

// -----------------------------------------------------------------------------
// Throughput Metrics
// -----------------------------------------------------------------------------

/// Throughput metrics from a benchmark run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThroughputMetrics {
    /// Requests per second.
    pub requests_per_sec: f64,

    /// Bytes per second.
    pub bytes_per_sec: f64,
}

// -----------------------------------------------------------------------------
// Resource Metrics
// -----------------------------------------------------------------------------

/// Resource utilization metrics from a benchmark run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceMetrics {
    /// Average CPU utilization (percentage).
    pub cpu_percent_avg: f64,

    /// Peak CPU utilization (percentage).
    pub cpu_percent_peak: f64,

    /// Average memory RSS in bytes.
    pub memory_rss_bytes_avg: u64,

    /// Peak memory RSS in bytes.
    pub memory_rss_bytes_peak: u64,
}

impl ResourceMetrics {
    /// Create zeroed resource metrics.
    pub fn zeroed() -> Self {
        Self {
            cpu_percent_avg: 0.0,
            cpu_percent_peak: 0.0,
            memory_rss_bytes_avg: 0,
            memory_rss_bytes_peak: 0,
        }
    }
}

// -----------------------------------------------------------------------------
// Error Metrics
// -----------------------------------------------------------------------------

/// Error counts from a benchmark run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorMetrics {
    /// Non-2xx HTTP responses (omitted for TCP).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub non_2xx: Option<u64>,

    /// Request timeouts.
    pub timeouts: u64,

    /// TCP connection failures.
    pub connect_failures: u64,
}

// -----------------------------------------------------------------------------
// Benchmark Result
// -----------------------------------------------------------------------------

/// Result of a single benchmark run.
///
/// ```
/// use benchmarks::result::{
///     BenchmarkResult, Environment, ErrorMetrics, LatencyMetrics, ThroughputMetrics,
/// };
///
/// let result = BenchmarkResult {
///     commit: "abc123".into(),
///     timestamp: "2026-01-01T00:00:00Z".into(),
///     scenario: "small-requests".into(),
///     proxy: "praxis".into(),
///     tool: "vegeta".into(),
///     environment: Environment {
///         cpu: "test".into(),
///         os: "linux".into(),
///     },
///     latency: LatencyMetrics::zeroed(),
///     throughput: ThroughputMetrics {
///         requests_per_sec: 1000.0,
///         bytes_per_sec: 50000.0,
///     },
///     resource: None,
///     errors: ErrorMetrics {
///         non_2xx: Some(0),
///         timeouts: 0,
///         connect_failures: 0,
///     },
///     raw_report: None,
/// };
/// assert_eq!(result.proxy, "praxis");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    /// Git commit SHA.
    pub commit: String,

    /// ISO 8601 timestamp.
    pub timestamp: String,

    /// Scenario name.
    pub scenario: String,

    /// Proxy name.
    pub proxy: String,

    /// Load generator tool that produced this result.
    pub tool: String,

    /// Environment metadata.
    pub environment: Environment,

    /// Latency metrics.
    pub latency: LatencyMetrics,

    /// Throughput metrics.
    pub throughput: ThroughputMetrics,

    /// Resource utilization metrics (populated via `docker stats`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceMetrics>,

    /// Error counts.
    pub errors: ErrorMetrics,

    /// Raw tool report (Vegeta or Fortio JSON).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_report: Option<serde_json::Value>,
}

// -----------------------------------------------------------------------------
// Scenario Results
// -----------------------------------------------------------------------------

/// Aggregated results from running a scenario (multiple runs).
///
/// ```
/// use benchmarks::result::ScenarioResults;
///
/// let results = ScenarioResults {
///     scenario: "test".into(),
///     proxy: "praxis".into(),
///     runs: vec![],
///     median: None,
/// };
/// assert_eq!(results.scenario, "test");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResults {
    /// Scenario name.
    pub scenario: String,

    /// Proxy name.
    pub proxy: String,

    /// Individual run results.
    pub runs: Vec<BenchmarkResult>,

    /// Median result (computed from runs).
    pub median: Option<BenchmarkResult>,
}

impl ScenarioResults {
    /// Compute the median result from the collected runs.
    #[allow(clippy::indexing_slicing, reason = "guarded")]
    pub fn compute_median(&mut self) {
        if self.runs.is_empty() {
            self.median = None;
            return;
        }

        let mut indices: Vec<usize> = (0..self.runs.len()).collect();
        indices.sort_by(|&a, &b| {
            self.runs[a]
                .latency
                .p99
                .partial_cmp(&self.runs[b].latency.p99)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mid = indices.len() / 2;
        self.median = Some(self.runs[indices[mid]].clone());
    }

    /// Compare these results against a baseline, producing a [`ComparativeResults`] that indicates regressions.
    pub fn compare(&self, baseline: &ScenarioResults, threshold: f64) -> ComparativeResults {
        let (current_p99, current_rps) = extract_metrics(&self.median);
        let (baseline_p99, baseline_rps) = extract_metrics(&baseline.median);

        let p99_change = relative_change(current_p99, baseline_p99);
        let throughput_change = relative_change(current_rps, baseline_rps);

        let regressed = p99_change > threshold || throughput_change < -threshold;
        tracing::debug!(p99_change, throughput_change, threshold, regressed, "regression check");

        let improved = p99_change < -threshold && throughput_change > threshold;
        tracing::debug!(improved, "improvement check");

        ComparativeResults {
            scenario: self.scenario.clone(),
            proxy: self.proxy.clone(),
            regressed,
            improved,
            p99_latency_change: p99_change,
            throughput_change,
        }
    }

    /// Save results to a YAML file.
    ///
    /// # Errors
    ///
    /// Returns [`BenchmarkError`] if serialization or file I/O fails.
    pub fn save_yaml(&self, path: &Path) -> Result<(), BenchmarkError> {
        let yaml = serde_yaml::to_string(self)?;
        std::fs::write(path, yaml)?;
        Ok(())
    }

    /// Load results from a YAML file.
    ///
    /// # Errors
    ///
    /// Returns [`BenchmarkError`] if the file cannot be read or parsed.
    pub fn load_yaml(path: &Path) -> Result<Self, BenchmarkError> {
        let contents = std::fs::read_to_string(path)?;
        let results = serde_yaml::from_str(&contents)?;
        Ok(results)
    }
}

/// Extract p99 and rps from a median result.
fn extract_metrics(median: &Option<BenchmarkResult>) -> (f64, f64) {
    median
        .as_ref()
        .map_or((0.0, 0.0), |m| (m.latency.p99, m.throughput.requests_per_sec))
}

/// Compute relative change between current and baseline values.
fn relative_change(current: f64, baseline: f64) -> f64 {
    if baseline > 0.0 {
        (current - baseline) / baseline
    } else if current > 0.0 {
        f64::INFINITY
    } else {
        0.0
    }
}

// -----------------------------------------------------------------------------
// Comparative Results
// -----------------------------------------------------------------------------

/// Result of comparing two [`ScenarioResults`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparativeResults {
    /// Scenario name.
    pub scenario: String,

    /// Proxy name.
    pub proxy: String,

    /// Whether performance degraded beyond the threshold.
    pub regressed: bool,

    /// Whether performance improved beyond the threshold.
    pub improved: bool,

    /// Percentage change in p99 latency.
    pub p99_latency_change: f64,

    /// Percentage change in throughput.
    pub throughput_change: f64,
}

// -----------------------------------------------------------------------------
// Environment
// -----------------------------------------------------------------------------

/// Environment metadata for reproducibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    /// CPU model string.
    pub cpu: String,

    /// Operating system identifier.
    pub os: String,
}

/// Detect the current environment's CPU and OS.
///
/// Falls back to "unknown" if detection fails.
pub fn current_environment() -> Environment {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|info| {
            info.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split(':').nth(1))
                .map(|s| s.trim().to_owned())
        })
        .unwrap_or_else(|| "unknown".into());

    let os = std::env::consts::OS.to_owned();

    Environment { cpu, os }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "tests"
)]
mod tests {
    use super::*;
    #[test]
    fn compare_detects_p99_regression() {
        let baseline = ScenarioResults {
            scenario: "test".into(),
            proxy: "praxis".into(),
            runs: vec![],
            median: Some(sample_result(0.010, 10_000.0)),
        };
        let current = ScenarioResults {
            scenario: "test".into(),
            proxy: "praxis".into(),
            runs: vec![],
            median: Some(sample_result(0.0115, 10_000.0)),
        };

        let cmp = current.compare(&baseline, 0.05);
        assert!(cmp.regressed, "15% p99 increase should regress at 5% threshold");
        assert!(
            (cmp.p99_latency_change - 0.15).abs() < 0.01,
            "p99 change should be ~15%"
        );
    }

    #[test]
    fn compare_detects_throughput_regression() {
        let baseline = ScenarioResults {
            scenario: "test".into(),
            proxy: "praxis".into(),
            runs: vec![],
            median: Some(sample_result(0.010, 10_000.0)),
        };
        let current = ScenarioResults {
            scenario: "test".into(),
            proxy: "praxis".into(),
            runs: vec![],
            median: Some(sample_result(0.010, 9_000.0)),
        };

        let cmp = current.compare(&baseline, 0.05);
        assert!(cmp.regressed, "10% throughput drop should regress at 5% threshold");
        assert!(
            (cmp.throughput_change - (-0.10)).abs() < 0.01,
            "throughput change should be ~-10%"
        );
    }

    #[test]
    fn compare_no_regression_within_threshold() {
        let baseline = ScenarioResults {
            scenario: "test".into(),
            proxy: "praxis".into(),
            runs: vec![],
            median: Some(sample_result(0.010, 10_000.0)),
        };
        let current = ScenarioResults {
            scenario: "test".into(),
            proxy: "praxis".into(),
            runs: vec![],
            median: Some(sample_result(0.0103, 9_800.0)),
        };

        let cmp = current.compare(&baseline, 0.05);
        assert!(!cmp.regressed, "3% changes should not regress at 5% threshold");
        assert!(!cmp.improved, "3% changes should not count as improved");
    }

    #[test]
    fn compare_detects_improvement() {
        let baseline = ScenarioResults {
            scenario: "test".into(),
            proxy: "praxis".into(),
            runs: vec![],
            median: Some(sample_result(0.010, 10_000.0)),
        };
        let current = ScenarioResults {
            scenario: "test".into(),
            proxy: "praxis".into(),
            runs: vec![],
            median: Some(sample_result(0.008, 11_500.0)),
        };

        let cmp = current.compare(&baseline, 0.05);
        assert!(!cmp.regressed, "improvement should not be flagged as regression");
        assert!(
            cmp.improved,
            "20% latency drop + 15% throughput gain should flag as improved"
        );
    }

    #[test]
    fn compare_marginal_improvement_not_flagged() {
        let baseline = ScenarioResults {
            scenario: "test".into(),
            proxy: "praxis".into(),
            runs: vec![],
            median: Some(sample_result(0.010, 10_000.0)),
        };
        let current = ScenarioResults {
            scenario: "test".into(),
            proxy: "praxis".into(),
            runs: vec![],
            median: Some(sample_result(0.0097, 10_300.0)),
        };

        let cmp = current.compare(&baseline, 0.05);
        assert!(!cmp.regressed, "marginal change should not flag as regression");
        assert!(!cmp.improved, "3%/3% changes should not flag as improved at 5% bar");
    }

    #[test]
    fn compute_median_selects_middle_run() {
        let mut results = ScenarioResults {
            scenario: "test".into(),
            proxy: "praxis".into(),
            runs: vec![
                sample_result(0.020, 5000.0),
                sample_result(0.010, 10_000.0),
                sample_result(0.015, 7500.0),
            ],
            median: None,
        };
        results.compute_median();
        let median = results.median.as_ref().unwrap();
        assert!(
            (median.latency.p99 - 0.015).abs() < 1e-9,
            "median should select the middle p99"
        );
    }

    #[test]
    fn yaml_round_trip() {
        let results = ScenarioResults {
            scenario: "test".into(),
            proxy: "praxis".into(),
            runs: vec![sample_result(0.01, 10_000.0)],
            median: Some(sample_result(0.01, 10_000.0)),
        };

        let dir = std::env::temp_dir();
        let path = dir.join("praxis_bench_test.yaml");
        results.save_yaml(&path).unwrap();
        let loaded = ScenarioResults::load_yaml(&path).unwrap();

        assert_eq!(loaded.scenario, "test");
        assert_eq!(loaded.runs.len(), 1);
        assert!(loaded.median.is_some(), "loaded results should contain median");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn relative_change_positive() {
        let change = relative_change(1.1, 1.0);
        assert!(
            (change - 0.1).abs() < 1e-9,
            "10% increase should yield ~0.1, got {change}"
        );
    }

    #[test]
    fn relative_change_negative() {
        let change = relative_change(0.9, 1.0);
        assert!(
            (change - (-0.1)).abs() < 1e-9,
            "10% decrease should yield ~-0.1, got {change}"
        );
    }

    #[test]
    fn relative_change_zero_baseline_positive_current() {
        let change = relative_change(5.0, 0.0);
        assert!(
            change.is_infinite() && change.is_sign_positive(),
            "positive current with zero baseline should be +Infinity"
        );
    }

    #[test]
    fn relative_change_both_zero() {
        let change = relative_change(0.0, 0.0);
        assert!((change - 0.0).abs() < 1e-9, "both zero should yield 0.0, got {change}");
    }

    #[test]
    fn relative_change_no_change() {
        let change = relative_change(1.0, 1.0);
        assert!(change.abs() < 1e-9, "identical values should yield 0.0, got {change}");
    }

    #[test]
    fn extract_metrics_with_none() {
        let (p99, rps) = extract_metrics(&None);
        assert!((p99 - 0.0).abs() < 1e-9, "p99 should be 0.0 for None median");
        assert!((rps - 0.0).abs() < 1e-9, "rps should be 0.0 for None median");
    }

    #[test]
    fn extract_metrics_with_valid_result() {
        let result = sample_result(0.025, 5000.0);
        let (p99, rps) = extract_metrics(&Some(result));
        assert!((p99 - 0.025).abs() < 1e-9, "p99 should match result, got {p99}");
        assert!((rps - 5000.0).abs() < 1e-9, "rps should match result, got {rps}");
    }

    // ---------------------------------------------------------------------------
    // Test Utilities
    // ---------------------------------------------------------------------------

    /// Create a minimal [`BenchmarkResult`] for tests.
    fn sample_result(p99: f64, rps: f64) -> BenchmarkResult {
        BenchmarkResult {
            commit: "abc123".into(),
            timestamp: "2026-03-31T00:00:00Z".into(),
            scenario: "test".into(),
            proxy: "praxis".into(),
            tool: "vegeta".into(),
            environment: Environment {
                cpu: "test".into(),
                os: "linux".into(),
            },
            latency: LatencyMetrics {
                min: 0.001,
                max: 0.1,
                mean: 0.01,
                p50: 0.005,
                p90: 0.02,
                p95: 0.03,
                p99,
                p99_9: 0.09,
            },
            throughput: ThroughputMetrics {
                requests_per_sec: rps,
                bytes_per_sec: rps * 100.0,
            },
            resource: None,
            errors: ErrorMetrics {
                non_2xx: Some(0),
                timeouts: 0,
                connect_failures: 0,
            },
            raw_report: None,
        }
    }
}
