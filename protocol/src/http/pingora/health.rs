//! Admin health-check HTTP service (`/ready`, `/healthy`).

use async_trait::async_trait;
use http::Response;
use pingora_core::{
    apps::http_app::ServeHttp, protocols::http::ServerSession, server::Server, services::listening::Service,
};
use praxis_core::health::HealthRegistry;
use tracing::info;

use super::json::json_response;

// -----------------------------------------------------------------------------
// JSON Escaping
// -----------------------------------------------------------------------------

/// Escape `\` and `"` for safe inclusion in a JSON string value.
///
/// ```
/// use praxis_protocol::http::pingora::health::escape_json_string;
///
/// assert_eq!(escape_json_string("simple"), "simple");
/// assert_eq!(escape_json_string(r#"a"b"#), r#"a\"b"#);
/// assert_eq!(escape_json_string(r"a\b"), r"a\\b");
/// ```
pub fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out
}

// -----------------------------------------------------------------------------
// HealthService
// -----------------------------------------------------------------------------

/// HTTP service for health check endpoints.
///
/// `/healthy` returns 200 once the server is accepting connections (liveness).
/// `/ready` returns cluster health details when a [`HealthRegistry`] is
/// present, or a simple `{"status":"ok"}` otherwise.
///
/// [`HealthRegistry`]: praxis_core::health::HealthRegistry
///
/// ```no_run
/// use praxis_protocol::http::pingora::health::HealthService;
///
/// let _svc = HealthService::new(None);
/// ```
pub struct HealthService {
    /// Shared health registry for per-cluster status reporting.
    registry: Option<HealthRegistry>,
}

impl HealthService {
    /// Create a health service with an optional health registry.
    ///
    /// ```
    /// use praxis_protocol::http::pingora::health::HealthService;
    ///
    /// let svc = HealthService::new(None);
    /// assert_eq!(svc.ready_response().0, 200);
    /// ```
    pub fn new(registry: Option<HealthRegistry>) -> Self {
        Self { registry }
    }

