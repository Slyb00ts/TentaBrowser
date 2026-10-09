//! The listener: accepts sockets, bounds them, shuts them down.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tenta_browser::Engine;
use tenta_core::{Error, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::timeout;

use crate::backend::{Backend, Stats};
use crate::domain::DomainRegistry;
use crate::http::{self, Env};
use crate::options::CdpOptions;

/// Refusals being written at once; beyond it a refused socket is just closed.
const MAX_REFUSALS: usize = 32;
const REFUSAL_TIMEOUT: Duration = Duration::from_secs(1);

const BUSY_RESPONSE: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Length: 11\r\nConnection: close\r\n\r\nServer busy";

/// A running CDP endpoint.
///
/// Must be bound from inside a tokio runtime; connections run on it. Dropping
/// the server stops accepting and tells open connections to close; call
/// [`CdpServer::shutdown`] to wait until they have.
pub struct CdpServer {
    local_addr: SocketAddr,
    backend: Arc<Backend>,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl CdpServer {
    /// Binds with every domain this crate ships.
    pub async fn bind(engine: Engine, options: CdpOptions) -> Result<Self> {
        Self::bind_with(engine, options, DomainRegistry::with_defaults()).await
    }

    /// Binds with a caller-built registry, to add or replace domains.
    pub async fn bind_with(
        engine: Engine,
        options: CdpOptions,
        registry: DomainRegistry,
    ) -> Result<Self> {
        options.validate()?;
        let max_connections = options.max_connections;
        let listener = TcpListener::bind(options.bind).await.map_err(Error::from)?;
        let local_addr = listener.local_addr().map_err(Error::from)?;
        let backend = Backend::new(engine, options)?;
        let (shutdown, stop) = watch::channel(false);
        let env = Env {
            backend: Arc::clone(&backend),
            registry: Arc::new(registry),
            shutdown: stop,
            local_addr,
        };
        let task = tokio::spawn(accept_loop(listener, env, max_connections));
        Ok(Self {
            local_addr,
            backend,
            shutdown,
            task,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The browser-level endpoint, with the token when one is required.
    pub fn browser_ws_url(&self) -> String {
        let token = self
            .backend
            .options()
            .token
            .as_ref()
            .map_or_else(String::new, |token| format!("?token={token}"));
        format!(
            "ws://{}/devtools/browser/{}{token}",
            self.local_addr,
            self.backend.browser_id()
        )
    }

    pub fn backend(&self) -> &Arc<Backend> {
        &self.backend
    }

    /// Connection and session counts; the receiver wakes on every change.
    pub fn stats(&self) -> watch::Receiver<Stats> {
        self.backend.stats()
    }

    /// Stops accepting, closes every connection and waits for them.
    pub async fn shutdown(self) {
        self.shutdown.send_replace(true);
        let _ = self.task.await;
    }
}

/// Answers 503 off the accept loop, so a slow refused client cannot stall it.
/// The request head is read first: closing a socket with unread data resets
/// the connection and can destroy the response before the client reads it.
fn refuse(mut stream: TcpStream, refusals: &Arc<Semaphore>) {
    let Ok(permit) = Arc::clone(refusals).try_acquire_owned() else {
        return;
    };
    tokio::spawn(async move {
        let _permit = permit;
        let answer = async {
            let mut head = [0_u8; 2048];
            let _ = stream.read(&mut head).await;
            let _ = stream.write_all(BUSY_RESPONSE).await;
            let _ = stream.shutdown().await;
        };
        let _ = timeout(REFUSAL_TIMEOUT, answer).await;
    });
}

/// Releases the connection slot and the count however the task ends.
struct ConnectionGuard {
    backend: Arc<Backend>,
    _permit: OwnedSemaphorePermit,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.backend.connection_closed();
    }
}

async fn accept_loop(listener: TcpListener, env: Env, max_connections: usize) {
    let permits = Arc::new(Semaphore::new(max_connections));
    let refusals = Arc::new(Semaphore::new(MAX_REFUSALS));
    let mut shutdown = env.shutdown.clone();
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            () = crate::connection::signalled(&mut shutdown) => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    // Out of descriptors or a connection reset before accept:
                    // neither is a reason to stop serving everyone else.
                    continue;
                };
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                    refuse(stream, &refusals);
                    continue;
                };
                let env = Env {
                    backend: Arc::clone(&env.backend),
                    registry: Arc::clone(&env.registry),
                    shutdown: env.shutdown.clone(),
                    local_addr: env.local_addr,
                };
                env.backend.connection_opened();
                let guard = ConnectionGuard {
                    backend: Arc::clone(&env.backend),
                    _permit: permit,
                };
                connections.spawn(async move {
                    http::serve(stream, env).await;
                    drop(guard);
                });
            }
        }
    }
    drop(listener);
    while connections.join_next().await.is_some() {}
}
