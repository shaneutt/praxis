#![deny(unsafe_code)]
#![deny(unreachable_pub)]

//! TLS configuration types for the Praxis proxy.

mod config;
mod error;

pub use config::TlsConfig;
pub use error::TlsError;
