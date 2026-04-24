// SPDX-License-Identifier: MIT
// Copyright (c) 2024 Shane Utt

//! Integration test suite for Praxis.

mod adversarial;
mod body;
mod body_pipeline;
mod compression;
mod conditions;
mod cors;
mod downstream_read_timeout;
mod examples;
mod failure_mode;
mod filter_composition;
mod guardrails;
mod health_check;
mod ip_acl;
mod json_body_field;
mod path_rewrite;
mod payload_processing;
mod per_listener_pipeline;
mod rate_limit;
mod retry;
mod routing;
mod security;
mod sni_router;
mod tcp_access_log;
mod tls;
mod url_rewrite;
mod wildcard_routing;
