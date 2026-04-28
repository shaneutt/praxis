// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Path, host, and header matching logic for the router filter.

use std::collections::HashMap;

use http::HeaderMap;
use praxis_core::config::Route;

use super::ResolvedRoute;

// -----------------------------------------------------------------------------
// Route Matching
// -----------------------------------------------------------------------------

/// Check whether a resolved route matches the request path, host, and headers.
pub(super) fn route_matches_request(
    resolved: &ResolvedRoute,
    path: &str,
    host: Option<&str>,
    req_headers: &HeaderMap,
) -> bool {
    let route = &resolved.route;
    if !path.starts_with(&route.path_prefix) {
        return false;
    }
    let host_ok = match &route.host {
        Some(h) => host.is_some_and(|req_host| {
            let req_host = strip_port(req_host);
            host_matches(h, resolved.wildcard_suffix.as_deref(), req_host)
        }),
        None => true,
    };
    host_ok && headers_match(&route.headers, req_headers)
}

/// Update the best match if the current route has more constraints.
pub(super) fn update_best_match<'a>(
    best: Option<(usize, usize, &'a Route)>,
    route: &'a Route,
) -> Option<(usize, usize, &'a Route)> {
    let prefix_len = route.path_prefix.len();
    let constraints = usize::from(route.host.is_some()) + route.headers.as_ref().map_or(0, HashMap::len);
    let dominated = best.is_some_and(|(bp, bc, _)| (prefix_len, constraints) <= (bp, bc));
    if dominated {
        best
    } else {
        Some((prefix_len, constraints, route))
    }
}

/// Return `true` if shorter prefixes cannot improve on the current best.
pub(super) fn should_stop_early(best: Option<(usize, usize, &Route)>, route: &Route) -> bool {
    best.is_some_and(|(bp, ..)| route.path_prefix.len() < bp)
}

// -----------------------------------------------------------------------------
// Wildcard Host Matching
// -----------------------------------------------------------------------------

/// Check whether a request host matches a route host pattern.
///
/// When `wildcard_suffix` is `Some`, the pattern is a wildcard
/// (e.g. `*.example.com`) and `wildcard_suffix` holds the
/// pre-lowercased suffix (`.example.com`). Zero allocations.
fn host_matches(pattern: &str, wildcard_suffix: Option<&str>, host: &str) -> bool {
    if let Some(suffix) = wildcard_suffix {
        if host.len() <= suffix.len() {
            return false;
        }
        let host_suffix = &host[host.len() - suffix.len()..];
        if !host_suffix.eq_ignore_ascii_case(suffix) {
            return false;
        }
        let subdomain = &host[..host.len() - suffix.len()];
        !subdomain.is_empty() && !subdomain.contains('.')
    } else {
        host.eq_ignore_ascii_case(pattern)
    }
}

// -----------------------------------------------------------------------------
// Header Matching
// -----------------------------------------------------------------------------

/// Returns `true` if the request headers satisfy all route header constraints.
fn headers_match(required: &Option<HashMap<String, String>>, actual: &HeaderMap) -> bool {
    let Some(required) = required else {
        return true;
    };
    required.iter().all(|(key, val)| {
        actual
            .get_all(key.as_str())
            .iter()
            .any(|v| v.to_str().ok().is_some_and(|v| v == val))
    })
}

// -----------------------------------------------------------------------------
// Host Utilities
// -----------------------------------------------------------------------------

/// Strip the port from a host string, handling both IPv4 and bracketed IPv6.
fn strip_port(host: &str) -> &str {
    if host.starts_with('[') {
        match host.find(']') {
            Some(i) => &host[..=i],
            None => host,
        }
    } else {
        host.split(':').next().unwrap_or(host)
    }
}
