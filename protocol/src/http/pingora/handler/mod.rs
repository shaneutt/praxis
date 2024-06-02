//! Pingora `ProxyHttp` implementation: the main HTTP reverse-proxy handler.
//!
//! Delegates each lifecycle hook (request, response, body, upstream selection) to focused submodules.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use bytes::Bytes;
use pingora_core::{
    Result,
    apps::HttpServerOptions,
    modules::http::{HttpModules, compression::ResponseCompressionBuilder},
    server::Server,
    services::listening::Service,
    upstreams::peer::HttpPeer,
};
use pingora_proxy::{ProxyHttp, Session, http_proxy};
use praxis_filter::{CompressionConfig, FilterPipeline};
use tracing::{debug, warn};

use super::context::RequestCtx;

/// Maximum number of upstream connection retries for idempotent requests.
const MAX_RETRIES: usize = 3;

/// Shared hop-by-hop header stripping logic.
mod hop_by_hop;
/// Request body filter hook.
mod request_body_filter;
/// Request filter hook.
mod request_filter;
/// Response body filter hook.
mod response_body_filter;
/// Response filter hook.
mod response_filter;
/// Upstream peer selection hook.
mod upstream_peer;
/// Upstream request transformation hook.
mod upstream_request;
/// Upstream response hop-by-hop stripping hook.
mod upstream_response;

// -----------------------------------------------------------------------------
// Shared Helpers
// -----------------------------------------------------------------------------

/// Apply compression settings from the pipeline config to the Pingora response.
fn adjust_compression(
    session: &mut Session,
    upstream_response: &pingora_http::ResponseHeader,
    compression: Option<&CompressionConfig>,
) {
    use pingora_core::{modules::http::compression::ResponseCompression, protocols::http::compression::Algorithm};

    let Some(cfg) = compression else {
        return;
    };

    let Some(module) = session.downstream_modules_ctx.get_mut::<ResponseCompression>() else {
        return;
    };

    let headers = &upstream_response.headers;

    if !cfg.should_compress(headers) {
        debug!("disabling compression: response does not qualify");
        module.adjust_level(0);
        return;
    }

    if !cfg.gzip_enabled {
        module.adjust_algorithm_level(Algorithm::Gzip, 0);
    } else if let Some(level) = cfg.gzip_level {
        module.adjust_algorithm_level(Algorithm::Gzip, level);
    }

    if !cfg.brotli_enabled {
        module.adjust_algorithm_level(Algorithm::Brotli, 0);
    } else if let Some(level) = cfg.brotli_level {
        module.adjust_algorithm_level(Algorithm::Brotli, level);
    }

    if !cfg.zstd_enabled {
        module.adjust_algorithm_level(Algorithm::Zstd, 0);
    } else if let Some(level) = cfg.zstd_level {
        module.adjust_algorithm_level(Algorithm::Zstd, level);
    }
}

/// Handle upstream connect failures with retry logic.
fn handle_connect_failure(ctx: &mut RequestCtx, e: Box<pingora_core::Error>) -> Box<pingora_core::Error> {
    if ctx.request_is_idempotent {
        if (ctx.retries as usize) < MAX_RETRIES {
            ctx.retries += 1;
            debug!(
                retries = ctx.retries,
                max = MAX_RETRIES,
                "retrying idempotent request after connect failure"
            );
            let mut e = e;
            e.set_retry(true);
            return e;
        }
        warn!(
            retries = ctx.retries,
            max = MAX_RETRIES,
            "retry limit reached for idempotent request"
        );
    }
    e
}

/// Run response filters during the logging phase if the
/// response phase never executed (upstream error, filter
/// rejection, etc.).
async fn logging_cleanup(pipeline: &FilterPipeline, ctx: &mut RequestCtx) {
    if !ctx.response_phase_done
        && let Some(mut filter_ctx) = ctx.filter_context_for(pipeline, None)
    {
        let _ = pipeline.execute_http_response(&mut filter_ctx).await;
    }
}

// -----------------------------------------------------------------------------
// HTTPHandler (with body hooks)
// -----------------------------------------------------------------------------

/// HTTP handler that overrides body filter hooks.
///
/// Used when the pipeline contains filters that declare
/// body access via [`BodyAccess`].
///
/// ```ignore
/// // Requires a `FilterPipeline` and Pingora server runtime.
/// use std::sync::Arc;
/// use praxis_protocol::http::pingora::handler::HTTPHandler;
///
/// let handler = HTTPHandler::new(Arc::new(pipeline));
/// ```
///
/// [`BodyAccess`]: praxis_filter::BodyAccess
pub struct HTTPHandler {
    /// Shared filter pipeline.
    pipeline: Arc<FilterPipeline>,

