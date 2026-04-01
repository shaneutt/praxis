//! Security test suite for Praxis.
//!
//! Adversarial tests verifying that security filters
//! correctly reject, sanitize, or neutralize malicious
//! input. Each module focuses on one security boundary.

mod common;
mod forwarded_headers;
mod header_injection;
mod info_leakage;
mod ip_acl;
