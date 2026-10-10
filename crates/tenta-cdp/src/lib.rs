//! Chrome DevTools Protocol server (`docs/spec/09-cdp-compat.md`).
//!
//! [`CdpServer`] owns the listening socket, the `/json/*` HTTP endpoints and
//! the WebSocket transport. Protocol domains plug in through
//! [`DomainHandler`] and [`DomainRegistry`]; the transport itself knows no
//! domain.
//!
//! Dependency direction is enforced by `cargo xtask lint-deps`
//! (`docs/spec/02-architecture.md` §2).

mod backend;
mod clock;
mod connection;
mod domain;
mod domains;
mod error;
mod http;
mod ids;
mod interaction;
mod names;
mod options;
mod overrides;
mod page_runtime;
mod server;
mod session;
mod wire;

pub use backend::{Backend, Stats, Target, TargetEvent};
pub use connection::{Connection, SessionIdKind};
pub use domain::{
    CommandContext, CommandFuture, CommandResult, DomainHandler, DomainRegistry, parse_params,
};
pub use error::CdpError;
pub use names::{EVENTS, METHODS};
pub use options::{CdpOptions, MIN_REMOTE_TOKEN_LEN};
pub use server::CdpServer;
pub use session::Session;