    /// Build the `/ready` response status and body.
    ///
    /// When a health registry is present, returns per-cluster
    /// counts and 503 if any cluster has zero healthy endpoints.
    ///
    /// ```
    /// use praxis_protocol::http::pingora::health::HealthService;
    ///
    /// let svc = HealthService::new(None);
    /// let (status, body) = svc.ready_response();
    /// assert_eq!(status, 200);
    /// assert!(body.contains("ok"));
    /// ```
    pub fn ready_response(&self) -> (u16, String) {
        let Some(ref registry) = self.registry else {
            return (200, r#"{"status":"ok"}"#.to_owned());
        };

        if registry.is_empty() {
            return (200, r#"{"status":"ok","clusters":{}}"#.to_owned());
        }

        let mut clusters_json = String::from("{");
        let mut any_down = false;
        let mut first = true;

        for (name, state) in registry.iter() {
            let healthy = state.iter().filter(|ep| ep.is_healthy()).count();
            let total = state.len();
            let unhealthy = total - healthy;

            if healthy == 0 {
                any_down = true;
            }

            if !first {
                clusters_json.push(',');
            }
            first = false;
            let escaped = escape_json_string(name);
            clusters_json.push_str(&format!(
                r#""{escaped}":{{"healthy":{healthy},"unhealthy":{unhealthy},"total":{total}}}"#,
            ));
        }
        clusters_json.push('}');

        let status_str = if any_down { "degraded" } else { "ok" };
        let status_code = if any_down { 503 } else { 200 };

        let body = format!(r#"{{"status":"{status_str}","clusters":{clusters_json}}}"#);

        (status_code, body)
    }
}

/// Add the health check endpoints to a Pingora server.
///
/// Binds a [`HealthService`] to `admin_addr`, exposing `/ready` and `/healthy` endpoints.
///
/// ```no_run
/// use pingora_core::server::Server;
/// use praxis_protocol::http::pingora::health::add_health_endpoint_to_pingora_server;
///
/// let mut server = Server::new(None).unwrap();
/// server.bootstrap();
/// add_health_endpoint_to_pingora_server(&mut server, "127.0.0.1:9090", None);
/// ```
pub fn add_health_endpoint_to_pingora_server(server: &mut Server, admin_addr: &str, registry: Option<HealthRegistry>) {
    let mut health_service = Service::new("health".to_owned(), HealthService::new(registry));
    health_service.add_tcp(admin_addr);
    info!(address = %admin_addr, "health endpoints enabled");
    server.add_service(health_service);
}

#[async_trait]
impl ServeHttp for HealthService {
    async fn response(&self, http_session: &mut ServerSession) -> Response<Vec<u8>> {
        let path = http_session.req_header().uri.path().to_owned();

        match path.as_str() {
            "/healthy" => json_response(200, br#"{"status":"ok"}"#),
            "/ready" => {
                let (status, body) = self.ready_response();
                json_response(status, body.as_bytes())
            },
            _ => json_response(404, br#"{"error":"not found"}"#),
        }
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use praxis_core::health::EndpointHealth;

    use super::*;

    #[test]
    fn json_response_200() {
        let resp = json_response(200, b"{}");
        assert_eq!(resp.status(), 200, "status should be 200");
        assert_eq!(
            resp.headers()["Content-Type"],
            "application/json",
            "content-type should be JSON"
        );
        assert_eq!(resp.body(), b"{}", "body should match input");
    }

    #[test]
    fn json_response_404() {
        let resp = json_response(404, br#"{"error":"not found"}"#);
        assert_eq!(resp.status(), 404, "status should be 404");
        assert_eq!(resp.body(), br#"{"error":"not found"}"#, "body should match input");
    }

    #[test]
    fn json_response_content_type_is_application_json() {
        let resp = json_response(503, b"{}");
        assert_eq!(
            resp.headers()["Content-Type"],
            "application/json",
            "content-type should be application/json"
        );
    }

    #[test]
    fn ready_no_registry_returns_200() {
        let svc = HealthService::new(None);
        let (status, body) = svc.ready_response();
        assert_eq!(status, 200, "no registry should return 200");
        assert!(body.contains("ok"), "body should contain ok");
    }

    #[test]
    fn ready_empty_registry_returns_200() {
        let registry: HealthRegistry = Arc::new(HashMap::new());
        let svc = HealthService::new(Some(registry));
        let (status, body) = svc.ready_response();
        assert_eq!(status, 200, "empty registry should return 200");
        assert!(body.contains("ok"), "body should contain ok");
        assert!(body.contains("clusters"), "body should contain clusters key");
    }

    #[test]
    fn ready_all_healthy_returns_200() {
        let mut map = HashMap::new();
        map.insert(
            Arc::from("backend"),
            Arc::new(vec![EndpointHealth::new(), EndpointHealth::new()]),
        );
        let registry: HealthRegistry = Arc::new(map);
        let svc = HealthService::new(Some(registry));
        let (status, body) = svc.ready_response();
        assert_eq!(status, 200, "all-healthy should return 200");
        assert!(body.contains(r#""healthy":2"#), "should report 2 healthy: {body}");
        assert!(body.contains(r#""unhealthy":0"#), "should report 0 unhealthy: {body}");
        assert!(body.contains(r#""total":2"#), "should report total 2: {body}");
    }

    #[test]
    fn ready_some_unhealthy_returns_200() {
        let mut map = HashMap::new();
        let eps = vec![EndpointHealth::new(), EndpointHealth::new()];
        eps[1].mark_unhealthy();
        map.insert(Arc::from("backend"), Arc::new(eps));
        let registry: HealthRegistry = Arc::new(map);
        let svc = HealthService::new(Some(registry));
        let (status, body) = svc.ready_response();
        assert_eq!(status, 200, "partial healthy should return 200");
        assert!(body.contains(r#""healthy":1"#), "should report 1 healthy: {body}");
        assert!(body.contains(r#""unhealthy":1"#), "should report 1 unhealthy: {body}");
    }

    #[test]
    fn ready_all_unhealthy_returns_503() {
        let mut map = HashMap::new();
        let eps = vec![EndpointHealth::new()];
        eps[0].mark_unhealthy();
        map.insert(Arc::from("backend"), Arc::new(eps));
        let registry: HealthRegistry = Arc::new(map);
        let svc = HealthService::new(Some(registry));
        let (status, body) = svc.ready_response();
        assert_eq!(status, 503, "all-unhealthy should return 503");
        assert!(body.contains("degraded"), "status should be degraded: {body}");
        assert!(body.contains(r#""healthy":0"#), "should report 0 healthy: {body}");
    }

    #[test]
    fn ready_multiple_clusters_one_down_returns_503() {
        let mut map = HashMap::new();
        map.insert(Arc::from("good"), Arc::new(vec![EndpointHealth::new()]));
        let bad = vec![EndpointHealth::new()];
        bad[0].mark_unhealthy();
        map.insert(Arc::from("bad"), Arc::new(bad));
        let registry: HealthRegistry = Arc::new(map);
        let svc = HealthService::new(Some(registry));
        let (status, _body) = svc.ready_response();
        assert_eq!(status, 503, "any cluster with zero healthy should trigger 503");
    }

    #[test]
    fn ready_escapes_cluster_names_with_special_chars() {
        let mut map = HashMap::new();
        map.insert(Arc::from(r#"back"end"#), Arc::new(vec![EndpointHealth::new()]));
        let registry: HealthRegistry = Arc::new(map);
        let svc = HealthService::new(Some(registry));
        let (_status, body) = svc.ready_response();
        assert!(
            body.contains(r#"back\"end"#),
            "cluster name with quotes should be escaped: {body}"
        );
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&body);
        assert!(parsed.is_ok(), "output should be valid JSON: {body}");
    }

    #[test]
    fn escape_json_string_handles_backslash() {
        assert_eq!(escape_json_string(r"a\b"), r"a\\b", "backslash should be escaped");
    }

    #[test]
    fn escape_json_string_handles_quote() {
        assert_eq!(escape_json_string(r#"a"b"#), r#"a\"b"#, "quote should be escaped");
    }

    #[test]
    fn escape_json_string_noop_for_plain() {
        assert_eq!(
            escape_json_string("simple"),
            "simple",
            "plain string should pass through"
        );
    }
}
