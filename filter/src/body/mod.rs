// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Body access declarations, buffering, and capability computation.

mod access;
mod buffer;
mod builder;
mod mode;

pub use access::BodyAccess;
pub use buffer::{BodyBuffer, BodyBufferOverflow};
pub use builder::BodyCapabilities;
pub use mode::BodyMode;
