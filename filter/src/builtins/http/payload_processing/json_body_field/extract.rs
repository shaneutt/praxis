// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Field extraction logic and control character validation.

use std::borrow::Cow;

use tracing::{trace, warn};

// -----------------------------------------------------------------------------
// Field Extraction
// -----------------------------------------------------------------------------

/// Extract mapped JSON fields into request headers, skipping values
/// with control characters. Returns `true` if any field was promoted.
pub(super) fn extract_fields(
    mappings: &[(String, String)],
    value: &serde_json::Value,
    headers: &mut Vec<(Cow<'static, str>, String)>,
) -> bool {
    let mut found_any = false;
    for (field, header) in mappings {
        if let Some(field_val) = value.get(field.as_str()) {
            let text = match field_val {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if contains_control_chars(&text) {
                warn!(
                    field = %field,
                    header = %header,
                    "skipping header injection: value contains control characters"
                );
                continue;
            }
            trace!(
                field = %field,
                header = %header,
                value = %text,
                "promoting JSON field to header"
            );
            headers.push((Cow::Owned(header.clone()), text));
            found_any = true;
        }
    }
    found_any
}

// -----------------------------------------------------------------------------
// Control Character Validation
// -----------------------------------------------------------------------------

/// Check whether a string contains control characters (0x00..0x1F
/// or 0x7F DEL) other than horizontal tab (0x09).
///
/// ```ignore
/// # use praxis_filter::JsonBodyFieldFilter;
/// // This function is internal; tested via filter behavior.
/// ```
pub(super) fn contains_control_chars(s: &str) -> bool {
    s.bytes().any(|b| (b < 0x20 && b != 0x09) || b == 0x7F)
}
