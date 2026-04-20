// SPDX-License-Identifier: MIT
// Copyright (c) 2024 Shane Utt

//! Ordering validation checks for filter pipelines.

use praxis_core::config::FilterEntry;
use tracing::warn;

use super::ConditionalFilter;

// -----------------------------------------------------------------------------
// Constants
// -----------------------------------------------------------------------------

/// Filters classified as security-critical (bypass risk when conditional).
const SECURITY_FILTERS: &[&str] = &["ip_acl", "forwarded_headers"];

/// Filters that rewrite the request path.
const REWRITE_FILTERS: &[&str] = &["path_rewrite", "url_rewrite"];

// -----------------------------------------------------------------------------
// Error Checks
// -----------------------------------------------------------------------------

/// LB without a preceding router.
#[allow(clippy::indexing_slicing, reason = "position() is within bounds")]
pub(super) fn check_lb_without_router(names: &[&str], errors: &mut Vec<String>) {
    if let Some(lb_pos) = names.iter().position(|n| *n == "load_balancer") {
        let has_router_before = names[..lb_pos].contains(&"router");
        if !has_router_before {
            errors.push(
                "load_balancer without a preceding router \
                 filter; requests will fail with \
                 'no cluster selected'"
                    .to_owned(),
            );
        }
    }
}

/// Unconditional `static_response` blocking subsequent filters.
#[allow(clippy::indexing_slicing, reason = "enumeration bounds")]
pub(super) fn check_unconditional_static_response(
    names: &[&str],
    filters: &[ConditionalFilter],
    errors: &mut Vec<String>,
) {
    for (i, name) in names.iter().enumerate() {
        if *name == "static_response" && i + 1 < names.len() {
            let (_, conditions, ..) = &filters[i];
            if conditions.is_empty() {
                errors.push(format!(
                    "unconditional static_response at \
                     position {i} makes subsequent filters \
                     unreachable: {}",
                    names[i + 1..].join(", ")
                ));
            }
        }
    }
}

/// Security filters with request conditions (bypass risk).
#[allow(clippy::indexing_slicing, reason = "enumeration bounds")]
pub(super) fn check_conditional_security(names: &[&str], filters: &[ConditionalFilter], errors: &mut Vec<String>) {
    for (i, name) in names.iter().enumerate() {
        if SECURITY_FILTERS.contains(name) {
            let (_, conditions, ..) = &filters[i];
            if !conditions.is_empty() {
                errors.push(format!(
                    "security filter '{name}' at position {i} has \
                     request conditions; it will be bypassed for \
                     non-matching requests"
                ));
            }
        }
    }
}

/// Duplicate router filters.
pub(super) fn check_duplicate_routers(names: &[&str], errors: &mut Vec<String>) {
    let router_count = names.iter().filter(|n| **n == "router").count();
    if router_count > 1 {
        errors.push(format!(
            "multiple router filters in chain ({router_count}); \
             only the last one's cluster selection will take effect"
        ));
    }
}

/// Duplicate `load_balancer` filters.
pub(super) fn check_duplicate_load_balancers(names: &[&str], errors: &mut Vec<String>) {
    let lb_count = names.iter().filter(|n| **n == "load_balancer").count();
    if lb_count > 1 {
        errors.push(format!(
            "multiple load_balancer filters in chain ({lb_count}); \
             only the last one's upstream selection will take effect"
        ));
    }
}

/// Cross-reference router cluster names against LB cluster names.
pub(super) fn check_misaligned_clusters(entries: &[FilterEntry], errors: &mut Vec<String>) {
    let router_clusters = super::clusters::extract_router_clusters(entries);
    let lb_clusters = super::clusters::extract_lb_clusters(entries);

    if router_clusters.is_empty() || lb_clusters.is_empty() {
        return;
    }

    for cluster in &router_clusters {
        if !lb_clusters.contains(cluster.as_str()) {
            errors.push(format!(
                "router routes to cluster '{cluster}' which is not \
                 defined in the load_balancer configuration"
            ));
        }
    }
}

/// Multiple path rewriting filters (`path_rewrite` / `url_rewrite`).
#[allow(clippy::indexing_slicing, reason = "checked before usage")]
pub(super) fn check_duplicate_rewrite_filters(names: &[&str], entries: &[FilterEntry], errors: &mut Vec<String>) {
    let rewrite_indices: Vec<usize> = names
        .iter()
        .enumerate()
        .filter(|(_, n)| REWRITE_FILTERS.contains(n))
        .map(|(i, _)| i)
        .collect();

    if rewrite_indices.len() < 2 {
        return;
    }

    let first_idx = rewrite_indices[0];
    let first_name = names[first_idx];

    for &idx in &rewrite_indices[1..] {
        let later_name = names[idx];
        let allows_override = has_allow_rewrite_override(entries, idx);

        if allows_override {
            warn!(
                first = first_name,
                later = later_name,
                "multiple rewrite filters: '{later_name}' will override '{first_name}' (allow_rewrite_override=true)"
            );
        } else {
            errors.push(format!(
                "multiple path rewriting filters in pipeline: both \
                 '{first_name}' and '{later_name}' write to \
                 rewritten_path. Set `allow_rewrite_override: true` \
                 on the later filter to allow this (last writer wins)"
            ));
        }
    }
}

// -----------------------------------------------------------------------------
// Warning Checks
// -----------------------------------------------------------------------------

/// Router without any following LB (requests will 502).
pub(super) fn check_router_without_lb(names: &[&str], warnings: &mut Vec<String>) {
    let has_router = names.contains(&"router");
    let has_lb = names.contains(&"load_balancer");
    if has_router && !has_lb {
        warnings.push(
            "router filter without a load_balancer; \
             routed requests will fail with 502"
                .to_owned(),
        );
    }
}

/// All routers conditional with no unconditional fallback.
#[allow(clippy::indexing_slicing, reason = "enumeration bounds")]
pub(super) fn check_all_routers_conditional(names: &[&str], filters: &[ConditionalFilter], warnings: &mut Vec<String>) {
    let router_indices: Vec<usize> = names
        .iter()
        .enumerate()
        .filter(|(_, n)| **n == "router")
        .map(|(i, _)| i)
        .collect();

    if router_indices.is_empty() {
        return;
    }

    let all_conditional = router_indices.iter().all(|&i| {
        let (_, conditions, ..) = &filters[i];
        !conditions.is_empty()
    });

    if all_conditional {
        warnings.push(
            "all router filters are conditional; requests \
             not matching any condition will have no route"
                .to_owned(),
        );
    }
}

// -----------------------------------------------------------------------------
// Utilities
// -----------------------------------------------------------------------------

/// Check whether the filter entry at `idx` has
/// `allow_rewrite_override: true` in its YAML config.
///
/// Pipeline indices correspond 1:1 with `entries` indices.
fn has_allow_rewrite_override(entries: &[FilterEntry], idx: usize) -> bool {
    entries
        .get(idx)
        .and_then(|e| e.config.get("allow_rewrite_override"))
        .and_then(serde_yaml::Value::as_bool)
        .unwrap_or(false)
}
