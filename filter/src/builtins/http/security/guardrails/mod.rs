// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Rejects requests matching string or regex guardrail rules.

mod config;
mod filter;
mod rule;

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::needless_raw_strings,
    reason = "tests"
)]
mod tests;

pub use self::{config::GuardrailsAction, filter::GuardrailsFilter};
