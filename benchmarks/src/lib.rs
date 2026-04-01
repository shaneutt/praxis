#![deny(unsafe_code)]
#![deny(unreachable_pub)]

//! Benchmark tool and library for the Praxis proxy.
//!
//! `benchmarks` orchestrates external load generators
//! ([Vegeta], [Fortio]) to benchmark proxy servers,
//! collect structured results, and produce comparison reports.
//!
//! This crate is both a library (used by the `xtask benchmark`
//! command) and a standalone benchmark suite (criterion benches).
//!
//! # Architecture
//!
//! - [`proxy::ProxyConfig`] trait: pluggable proxy definitions
//! - [`scenario::Scenario`]: configures which proxies and
//!   workloads to run
//! - [`result::ScenarioResults`]: collected metrics with JSON
//!   serialization, YAML reports, and comparison support
//! - [`runner::Runner`]: orchestrates warmup, measurement,
//!   multiple runs, and median reporting
//! - [`tools`]: wrappers around external load generators
//!
//! [Vegeta]: https://github.com/tsenart/vegeta
//! [Fortio]: https://fortio.org/

/// Error types for benchmark operations.
pub mod error;
/// Proxy configuration trait and built-in implementations.
pub mod proxy;
/// Top-level benchmark report type.
pub mod report;
/// Benchmark result types and comparison logic.
pub mod result;
/// Runner orchestration (warmup, measurement, repetition).
pub mod runner;
/// Scenario definition and configuration.
pub mod scenario;
/// External load generator tool wrappers.
pub mod tools;