    /// Compression configuration, if enabled.
    compression: Option<CompressionConfig>,

    /// Per-listener downstream read timeout.
    downstream_read_timeout: Option<Duration>,
}

impl HTTPHandler {
    /// Create a handler with body filter support.
    fn new(pipeline: Arc<FilterPipeline>, downstream_read_timeout: Option<Duration>) -> Self {
        let compression = pipeline.compression_config().cloned();
        Self {
            pipeline,
            compression,
            downstream_read_timeout,
        }
    }
}

#[async_trait]
impl ProxyHttp for HTTPHandler {
    type CTX = RequestCtx;

    fn new_ctx(&self) -> Self::CTX {
        RequestCtx::default()
    }

    /// Registers Pingora's compression module when compression is
    /// configured. Otherwise skips module registration to avoid
    /// per-request `Box` allocation overhead.
    fn init_downstream_modules(&self, modules: &mut HttpModules) {
        if let Some(ref cfg) = self.compression {
            debug!(level = cfg.default_level, "registering compression module");
            modules.add_module(ResponseCompressionBuilder::enable(cfg.default_level));
        }
    }

    async fn early_request_filter(&self, session: &mut Session, _ctx: &mut Self::CTX) -> Result<()>
    where
        Self::CTX: Send + Sync,
    {
        if let Some(timeout) = self.downstream_read_timeout {
            debug!(
                timeout_ms = timeout.as_millis() as u64,
                "applying downstream read timeout"
            );
            session.set_read_timeout(Some(timeout));
        }
        Ok(())
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        request_filter::execute(&self.pipeline, session, ctx).await
    }

