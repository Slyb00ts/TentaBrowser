//! MCP server: tools, transports and schemas (`docs/spec/10-mcp.md`).
//!
//! [`McpServer`] wraps an engine and hands out one [`Connection`] per agent;
//! a connection answers JSON-RPC messages and owns the agent's browser. Two
//! transports move the messages: [`stdio`] (newline-delimited JSON, the
//! default for local agents) and [`http`] (streamable HTTP, loopback by
//! default). The protocol engine speaks the 2026-07-28 stateless revision and
//! the session-based revisions 2024-11-05 to 2025-11-25.
//!
//! The protocol is implemented here rather than with `rmcp`: its server side
//! adds sixty crates to the stdio path alone, the HTTP side an HTTP stack the
//! workspace otherwise avoids, and the tool schemas, hardening rules and
//! typed errors of the specification have to be written by hand either way.
//! `docs/dev/mcp.md` has the numbers.
//!
//! Dependency direction is enforced by `cargo xtask lint-deps`
//! (`docs/spec/02-architecture.md` §2).

pub mod error;
pub mod http;
mod interaction;
pub mod jsonrpc;
pub mod keys;
mod schema;
pub mod server;
pub mod session;
mod snapshot;
pub mod stdio;
pub mod tools;

// MC3 builds `browser_web_search` on this crate.
use tenta_search as _;

pub use error::{ErrorKind, ToolError};
pub use http::{HttpOptions, HttpServer, MIN_REMOTE_TOKEN_LEN};
pub use server::{Connection, McpServer, supported_versions};
pub use session::McpOptions;
pub use tools::SCHEMA_VERSION;

/// The settle tunables MCP engines are built with: `busy_cap` 500 ms and
/// `foreground_request_max` 2 s (`docs/spec/10-mcp.md` §3). Pass them to the
/// engine builder before handing the engine to [`McpServer::new`].
pub fn settle_config() -> tenta_browser::SettleConfig {
    tenta_browser::SettleConfig::mcp()
}

/// The `tools/list` array, for `--schema-version` style tooling and goldens.
pub fn tool_list() -> serde_json::Value {
    tools::list()
}

/// A fresh random bearer token (32 hex digits) for an HTTP endpoint started
/// without one.
pub fn generate_token() -> String {
    server::random_id()
}
