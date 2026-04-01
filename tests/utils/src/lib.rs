#![deny(unsafe_code)]

//! Shared test utilities for the Praxis workspace.
//!
//! Provides mock backends, HTTP client helpers, network
//! utilities, and proxy startup functions for integration
//! and other test suites.

pub mod backend;
pub mod http_client;
pub mod network;
pub mod proxy;

pub use backend::{
    Backend, RoutedBackend, start_backend, start_echo_backend, start_header_echo_backend, start_slow_backend,
};
pub use http_client::{decode_chunked, http_get, http_post, http_send, parse_body, parse_header, parse_status};
pub use network::{PortGuard, free_port, free_port_guard, wait_for_tcp};
pub use proxy::{
    build_pipeline, custom_filter_yaml, registry_with, simple_proxy_yaml, start_proxy, start_proxy_with_registry,
};
