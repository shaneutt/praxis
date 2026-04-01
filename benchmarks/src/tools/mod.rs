//! External load generator tool wrappers.
//!
//! Each tool module wraps a CLI load generator, handles
//! invocation, output parsing, and conversion to
//! [`BenchmarkResult`].
//!
//! [`BenchmarkResult`]: crate::result::BenchmarkResult

/// Fortio HTTP/TCP load generator.
pub mod fortio;
/// Vegeta HTTP load generator (open-loop).
pub mod vegeta;
