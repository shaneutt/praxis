// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Tests for the header manipulation filter.

use super::{
    HeaderFilter, HeaderFilterConfig,
    ops::{append_headers, remove_headers, set_headers},
};
use crate::filter::HttpFilter;

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[tokio::test]
async fn request_add_populates_extra_headers() {
    let filter = make_header_filter(
        r#"request_add:
  - name: X-Forwarded-By
    value: praxis"#,
    );
    let req = crate::test_utils::make_request(http::Method::GET, "/");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    drop(filter.on_request(&mut ctx).await.unwrap());
    assert_eq!(
        ctx.extra_request_headers.len(),
        1,
        "should add exactly one request header"
    );
    let (ref name, ref value) = ctx.extra_request_headers[0];
    assert_eq!(name, "X-Forwarded-By", "header name should match");
    assert_eq!(value, "praxis", "header value should match");
}

#[tokio::test]
async fn response_set_overwrites_header() {
    let filter = make_header_filter(
        r#"response_set:
  - name: server
    value: praxis"#,
    );
    let req = crate::test_utils::make_request(http::Method::GET, "/");
    let mut resp = crate::test_utils::make_response();
    resp.headers.insert("server", "nginx".parse().unwrap());
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.response_header = Some(&mut resp);
    drop(filter.on_response(&mut ctx).await.unwrap());
    assert_eq!(
        resp.headers["server"], "praxis",
        "response_set should overwrite existing header"
    );
}

#[tokio::test]
async fn response_remove_deletes_header() {
    let filter = make_header_filter(
        r#"response_remove:
  - x-backend-server"#,
    );
    let req = crate::test_utils::make_request(http::Method::GET, "/");
    let mut resp = crate::test_utils::make_response();
    resp.headers.insert("x-backend-server", "internal".parse().unwrap());
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.response_header = Some(&mut resp);
    drop(filter.on_response(&mut ctx).await.unwrap());
    assert!(
        !resp.headers.contains_key("x-backend-server"),
        "response_remove should delete header"
    );
}

#[tokio::test]
async fn response_add_appends_without_overwriting() {
    let filter = make_header_filter(
        r#"response_add:
  - name: x-custom
    value: second"#,
    );
    let req = crate::test_utils::make_request(http::Method::GET, "/");
    let mut resp = crate::test_utils::make_response();
    resp.headers.insert("x-custom", "first".parse().unwrap());
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.response_header = Some(&mut resp);
    drop(filter.on_response(&mut ctx).await.unwrap());
    let values: Vec<&str> = resp
        .headers
        .get_all("x-custom")
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    assert_eq!(
        values,
        vec!["first", "second"],
        "response_add should append without overwriting"
    );
}

#[tokio::test]
async fn from_config_empty_is_noop() {
    let config = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    let filter = HeaderFilter::from_config(&config).unwrap();
    assert_eq!(filter.name(), "headers", "empty config should produce valid filter");
}

#[test]
fn from_config_rejects_invalid_header_name_in_response_add() {
    let yaml: serde_yaml::Value = serde_yaml::from_str(
        r#"
response_add:
  - name: "invalid header"
    value: "value"
"#,
    )
    .unwrap();
    let err = expect_config_err(&yaml);
    assert!(
        err.contains("invalid header name"),
        "should reject invalid header name at config time: {err}"
    );
}

#[test]
fn from_config_rejects_invalid_header_value() {
    let yaml: serde_yaml::Value =
        serde_yaml::from_str("response_add:\n  - name: x-good-name\n    value: \"bad\\x00value\"\n").unwrap();
    let err = expect_config_err(&yaml);
    assert!(
        err.contains("invalid header value"),
        "should reject invalid header value at config time: {err}"
    );
}

#[test]
fn from_config_rejects_invalid_set_header_name() {
    let yaml: serde_yaml::Value = serde_yaml::from_str(
        r#"
response_set:
  - name: "bad name!"
    value: "value"
"#,
    )
    .unwrap();
    let err = expect_config_err(&yaml);
    assert!(
        err.contains("invalid header name"),
        "should reject invalid set header name at config time: {err}"
    );
}

#[test]
fn from_config_rejects_invalid_request_add_header_name() {
    let yaml: serde_yaml::Value = serde_yaml::from_str(
        r#"
request_add:
  - name: "bad name"
    value: "value"
"#,
    )
    .unwrap();
    let err = expect_config_err(&yaml);
    assert!(
        err.contains("invalid header name"),
        "should reject invalid request_add header name at config time: {err}"
    );
}

