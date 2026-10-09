//! The seam between the transport and the protocol domains.
//!
//! A domain lives in its own module under `domains/` and exposes a
//! `register` function that adds its [`DomainHandler`] to a
//! [`DomainRegistry`]. The dispatcher knows nothing about any domain.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::backend::{Backend, Target, lock};
use crate::connection::Connection;
use crate::error::CdpError;
use crate::session::{Session, Turn};

pub type CommandResult = Result<Value, CdpError>;
pub type CommandFuture<'a> = BoxFuture<'a, CommandResult>;

/// One protocol domain.
pub trait DomainHandler: Send + Sync + 'static {
    /// The domain name, e.g. `"Page"`.
    fn domain(&self) -> &'static str;

    /// The commands this handler implements, without the domain prefix. A
    /// command outside the list is answered `-32601` by the dispatcher; the
    /// list also feeds `/json/protocol`.
    fn methods(&self) -> &'static [&'static str];

    /// Runs one command of [`DomainHandler::methods`]. The future may await
    /// page-thread work; the server's runtime is never blocked on it.
    fn handle<'a>(
        &'a self,
        cx: CommandContext,
        method: &'a str,
        params: Value,
    ) -> CommandFuture<'a>;
}

/// Everything a command can see.
#[derive(Clone)]
pub struct CommandContext {
    backend: Arc<Backend>,
    connection: Arc<Connection>,
    session: Arc<Session>,
    turn: Arc<Mutex<Option<Turn>>>,
}

impl CommandContext {
    pub(crate) fn new(
        backend: Arc<Backend>,
        connection: Arc<Connection>,
        session: Arc<Session>,
        turn: Turn,
    ) -> Self {
        Self {
            backend,
            connection,
            session,
            turn: Arc::new(Mutex::new(Some(turn))),
        }
    }

    pub fn backend(&self) -> &Arc<Backend> {
        &self.backend
    }

    pub fn connection(&self) -> &Arc<Connection> {
        &self.connection
    }

    /// The session the command arrived on.
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// The page of a page session.
    pub fn target(&self) -> Result<&Arc<Target>, CdpError> {
        self.session
            .target()
            .ok_or_else(|| CdpError::server("This command needs a page session"))
    }

    /// Emits an event on the session the command arrived on.
    pub async fn emit(&self, method: &str, params: Value) {
        self.session.emit(method, params).await;
    }

    /// Lets the session's next command start before this one finishes.
    ///
    /// Commands of one session run one after another by default, which
    /// matches Chromium for everything that answers quickly. A command that
    /// waits on something the client controls (a navigation, a dialog) calls
    /// this once its synchronous part is done, so later commands are not
    /// stuck behind it.
    pub fn release_order(&self) {
        drop(lock(&self.turn).take());
    }
}

/// Decodes a command's `params` into `T`; a mismatch is `-32602`.
pub fn parse_params<T: DeserializeOwned>(params: Value) -> Result<T, CdpError> {
    serde_json::from_value(params).map_err(|error| CdpError::invalid_params(error.to_string()))
}

/// The handlers of a server, by domain.
#[derive(Default)]
pub struct DomainRegistry {
    handlers: BTreeMap<&'static str, Arc<dyn DomainHandler>>,
}

impl DomainRegistry {
    /// A registry with every domain this crate ships.
    pub fn with_defaults() -> Self {
        let mut registry = Self::default();
        crate::domains::register_all(&mut registry);
        registry
    }

    /// Adds a handler.
    ///
    /// # Panics
    /// When the domain is already registered: two handlers for one domain is
    /// a wiring mistake that would otherwise silently drop one of them.
    pub fn register(&mut self, handler: impl DomainHandler) {
        let name = handler.domain();
        let previous = self.handlers.insert(name, Arc::new(handler));
        assert!(previous.is_none(), "domain {name} is registered twice");
    }

    pub(crate) fn lookup(&self, domain: &str, method: &str) -> Option<Arc<dyn DomainHandler>> {
        let handler = self.handlers.get(domain)?;
        handler
            .methods()
            .contains(&method)
            .then(|| Arc::clone(handler))
    }

    /// `(domain, method)` of everything registered.
    pub fn methods(&self) -> Vec<(&'static str, &'static str)> {
        self.handlers
            .values()
            .flat_map(|handler| {
                handler
                    .methods()
                    .iter()
                    .map(|method| (handler.domain(), *method))
            })
            .collect()
    }

    /// The `/json/protocol` document: the commands this server answers.
    pub(crate) fn describe(&self) -> Value {
        let domains: Vec<Value> = self
            .handlers
            .values()
            .map(|handler| {
                let commands: Vec<Value> = handler
                    .methods()
                    .iter()
                    .map(|name| json!({ "name": name }))
                    .collect();
                json!({ "domain": handler.domain(), "commands": commands })
            })
            .collect();
        json!({ "version": { "major": "1", "minor": "3" }, "domains": domains })
    }
}
