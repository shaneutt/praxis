// SPDX-License-Identifier: LGPL-3.0-only
// Copyright (c) 2024 Shane Utt

//! Server factory and lifecycle management.

/// Pingora-specific server factory and runtime.
pub mod pingora;
mod runtime;

pub use pingora::{PingoraServerRuntime, build_http_server};
pub use runtime::RuntimeOptions;
