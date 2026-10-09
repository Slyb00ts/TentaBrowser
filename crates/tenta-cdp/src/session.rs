//! A session: the addressable unit a command is routed to.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;

use crate::backend::{Backend, Target, lock};
use crate::wire::event_message;

/// The connection itself (no `sessionId` on the wire) or one flattened
/// attachment to a target.
///
/// Domain handlers keep per-session state in [`Session::state`] and run
/// background work with [`Session::spawn`]; both are released when the
/// session detaches or the connection closes, so handlers need no teardown
/// hook.
pub struct Session {
    id: Option<String>,
    parent: Option<String>,
    target: Option<Arc<Target>>,
    out: mpsc::Sender<Message>,
    backend: Arc<Backend>,
    state: Mutex<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    closed: watch::Sender<bool>,
    /// Completion of the newest command: the next command waits for it.
    tail: Mutex<Option<oneshot::Receiver<()>>>,
}

/// Held by a running command; dropping it lets the session's next command
/// start.
pub(crate) type Turn = oneshot::Sender<()>;

impl Session {
    pub(crate) fn new(
        backend: Arc<Backend>,
        id: Option<String>,
        parent: Option<String>,
        target: Option<Arc<Target>>,
        out: mpsc::Sender<Message>,
    ) -> Arc<Self> {
        if let Some(target) = &target {
            target.session_attached();
        }
        if id.is_some() {
            backend.session_opened();
        }
        Arc::new(Self {
            id,
            parent,
            target,
            out,
            backend,
            state: Mutex::new(HashMap::new()),
            closed: watch::channel(false).0,
            tail: Mutex::new(None),
        })
    }

    /// `None` for the connection's own session.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// The session whose command created this one; `None` for the
    /// connection's own session as parent.
    pub fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    /// The page this session drives; `None` for the browser session.
    pub fn target(&self) -> Option<&Arc<Target>> {
        self.target.as_ref()
    }

    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// The session's value of type `T`, created with `T::default()` on first
    /// use. Interior mutability is the handler's business.
    pub fn state<T: Default + Send + Sync + 'static>(&self) -> Arc<T> {
        let mut state = lock(&self.state);
        let entry = state
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Arc::new(T::default()));
        Arc::clone(entry)
            .downcast::<T>()
            .expect("the entry is keyed by the type it holds")
    }

    /// Sends an event on this session, in order with every other message the
    /// session sends. Does nothing once the session is closed.
    pub async fn emit(&self, method: &str, params: Value) {
        if self.is_closed() {
            return;
        }
        self.send(event_message(method, params, self.id())).await;
    }

    /// Runs `task` until it completes or the session closes, whichever is
    /// first.
    pub fn spawn<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut closed = self.closed.subscribe();
        tokio::spawn(async move {
            // The wait is wrapped so its read guard on the close signal is
            // gone before `select!` drops `task`: dropping the last
            // `Arc<Session>` captured by `task` writes that same signal.
            let wait_closed = async {
                let _ = closed.wait_for(|closed| *closed).await;
            };
            tokio::select! {
                () = wait_closed => {}
                () = task => {}
            }
        });
    }

    pub(crate) async fn send(&self, message: Message) {
        // A closed channel means the connection is going away; the reader
        // loop notices that on its own.
        let _ = self.out.send(message).await;
    }

    pub(crate) fn next_turn(&self) -> (Option<oneshot::Receiver<()>>, Turn) {
        let (turn, queued) = oneshot::channel();
        let previous = lock(&self.tail).replace(queued);
        (previous, turn)
    }

    /// Idempotent.
    pub(crate) fn close(&self) {
        if self.closed.send_replace(true) {
            return;
        }
        if let Some(target) = &self.target {
            target.session_detached();
        }
        if self.id.is_some() {
            self.backend.session_closed();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}
