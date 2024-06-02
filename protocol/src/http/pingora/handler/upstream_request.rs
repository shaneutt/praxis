//! Hop-by-hop header stripping on upstream requests ([RFC 9110]).
//!
//! [RFC 9110]: https://datatracker.ietf.org/doc/html/rfc9110

use pingora_http::RequestHeader;

use super::hop_by_hop::{self, REQUEST_HOP_BY_HOP};

// -----------------------------------------------------------------------------
// Hop-by-hop Header Stripping
// -----------------------------------------------------------------------------

/// Strip hop-by-hop headers from an upstream request.
///
/// Removes all RFC-defined hop-by-hop headers plus any custom
/// headers declared in the `Connection` header value.
pub(crate) fn strip_hop_by_hop(req: &mut RequestHeader) {
    let extra = hop_by_hop::connection_tokens(&req.headers, REQUEST_HOP_BY_HOP);

    for name in REQUEST_HOP_BY_HOP {
        let _ = req.remove_header(*name);
    }
    for name in &extra {
        let _ = req.remove_header(name.as_str());
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a GET request with the given headers for tests.
    fn make_request(headers: &[(&str, &str)]) -> RequestHeader {
        let mut req = RequestHeader::build("GET", b"/", None).unwrap();
        for (name, value) in headers {
            let _ = req.insert_header(name.to_string(), value.to_string());
        }
        req
    }

    #[test]
    fn strips_standard_hop_by_hop() {
        let mut req = make_request(&[
            ("connection", "close"),
            ("keep-alive", "300"),
            ("transfer-encoding", "chunked"),
            ("upgrade", "websocket"),
            ("te", "trailers"),
            ("trailer", "X-Checksum"),
            ("proxy-authorization", "Basic abc"),
            ("proxy-authenticate", "Basic"),
            ("x-real-header", "keep-me"),
        ]);

        strip_hop_by_hop(&mut req);

        assert!(
            req.headers.get("connection").is_none(),
            "connection header should be stripped"
        );
        assert!(
            req.headers.get("keep-alive").is_none(),
            "keep-alive header should be stripped"
        );
        assert!(
            req.headers.get("transfer-encoding").is_none(),
            "transfer-encoding header should be stripped"
        );
        assert!(
            req.headers.get("upgrade").is_none(),
            "upgrade header should be stripped"
        );
        assert!(req.headers.get("te").is_none(), "te header should be stripped");
        assert!(
            req.headers.get("trailer").is_none(),
            "trailer header should be stripped"
        );
        assert!(
            req.headers.get("proxy-authorization").is_none(),
            "proxy-authorization header should be stripped"
        );
        assert!(
            req.headers.get("proxy-authenticate").is_none(),
            "proxy-authenticate header should be stripped"
        );
        assert_eq!(
            req.headers.get("x-real-header").unwrap(),
            "keep-me",
            "end-to-end header should be preserved"
        );
    }

    #[test]
    fn strips_custom_connection_headers() {
        let mut req = make_request(&[
            ("connection", "X-Custom, X-Debug"),
            ("x-custom", "secret"),
            ("x-debug", "true"),
            ("x-safe", "keep"),
        ]);

        strip_hop_by_hop(&mut req);

        assert!(
            req.headers.get("connection").is_none(),
            "connection header should be stripped"
        );
        assert!(
            req.headers.get("x-custom").is_none(),
            "custom connection-listed header should be stripped"
        );
        assert!(
            req.headers.get("x-debug").is_none(),
            "custom connection-listed header should be stripped"
        );
        assert_eq!(
            req.headers.get("x-safe").unwrap(),
            "keep",
            "header not listed in connection should be preserved"
        );
    }

    #[test]
    fn no_hop_by_hop_headers_is_noop() {
        let mut req = make_request(&[
            ("host", "example.com"),
            ("accept", "text/html"),
            ("authorization", "Bearer tok"),
            ("content-type", "application/json"),
        ]);

        strip_hop_by_hop(&mut req);

        assert_eq!(
            req.headers.get("host").unwrap(),
            "example.com",
            "host header should be preserved"
        );
        assert_eq!(
            req.headers.get("accept").unwrap(),
            "text/html",
            "accept header should be preserved"
        );
        assert_eq!(
            req.headers.get("authorization").unwrap(),
            "Bearer tok",
            "authorization header should be preserved"
        );
        assert_eq!(
            req.headers.get("content-type").unwrap(),
            "application/json",
            "content-type header should be preserved"
        );
    }

    #[test]
    fn connection_header_with_single_value() {
        let mut req = make_request(&[("connection", "X-Only"), ("x-only", "gone"), ("x-keep", "stay")]);

        strip_hop_by_hop(&mut req);

        assert!(
            req.headers.get("connection").is_none(),
            "connection header should be stripped"
        );
        assert!(
            req.headers.get("x-only").is_none(),
            "single connection-listed header should be stripped"
        );
        assert_eq!(
            req.headers.get("x-keep").unwrap(),
            "stay",
            "header not listed in connection should be preserved"
        );
    }

    #[test]
    fn connection_value_with_whitespace_variations() {
        let mut req = make_request(&[
            ("connection", " X-A ,  X-B  , X-C "),
            ("x-a", "1"),
            ("x-b", "2"),
            ("x-c", "3"),
            ("x-d", "4"),
        ]);

        strip_hop_by_hop(&mut req);

        assert!(
            req.headers.get("x-a").is_none(),
            "x-a should be stripped despite whitespace"
        );
        assert!(
            req.headers.get("x-b").is_none(),
            "x-b should be stripped despite whitespace"
        );
        assert!(
            req.headers.get("x-c").is_none(),
            "x-c should be stripped despite whitespace"
        );
        assert_eq!(
            req.headers.get("x-d").unwrap(),
            "4",
            "x-d not in connection list should be preserved"
        );
    }

    #[test]
    fn connection_value_case_insensitive() {
        let mut req = make_request(&[("connection", "X-MiXeD-CaSe"), ("x-mixed-case", "stripped")]);

        strip_hop_by_hop(&mut req);

        assert!(
            req.headers.get("x-mixed-case").is_none(),
            "connection header matching should be case-insensitive"
        );
    }

    #[test]
    fn connection_value_referencing_standard_hop_by_hop() {
        // Connection: keep-alive is redundant with the static
        // list, but should not cause issues.
        let mut req = make_request(&[("connection", "keep-alive"), ("keep-alive", "timeout=5")]);

        strip_hop_by_hop(&mut req);

        assert!(
            req.headers.get("connection").is_none(),
            "connection header should be stripped"
        );
        assert!(
            req.headers.get("keep-alive").is_none(),
            "keep-alive referenced in connection should be stripped"
        );
    }

    #[test]
    fn empty_connection_header_value() {
        let mut req = make_request(&[("connection", ""), ("x-safe", "keep")]);

        strip_hop_by_hop(&mut req);

        assert!(
            req.headers.get("connection").is_none(),
            "empty connection header should be stripped"
        );
        assert_eq!(
            req.headers.get("x-safe").unwrap(),
            "keep",
            "unrelated header should be preserved with empty connection"
        );
    }

    #[test]
    fn only_hop_by_hop_headers_all_removed() {
        let mut req = make_request(&[("connection", "close"), ("keep-alive", "300"), ("upgrade", "h2c")]);

        strip_hop_by_hop(&mut req);

        assert!(
            req.headers.get("connection").is_none(),
            "connection header should be stripped"
        );
        assert!(
            req.headers.get("keep-alive").is_none(),
            "keep-alive header should be stripped"
        );
        assert!(
            req.headers.get("upgrade").is_none(),
            "upgrade header should be stripped"
        );
        assert_eq!(req.headers.len(), 0, "all hop-by-hop headers should be removed");
    }

    #[test]
    fn preserves_standard_end_to_end_headers() {
        let mut req = make_request(&[
            ("connection", "close"),
            ("host", "example.com"),
            ("accept", "*/*"),
            ("user-agent", "test/1.0"),
            ("content-length", "42"),
            ("cache-control", "no-cache"),
            ("authorization", "Bearer xyz"),
            ("cookie", "session=abc"),
        ]);

        strip_hop_by_hop(&mut req);

        assert!(
            req.headers.get("connection").is_none(),
            "connection header should be stripped"
        );
        assert_eq!(
            req.headers.get("host").unwrap(),
            "example.com",
            "host should be preserved"
        );
        assert_eq!(req.headers.get("accept").unwrap(), "*/*", "accept should be preserved");
        assert_eq!(
            req.headers.get("user-agent").unwrap(),
            "test/1.0",
            "user-agent should be preserved"
        );
        assert_eq!(
            req.headers.get("content-length").unwrap(),
            "42",
            "content-length should be preserved"
        );
        assert_eq!(
            req.headers.get("cache-control").unwrap(),
            "no-cache",
            "cache-control should be preserved"
        );
        assert_eq!(
            req.headers.get("authorization").unwrap(),
            "Bearer xyz",
            "authorization should be preserved"
        );
        assert_eq!(
            req.headers.get("cookie").unwrap(),
            "session=abc",
            "cookie should be preserved"
        );
    }

    #[test]
    fn empty_request_no_panic() {
        let mut req = RequestHeader::build("GET", b"/", None).unwrap();
        strip_hop_by_hop(&mut req);
    }
}
