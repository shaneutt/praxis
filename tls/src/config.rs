//! TLS certificate and key path configuration.

use std::path::{Component, Path};

use serde::{Deserialize, Deserializer, de};

use crate::TlsError;

// -----------------------------------------------------------------------------
// TlsConfig
// -----------------------------------------------------------------------------

/// TLS certificate and key paths.
///
/// Deserialization automatically validates that neither path
/// contains parent-directory (`..`) traversal components.
///
/// ```
/// use praxis_tls::TlsConfig;
///
/// let tls: TlsConfig = serde_yaml::from_str(r#"
/// cert_path: "/etc/ssl/cert.pem"
/// key_path: "/etc/ssl/key.pem"
/// "#).unwrap();
/// assert_eq!(tls.cert_path, "/etc/ssl/cert.pem");
/// assert_eq!(tls.key_path, "/etc/ssl/key.pem");
///
/// // Path traversal is rejected during deserialization:
/// let err = serde_yaml::from_str::<TlsConfig>(r#"
/// cert_path: "/etc/../../bad.pem"
/// key_path: "/etc/ssl/key.pem"
/// "#);
/// assert!(err.is_err());
/// ```
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// Path to the TLS certificate file.
    pub cert_path: String,

    /// Path to the TLS private key file.
    pub key_path: String,
}

/// Raw deserialization helper (no validation).
#[derive(Deserialize)]
struct TlsConfigRaw {
    /// Path to the TLS certificate file.
    cert_path: String,

    /// Path to the TLS private key file.
    key_path: String,
}

impl<'de> Deserialize<'de> for TlsConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = TlsConfigRaw::deserialize(deserializer)?;
        let config = Self {
            cert_path: raw.cert_path,
            key_path: raw.key_path,
        };
        config.validate().map_err(de::Error::custom)?;
        Ok(config)
    }
}

impl TlsConfig {
    /// Create a [`TlsConfig`] and validate it in one step.
    ///
    /// Returns [`TlsError::PathTraversal`] if either path contains `..`.
    ///
    /// ```
    /// use praxis_tls::TlsConfig;
    ///
    /// let tls = TlsConfig::new_validated("/etc/ssl/cert.pem", "/etc/ssl/key.pem").unwrap();
    /// assert_eq!(tls.cert_path, "/etc/ssl/cert.pem");
    ///
    /// let err = TlsConfig::new_validated("/etc/../../bad.pem", "/etc/ssl/key.pem").unwrap_err();
    /// assert!(err.to_string().contains("path traversal"));
    /// ```
    ///
    /// [`TlsConfig`]: crate::TlsConfig
    /// [`TlsError::PathTraversal`]: crate::TlsError::PathTraversal
    pub fn new_validated(cert_path: impl Into<String>, key_path: impl Into<String>) -> Result<Self, TlsError> {
        let config = Self {
            cert_path: cert_path.into(),
            key_path: key_path.into(),
        };
        config.validate()?;
        Ok(config)
    }

    /// Validate that neither path contains parent directory traversal.
    ///
    /// ```
    /// use praxis_tls::TlsConfig;
    ///
    /// // Dots within a filename component are allowed.
    /// let ok = TlsConfig::new_validated("/etc/ssl/my..cert.pem", "/etc/ssl/key.pem").unwrap();
    /// assert!(ok.validate().is_ok());
    ///
    /// // Deserialized configs that contain `..` components are rejected
    /// // at deserialization time (validate is called automatically).
    /// let result = serde_yaml::from_str::<TlsConfig>(r#"
    /// cert_path: "/etc/../../tmp/evil.pem"
    /// key_path: "/etc/ssl/key.pem"
    /// "#);
    /// assert!(result.is_err());
    /// ```
    pub fn validate(&self) -> Result<(), TlsError> {
        for (field, path) in [("cert_path", &self.cert_path), ("key_path", &self.key_path)] {
            if has_parent_dir_component(path) {
                return Err(TlsError::PathTraversal {
                    field: field.into(),
                    path: path.clone(),
                });
            }
        }
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Path Validation
// -----------------------------------------------------------------------------

/// Check whether a path string contains a [`Component::ParentDir`] (`..`).
///
/// [`Component::ParentDir`]: std::path::Component::ParentDir
fn has_parent_dir_component(path: &str) -> bool {
    Path::new(path).components().any(|c| matches!(c, Component::ParentDir))
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_paths_pass() {
        let tls = TlsConfig::new_validated("/etc/ssl/cert.pem", "/etc/ssl/key.pem").unwrap();
        assert_eq!(tls.cert_path, "/etc/ssl/cert.pem");
        assert_eq!(tls.key_path, "/etc/ssl/key.pem");
    }

    #[test]
    fn cert_path_traversal_rejected() {
        let err = TlsConfig::new_validated("/etc/../../tmp/evil.pem", "/etc/ssl/key.pem").unwrap_err();
        assert!(err.to_string().contains("cert_path"));
        assert!(err.to_string().contains("path traversal"));
    }

    #[test]
    fn key_path_traversal_rejected() {
        let err = TlsConfig::new_validated("/etc/ssl/cert.pem", "../secret/key.pem").unwrap_err();
        assert!(err.to_string().contains("key_path"));
        assert!(err.to_string().contains("path traversal"));
    }

    #[test]
    fn double_dots_in_filename_not_rejected() {
        let tls = TlsConfig::new_validated("/etc/ssl/my..cert.pem", "/etc/ssl/key..pem").unwrap();
        assert_eq!(tls.cert_path, "/etc/ssl/my..cert.pem");
        assert_eq!(tls.key_path, "/etc/ssl/key..pem");
    }

    #[test]
    fn validate_on_deserialized_config() {
        let tls: TlsConfig = serde_yaml::from_str("cert_path: /a\nkey_path: /b\n").unwrap();
        assert!(tls.validate().is_ok());
    }

    #[test]
    fn deserialize_rejects_traversal_automatically() {
        let result = serde_yaml::from_str::<TlsConfig>("cert_path: /a/../b\nkey_path: /c\n");
        assert!(result.is_err(), "deserialization should reject path traversal");
    }
}
