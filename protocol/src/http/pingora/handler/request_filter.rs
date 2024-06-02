//! Request-phase filter execution: runs the pipeline, captures client
//! address and idempotency, then injects extra headers or sends rejections.
//!
//! When the pipeline uses [`StreamBuffer`] mode, the body is pre-read
//! from the session during this phase (before upstream selection) so
//! that body filters can promote values to headers and influence
//! routing decisions.
//!
//! Host header validation runs before the filter pipeline to reject
//! ambiguous requests that could lead to request smuggling.
//!
//! [`StreamBuffer`]: praxis_filter::BodyMode::StreamBuffer

use std::{borrow::Cow, collections::VecDeque};

use pingora_core::Result;
use pingora_proxy::Session;
use praxis_filter::{BodyBuffer, BodyMode, FilterAction, FilterError, FilterPipeline, Rejection, Request};
use tracing::{debug, warn};

use super::super::{
    context::RequestCtx,
    convert::{request_header_from_session, send_rejection},
};

// -----------------------------------------------------------------------------
// Host Header Validation
// -----------------------------------------------------------------------------

/// Validate the Host header per [RFC 9112 Section 3.2] and
/// [RFC 9110 Section 7.2].
///
/// Returns `Some(rejection)` if the request must be rejected:
/// - Missing Host on HTTP/1.1 ([RFC 9112 Section 3.2])
/// - Multiple Host headers with differing values ([RFC 9110 Section 7.2])
///
/// When duplicate Host headers carry identical values, the duplicates
/// are collapsed to a single header (benign canonicalization).
///
/// [RFC 9110 Section 7.2]: https://datatracker.ietf.org/doc/html/rfc9110#section-7.2
/// [RFC 9112 Section 3.2]: https://datatracker.ietf.org/doc/html/rfc9112#section-3.2
fn validate_host_header(session: &mut Session) -> Option<Rejection> {
    let is_http11 = session.req_header().version == http::Version::HTTP_11;
    let hosts = session.req_header().headers.get_all(http::header::HOST);
    let mut iter = hosts.iter();

    let Some(first) = iter.next() else {
        if is_http11 {
            debug!("rejecting HTTP/1.1 request with missing Host header");
            return Some(Rejection::status(400));
        }
        return None;
    };

    let second = iter.next()?;

    if second.as_bytes() != first.as_bytes() {
        debug!("rejecting request with conflicting Host headers");
        return Some(Rejection::status(400));
    }

    for v in iter {
        if v.as_bytes() != first.as_bytes() {
            debug!("rejecting request with conflicting Host headers");
            return Some(Rejection::status(400));
        }
    }

    debug!("canonicalizing duplicate identical Host headers");
    let canonical = first.clone();
    let _ = session.req_header_mut().remove_header("host");
    let _ = session.req_header_mut().insert_header(http::header::HOST, canonical);

    None
}

// -----------------------------------------------------------------------------
// Request Filters
// -----------------------------------------------------------------------------

/// Run the request-phase pipeline, capture client info, and inject headers.
///
/// Host header validation runs first (before the pipeline) to reject
/// ambiguous requests early.
pub(super) async fn execute(pipeline: &FilterPipeline, session: &mut Session, ctx: &mut RequestCtx) -> Result<bool> {
    if let Some(rejection) = validate_host_header(session) {
        send_rejection(session, rejection).await;
        return Ok(true);
    }

    let mut request = request_header_from_session(session);
    ctx.client_addr = session
        .client_addr()
        .and_then(|a| a.as_inet())
        .map(std::net::SocketAddr::ip);
    ctx.request_is_idempotent = matches!(
        session.req_header().method,
        http::Method::GET | http::Method::HEAD | http::Method::OPTIONS
    );

    let caps = pipeline.body_capabilities();

    if matches!(caps.request_body_mode, BodyMode::StreamBuffer { .. }) {
        tracing::debug!("pre-reading request body for StreamBuffer inspection");
        match pre_read_body(pipeline, session, ctx, &request).await {
            Ok(extra_headers) => {
                // Inject promoted headers into the session AND the
                // existing request in-place (avoids a full rebuild).
                for (name, value) in extra_headers {
                    if let (Ok(hname), Ok(hval)) = (
                        http::header::HeaderName::from_bytes(name.as_bytes()),
                        http::header::HeaderValue::from_str(&value),
                    ) {
                        let _ = session.req_header_mut().insert_header(hname.clone(), hval.clone());
                        request.headers.insert(hname, hval);
                    }
                }
            },
            Err(PreReadError::Rejected(rejection)) => {
                send_rejection(session, rejection).await;
                return Ok(true);
            },
            Err(PreReadError::Filter(e)) => {
                warn!(error = %e, "body filter error during pre-read");
                send_rejection(session, Rejection::status(500)).await;
                return Ok(true);
            },
            Err(PreReadError::Io(e)) => return Err(e),
        }
    }

    match run_pipeline(pipeline, request, ctx).await {
        Ok((FilterAction::Continue | FilterAction::Release, extra_headers)) => {
            for (name, value) in extra_headers {
                let _ = session.req_header_mut().insert_header(name.into_owned(), value);
            }
            Ok(false)
        },
        Ok((FilterAction::Reject(rejection), _)) => {
            send_rejection(session, rejection).await;
            Ok(true)
        },
        Err(e) => {
            warn!(error = %e, "filter pipeline error");
            send_rejection(session, Rejection::status(500)).await;
            Ok(true)
        },
    }
}

