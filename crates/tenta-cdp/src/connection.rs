//! One WebSocket connection: framing, session table, in-order dispatch.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::FutureExt;
use futures_util::sink::SinkExt;
use futures_util::stream::{SplitSink, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep_until, timeout};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use crate::backend::{Backend, Target, lock};
use crate::domain::{CommandContext, DomainRegistry};
use crate::error::CdpError;
use crate::session::Session;
use crate::wire::{self, Command, Inbound, error_message, result_message};

/// Outbound messages queued per connection before senders wait.
const OUTBOUND_QUEUE: usize = 1024;
/// Commands in flight per connection; beyond it a command is refused.
const MAX_PENDING_COMMANDS: usize = 1024;
/// How long a closing connection waits for its close frame to go out.
const CLOSE_GRACE: Duration = Duration::from_secs(1);
/// How long one outbound message may take to be accepted by the socket.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// How an attachment is named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionIdKind {
    /// `"{targetId}-session"`, the id auto-attached sessions use.
    Managed,
    /// A fresh unique id, so a client can open several raw sessions on one
    /// target.
    Explicit,
}

/// The sessions of one client connection.
pub struct Connection {
    backend: Arc<Backend>,
    out: mpsc::Sender<Message>,
    root: Arc<Session>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

impl Connection {
    fn new(
        backend: Arc<Backend>,
        out: mpsc::Sender<Message>,
        root_target: Option<Arc<Target>>,
    ) -> Arc<Self> {
        let root = Session::new(Arc::clone(&backend), None, None, root_target, out.clone());
        Arc::new(Self {
            backend,
            out,
            root,
            sessions: Mutex::new(HashMap::new()),
        })
    }

    pub fn backend(&self) -> &Arc<Backend> {
        &self.backend
    }

    /// The session of messages without a `sessionId`.
    pub fn root(&self) -> &Arc<Session> {
        &self.root
    }

    pub fn session(&self, id: &str) -> Option<Arc<Session>> {
        lock(&self.sessions).get(id).cloned()
    }

    /// Sessions attached through this connection.
    pub fn sessions(&self) -> Vec<Arc<Session>> {
        lock(&self.sessions).values().cloned().collect()
    }

    /// Attaches a flattened session to `target`, on behalf of `parent`
    /// (`None` is the connection's own session).
    pub fn attach(
        &self,
        target: &Arc<Target>,
        parent: Option<&str>,
        kind: SessionIdKind,
    ) -> Result<Arc<Session>, CdpError> {
        let id = match kind {
            SessionIdKind::Managed => format!("{}-session", target.id()),
            SessionIdKind::Explicit => crate::ids::random_id(),
        };
        let mut sessions = lock(&self.sessions);
        if sessions.contains_key(&id) {
            return Err(CdpError::server("Session with given id already exists"));
        }
        let session = Session::new(
            Arc::clone(&self.backend),
            Some(id.clone()),
            parent.map(str::to_owned),
            Some(Arc::clone(target)),
            self.out.clone(),
        );
        sessions.insert(id, Arc::clone(&session));
        Ok(session)
    }

    /// Closes a session and tells its parent with `Target.detachedFromTarget`.
    /// `None` when there is no such session.
    pub async fn detach(&self, id: &str) -> Option<Arc<Session>> {
        let session = lock(&self.sessions).remove(id)?;
        session.close();
        let parent = match session.parent() {
            Some(parent) => self.session(parent),
            None => Some(Arc::clone(&self.root)),
        };
        if let (Some(parent), Some(target)) = (parent, session.target()) {
            parent
                .emit(
                    "Target.detachedFromTarget",
                    json!({ "sessionId": id, "targetId": target.id() }),
                )
                .await;
        }
        Some(session)
    }

    /// Detaches the sessions whose target is gone. `false` when the target of
    /// a page-scoped connection itself is gone.
    async fn reconcile(&self) -> bool {
        let orphans: Vec<String> = self
            .sessions()
            .into_iter()
            .filter(|session| session.target().is_some_and(|t| self.is_gone(t)))
            .filter_map(|session| session.id().map(str::to_owned))
            .collect();
        for id in orphans {
            self.detach(&id).await;
        }
        !self
            .root
            .target()
            .is_some_and(|target| self.is_gone(target))
    }

    fn is_gone(&self, target: &Arc<Target>) -> bool {
        self.backend.target(target.id()).is_none()
    }

