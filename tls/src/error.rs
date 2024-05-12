//! TLS error types.

use thiserror::Error;

// -----------------------------------------------------------------------------
// Error
// -----------------------------------------------------------------------------

/// Errors from TLS configuration validation.
///
/// ```
/// use praxis_tls::TlsError;
///
/// let e = TlsError::PathTraversal {
///     field: "cert_path".into(),
///     path: "/etc/../../tmp/evil.pem".into(),
/// };
/// assert!(e.to_string().contains("path traversal"));
/// ```
#[derive(Debug, Error)]

pub enum TlsError {
    /// A TLS path contains `..` (directory traversal).
    #[error("TLS {field} must not contain path traversal (..): {path}")]
    PathTraversal {
        /// Which field failed validation (e.g. "`cert_path`").
        field: String,
        /// The offending path value.
        path: String,
    },
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display() {
        let e = TlsError::PathTraversal {
            field: "key_path".into(),
            path: "../secret/key.pem".into(),
        };
        assert!(e.to_string().contains("path traversal"));
        assert!(e.to_string().contains("key_path"));
    }
}
