// SPDX-License-Identifier: LGPL-3.0-only
// Copyright (c) 2024 Shane Utt

//! Upstream connectivity types used by the filter pipeline and protocol layer.

mod connection_options;
mod network;
mod upstream;

pub use connection_options::ConnectionOptions;
pub use network::{CidrRange, normalize_mapped_ipv4};
pub use upstream::Upstream;
