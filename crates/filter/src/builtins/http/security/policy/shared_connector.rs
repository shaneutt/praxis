// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 Praxis Contributors

//! The process-wide sub-request connector the policy engine's outbound
//! calls borrow.
//!
//! `PolicyFilter::new` drives `PolicyEngine::initialize()`, and that is
//! where the boot JWKS fetch happens — before the pipeline exists and
//! therefore before the shared [`SubRequestClient`] reaches any filter.
//! The filter-factory signature cannot carry a client, so the host
//! registers the connector here and the transport reads it when it first
//! needs a socket.
//!
//! A host registers once, before building pipelines:
//!
//! ```rust,ignore
//! use praxis_filter::set_policy_subrequest_connector;
//!
//! let client = praxis::build_subrequest_client(&config);
//! set_policy_subrequest_connector(client.connector());
//! ```
//!
//! [`SubRequestClient`]: praxis_core::subrequest::SubRequestClient

use std::sync::OnceLock;

use praxis_core::subrequest::SubRequestConnector;

/// Set-once storage for the connector policy calls borrow.
#[derive(Debug)]
struct ConnectorHolder(OnceLock<SubRequestConnector>);

impl ConnectorHolder {
    /// An empty holder.
    const fn new() -> Self {
        Self(OnceLock::new())
    }

    /// Store `connector`, or keep the one already held.
    ///
    /// Returns whether the held pool is the one passed in, so storing the
    /// same pool twice succeeds and a second pool is refused.
    fn set(&self, connector: &SubRequestConnector) -> bool {
        std::ptr::eq(
            self.0.get_or_init(|| connector.clone()).connector(),
            connector.connector(),
        )
    }

    /// The held connector, if one was stored.
    fn get(&self) -> Option<&SubRequestConnector> {
        self.0.get()
    }
}

/// The registered connector, or none when the host never registered one.
static POLICY_CONNECTOR: ConnectorHolder = ConnectorHolder::new();

/// Register the connector policy calls share with the data plane.
///
/// Call before pipelines are built. Every policy call made afterwards
/// borrows this connector, so the keepalive pool, the admission limit, and
/// the circuit-breaker registry are shared with proxy sub-requests.
///
/// Re-registering the pool that is already held succeeds and changes
/// nothing, which is what a config reload does. Registering a *different*
/// one returns `false` and keeps the first: a second pool is the thing
/// this registration exists to prevent.
#[must_use]
pub fn set_policy_subrequest_connector(connector: &SubRequestConnector) -> bool {
    if POLICY_CONNECTOR.set(connector) {
        return true;
    }
    tracing::warn!(
        target: "policy.transport",
        "policy: a different sub-request connector is already registered; keeping the first"
    );
    false
}

/// The registered connector, if the host provided one.
pub(super) fn shared_policy_connector() -> Option<&'static SubRequestConnector> {
    POLICY_CONNECTOR.get()
}

#[cfg(test)]
#[expect(clippy::allow_attributes, reason = "blanket test suppressions")]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "tests")]
mod tests {
    use super::*;

    #[test]
    fn a_holder_hands_back_the_connector_it_was_given() {
        let holder = ConnectorHolder::new();
        assert!(holder.get().is_none(), "an empty holder holds nothing");

        let first = SubRequestConnector::new(8, None);
        assert!(holder.set(&first), "the first registration is accepted");
        assert!(
            std::ptr::eq(holder.get().expect("registered").connector(), first.connector()),
            "readers see the registered pool, not a fresh one"
        );
    }

    #[test]
    fn storing_the_held_connector_again_succeeds() {
        let holder = ConnectorHolder::new();
        let held = SubRequestConnector::new(8, None);
        assert!(holder.set(&held));
        assert!(holder.set(&held), "a reload must not fail on the pool it already holds");
    }

    #[test]
    fn a_second_connector_is_refused_and_the_first_survives() {
        let holder = ConnectorHolder::new();
        let first = SubRequestConnector::new(8, None);
        assert!(holder.set(&first));
        assert!(
            !holder.set(&SubRequestConnector::new(1, None)),
            "a second pool is what this holder exists to refuse"
        );
        assert!(
            std::ptr::eq(holder.get().expect("still registered").connector(), first.connector()),
            "the refused registration did not replace the first"
        );
    }

    #[test]
    fn the_public_setter_and_reader_agree_on_the_process_holder() {
        let connector = SubRequestConnector::new(4, None);
        let accepted = set_policy_subrequest_connector(&connector);
        let held = shared_policy_connector().expect("a connector is registered now");
        assert_eq!(
            accepted,
            std::ptr::eq(held.connector(), connector.connector()),
            "the return value must say whether the reader sees this pool"
        );
    }
}
