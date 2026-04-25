# CLAUDE.md

Guidance for Claude Code when working in this repository.

## Requirements

- Rust stable 1.94+
- Rust nightly (for `rustfmt`)
- CMake 3.31+
- Docker 29.3.0+ or Podman (for container builds)

## Quick Reference

```console
make build          # workspace build (includes benches)
make test           # all tests
make fmt            # format with nightly rustfmt
make lint           # clippy + nightly fmt check
make audit          # cargo audit + cargo deny check
make container      # container image build
cargo run -p praxis # run the proxy
```

Run a single test or suite:

```console
cargo test -p praxis-tests-integration --test suite -- test_name
make test-integration V=1   # with --nocapture
```

See `docs/development.md` for the full command reference and dev tool usage.

## Architecture

See `docs/architecture.md` for the full design.

**Crate dependency flow:**

```console
server -> protocol -> filter -> core -> tls
```

- **server**: binary entry point, config loading, protocol registration
- **core**: configuration (YAML/serde), error types, health state, server runtime
- **filter**: `HttpFilter` and `TcpFilter` traits, pipeline engine, all built-in filters
- **protocol**: `Protocol` trait, HTTP and TCP/TLS backends
- **tls**: TLS configuration types, SNI parsing

## Conventions

See `docs/conventions.md` for the full coding style
guide. Key points:

- `#![deny(unsafe_code)]` in all crates
- All items (public and private) require `///` doc
  comments; enforced by `missing_docs` lint
- Comments answer "why?", never "what?"; use
  `tracing` for runtime narration
- Prefer `to_owned()` over `to_string()` for `&str` to `String`
- Use inline format args: `format!("{var}")`
- Use let-chains, `is_some_and()`, `strip_prefix()`
- Reference-style rustdoc links, not inline
- Do not document memory efficiency in rustdoc
  (e.g. "avoids allocation", "zero-copy", "cheap
  clone"). Correct memory use is expected; it does
  not need narration.
- Do not create re-export-only files. Import directly
  from the source module. No `pub use` shim files.

## File Ordering

1. Constants (with separator comment)
2. Public types, impls, functions
3. Private types and impls
4. Private utility functions (with separator)
5. `#[cfg(test)] mod tests` (always last)

Inside `mod tests`: imports, test functions, then
test utilities (with `// Test Utilities` separator).

Struct fields: `name` first (if present), then
alphabetical. Impl blocks: `new()` first, then
`name()`, then alphabetical.

## Test Requirements

New capabilities require:

1. Unit tests
2. Integration tests
3. Example config in `examples/configs/`
4. Functional integration test for the example config
   in `tests/integration/tests/suite/examples/`

Example config tests must exercise the actual
functionality end-to-end (e.g. a WebSocket config
must perform a real WebSocket handshake and message
exchange). Parse-only validation is not sufficient;
every example must prove its feature works with all
configured variants.

See `docs/conventions.md` for full test conventions
(no inline comments in test bodies, no doc comments
on test functions, full-width separators only).

## Adding a Filter

See `docs/extensions.md` for the full guide.

1. Create module under
   `filter/src/builtins/<protocol>/<category>/`
2. Implement `HttpFilter` or `TcpFilter` with a
   `from_config` factory
3. Register in `filter/src/registry.rs`
4. Add unit tests, doctests, example config, and
   integration test

## Adding a Protocol

1. Implement `Protocol` trait under `protocol/src/`
2. Add variant to `ProtocolKind` in
   `core/src/config/listener.rs`
3. Wire in `server/src/server.rs`

## Branch Chains

Conditional branching in filter pipelines based on
filter results. Key files:

- `core/src/config/branch_chain.rs`: config types
- `core/src/config/chain_ref.rs`: `ChainRef` enum
- `core/src/config/validate/branch_chain.rs`: validation
- `filter/src/results.rs`: `FilterResultSet` type
- `filter/src/pipeline/filter.rs`: `PipelineFilter`
- `filter/src/pipeline/branch.rs`: runtime types
- `filter/src/pipeline/build_branch.rs`: resolution
- `filter/src/pipeline/evaluate.rs`: execution

Filters write results to `FilterResultSet` without
knowing about branches. The pipeline executor reads
results to evaluate branch conditions and dispatch.
Branches rejoin at configurable points (next,
terminal, named filter, re-entrance with iteration
limits).

## Filter Organization

Filters live under
`filter/src/builtins/<protocol>/<category>/`.
See `docs/filters.md` for the full filter reference.

Example configs: `examples/configs/<category>/`.

## Pingora Boundary

See `docs/security-hardening.md` for details.

Pingora handles: request smuggling prevention, H2
backpressure, connection pool safety, HTTP/1.1
upgrade detection and bidirectional forwarding
(WebSocket, etc.).

Praxis handles: hop-by-hop header stripping (with
conditional preservation for upgrade requests),
Host validation, X-Forwarded-* injection, retry
logic.
