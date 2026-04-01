//! Error types for benchmark operations.

/// Errors that can occur during benchmark execution.
#[derive(Debug, thiserror::Error)]
pub enum BenchmarkError {
    /// A required external tool is not installed.
    #[error("tool not found: {0}")]
    ToolNotFound(String),

    /// An external tool exited with a non-zero status.
    #[error("tool failed: {tool} exited with {code}")]
    ToolFailed {
        /// The tool that failed.
        tool: String,
        /// The exit code.
        code: i32,
        /// Stderr output.
        stderr: String,
    },

    /// Failed to parse tool output.
    #[error("failed to parse {tool} output: {reason}")]
    ParseError {
        /// The tool whose output could not be parsed.
        tool: String,
        /// What went wrong.
        reason: String,
    },

    /// I/O error during benchmark execution.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// YAML serialization/deserialization error.
    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),
}
