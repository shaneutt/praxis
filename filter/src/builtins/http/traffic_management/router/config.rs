// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Deserialized YAML configuration types for the router filter.

use praxis_core::config::Route;
use serde::Deserialize;

// -----------------------------------------------------------------------------
// RouterConfig
// -----------------------------------------------------------------------------

/// Deserialization wrapper for the router's YAML config.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RouterConfig {
    /// Route table entries.
    #[serde(default)]
    pub routes: Vec<Route>,
}
