// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Rewrite operations: `strip_prefix`, `add_prefix`, and regex replace.

use std::borrow::Cow;

use regex::{Regex, RegexBuilder};

use super::config::PathRewriteConfig;
use crate::FilterError;

// -----------------------------------------------------------------------------
// PathRewriteOp
// -----------------------------------------------------------------------------

/// Compiled path rewrite operation.
#[derive(Debug)]
pub(super) enum PathRewriteOp {
    /// Strip a leading prefix from the path.
    StripPrefix(String),

    /// Prepend a prefix to the path.
    AddPrefix(String),

    /// Regex replacement on the path.
    Replace {
        /// Compiled regex pattern.
        pattern: Regex,

        /// Replacement template.
        replacement: String,
    },
}

// -----------------------------------------------------------------------------
// Build Operation
// -----------------------------------------------------------------------------

/// Build a compiled operation from the deserialized config.
pub(super) fn build_op(cfg: PathRewriteConfig) -> Result<PathRewriteOp, FilterError> {
    let count =
        u8::from(cfg.strip_prefix.is_some()) + u8::from(cfg.add_prefix.is_some()) + u8::from(cfg.replace.is_some());

    if count == 0 {
        return Err("path_rewrite: exactly one of strip_prefix, add_prefix, or replace must be set".into());
    }
    if count > 1 {
        return Err("path_rewrite: only one of strip_prefix, add_prefix, or replace may be set".into());
    }

    if let Some(prefix) = cfg.strip_prefix {
        return Ok(PathRewriteOp::StripPrefix(prefix));
    }

    if let Some(prefix) = cfg.add_prefix {
        return Ok(PathRewriteOp::AddPrefix(prefix));
    }

    if let Some(replace) = cfg.replace {
        let pattern = RegexBuilder::new(&replace.pattern)
            .size_limit(1 << 20)
            .build()
            .map_err(|e| -> FilterError { format!("path_rewrite: invalid regex: {e}").into() })?;
        return Ok(PathRewriteOp::Replace {
            pattern,
            replacement: replace.replacement,
        });
    }

    Err("path_rewrite: no operation configured (expected strip_prefix, add_prefix, or replace)".into())
}

// -----------------------------------------------------------------------------
// Rewrite Logic
// -----------------------------------------------------------------------------

/// Apply the rewrite operation to a path, returning a borrowed path
/// when no rewrite occurs or an owned path when it does.
pub(super) fn rewrite_path<'a>(op: &PathRewriteOp, path: &'a str) -> Cow<'a, str> {
    match op {
        PathRewriteOp::StripPrefix(prefix) => strip_prefix(path, prefix),
        PathRewriteOp::AddPrefix(prefix) => add_prefix(path, prefix),
        PathRewriteOp::Replace { pattern, replacement } => {
            let result = pattern.replace(path, replacement.as_str());
            match result {
                Cow::Borrowed(_) => Cow::Borrowed(path),
                Cow::Owned(s) if s == path => Cow::Borrowed(path),
                Cow::Owned(s) => Cow::Owned(s),
            }
        },
    }
}

/// Strip a prefix from the path at a segment boundary.
///
/// The prefix matches only when what follows is `/` or end-of-path.
/// Returns [`Cow::Borrowed`] when the prefix does not match.
///
/// [`Cow::Borrowed`]: std::borrow::Cow::Borrowed
pub(super) fn strip_prefix<'a>(path: &'a str, prefix: &str) -> Cow<'a, str> {
    if let Some(rest) = path.strip_prefix(prefix) {
        if rest.is_empty() {
            Cow::Owned("/".to_owned())
        } else if rest.starts_with('/') {
            Cow::Owned(rest.to_owned())
        } else {
            Cow::Borrowed(path)
        }
    } else {
        Cow::Borrowed(path)
    }
}

/// Prepend a prefix to the path, avoiding double slashes.
///
/// Always produces a new string, so returns [`Cow::Owned`].
///
/// [`Cow::Owned`]: std::borrow::Cow::Owned
pub(super) fn add_prefix<'a>(path: &'a str, prefix: &str) -> Cow<'a, str> {
    let prefix = prefix.trim_end_matches('/');
    if path.starts_with('/') {
        Cow::Owned(format!("{prefix}{path}"))
    } else {
        Cow::Owned(format!("{prefix}/{path}"))
    }
}

/// Re-attach the query string to a rewritten path.
pub(super) fn append_query(path: &str, query: Option<&str>) -> String {
    match query {
        Some(q) => format!("{path}?{q}"),
        None => path.to_owned(),
    }
}
