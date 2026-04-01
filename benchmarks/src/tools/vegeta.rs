//! Vegeta HTTP load generator wrapper.
//!
//! See: <https://github.com/tsenart/vegeta>

use std::time::Duration;

use serde::Deserialize;

use crate::{error::BenchmarkError, result::BenchmarkResult};

// -----------------------------------------------------------------------------
// Configuration
// -----------------------------------------------------------------------------

/// Configuration for a Vegeta load test.
#[derive(Debug, Clone)]
pub struct VegetaConfig {
    /// Target URL.
    pub target: String,

    /// Requests per second (constant rate).
    pub rate: u32,

    /// Test duration.
    pub duration: Duration,

    /// Number of workers.
    pub workers: u32,

    /// HTTP method (GET, POST, etc.).
    pub method: String,

    /// Optional request body.
    pub body: Option<Vec<u8>>,
}

// -----------------------------------------------------------------------------
// JSON Types
// -----------------------------------------------------------------------------

/// Vegeta JSON report: latencies section (nanoseconds).
#[derive(Debug, Deserialize)]
struct VegetaLatencies {
    /// Mean latency in nanoseconds.
    mean: u64,

    /// 50th percentile latency in nanoseconds.
    #[serde(rename = "50th")]
    p50: u64,

    /// 90th percentile latency in nanoseconds.
    #[serde(rename = "90th")]
    p90: u64,

    /// 95th percentile latency in nanoseconds.
    #[serde(rename = "95th")]
    p95: u64,

    /// 99th percentile latency in nanoseconds.
    #[serde(rename = "99th")]
    p99: u64,

    /// Maximum latency in nanoseconds.
    max: u64,

    /// Minimum latency in nanoseconds.
    min: u64,
}

/// Vegeta JSON report: bytes section.
#[derive(Debug, Deserialize)]
struct VegetaBytes {
    /// Total bytes.
    total: u64,
}

/// Top-level Vegeta JSON report structure.
///
/// Produced by `vegeta report --type=json`.
#[derive(Debug, Deserialize)]
struct VegetaReport {
    /// Latency percentiles (nanoseconds).
    latencies: VegetaLatencies,

    /// Incoming bytes (body only).
    bytes_in: VegetaBytes,

    /// Outgoing bytes (body only).
    bytes_out: VegetaBytes,

    /// Total number of requests sent.
    requests: u64,

    /// Actual request send rate (req/s).
    #[serde(default)]
    #[expect(dead_code)]
    rate: f64,

    /// Successful response rate (req/s).
    throughput: f64,

    /// Duration of the attack in nanoseconds.
    duration: u64,

    /// Fraction of successful (2xx) responses.
    success: f64,

    /// Map of status code to count.
    #[serde(default)]
    #[expect(dead_code)]
    status_codes: std::collections::HashMap<String, u64>,

    /// List of error strings.
    #[serde(default)]
    errors: Vec<String>,
}

// -----------------------------------------------------------------------------
// Execution
// -----------------------------------------------------------------------------

/// Run a Vegeta load test and return raw JSON output.
pub async fn run(config: &VegetaConfig) -> Result<String, BenchmarkError> {
    let dir = std::env::temp_dir().join("praxis-bench");
    std::fs::create_dir_all(&dir).ok();

    let target_spec = format!("{method} {url}\n", method = config.method, url = config.target,);
    let target_path = dir.join("vegeta-targets.txt");
    std::fs::write(&target_path, &target_spec).map_err(BenchmarkError::Io)?;

    let mut body_flag = String::new();
    if let Some(body) = &config.body {
        let body_path = dir.join("vegeta-body.bin");
        std::fs::write(&body_path, body).map_err(BenchmarkError::Io)?;
        body_flag = format!("-body {}", body_path.display());
    }

    let duration_secs = config.duration.as_secs();

    // rate=0 means max rate; Vegeta requires -max-workers
    // in that mode instead of -workers.
    let worker_flag = if config.rate == 0 {
        format!("-max-workers {}", config.workers)
    } else {
        format!("-workers {}", config.workers)
    };

    let pipeline = format!(
        "vegeta attack \
         -targets {targets} \
         -rate {rate} -duration {dur}s {worker_flag} \
         {body} \
         | vegeta report --type=json",
        targets = target_path.display(),
        rate = config.rate,
        dur = duration_secs,
        body = body_flag,
    );

    run_vegeta_pipeline(&pipeline).await
}

