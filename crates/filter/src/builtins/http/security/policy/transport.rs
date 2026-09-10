// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 Praxis Contributors

//! Policy-engine HTTP over Praxis's shared sub-request connector.
//!
//! Destinations are resolved, checked against the policy egress rules,
//! and dialled by literal address within the caller's deadline. Calls use
//! HTTP/1.1, the platform trust store, and a policy-specific connection
//! pool partition.

use std::{net::SocketAddr, sync::OnceLock, time::Duration};

use async_trait::async_trait;
use pingora_core::upstreams::peer::HttpPeer;
use ppe::praxis_policy_core::{
    http::{
        DEFAULT_CONNECT_TIMEOUT, DEFAULT_MAX_RESPONSE_BYTES, HttpRequest, HttpResponse, HttpTransport,
        HttpTransportError,
    },
    http_addr::private_address_reason,
};
use praxis_core::{
    config::DEFAULT_SUBREQUEST_POOL_SIZE,
    connectivity::{ConnectionOptions, peer as peer_utils},
    subrequest::{SubRequest, SubRequestClient, SubRequestConnector, SubRequestError, SubResponse},
};

use super::shared_connector::shared_policy_connector;

/// Isolates policy connections from cluster-specific TLS state.
///
/// Pingora's reuse hash omits `options.ca`, so a distinct group key keeps
/// cluster private-CA connections out of the policy pool partition.
const POLICY_PEER_GROUP: u64 = 0x706F_6C69_6379_5F31; // "policy_1"

/// Performs the policy engine's outbound HTTP over the proxy's connector.
#[derive(Debug)]
pub(super) struct PolicyHttpTransport {
    /// Built on first call from the registered connector.
    client: OnceLock<SubRequestClient>,

    /// Whether private and loopback destinations are permitted.
    allow_private: bool,
}

impl PolicyHttpTransport {
    /// Build a transport that refuses, or permits, non-public destinations.
    pub(super) fn new(allow_private: bool) -> Self {
        Self {
            client: OnceLock::new(),
            allow_private,
        }
    }

    /// The client, built from the registered connector on first call.
    fn client(&self) -> &SubRequestClient {
        self.client.get_or_init(|| build_client(shared_policy_connector()))
    }

    /// Resolve the destination within `budget` and return the remaining time.
    ///
    /// # Errors
    ///
    /// Returns [`HttpTransportError::Connect`] when resolution fails or
    /// exhausts the budget because no request was sent.
    async fn resolve_within(
        &self,
        target: &Target,
        budget: Duration,
    ) -> Result<(SocketAddr, Duration), HttpTransportError> {
        let authority = &target.dial_authority;
        let started = tokio::time::Instant::now();

        let address = tokio::time::timeout(budget, peer_utils::resolve_address(authority))
            .await
            .map_err(|_elapsed| unsent(format!("resolve '{authority}': deadline exceeded")))?
            .map_err(|e| unsent(format!("resolve '{authority}': {e}")))?;

        // Preserve the unsent classification instead of passing a zero budget
        // to the client, which reports a possibly-delivered timeout.
        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(unsent(format!(
                "resolve '{authority}': deadline exceeded before the request could be sent"
            )));
        }
        Ok((address, remaining))
    }

    /// Refuse a destination the shared address table rules out.
    fn check_egress(&self, address: SocketAddr, host: &str) -> Result<(), HttpTransportError> {
        match private_address_reason(&address.ip()).filter(|_| !self.allow_private) {
            None => Ok(()),
            Some(reason) => {
                tracing::warn!(
                    target: "policy.transport",
                    host,
                    address = %address,
                    reason,
                    "policy: refusing an outbound call to a non-public address"
                );
                Err(HttpTransportError::Rejected(reason.to_owned()))
            },
        }
    }
}

#[async_trait]
impl HttpTransport for PolicyHttpTransport {
    #[expect(
        clippy::large_stack_frames,
        clippy::large_futures,
        reason = "Pingora session types are large"
    )]
    async fn execute(&self, req: HttpRequest) -> Result<HttpResponse, HttpTransportError> {
        let target = Target::parse(&req.url)?;
        let (address, remaining) = self.resolve_within(&target, req.timeout).await?;
        self.check_egress(address, &target.host_header)?;

        let peer = target.peer(address, req.connect_timeout);
        let sub_request = target.sub_request(&req)?;

        tracing::debug!(
            target: "policy.transport",
            method = %req.method,
            url = %req.url,
            address = %address,
            "policy: dispatching an outbound call over the proxy connector"
        );

        self.client()
            .execute(&peer, &sub_request, req.max_response_bytes, remaining, None)
            .await
            .map(into_http_response)
            .map_err(|e| map_error(&e))
    }
}