    fn close_all(&self) {
        let sessions: Vec<_> = lock(&self.sessions).drain().map(|(_, s)| s).collect();
        for session in sessions {
            session.close();
        }
        self.root.close();
    }
}

type Sink = SplitSink<WebSocketStream<TcpStream>, Message>;

async fn write_loop(mut sink: Sink, mut queue: mpsc::Receiver<Message>) {
    while let Some(message) = queue.recv().await {
        let last = matches!(message, Message::Close(_));
        // A client that stops reading must not pin this task, the queue
        // behind it and the connection slot forever.
        match timeout(WRITE_TIMEOUT, sink.send(message)).await {
            Ok(Ok(())) if !last => {}
            _ => break,
        }
    }
    let _ = timeout(WRITE_TIMEOUT, sink.close()).await;
}

pub(crate) async fn signalled(shutdown: &mut watch::Receiver<bool>) {
    // A dropped sender means the server is gone, which is a shutdown too.
    let _ = shutdown.wait_for(|stop| *stop).await;
}

/// Drives a connection until the client leaves, the server shuts down or the
/// page of a page-scoped connection closes.
pub(crate) async fn serve(
    socket: WebSocketStream<TcpStream>,
    backend: Arc<Backend>,
    registry: Arc<DomainRegistry>,
    mut shutdown: watch::Receiver<bool>,
    root_target: Option<Arc<Target>>,
) {
    let (sink, mut stream) = socket.split();
    let (out, queue) = mpsc::channel(OUTBOUND_QUEUE);
    let mut writer = tokio::spawn(write_loop(sink, queue));
    let mut targets = backend.subscribe();
    let connection = Connection::new(Arc::clone(&backend), out.clone(), root_target);
    let mut commands: JoinSet<()> = JoinSet::new();
    let idle_timeout = backend.options().idle_limit();
    let mut last_activity = Instant::now();
    let mut writer_done = false;

    loop {
        // Everything that can wait on the outbound queue runs inside `step`,
        // so a client that stopped reading cannot keep the connection from
        // noticing a shutdown or a dead writer.
        let step = async {
            tokio::select! {
                message = stream.next() => {
                    last_activity = Instant::now();
                    match message {
                        Some(Ok(Message::Text(text))) => {
                            handle_text(&connection, &registry, &mut commands, text.as_str()).await;
                            true
                        }
                        Some(Ok(Message::Binary(_))) => {
                            let error = CdpError::new(CdpError::PARSE_ERROR, "Message must be text");
                            connection.root.send(error_message(None, None, &error)).await;
                            true
                        }
                        // tungstenite answers pings itself while it reads.
                        Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => true,
                        Some(Ok(Message::Close(_)) | Err(_)) | None => false,
                    }
                }
                Some(_) = commands.join_next(), if !commands.is_empty() => true,
                event = targets.recv() => {
                    if let Err(broadcast::error::RecvError::Closed) = event {
                        return false;
                    }
                    // Lagging loses events, so check the whole table instead.
                    connection.reconcile().await
                }
                () = async {
                    match idle_timeout {
                        Some(limit) => sleep_until(last_activity + limit).await,
                        None => std::future::pending().await,
                    }
                } => false,
            }
        };
        tokio::select! {
            keep_going = step => {
                if !keep_going {
                    break;
                }
            }
            () = signalled(&mut shutdown) => break,
            _ = &mut writer => {
                writer_done = true;
                break;
            }
        }
    }

    // Abort in-flight commands first: they hold senders into the queue.
    commands.shutdown().await;
    connection.close_all();
    // A full queue means a stalled client; the grace period below ends it.
    let _ = out.try_send(Message::Close(None));
    drop(out);
    drop(connection);
    // A finished JoinHandle must not be polled again: tokio panics on it.
    // The reset test cannot force the writer to finish before the reader, so
    // it will not catch this guard being removed.
    if !writer_done && timeout(CLOSE_GRACE, &mut writer).await.is_err() {
        writer.abort();
    }
}

async fn handle_text(
    connection: &Arc<Connection>,
    registry: &DomainRegistry,
    commands: &mut JoinSet<()>,
    text: &str,
) {
    let command = match wire::parse(text) {
        Inbound::Command(command) => command,
        Inbound::Rejected {
            id,
            session_id,
            error,
        } => {
            connection
                .root
                .send(error_message(id, session_id.as_deref(), &error))
                .await;
            return;
        }
    };
    let Command {
        id,
        session_id,
        method,
        params,
    } = command;
    let session = match session_id.as_deref() {
        None => Arc::clone(&connection.root),
        Some(session_id) => match connection.session(session_id) {
            Some(session) => session,
            None => {
                let error = CdpError::session_not_found();
                connection
                    .root
                    .send(error_message(Some(id), Some(session_id), &error))
                    .await;
                return;
            }
        },
    };
    let handler = method
        .split_once('.')
        .and_then(|(domain, name)| registry.lookup(domain, name));
    let Some(handler) = handler else {
        let error = CdpError::method_not_found(&method);
        session
            .send(error_message(Some(id), session.id(), &error))
            .await;
        return;
    };
    if commands.len() >= MAX_PENDING_COMMANDS {
        let error = CdpError::server("Too many commands are pending on this connection");
        session
            .send(error_message(Some(id), session.id(), &error))
            .await;
        return;
    }

    // The turn is claimed here, on the reader task, so commands of a session
    // queue in the order they arrived whatever the scheduler does later.
    let limit = connection.backend().options().command_timeout;
    let (previous, turn) = session.next_turn();
    let cx = CommandContext::new(
        Arc::clone(connection.backend()),
        Arc::clone(connection),
        Arc::clone(&session),
        turn,
    );
    commands.spawn(async move {
        if let Some(previous) = previous {
            // Resolves when the previous command finished or released the
            // session, and errors if its task was dropped; both mean "go".
            let _ = previous.await;
        }
        let name = method.split_once('.').map_or("", |(_, name)| name);
        let run = AssertUnwindSafe(handler.handle(cx.clone(), name, params)).catch_unwind();
        let outcome = match timeout(limit, run).await {
            Ok(finished) => {
                finished.unwrap_or_else(|_| Err(CdpError::internal("The command handler panicked")))
            }
            Err(_) => Err(CdpError::server(format!("{method} timed out"))),
        };
        // A session that detached while the command ran has no client-side
        // counterpart left to answer.
        if !session.is_closed() {
            session.send(response(id, session.id(), outcome)).await;
        }
        cx.release_order();
    });
}

fn response(id: u64, session_id: Option<&str>, outcome: Result<Value, CdpError>) -> Message {
    match outcome {
        Ok(result) => result_message(id, session_id, result),
        Err(error) => error_message(Some(id), session_id, &error),
    }
}