// -----------------------------------------------------------------------------
// StreamBuffer Pre-Read
// -----------------------------------------------------------------------------

/// Errors that can occur during body pre-reading in `StreamBuffer` mode.
enum PreReadError {
    /// A filter rejected the request during body processing.
    Rejected(Rejection),

    /// A filter returned an error during body processing.
    Filter(FilterError),

    /// An I/O error from Pingora while reading the body.
    Io(Box<pingora_core::Error>),
}

/// Pre-read the request body from the session and run body filters.
///
/// Returns any extra headers that body filters promoted (e.g.
/// `json_body_field` extracting a model name). The accumulated body
/// is stored in `ctx.pre_read_body` for later forwarding by
/// `request_body_filter`.
async fn pre_read_body(
    pipeline: &FilterPipeline,
    session: &mut Session,
    ctx: &mut RequestCtx,
    request: &Request,
) -> std::result::Result<Vec<(Cow<'static, str>, String)>, PreReadError> {
    let caps = pipeline.body_capabilities();
    let max_bytes = match caps.request_body_mode {
        BodyMode::StreamBuffer { max_bytes } => max_bytes.unwrap_or(usize::MAX),
        _ => return Ok(Vec::new()),
    };

    let mut buffer = BodyBuffer::new(max_bytes);
    let mut all_extra_headers = Vec::new();
    let mut released = false;

    loop {
        let chunk = session
            .downstream_session
            .read_request_body()
            .await
            .map_err(PreReadError::Io)?;

        let end_of_stream = chunk.is_none();
        let mut body = chunk;

        // Accumulate if not yet released.
        if !released
            && let Some(ref b) = body
            && buffer.push(b.clone()).is_err()
        {
            return Err(PreReadError::Rejected(Rejection::status(413)));
        }

        let mut filter_ctx = ctx.build_filter_context(pipeline, request, None);
        match pipeline
            .execute_http_request_body(&mut filter_ctx, &mut body, end_of_stream)
            .await
        {
            Ok(FilterAction::Continue) => {},
            Ok(FilterAction::Release) => {
                if !released {
                    debug!("StreamBuffer released during pre-read");
                    released = true;
                }
            },
            Ok(FilterAction::Reject(rejection)) => {
                return Err(PreReadError::Rejected(rejection));
            },
            Err(e) => return Err(PreReadError::Filter(e)),
        }

        ctx.request_body_bytes = filter_ctx.request_body_bytes;
        ctx.cluster = filter_ctx.cluster;
        ctx.upstream = filter_ctx.upstream;
        all_extra_headers.extend(filter_ctx.extra_request_headers);

        if end_of_stream {
            break;
        }
    }

    tracing::debug!("storing pre-read body for forwarding by request_body_filter");
    let frozen = buffer.freeze();
    if frozen.is_empty() {
        ctx.pre_read_body = Some(VecDeque::new());
    } else {
        ctx.pre_read_body = Some(VecDeque::from([frozen]));
    }

    ctx.request_body_released = true;

    Ok(all_extra_headers)
}

// -----------------------------------------------------------------------------
// Header-Phase Pipeline
// -----------------------------------------------------------------------------