/// Build the client a transport dispatches through.
///
/// Falls back to a private pool when the host registered nothing, so an
/// embedder who forgot the registration still gets working policy calls —
/// with a warning naming the call that would remove the second pool.
fn build_client(shared: Option<&SubRequestConnector>) -> SubRequestClient {
    let connector = shared.cloned().unwrap_or_else(|| {
        tracing::warn!(
            target: "policy.transport",
            "policy: no shared sub-request connector registered, so policy calls use a second \
             connection pool; call praxis_filter::set_policy_subrequest_connector before building pipelines"
        );
        SubRequestConnector::new(DEFAULT_SUBREQUEST_POOL_SIZE, None)
    });
    SubRequestClient::with_max_response_bytes(connector, DEFAULT_MAX_RESPONSE_BYTES)
}

/// Convert a completed exchange into the engine's response type.
fn into_http_response(response: SubResponse) -> HttpResponse {
    HttpResponse::new(response.status, response.body).with_headers(response.headers)
}

/// Classify a sub-request failure for the engine.
///
/// The variant a caller acts on is `may_have_reached_peer`: a token
/// exchange that reports an unknown outcome is reconciled, one that
/// reports a clean refusal is retried.
fn map_error(error: &SubRequestError) -> HttpTransportError {
    match error {
        SubRequestError::InvalidRequest(message) => HttpTransportError::InvalidRequest(message.clone()),
        // Admission exhaustion is local and unsent. `Connect` keeps it
        // retryable; `Rejected` would suppress the engine's retries.
        SubRequestError::AdmissionTimeout { max_connections } => HttpTransportError::Connect(format!(
            "sub-request admission timeout (all {max_connections} slots busy)"
        )),
        SubRequestError::CircuitOpen { peer } => HttpTransportError::Rejected(format!("circuit open for peer {peer}")),
        SubRequestError::Connect(message) => HttpTransportError::Connect(message.clone()),
        SubRequestError::Io(message) => HttpTransportError::Io(message.clone()),
        SubRequestError::DeadlineExceeded => HttpTransportError::Timeout,
        SubRequestError::StreamIdleTimeout { idle_timeout } => {
            HttpTransportError::Io(format!("upstream stream idle for {idle_timeout:?}"))
        },
        SubRequestError::ResponseTooLarge { actual, limit } => HttpTransportError::ResponseTooLarge {
            actual: *actual,
            limit: *limit,
        },
        // Unclassified: the peer may have seen the request, so callers must reconcile.
        _ => HttpTransportError::Io(error.to_string()),
    }
}

/// A destination URL resolved into the pieces a dial needs.
#[derive(Debug)]
struct Target {
    /// `host:port`, IPv6 bracketed — the form address resolution and SNI
    /// derivation both expect.
    dial_authority: String,

    /// The `Host` header value: the authority exactly as the URL wrote it.
    host_header: String,

    /// Path and query, or `/` when the URL carried neither.
    uri: http::Uri,

    /// Whether to dial TLS.
    tls: bool,

    /// SNI hostname, empty for plaintext.
    sni: String,
}

impl Target {
    /// Split a policy URL into the pieces needed to dial it.
    ///
    /// # Errors
    ///
    /// Returns [`HttpTransportError::InvalidRequest`] for a URL this
    /// transport will not dial: an unparseable one, a scheme other than
    /// `http` or `https`, a missing host, embedded userinfo that would be
    /// dropped rather than sent, or an `https` URL naming an IP literal —
    /// which has no SNI, leaving certificate verification with no hostname
    /// to check.
    fn parse(url: &str) -> Result<Self, HttpTransportError> {
        let uri: http::Uri = url.parse().map_err(|e| invalid(format!("url '{url}': {e}")))?;
        let tls = dial_tls(url, uri.scheme_str())?;
        let authority = checked_authority(url, &uri)?;
        let host = authority.host();
        let dial_authority = format!("{host}:{}", checked_port(url, authority, tls)?);

        if tls && peer_utils::is_ip_literal(host) {
            return Err(invalid(format!(
                "url '{url}' uses https with an IP literal, which carries no SNI for certificate verification"
            )));
        }

        Ok(Self {
            sni: if tls {
                peer_utils::derive_sni(&dial_authority)
            } else {
                String::new()
            },
            dial_authority,
            host_header: authority.as_str().to_owned(),
            uri: request_uri(&uri),
            tls,
        })
    }

