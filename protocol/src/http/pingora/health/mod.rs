// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Health check infrastructure: admin endpoints, probes, and background runner.

/// Health check probe functions (HTTP and TCP).
pub mod probe;
/// Background health check runner.
pub mod runner;
/// Admin health-check HTTP service (`/ready`, `/healthy`).
mod service;

pub use service::{PingoraHealthService, add_health_endpoint_to_pingora_server};