    async fn response_filter(
        &self,
        session: &mut Session,
        upstream_response: &mut pingora_http::ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()>
    where
        Self::CTX: Send + Sync,
    {
        let result = response_filter::execute(&self.pipeline, upstream_response, ctx).await;
        if result.is_ok() {
            adjust_compression(session, upstream_response, self.compression.as_ref());
        }
        result
    }

    async fn request_body_filter(
        &self,
        session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<()>
    where
        Self::CTX: Send + Sync,
    {
        request_body_filter::execute(&self.pipeline, session, body, end_of_stream, ctx).await
    }

    fn response_body_filter(
        &self,
        _session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<Option<Duration>>
    where
        Self::CTX: Send + Sync,
    {
        response_body_filter::execute(&self.pipeline, body, end_of_stream, ctx)
    }

    fn fail_to_connect(
        &self,
        _session: &mut Session,
        _peer: &HttpPeer,
        ctx: &mut Self::CTX,
        e: Box<pingora_core::Error>,
    ) -> Box<pingora_core::Error> {
        handle_connect_failure(ctx, e)
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        upstream_request: &mut pingora_http::RequestHeader,
        _ctx: &mut Self::CTX,
    ) -> Result<()>
    where
        Self::CTX: Send + Sync,
    {
        upstream_request::strip_hop_by_hop(upstream_request);
        Ok(())
    }

    async fn upstream_peer(&self, _session: &mut Session, ctx: &mut Self::CTX) -> Result<Box<HttpPeer>> {
        upstream_peer::execute(ctx)
    }

    async fn logging(&self, _session: &mut Session, _e: Option<&pingora_core::Error>, ctx: &mut Self::CTX) {
        logging_cleanup(&self.pipeline, ctx).await;
    }
}

// -----------------------------------------------------------------------------
// HTTPHandlerNoBody (no body hooks)
// -----------------------------------------------------------------------------

/// HTTP handler that skips body filter hooks.
///
/// Used when no filter in the pipeline declares body
/// access. Pingora's default no-op body hooks forward
/// bytes with zero overhead, avoiding the cost of
/// building [`FilterContext`] on every chunk.
///
/// [`FilterContext`]: praxis_filter::FilterContext
pub struct HTTPHandlerNoBody {
    /// Shared filter pipeline.
    pipeline: Arc<FilterPipeline>,

    /// Compression configuration, if enabled.
    compression: Option<CompressionConfig>,

    /// Per-listener downstream read timeout.
    downstream_read_timeout: Option<Duration>,
}

impl HTTPHandlerNoBody {
    /// Create a handler without body filter support.
    fn new(pipeline: Arc<FilterPipeline>, downstream_read_timeout: Option<Duration>) -> Self {
        let compression = pipeline.compression_config().cloned();
        Self {
            pipeline,
            compression,
            downstream_read_timeout,
        }
    }
}

#[async_trait]
impl ProxyHttp for HTTPHandlerNoBody {
    type CTX = RequestCtx;

    fn new_ctx(&self) -> Self::CTX {
        RequestCtx::default()
    }

    fn init_downstream_modules(&self, modules: &mut HttpModules) {
        if let Some(ref cfg) = self.compression {
            debug!(level = cfg.default_level, "registering compression module");
            modules.add_module(ResponseCompressionBuilder::enable(cfg.default_level));
        }
    }

    async fn early_request_filter(&self, session: &mut Session, _ctx: &mut Self::CTX) -> Result<()>
    where
        Self::CTX: Send + Sync,
    {
        if let Some(timeout) = self.downstream_read_timeout {
            debug!(
                timeout_ms = timeout.as_millis() as u64,
                "applying downstream read timeout"
            );
            session.set_read_timeout(Some(timeout));
        }
        Ok(())
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        request_filter::execute(&self.pipeline, session, ctx).await
    }

    async fn response_filter(
        &self,
        session: &mut Session,
        upstream_response: &mut pingora_http::ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()>
    where
        Self::CTX: Send + Sync,
    {
        let result = response_filter::execute(&self.pipeline, upstream_response, ctx).await;
        if result.is_ok() {
            adjust_compression(session, upstream_response, self.compression.as_ref());
        }
        result
    }

    fn fail_to_connect(
        &self,
        _session: &mut Session,
        _peer: &HttpPeer,
        ctx: &mut Self::CTX,
        e: Box<pingora_core::Error>,
    ) -> Box<pingora_core::Error> {
        handle_connect_failure(ctx, e)
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        upstream_request: &mut pingora_http::RequestHeader,
        _ctx: &mut Self::CTX,
    ) -> Result<()>
    where
        Self::CTX: Send + Sync,
    {
        upstream_request::strip_hop_by_hop(upstream_request);
        Ok(())
    }

    async fn upstream_peer(&self, _session: &mut Session, ctx: &mut Self::CTX) -> Result<Box<HttpPeer>> {
        upstream_peer::execute(ctx)
    }

    async fn logging(&self, _session: &mut Session, _e: Option<&pingora_core::Error>, ctx: &mut Self::CTX) {
        logging_cleanup(&self.pipeline, ctx).await;
    }
}

// -----------------------------------------------------------------------------
// Load Handler
// -----------------------------------------------------------------------------

/// Load an HTTP handler for a single listener.
///
/// ```no_run
/// use std::sync::Arc;
/// use pingora_core::server::Server;
/// use praxis_core::config::Listener;
/// use praxis_filter::{FilterPipeline, FilterRegistry};
/// use praxis_protocol::http::pingora::handler::load_http_handler;
///
/// let mut server = Server::new(None).unwrap();
/// server.bootstrap();
/// let registry = FilterRegistry::with_builtins();
/// let pipeline = Arc::new(FilterPipeline::build(&[], &registry).unwrap());
/// let listener = Listener {
///     name: "http".into(),
///     address: "127.0.0.1:8080".into(),
///     protocol: Default::default(),
///     tls: None,
///     upstream: None,
///     filter_chains: vec![],
///     tcp_idle_timeout_ms: None,
///     downstream_read_timeout_ms: None,
/// };
/// load_http_handler(&mut server, &listener, pipeline).unwrap();
/// ```
pub fn load_http_handler(
    server: &mut Server,
    listener: &praxis_core::config::Listener,
    pipeline: Arc<FilterPipeline>,
) -> Result<(), praxis_core::ProxyError> {
    let service_name = format!("http-proxy:{}", listener.name);
    let downstream_read_timeout = listener.downstream_read_timeout_ms.map(Duration::from_millis);

    if pipeline.needs_body_filters() {
        debug!(listener = %listener.name, "loading HTTP handler with body filters");
        let handler = HTTPHandler::new(pipeline, downstream_read_timeout);
        let mut proxy = http_proxy(&server.configuration, handler);
        proxy.server_options = Some(h2c_server_options());
        let mut service = Service::new(service_name, proxy);
        super::listener::add_listener(&mut service, listener)?;
        server.add_service(service);
    } else {
        debug!(listener = %listener.name, "loading HTTP handler (no body filters)");
        let handler = HTTPHandlerNoBody::new(pipeline, downstream_read_timeout);
        let mut proxy = http_proxy(&server.configuration, handler);
        proxy.server_options = Some(h2c_server_options());
        let mut service = Service::new(service_name, proxy);
        super::listener::add_listener(&mut service, listener)?;
        server.add_service(service);
    }
    Ok(())
}

/// Build [`HttpServerOptions`] with h2c enabled.
///
/// [`HttpServerOptions`]: pingora_core::apps::HttpServerOptions
fn h2c_server_options() -> HttpServerOptions {
    let mut opts = HttpServerOptions::default();
    opts.h2c = true;
    opts
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn first_failure_idempotent_sets_retry() {
        let mut ctx = RequestCtx::default();
        ctx.request_is_idempotent = true;
        let e = handle_connect_failure(&mut ctx, make_error());
        assert!(e.retry(), "first failure should set retry flag");
        assert_eq!(ctx.retries, 1);
    }

    #[test]
    fn max_retries_exhausted_does_not_retry() {
        let mut ctx = RequestCtx::default();
        ctx.request_is_idempotent = true;
        ctx.retries = MAX_RETRIES as u32;
        let e = handle_connect_failure(&mut ctx, make_error());
        assert!(!e.retry(), "should not retry after MAX_RETRIES");
        assert_eq!(ctx.retries as usize, MAX_RETRIES);
    }

    #[test]
    fn counter_increments_across_calls() {
        let mut ctx = RequestCtx::default();
        ctx.request_is_idempotent = true;
        for expected in 1..=MAX_RETRIES {
            let _ = handle_connect_failure(&mut ctx, make_error());
            assert_eq!(ctx.retries as usize, expected);
        }
        let e = handle_connect_failure(&mut ctx, make_error());
        assert!(!e.retry(), "should not retry after reaching MAX_RETRIES");
        assert_eq!(ctx.retries as usize, MAX_RETRIES);
    }

    #[test]
    fn non_idempotent_request_never_retries() {
        let mut ctx = RequestCtx::default();
        ctx.request_is_idempotent = false;
        let e = handle_connect_failure(&mut ctx, make_error());
        assert!(!e.retry());
        assert_eq!(ctx.retries, 0);
    }

    #[tokio::test]
    async fn logging_cleanup_noop_when_response_phase_done() {
        let registry = praxis_filter::FilterRegistry::with_builtins();
        let pipeline = FilterPipeline::build(&[], &registry).unwrap();
        let mut ctx = RequestCtx::default();
        ctx.response_phase_done = true;
        ctx.request_snapshot = Some(praxis_filter::Request {
            method: http::Method::GET,
            uri: "/".parse().unwrap(),
            headers: http::HeaderMap::new(),
        });
        logging_cleanup(&pipeline, &mut ctx).await;
    }

    #[tokio::test]
    async fn logging_cleanup_noop_when_no_snapshot() {
        let registry = praxis_filter::FilterRegistry::with_builtins();
        let pipeline = FilterPipeline::build(&[], &registry).unwrap();
        let mut ctx = RequestCtx::default();
        ctx.response_phase_done = false;
        ctx.request_snapshot = None;
        logging_cleanup(&pipeline, &mut ctx).await;
    }

    #[tokio::test]
    async fn logging_cleanup_runs_response_pipeline_when_needed() {
        let registry = praxis_filter::FilterRegistry::with_builtins();
        let pipeline = FilterPipeline::build(&[], &registry).unwrap();
        let mut ctx = RequestCtx::default();
        ctx.response_phase_done = false;
        ctx.cluster = Some(Arc::from("test-cluster"));
        ctx.request_snapshot = Some(praxis_filter::Request {
            method: http::Method::GET,
            uri: "/test".parse().unwrap(),
            headers: http::HeaderMap::new(),
        });
        logging_cleanup(&pipeline, &mut ctx).await;
        assert!(ctx.cluster.is_none(), "cluster should be taken by logging_cleanup");
        assert!(ctx.upstream.is_none(), "upstream should be taken by logging_cleanup");
    }

    /// Create a connect error for tests.
    fn make_error() -> Box<pingora_core::Error> {
        pingora_core::Error::explain(pingora_core::ErrorType::ConnectError, "test connect failure")
    }
}
