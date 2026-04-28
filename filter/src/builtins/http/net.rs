// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Shared IP address utilities for HTTP filters.

use std::net::IpAddr;

// -----------------------------------------------------------------------------
// Normalize IPs
// -----------------------------------------------------------------------------

/// Convert IPv4-mapped IPv6 addresses (`::ffff:A.B.C.D`) to plain IPv4.
///
/// All other addresses are returned unchanged. This prevents bypass
/// attacks where a client connects via IPv4-mapped IPv6 to evade rules
/// that only list plain IPv4 addresses.
///
/// ```
/// use std::net::IpAddr;
///
/// // IPv4-mapped IPv6 is normalized to plain IPv4.
/// let mapped: IpAddr = "::ffff:192.168.1.1".parse().unwrap();
/// assert_eq!(
///     praxis_filter::normalize_mapped_ipv4(mapped),
///     "192.168.1.1".parse::<IpAddr>().unwrap(),
/// );
///
/// // Plain IPv4 is unchanged.
/// let v4: IpAddr = "10.0.0.1".parse().unwrap();
/// assert_eq!(praxis_filter::normalize_mapped_ipv4(v4), v4);
///
/// // Non-mapped IPv6 is unchanged.
/// let v6: IpAddr = "2001:db8::1".parse().unwrap();
/// assert_eq!(praxis_filter::normalize_mapped_ipv4(v6), v6);
/// ```
pub fn normalize_mapped_ipv4(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        IpAddr::V4(v4) => IpAddr::V4(v4),
    }
}