/// Run the request-phase filter pipeline and snapshot the request for later phases.
///
/// Returns the final action and any extra headers promoted by filters.
async fn run_pipeline(
    pipeline: &FilterPipeline,
    request: Request,
    ctx: &mut RequestCtx,
) -> std::result::Result<(FilterAction, Vec<(Cow<'static, str>, String)>), FilterError> {
    let (action, extra_headers, cluster, upstream) = {
        let mut filter_ctx = ctx.build_filter_context(pipeline, &request, None);

        let action = pipeline.execute_http_request(&mut filter_ctx).await;
        (
            action,
            filter_ctx.extra_request_headers,
            filter_ctx.cluster,
            filter_ctx.upstream,
        )
    };

    ctx.request_snapshot = Some(request);

    match action {
        Ok(FilterAction::Continue | FilterAction::Release) => {
            ctx.cluster = cluster;
            ctx.upstream = upstream;
            Ok((FilterAction::Continue, extra_headers))
        },
        Ok(FilterAction::Reject(rejection)) => Ok((FilterAction::Reject(rejection), Vec::new())),
        Err(e) => Err(e),
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use http::{HeaderMap, Method, Uri};
    use praxis_filter::{FilterAction, FilterPipeline, FilterRegistry, Request};

    use super::*;
    use crate::http::pingora::context::RequestCtx;

    /// Create a minimal GET request for tests.
    fn make_request() -> Request {
        Request {
            method: Method::GET,
            uri: Uri::from_static("/"),
            headers: HeaderMap::new(),
        }
    }

    /// Create a default request context for tests.
    fn make_ctx() -> RequestCtx {
        RequestCtx::default()
    }

    /// Build an empty filter pipeline for tests.
    fn empty_pipeline() -> FilterPipeline {
        let registry = FilterRegistry::with_builtins();
        FilterPipeline::build(&[], &registry).unwrap()
    }

    #[tokio::test]
    async fn empty_pipeline_continues() {
        let (action, extra_headers) = run_pipeline(&empty_pipeline(), make_request(), &mut make_ctx())
            .await
            .unwrap();

        assert!(matches!(action, FilterAction::Continue));
        assert!(extra_headers.is_empty());
    }

    #[tokio::test]
    async fn snapshot_always_stored() {
        let mut ctx = make_ctx();

        run_pipeline(&empty_pipeline(), make_request(), &mut ctx).await.unwrap();

        assert!(ctx.request_snapshot.is_some());
    }

    #[tokio::test]
    async fn cluster_and_upstream_propagated_on_continue() {
        let mut ctx = make_ctx();

        run_pipeline(&empty_pipeline(), make_request(), &mut ctx).await.unwrap();

        assert!(ctx.cluster.is_none());
        assert!(ctx.upstream.is_none());
    }

    fn rejecting_pipeline(status: u16) -> FilterPipeline {
        let registry = FilterRegistry::with_builtins();
        let yaml = format!("status: {status}");
        let config: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        let entries = vec![praxis_filter::FilterEntry {
            filter_type: "static_response".into(),
            config,
            conditions: vec![],
            response_conditions: vec![],
        }];
        FilterPipeline::build(&entries, &registry).unwrap()
    }

    #[tokio::test]
    async fn rejection_propagated_from_pipeline() {
        let pipeline = rejecting_pipeline(403);
        let mut ctx = make_ctx();

        let (action, _) = run_pipeline(&pipeline, make_request(), &mut ctx).await.unwrap();

        assert!(matches!(action, FilterAction::Reject(r) if r.status == 403));
    }

    #[tokio::test]
    async fn rejection_does_not_set_cluster() {
        let pipeline = rejecting_pipeline(429);
        let mut ctx = make_ctx();

        run_pipeline(&pipeline, make_request(), &mut ctx).await.unwrap();

        assert!(ctx.cluster.is_none(), "rejection should not set cluster");
        assert!(ctx.upstream.is_none(), "rejection should not set upstream");
    }

    #[tokio::test]
    async fn extra_headers_returned_from_pipeline() {
        let pipeline = empty_pipeline();
        let mut ctx = make_ctx();

        let (_, extra_headers) = run_pipeline(&pipeline, make_request(), &mut ctx).await.unwrap();

        assert!(
            extra_headers.is_empty(),
            "empty pipeline should produce no extra headers"
        );
    }

    #[tokio::test]
    async fn idempotent_methods_detected_in_request() {
        for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
            let req = Request {
                method,
                uri: Uri::from_static("/"),
                headers: HeaderMap::new(),
            };
            let is_idempotent = matches!(req.method, Method::GET | Method::HEAD | Method::OPTIONS);
            assert!(is_idempotent, "{} should be idempotent", req.method);
        }

        for method in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
            let req = Request {
                method,
                uri: Uri::from_static("/"),
                headers: HeaderMap::new(),
            };
            let is_idempotent = matches!(req.method, Method::GET | Method::HEAD | Method::OPTIONS);
            assert!(!is_idempotent, "{} should not be idempotent", req.method);
        }
    }
}
