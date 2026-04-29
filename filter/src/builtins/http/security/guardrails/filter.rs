// SPDX-License-Identifier: MIT
// Copyright (c) 2024 Shane Utt

//! [`GuardrailsFilter`] implementation and `HttpFilter` trait impl.

use async_trait::async_trait;
use bytes::Bytes;

use super::{
    config::{DEFAULT_MAX_BODY_BYTES, GuardrailsAction, GuardrailsConfig},
    rule::{CompiledRule, RuleTarget, parse_matcher, parse_target},
};
use crate::{
    FilterAction, FilterError, FilterResultSet, Rejection,
    body::{BodyAccess, BodyMode},
    factory::parse_filter_config,
    filter::{HttpFilter, HttpFilterContext},
};

// -----------------------------------------------------------------------------
// GuardrailsFilter
// -----------------------------------------------------------------------------

/// Rejects requests matching string or regex rules against headers and/or body content.
///
/// # YAML configuration
///
/// ```yaml
/// filter: guardrails
/// rules:
///   # Block requests from bad bots
///   - target: header
///     name: "User-Agent"
///     pattern: "bad-bot.*"
///   # Block SQL injection in body
///   - target: body
///     contains: "DROP TABLE"
///   # Require body to look like JSON (reject if NOT matching)
///   - target: body
///     pattern: "^\\{.*\\}$"
///     negate: true
/// ```
///
/// # Example
///
/// ```
/// use praxis_filter::GuardrailsFilter;
///
/// let yaml: serde_yaml::Value = serde_yaml::from_str(
///     r#"
/// rules:
///   - target: header
///     name: User-Agent
///     contains: bad-bot
/// "#,
/// )
/// .unwrap();
/// let filter = GuardrailsFilter::from_config(&yaml).unwrap();
/// assert_eq!(filter.name(), "guardrails");
/// ```
pub struct GuardrailsFilter {
    /// What to do when a rule matches.
    pub(super) action: GuardrailsAction,

    /// Compiled rules for per-request evaluation.
    pub(super) rules: Vec<CompiledRule>,

    /// Whether any rule targets the body (pre-computed at init).
    pub(super) needs_body: bool,
}

impl GuardrailsFilter {
    /// Create a guardrails filter from parsed YAML config.
    ///
    /// Compiles all regex patterns at init time. Returns an error
    /// if a rule has an invalid regex, missing fields, or unknown
    /// target.
    ///
    /// ```
    /// use praxis_filter::GuardrailsFilter;
    ///
    /// let yaml: serde_yaml::Value = serde_yaml::from_str(
    ///     r#"
    /// rules:
    ///   - target: body
    ///     pattern: "SELECT.*FROM"
    /// "#,
    /// )
    /// .unwrap();
    /// let filter = GuardrailsFilter::from_config(&yaml).unwrap();
    /// assert_eq!(filter.name(), "guardrails");
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`FilterError`] if rules are empty or contain invalid regex.
    ///
    /// [`FilterError`]: crate::FilterError
    pub fn from_config(config: &serde_yaml::Value) -> Result<Box<dyn HttpFilter>, FilterError> {
        let cfg: GuardrailsConfig = parse_filter_config("guardrails", config)?;

        if cfg.rules.is_empty() {
            return Err("guardrails: 'rules' must not be empty".into());
        }

        let mut rules = Vec::with_capacity(cfg.rules.len());
        let mut needs_body = false;

        for rule in &cfg.rules {
            let target = parse_target(rule)?;
            let matcher = parse_matcher(rule)?;

            if matches!(target, RuleTarget::Body) {
                needs_body = true;
            }

            rules.push(CompiledRule {
                target,
                matcher,
                negate: rule.negate,
            });
        }

        Ok(Box::new(Self {
            action: cfg.action,
            rules,
            needs_body,
        }))
    }

    /// Return the appropriate [`FilterAction`] when a rule matches.
    fn blocked_action(&self) -> FilterAction {
        match self.action {
            GuardrailsAction::Reject => unauthorized(),
            GuardrailsAction::Flag => FilterAction::Continue,
        }
    }

    /// Check all header-targeted rules against the request headers.
    fn check_headers(&self, ctx: &HttpFilterContext<'_>) -> bool {
        for rule in &self.rules {
            let RuleTarget::Header(ref header_name) = rule.target else {
                continue;
            };

            let matched = ctx
                .request
                .headers
                .get(header_name.as_str())
                .and_then(|val| val.to_str().ok())
                .is_some_and(|s| rule.matches(s));

            let triggered = if rule.negate { !matched } else { matched };

            if triggered {
                tracing::info!(
                    header = %header_name,
                    negate = rule.negate,
                    "guardrails: header rule triggered, rejecting"
                );
                return true;
            }
        }
        false
    }

    /// Check all body-targeted rules against the request body.
    fn check_body(&self, body: &str) -> bool {
        for rule in &self.rules {
            if !matches!(rule.target, RuleTarget::Body) {
                continue;
            }

            let matched = rule.matches(body);
            let triggered = if rule.negate { !matched } else { matched };

            if triggered {
                tracing::info!(negate = rule.negate, "guardrails: body rule triggered, rejecting");
                return true;
            }
        }
        false
    }
}

#[async_trait]
impl HttpFilter for GuardrailsFilter {
    fn name(&self) -> &'static str {
        "guardrails"
    }

    fn request_body_access(&self) -> BodyAccess {
        if self.needs_body {
            BodyAccess::ReadOnly
        } else {
            BodyAccess::None
        }
    }

    fn request_body_mode(&self) -> BodyMode {
        if self.needs_body {
            BodyMode::StreamBuffer {
                max_bytes: Some(DEFAULT_MAX_BODY_BYTES),
            }
        } else {
            BodyMode::Stream
        }
    }

    async fn on_request(&self, ctx: &mut HttpFilterContext<'_>) -> Result<FilterAction, FilterError> {
        if self.check_headers(ctx) {
            write_result(ctx, "blocked");
            return Ok(self.blocked_action());
        }

        if !self.needs_body {
            write_result(ctx, "passed");
        }

        Ok(FilterAction::Continue)
    }

    async fn on_request_body(
        &self,
        ctx: &mut HttpFilterContext<'_>,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
    ) -> Result<FilterAction, FilterError> {
        if !end_of_stream {
            return Ok(FilterAction::Continue);
        }

        let Some(chunk) = body.as_ref() else {
            write_result(ctx, "passed");
            return Ok(FilterAction::Continue);
        };

        let text = String::from_utf8_lossy(chunk);
        if self.check_body(&text) {
            write_result(ctx, "blocked");
            return Ok(self.blocked_action());
        }

        write_result(ctx, "passed");
        Ok(FilterAction::Continue)
    }
}

// -----------------------------------------------------------------------------
// Utility Functions
// -----------------------------------------------------------------------------

/// Write a guardrails status result to the filter context.
fn write_result(ctx: &mut HttpFilterContext<'_>, status: &'static str) {
    let mut rs = FilterResultSet::new();
    if let Err(e) = rs.set("status", status) {
        tracing::warn!(error = %e, "failed to write guardrails result");
        return;
    }
    ctx.filter_results.insert("guardrails", rs);
    tracing::debug!(status, "guardrails result written");
}

/// Rejection response for guardrails violations.
fn unauthorized() -> FilterAction {
    FilterAction::Reject(Rejection::status(401).with_body(b"Unauthorized".as_slice()))
}
