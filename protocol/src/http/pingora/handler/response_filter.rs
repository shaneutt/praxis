//! Response-phase filter execution: runs the pipeline on upstream response headers and syncs modifications.

use pingora_core::Result;
use praxis_filter::{FilterAction, FilterPipeline};
use tracing::warn;

use super::super::{context::RequestCtx, convert::response_header_from_pingora};

// -----------------------------------------------------------------------------
// Response Filters
// -----------------------------------------------------------------------------

/// Run the response-phase pipeline and sync header changes to Pingora.
///
/// Strips [RFC 9110] hop-by-hop headers from the upstream response
/// before the filter pipeline sees them, ensuring they are never
/// forwarded to the client.
///
/// [RFC 9110]: https://datatracker.ietf.org/doc/html/rfc9110
pub(super) async fn execute(
    pipeline: &FilterPipeline,
    upstream_response: &mut pingora_http::ResponseHeader,
    ctx: &mut RequestCtx,
) -> Result<()> {
    super::upstream_response::strip_hop_by_hop_response(upstream_response);

    let mut resp = response_header_from_pingora(upstream_response);

    // Mark the response phase as complete in ALL exit paths so
    // the `logging()` hook does not re-run response filters.
    ctx.response_phase_done = true;

    let (result, headers_modified) = {
        let mut fctx = ctx.filter_context_for(pipeline, Some(&mut resp)).ok_or_else(|| {
            pingora_core::Error::explain(
                pingora_core::ErrorType::InternalError,
                "request snapshot not set during response phase",
            )
        })?;
        let r = pipeline.execute_http_response(&mut fctx).await;
        (r, fctx.response_headers_modified)
    };

    match result {
        Ok(FilterAction::Continue | FilterAction::Release) => {
            if headers_modified {
                write_headers_to_pingora(upstream_response, resp.headers);
            } else {
                tracing::trace!("skipping header sync: no filter modified response");
            }
            Ok(())
        },
        Ok(FilterAction::Reject(rejection)) => {
            warn!(status = rejection.status, "filter rejected response");
            Err(pingora_core::Error::explain(
                pingora_core::ErrorType::HTTPStatus(rejection.status),
                "response rejected by filter pipeline",
            ))
        },
        Err(e) => {
            warn!(error = %e, "filter pipeline error during response");
            Err(pingora_core::Error::explain(
                pingora_core::ErrorType::InternalError,
                format!("response filter error: {e}"),
            ))
        },
    }
}

/// Write a [`HeaderMap`] into a Pingora response via its insert API.
///
/// Uses Pingora's `insert_header` to maintain internal header name
/// tracking. Clears existing headers first so the result is an
/// exact replacement.
///
/// [`HeaderMap`]: http::HeaderMap
fn write_headers_to_pingora(resp: &mut pingora_http::ResponseHeader, headers: http::HeaderMap) {
    let stale: Vec<http::header::HeaderName> = resp.headers.keys().cloned().collect();
    for name in &stale {
        let _ = resp.remove_header(name.as_str());
    }
    for (name, value) in headers {
        if let Some(name) = name {
            let _ = resp.insert_header(name, value);
        }
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use praxis_filter::{FilterPipeline, FilterRegistry, Request};

    use super::*;
    use crate::http::pingora::context::RequestCtx;

    /// Build an empty filter pipeline for tests.
    fn make_pipeline() -> FilterPipeline {
        let registry = FilterRegistry::with_builtins();
        FilterPipeline::build(&[], &registry).unwrap()
    }

    /// Create a request context with a GET snapshot for tests.
    fn make_ctx() -> RequestCtx {
        RequestCtx {
            request_snapshot: Some(Request {
                method: http::Method::GET,
                uri: http::Uri::from_static("/"),
                headers: http::HeaderMap::new(),
            }),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn empty_pipeline_passes_through() {
        let pipeline = make_pipeline();
        let mut upstream_response = pingora_http::ResponseHeader::build(200, None).unwrap();
        let mut ctx = make_ctx();

        let result = execute(&pipeline, &mut upstream_response, &mut ctx).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn response_status_preserved() {
        let pipeline = make_pipeline();
        let mut upstream_response = pingora_http::ResponseHeader::build(404, None).unwrap();
        let mut ctx = make_ctx();

        execute(&pipeline, &mut upstream_response, &mut ctx).await.unwrap();

        assert_eq!(upstream_response.status, 404);
    }

    #[tokio::test]
    async fn unmodified_headers_skip_sync() {
        let pipeline = make_pipeline();
        let mut upstream_response = pingora_http::ResponseHeader::build(200, Some(2)).unwrap();
        let _ = upstream_response.insert_header("x-original", "keep-me");
        let _ = upstream_response.insert_header("content-type", "text/plain");
        let mut ctx = make_ctx();

        execute(&pipeline, &mut upstream_response, &mut ctx).await.unwrap();

        assert_eq!(upstream_response.headers.get("x-original").unwrap(), "keep-me");
        assert_eq!(upstream_response.headers.get("content-type").unwrap(), "text/plain");
        assert_eq!(upstream_response.headers.len(), 2);
    }
}