    /// Build the peer to dial at an already-checked address.
    ///
    /// A connect bound is always set. Without one the overall deadline
    /// fires first and an unreachable peer reports a timeout, which a
    /// delegating caller must treat as a possibly-minted token; with one,
    /// the same failure reports a connect error it can safely retry.
    fn peer(&self, address: SocketAddr, connect_timeout: Option<Duration>) -> HttpPeer {
        let connect = connect_timeout.unwrap_or(DEFAULT_CONNECT_TIMEOUT);
        let mut peer = HttpPeer::new(address, self.tls, self.sni.clone());
        peer.group_key = POLICY_PEER_GROUP;
        peer_utils::apply_connection_options(
            &mut peer,
            &ConnectionOptions {
                connection_timeout: Some(connect),
                total_connection_timeout: Some(connect),
                ..ConnectionOptions::default()
            },
        );
        peer
    }

    /// Build the sub-request to send.
    ///
    /// # Errors
    ///
    /// Returns [`HttpTransportError::InvalidRequest`] when the authority
    /// cannot be a header value.
    fn sub_request(&self, req: &HttpRequest) -> Result<SubRequest, HttpTransportError> {
        let mut headers = req.headers.clone();
        // The client fills a missing `Host` with the socket address, which
        // is the wrong name for a virtual-hosted IdP.
        headers.insert(
            http::header::HOST,
            http::HeaderValue::from_str(&self.host_header)
                .map_err(|e| invalid(format!("host '{}' is not a valid header value: {e}", self.host_header)))?,
        );
        Ok(SubRequest {
            method: req.method.clone(),
            uri: self.uri.clone(),
            headers,
            body: req.body.clone(),
        })
    }
}

/// Whether a URL's scheme means dialling TLS.
///
/// # Errors
///
/// Returns [`HttpTransportError::InvalidRequest`] for a missing scheme or
/// one other than `http` or `https`.
fn dial_tls(url: &str, scheme: Option<&str>) -> Result<bool, HttpTransportError> {
    match scheme {
        Some("https") => Ok(true),
        Some("http") => Ok(false),
        Some(other) => Err(invalid(format!("url '{url}' has unsupported scheme '{other}'"))),
        None => Err(invalid(format!("url '{url}' has no scheme"))),
    }
}

/// A URL's authority, refused when it names no host or carries userinfo.
///
/// # Errors
///
/// Returns [`HttpTransportError::InvalidRequest`] in both refused cases.
fn checked_authority<'a>(url: &str, uri: &'a http::Uri) -> Result<&'a http::uri::Authority, HttpTransportError> {
    let authority = uri
        .authority()
        .filter(|authority| !authority.host().is_empty())
        .ok_or_else(|| invalid(format!("url '{url}' has no host")))?;
    // `Authority::host()` drops userinfo silently, so dialling would
    // discard credentials the operator wrote into the URL.
    if authority.as_str().contains('@') {
        // The URL is deliberately not echoed: this is the one refusal that
        // fires when the string is known to hold a credential, and a log
        // line travels further than the config it came from.
        return Err(invalid(
            "url carries userinfo, which is not forwarded; use a header or a client-credentials flow".to_owned(),
        ));
    }
    Ok(authority)
}

/// The port to dial: the URL's own, or the scheme default.
///
/// # Errors
///
/// Returns [`HttpTransportError::InvalidRequest`] for port zero or a
/// value outside the `u16` range.
fn checked_port(url: &str, authority: &http::uri::Authority, tls: bool) -> Result<u16, HttpTransportError> {
    match authority.port_u16() {
        Some(0) => Err(invalid(format!("url '{url}' names port 0, which cannot be dialled"))),
        Some(port) => Ok(port),
        // `port_u16` also returns `None` for an overflowing explicit port.
        None if authority.as_str().len() > authority.host().len() => {
            Err(invalid(format!("url '{url}' has a port outside the range 1-65535")))
        },
        None => Ok(if tls { 443 } else { 80 }),
    }
}

/// The origin-form request target: path and query, or `/` when the URL
/// carried neither.
fn request_uri(url: &http::Uri) -> http::Uri {
    url.path_and_query()
        .and_then(|pq| http::Uri::builder().path_and_query(pq.clone()).build().ok())
        .unwrap_or_else(|| http::Uri::from_static("/"))
}

/// Shorthand for the malformed-request case.
fn invalid(message: String) -> HttpTransportError {
    HttpTransportError::InvalidRequest(message)
}

/// Build a retry-safe failure for a request that was not sent.
fn unsent(message: String) -> HttpTransportError {
    HttpTransportError::Connect(message)
}

#[cfg(test)]
#[expect(clippy::allow_attributes, reason = "blanket test suppressions")]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "tests"
)]
mod tests;