#[test]
fn from_config_rejects_invalid_response_remove_header_name() {
    let yaml: serde_yaml::Value = serde_yaml::from_str(
        r#"
response_remove:
  - "bad name!"
"#,
    )
    .unwrap();
    let err = expect_config_err(&yaml);
    assert!(
        err.contains("invalid header name"),
        "should reject invalid response_remove header name at config time: {err}"
    );
}

#[test]
fn remove_headers_idempotent() {
    let mut headers = http::HeaderMap::new();
    headers.insert("x-remove", "val".parse().unwrap());
    let names = vec![hdr_name("x-remove")];
    remove_headers(&mut headers, &names);
    remove_headers(&mut headers, &names);
    assert!(!headers.contains_key("x-remove"), "double removal should be idempotent");
}

#[test]
fn remove_headers_missing_is_noop() {
    let mut headers = http::HeaderMap::new();
    headers.insert("x-keep", "val".parse().unwrap());
    remove_headers(&mut headers, &[hdr_name("x-absent")]);
    assert_eq!(
        headers.len(),
        1,
        "removing absent header should not affect existing ones"
    );
    assert_eq!(headers["x-keep"], "val", "existing header should remain");
}

#[test]
fn append_headers_preserves_existing() {
    let mut headers = http::HeaderMap::new();
    headers.insert("x-existing", "first".parse().unwrap());
    append_headers(&mut headers, &[hdr_pair("x-existing", "second")]);
    let values: Vec<&str> = headers
        .get_all("x-existing")
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    assert_eq!(
        values,
        vec!["first", "second"],
        "append should preserve existing and add new"
    );
}

#[test]
fn append_headers_to_empty_map() {
    let mut headers = http::HeaderMap::new();
    append_headers(&mut headers, &[hdr_pair("x-new", "value")]);
    assert_eq!(headers["x-new"], "value", "append to empty map should work");
}

#[test]
fn set_headers_overwrites_existing() {
    let mut headers = http::HeaderMap::new();
    headers.insert("server", "nginx".parse().unwrap());
    set_headers(&mut headers, &[hdr_pair("server", "praxis")]);
    assert_eq!(headers["server"], "praxis", "set should overwrite existing value");
    assert_eq!(
        headers.get_all("server").iter().count(),
        1,
        "set should result in exactly one value"
    );
}

#[test]
fn set_headers_creates_new() {
    let mut headers = http::HeaderMap::new();
    set_headers(&mut headers, &[hdr_pair("x-new", "value")]);
    assert_eq!(headers["x-new"], "value", "set should create header when absent");
}

#[test]
fn remove_headers_empty_list_is_noop() {
    let mut headers = http::HeaderMap::new();
    headers.insert("x-keep", "val".parse().unwrap());
    remove_headers(&mut headers, &[]);
    assert_eq!(headers.len(), 1, "empty remove list should not affect headers");
}

// -----------------------------------------------------------------------------
// Test Utilities
// -----------------------------------------------------------------------------

/// Build a [`HeaderFilter`] from a YAML string for testing.
fn make_header_filter(yaml: &str) -> HeaderFilter {
    let config: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
    drop(HeaderFilter::from_config(&config).unwrap());
    let cfg: HeaderFilterConfig = serde_yaml::from_value(config).unwrap();
    HeaderFilter {
        request_add: cfg.request_add.into_iter().map(|p| (p.name, p.value)).collect(),
        response_add: cfg
            .response_add
            .into_iter()
            .map(|p| hdr_pair(&p.name, &p.value))
            .collect(),
        response_remove: cfg.response_remove.into_iter().map(|n| hdr_name(&n)).collect(),
        response_set: cfg
            .response_set
            .into_iter()
            .map(|p| hdr_pair(&p.name, &p.value))
            .collect(),
    }
}

/// Call `from_config` and assert it returns an error, returning the error string.
fn expect_config_err(yaml: &serde_yaml::Value) -> String {
    match HeaderFilter::from_config(yaml) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected from_config to return an error"),
    }
}

/// Parse a header name string for tests.
fn hdr_name(name: &str) -> http::header::HeaderName {
    http::header::HeaderName::from_bytes(name.as_bytes()).unwrap()
}

/// Parse a header name/value pair for tests.
fn hdr_pair(name: &str, value: &str) -> (http::header::HeaderName, http::header::HeaderValue) {
    (
        http::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
        http::header::HeaderValue::from_str(value).unwrap(),
    )
}
