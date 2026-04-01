//! Fortio HTTP/TCP load generator wrapper.
//!
//! See: <https://fortio.org/>

use std::time::Duration;

use serde::Deserialize;

use crate::{error::BenchmarkError, result::BenchmarkResult};

// -----------------------------------------------------------------------------
// FortioConfig
// -----------------------------------------------------------------------------

/// Configuration for a Fortio load test.
#[derive(Debug, Clone)]
pub struct FortioConfig {
    /// Target URL or address.
    pub target: String,

    /// Protocol to test.
    pub protocol: FortioProtocol,

    /// Requests per second (0 = max rate).
    pub qps: u32,

    /// Test duration.
    pub duration: Duration,

    /// Number of connections.
    pub connections: u32,

    /// Disable catch-up behavior (open-loop mode).
    pub no_catchup: bool,

    /// Use HTTP/2 (h2c). Implies `-stdclient`.
    pub h2: bool,
}

/// Protocol for Fortio load test.
#[derive(Debug, Clone)]
pub enum FortioProtocol {
    /// HTTP load test.
    Http,

    /// TCP load test.
    Tcp,
}

// -----------------------------------------------------------------------------
// JSON Types
// -----------------------------------------------------------------------------

/// Fortio JSON: percentile entry within the histogram.
#[derive(Debug, Deserialize)]
struct FortioPercentile {
    /// Percentile value (0.0 to 100.0).
    #[serde(rename = "Percentile")]
    percentile: f64,

    /// Latency value in seconds.
    #[serde(rename = "Value")]
    value: f64,
}

/// Fortio JSON: duration histogram section.
#[derive(Debug, Deserialize)]
struct FortioDurationHistogram {
    /// Percentile buckets.
    #[serde(rename = "Percentiles", default)]
    percentiles: Vec<FortioPercentile>,

    /// Average latency in seconds.
    #[serde(rename = "Avg")]
    avg: f64,

    /// Minimum latency in seconds.
    #[serde(rename = "Min")]
    min: f64,

    /// Maximum latency in seconds.
    #[serde(rename = "Max")]
    max: f64,

    /// Total count of data points.
    #[serde(rename = "Count")]
    #[expect(dead_code)]
    count: u64,
}

/// Fortio JSON: top-level report structure.
#[derive(Debug, Deserialize)]
struct FortioReport {
    /// Duration histogram with percentile data.
    #[serde(rename = "DurationHistogram")]
    duration_histogram: FortioDurationHistogram,

    /// Actual queries per second achieved.
    #[serde(rename = "ActualQPS")]
    actual_qps: f64,

    /// Total bytes sent.
    #[serde(rename = "BytesSent", default)]
    bytes_sent: u64,

    /// Total bytes received.
    #[serde(rename = "BytesReceived", default)]
    bytes_received: u64,

    /// HTTP return codes (status code string to count).
    #[serde(rename = "RetCodes", default)]
    ret_codes: std::collections::HashMap<String, u64>,

    /// Actual test duration in nanoseconds (`time.Duration`).
    #[serde(rename = "ActualDuration", default)]
    actual_duration_ns: f64,
}

// -----------------------------------------------------------------------------
// Execution
// -----------------------------------------------------------------------------

/// Start Fortio's built-in echo server on the given port.
///
/// Returns the child process handle. The caller is
/// responsible for killing the child when done.
pub async fn start_echo_server(port: u16) -> Result<tokio::process::Child, BenchmarkError> {
    let child = tokio::process::Command::new("fortio")
        .args(["server", "-http-port", &port.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                BenchmarkError::ToolNotFound("fortio".into())
            } else {
                BenchmarkError::Io(e)
            }
        })?;

    Ok(child)
}

