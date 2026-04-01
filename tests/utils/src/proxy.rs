//! Proxy startup and configuration helpers for integration tests.

use std::{collections::HashMap, sync::Arc};

use praxis_core::config::{Config, Listener};
use praxis_filter::{FilterEntry, FilterFactory, FilterPipeline, FilterRegistry, HttpFilter};
use praxis_protocol::http::load_http_handler;

// -----------------------------------------------------------------------------
// Pipeline Building
// -----------------------------------------------------------------------------

/// Resolve a listener's filter chains into a [`FilterPipeline`].
///
/// Collects all [`FilterEntry`] items from the named chains
/// referenced by the listener, then builds the pipeline via
/// the provided registry.
///
/// [`FilterPipeline`]: praxis_filter::FilterPipeline
/// [`FilterEntry`]: praxis_filter::FilterEntry
fn resolve_listener_pipeline(config: &Config, listener: &Listener, registry: &FilterRegistry) -> Arc<FilterPipeline> {
    let chains: HashMap<&str, &Vec<_>> = config
        .filter_chains
        .iter()
        .map(|c| (c.name.as_str(), &c.filters))
        .collect();

    let mut entries = Vec::new();
    for chain_name in &listener.filter_chains {
        if let Some(filters) = chains.get(chain_name.as_str()) {
            for entry in *filters {
                entries.push(FilterEntry::from(entry));
            }
        }
    }

    Arc::new(FilterPipeline::build(&entries, registry).unwrap())
}

/// Build the filter pipeline from the config using the
/// builtin registry (uses first listener).
///
/// # Panics
///
/// Panics if `config.listeners` is empty.
pub fn build_pipeline(config: &Config) -> FilterPipeline {
    let registry = FilterRegistry::with_builtins();
    let listener = config
        .listeners
        .first()
        .expect("config must have at least one listener");

    Arc::try_unwrap(resolve_listener_pipeline(config, listener, &registry))
        .unwrap_or_else(|_| panic!("pipeline Arc should have single owner"))
}

// -----------------------------------------------------------------------------
// Proxy Startup
// -----------------------------------------------------------------------------

/// Start the proxy server in a background thread.
///
/// Returns the address string (e.g. `"127.0.0.1:12345"`).
///
/// # Panics
///
/// Panics if `config.listeners` is empty.
pub fn start_proxy(config: &Config) -> String {
    start_proxy_with_registry(config, &FilterRegistry::with_builtins())
}

/// Start the proxy with a custom filter registry.
///
/// # Panics
///
/// Panics if `config.listeners` is empty.
pub fn start_proxy_with_registry(config: &Config, registry: &FilterRegistry) -> String {
    let addr = config
        .listeners
        .first()
        .expect("config must have at least one listener")
        .address
        .clone();
    let mut server = praxis_core::server::build_http_server(config.shutdown_timeout_secs, &Default::default());

    for listener in &config.listeners {
        let pipeline = resolve_listener_pipeline(config, listener, registry);
        load_http_handler(&mut server, listener, pipeline).unwrap();
    }

    if let Some(admin_addr) = &config.admin_address {
        praxis_protocol::http::pingora::health::add_health_endpoint_to_pingora_server(&mut server, admin_addr);
    }

    std::thread::spawn(move || {
        server.run_forever();
    });

    crate::network::wait_for_tcp(&addr);
    addr
}

// -----------------------------------------------------------------------------
// YAML Config Helpers
// -----------------------------------------------------------------------------

/// Simple route/cluster YAML: one listener, catch-all
/// route, one backend.
pub fn simple_proxy_yaml(proxy_port: u16, backend_port: u16) -> String {
    format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
routes:
  - path_prefix: "/"
    cluster: "backend"
clusters:
  - name: "backend"
    endpoints:
      - "127.0.0.1:{backend_port}"
"#
    )
}

/// Pipeline YAML: one listener, a custom filter first, then
/// router + `load_balancer`.
pub fn custom_filter_yaml(proxy_port: u16, backend_port: u16, filter_name: &str) -> String {
    format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
pipeline:
  - filter: {filter_name}
  - filter: router
    routes:
      - path_prefix: "/"
        cluster: "backend"
  - filter: load_balancer
    clusters:
      - name: "backend"
        endpoints:
          - "127.0.0.1:{backend_port}"
"#
    )
}

// -----------------------------------------------------------------------------
// Registry Helpers
// -----------------------------------------------------------------------------

/// Build a [`FilterRegistry`] with builtins plus one custom
/// test filter.
///
/// [`FilterRegistry`]: praxis_filter::FilterRegistry
pub fn registry_with(name: &str, make: fn() -> Box<dyn HttpFilter>) -> FilterRegistry {
    let mut registry = FilterRegistry::with_builtins();
    registry.register(name, FilterFactory::Http(Arc::new(move |_| Ok(make()))));
    registry
}