/// Execute a vegeta shell pipeline and return stdout.
pub(crate) async fn run_vegeta_pipeline(pipeline: &str) -> Result<String, BenchmarkError> {
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(pipeline)
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                BenchmarkError::ToolNotFound("vegeta".into())
            } else {
                BenchmarkError::Io(e)
            }
        })?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not found") || stderr.contains("No such file") {
            return Err(BenchmarkError::ToolNotFound("vegeta".into()));
        }
        return Err(BenchmarkError::ToolFailed {
            tool: "vegeta".into(),
            code,
            stderr: stderr.into_owned(),
        });
    }

    let json = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(json)
}

// -----------------------------------------------------------------------------
// Parsing
// -----------------------------------------------------------------------------

/// Parse Vegeta JSON report output into a [`BenchmarkResult`].
///
/// Populates latency and throughput metrics. Resource metrics
/// are zeroed (Vegeta does not report resource usage).
///
/// [`BenchmarkResult`]: crate::result::BenchmarkResult
pub fn parse(
    json: &str,
    scenario: &str,
    proxy: &str,
    commit: &str,
    include_raw: bool,
) -> Result<BenchmarkResult, BenchmarkError> {
    let report: VegetaReport = serde_json::from_str(json).map_err(|e| BenchmarkError::ParseError {
        tool: "vegeta".into(),
        reason: e.to_string(),
    })?;

    let raw_report = if include_raw {
        serde_json::from_str(json).ok()
    } else {
        None
    };

    let total_requests = report.requests;
    let non_2xx = if total_requests > 0 {
        let success_count = (report.success * total_requests as f64).round() as u64;
        total_requests.saturating_sub(success_count)
    } else {
        0
    };

    let duration_secs = report.duration as f64 / 1_000_000_000.0;
    let total_bytes = report.bytes_in.total + report.bytes_out.total;
    let bytes_per_sec = if duration_secs > 0.0 {
        total_bytes as f64 / duration_secs
    } else {
        0.0
    };

    let timeout_count = report
        .errors
        .iter()
        .filter(|e| e.contains("timeout") || e.contains("deadline exceeded"))
        .count() as u64;
    let connect_failures = report
        .errors
        .iter()
        .filter(|e| e.contains("connection refused") || e.contains("connect:") || e.contains("dial"))
        .count() as u64;

    Ok(BenchmarkResult {
        commit: commit.into(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        scenario: scenario.into(),
        proxy: proxy.into(),
        tool: "vegeta".into(),
        environment: crate::result::current_environment(),
        latency: crate::result::LatencyMetrics {
            min: ns_to_secs(report.latencies.min),
            max: ns_to_secs(report.latencies.max),
            mean: ns_to_secs(report.latencies.mean),
            p50: ns_to_secs(report.latencies.p50),
            p90: ns_to_secs(report.latencies.p90),
            p95: ns_to_secs(report.latencies.p95),
            p99: ns_to_secs(report.latencies.p99),
            // Vegeta does not report p99.9; approximate with p99.
            p99_9: ns_to_secs(report.latencies.p99),
        },
        throughput: crate::result::ThroughputMetrics {
            requests_per_sec: report.throughput,
            bytes_per_sec,
        },
        resource: None,
        errors: crate::result::ErrorMetrics {
            non_2xx: Some(non_2xx),
            timeouts: timeout_count,
            connect_failures,
        },
        raw_report,
    })
}

/// Convert nanoseconds to seconds.
fn ns_to_secs(ns: u64) -> f64 {
    ns as f64 / 1_000_000_000.0
}
