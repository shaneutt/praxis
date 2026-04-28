// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! HTTP payload processing filters: compression, JSON body field extraction, etc.

mod compression;
pub(crate) mod compression_config;
mod json_body_field;

pub use compression::CompressionFilter;
pub use json_body_field::JsonBodyFieldFilter;