/// Run a Fortio load test and return raw JSON output.
///
/// Invokes `fortio load` with `-json -` to emit JSON to
/// stdout.
pub async fn run(config: &FortioConfig) -> Result<String, BenchmarkError> {
    let duration_secs = config.duration.as_secs();
    let mut args = vec![
        "load".to_owned(),
        "-json".to_owned(),
        "-".to_owned(),
        "-qps".to_owned(),
        config.qps.to_string(),
        "-c".to_owned(),
        config.connections.to_string(),
        "-t".to_owned(),
        format!("{duration_secs}s"),
    ];

    if config.no_catchup {
        args.push("-nocatchup".to_owned());
    }

    if config.h2 {
        args.push("-h2".to_owned());
    }

    // Fortio uses tcp:// prefix for TCP targets.
    let target = match config.protocol {
        FortioProtocol::Http => config.target.clone(),
        FortioProtocol::Tcp => {
            if config.target.starts_with("tcp://") {
                config.target.clone()
            } else {
                format!("tcp://{}", config.target)
            }
        },
    };
    args.push(target);

    let output = tokio::process::Command::new("fortio")
        .args(&args)
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                BenchmarkError::ToolNotFound("fortio".into())
            } else {
                BenchmarkError::Io(e)
            }
        })?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(BenchmarkError::ToolFailed {
            tool: "fortio".into(),
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

/// Look up a percentile value from Fortio's percentile list.
///
/// If an exact match exists (within 0.01 tolerance), returns
/// it. Otherwise linearly interpolates between the two
/// nearest surrounding percentiles.
fn lookup_percentile(percentiles: &[FortioPercentile], target: f64) -> f64 {
    // Exact match.
    if let Some(p) = percentiles.iter().find(|p| (p.percentile - target).abs() < 0.01) {
        return p.value;
    }

    // Interpolate between the two nearest surrounding values.
    let below = percentiles.iter().rev().find(|p| p.percentile < target);
    let above = percentiles.iter().find(|p| p.percentile > target);

    match (below, above) {
        (Some(lo), Some(hi)) => {
            let frac = (target - lo.percentile) / (hi.percentile - lo.percentile);
            lo.value + frac * (hi.value - lo.value)
        },
        (Some(lo), None) => lo.value,
        (None, Some(hi)) => hi.value,
        (None, None) => 0.0,
    }
}

/// Parse Fortio JSON output into a [`BenchmarkResult`].
///
/// [`BenchmarkResult`]: crate::result::BenchmarkResult
pub fn parse(
    json: &str,
    scenario: &str,
    proxy: &str,
    commit: &str,
    include_raw: bool,
) -> Result<BenchmarkResult, BenchmarkError> {
    let report: FortioReport = serde_json::from_str(json).map_err(|e| BenchmarkError::ParseError {
        tool: "fortio".into(),
        reason: e.to_string(),
    })?;

    let raw_report = if include_raw {
        serde_json::from_str(json).ok()
    } else {
        None
    };

    let hist = &report.duration_histogram;
    let pctiles = &hist.percentiles;

    let total_bytes = report.bytes_sent + report.bytes_received;
    let duration_secs = report.actual_duration_ns / 1_000_000_000.0;
    let bytes_per_sec = if duration_secs > 0.0 {
        total_bytes as f64 / duration_secs
    } else {
        0.0
    };

    // Count non-2xx HTTP responses. Omit for TCP.
    let is_http = report.ret_codes.keys().any(|c| c.parse::<u16>().is_ok());
    let non_2xx = if is_http {
        let count: u64 = report
            .ret_codes
            .iter()
            .filter(|(code, _)| code.parse::<u16>().is_ok_and(|c| !(200..300).contains(&c)))
            .map(|(_, count)| count)
            .sum();
        Some(count)
    } else {
        None
    };

    Ok(BenchmarkResult {
        commit: commit.into(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        scenario: scenario.into(),
        proxy: proxy.into(),
        tool: "fortio".into(),
        environment: crate::result::current_environment(),
        latency: crate::result::LatencyMetrics {
            min: hist.min,
            max: hist.max,
            mean: hist.avg,
            p50: lookup_percentile(pctiles, 50.0),
            p90: lookup_percentile(pctiles, 90.0),
            p95: lookup_percentile(pctiles, 95.0),
            p99: lookup_percentile(pctiles, 99.0),
            p99_9: lookup_percentile(pctiles, 99.9),
        },
        throughput: crate::result::ThroughputMetrics {
            requests_per_sec: report.actual_qps,
            bytes_per_sec,
        },
        resource: None,
        errors: crate::result::ErrorMetrics {
            non_2xx,
            timeouts: 0,
            connect_failures: 0,
        },
        raw_report,
    })
}
