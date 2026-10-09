//! Evaluating script in a page's realm from the host, with remote objects,
//! awaited promises and bindings (`docs/spec/11-library-api.md` §1).
//!
//! Every operation is an entry into the page's retained engine
//! ([`Page::with_engine`]), so it runs on the page thread under the per-entry
//! deadline and script budget, in the realm the page's own scripts run in. The
//! value model and the realm-side logic are `tenta_js::inspect`; this module is
//! the asynchronous shell around it.
//!
//! # Awaiting a promise
//!
//! The entry that evaluates attaches a reaction to the promise and returns at
//! once, leaving a waiter in the runtime's [`AwaitTable`]. The page thread keeps
//! turning the engine as usual, and when the reaction runs the waiter's channel
//! delivers the settlement to the caller. Nothing blocks the page thread, and a
//! timer, a fetch or a settle call the promise depends on all keep running. The
//! wait ends with the settlement, with the caller's timeout, or when the engine
//! ends (a navigation or a close), in which case the waiter's channel closes.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tenta_core::{Error, Result};
use tenta_js::inspect::{
    self, CallArgument, Completion, ExceptionDetails, InspectError, InspectErrorKind,
    PropertyDescriptor, PropertyQuery, RemoteObject, RemoteValue, Settings,
};
use tokio::sync::oneshot;

use super::{Page, PageInner};
use crate::runtime::{self, Settlement};

/// Which script world an operation runs in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum World {
    /// The world the page's own scripts run in.
    #[default]
    Main,
    /// A named world that shares the DOM but has its own global object. The
    /// engines have one world per realm today, so asking for this fails with
    /// [`Error::Unsupported`].
    Isolated(String),
}

/// How an evaluation or function call runs and what it returns.
#[derive(Clone, Debug, Default)]
pub struct EvalOptions {
    /// When the result is a promise, wait for it to settle and return what it
    /// settled with; a rejection is an exception.
    pub await_promise: bool,
    /// Copy the result by value instead of returning a handle for an object.
    /// Primitives are always returned by value.
    pub return_by_value: bool,
    /// Attach a preview to a returned handle.
    pub generate_preview: bool,
    /// The group a returned handle joins, for [`Page::release_object_group`].
    pub object_group: Option<String>,
    pub world: World,
    /// The deadline for the script and for the wait on a promise. Defaults to
    /// the page's `evaluate_time` budget.
    pub timeout: Option<Duration>,
}

/// How an evaluation ended in the realm.
#[derive(Clone, Debug, PartialEq)]
pub struct Evaluation {
    completion: Completion,
}

impl Evaluation {
    pub fn completion(&self) -> &Completion {
        &self.completion
    }

    pub fn into_completion(self) -> Completion {
        self.completion
    }

    /// The exception the script threw, if it did.
    pub fn exception(&self) -> Option<&ExceptionDetails> {
        match &self.completion {
            Completion::Threw(details) => Some(details),
            Completion::Value(_) => None,
        }
    }

    /// The result converted to `T`. An exception is [`Error::Script`].
    pub fn into_value<T: FromRemote>(self) -> Result<T> {
        match self.completion {
            Completion::Value(value) => T::from_remote(value),
            Completion::Threw(details) => Err(Error::Script {
                message: details.message,
                stack: details.stack,
            }),
        }
    }

    /// The result as a handle to an object that stayed in the realm.
    pub fn into_handle(self) -> Result<RemoteObject> {
        let value = self.into_value::<RemoteValue>()?;
        value
            .into_handle()
            .ok_or_else(|| Error::invalid("the result is a primitive, not an object"))
    }
}

/// A type an evaluation's result can be read as.
pub trait FromRemote: Sized {
    fn from_remote(value: RemoteValue) -> Result<Self>;
}

impl FromRemote for RemoteValue {
    fn from_remote(value: RemoteValue) -> Result<Self> {
        Ok(value)
    }
}

impl FromRemote for String {
    fn from_remote(value: RemoteValue) -> Result<Self> {
        match value {
            RemoteValue::String(text) => Ok(text),
            other => Err(Error::invalid(format!("expected a string, got {other:?}"))),
        }
    }
}

impl FromRemote for f64 {
    fn from_remote(value: RemoteValue) -> Result<Self> {
        value
            .as_f64()
            .ok_or_else(|| Error::invalid(format!("expected a number, got {value:?}")))
    }
}

impl FromRemote for bool {
    fn from_remote(value: RemoteValue) -> Result<Self> {
        value
            .as_bool()
            .ok_or_else(|| Error::invalid(format!("expected a boolean, got {value:?}")))
    }
}

/// What to run.
enum Job {
    Expression(String),
    Call {
        target: Option<String>,
        function: String,
        arguments: Vec<CallArgument>,
    },
}

/// What the entry produced on the page thread. Moved once, never stored.
#[allow(clippy::large_enum_variant)]
enum Entry {
    Done(Completion),
    Waiting(oneshot::Receiver<Settlement>),
}

/// Removes a waiter from the page's table when the wait ends for any reason,
/// including the caller dropping the future.
struct WaiterGuard {
    inner: Arc<PageInner>,
    ticket: u64,
}

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        let page = self.inner.config.id;
        let ticket = self.ticket;
        // A thread that is gone took the table with it.
        let _ = self.inner.config.thread.post_task(Box::new(move || {
            runtime::with_runtime(page, |runtime| runtime.awaits().cancel(ticket));
        }));
    }
}

fn next_ticket() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Page {
    /// Evaluates `expression` as a script in the page's main world. Objects come
    /// back as handles; use [`Page::evaluate_with`] for the other shapes.
    pub async fn evaluate(&self, expression: &str) -> Result<Evaluation> {
        self.evaluate_with(expression, EvalOptions::default()).await
    }

    /// Evaluates `expression` as a script. A script exception is part of the
    /// returned [`Evaluation`]; an `Err` means the evaluation could not run or
    /// finish: the page is closed, the deadline passed, a handle was stale, or a
    /// navigation ended the realm while a promise was awaited.
    pub async fn evaluate_with(
        &self,
        expression: &str,
        options: EvalOptions,
    ) -> Result<Evaluation> {
        self.run(Job::Expression(expression.to_owned()), options)
            .await
    }

    /// Calls `function`, the source of a function expression or declaration,
    /// with `this` bound to `target` (the global object when `None`).
    pub async fn call_function_on(
        &self,
        target: Option<&RemoteObject>,
        function: &str,
        arguments: Vec<CallArgument>,
        options: EvalOptions,
    ) -> Result<Evaluation> {
        self.call_function_on_id(
            target.map(|object| object.id.as_str()),
            function,
            arguments,
            options,
        )
        .await
    }

    /// [`Page::call_function_on`] for a receiver known only by its id, such as
    /// one a remote client names.
    pub async fn call_function_on_id(
        &self,
        target: Option<&str>,
        function: &str,
        arguments: Vec<CallArgument>,
        options: EvalOptions,
    ) -> Result<Evaluation> {
        let job = Job::Call {
            target: target.map(str::to_owned),
            function: function.to_owned(),
            arguments,
        };
        self.run(job, options).await
    }

    /// The members of the object `id` names, read without running any getter of
    /// the page. The handles in the result join the object's group.
    pub async fn get_properties(
        &self,
        id: &str,
        query: PropertyQuery,
    ) -> Result<Vec<PropertyDescriptor>> {
        let id = id.to_owned();
        let timeout = self.entry_timeout();
        self.with_engine(move |engine| inspect::properties(engine, &id, query, timeout))
            .await?
            .map_err(inspect_error)
    }

    /// Whether a navigation is replacing the document right now. Script cannot
    /// run in the page until it finishes installing it.
    pub fn navigation_in_flight(&self) -> bool {
        self.inner.state.lock().navigations_in_flight > 0
    }

    /// Resolves once no navigation is in flight. The caller bounds the wait.
    pub async fn navigation_settled(&self) {
        loop {
            let done = self.inner.navigation_done.notified();
            tokio::pin!(done);
            done.as_mut().enable();
            if !self.navigation_in_flight() {
                return;
            }
            done.await;
        }
    }

    /// The number of the script realm the page runs now: the number remote
    /// object ids, console calls and binding calls carry. A realm lives as long
    /// as its document, so a new number means the old context is gone.
    ///
    /// Builds the engine of a document that has none yet, and waits for a
    /// navigation in flight to finish installing its document first.
    pub async fn execution_context(&self) -> Result<u64> {
        loop {
            self.navigation_settled().await;
            let timeout = self.entry_timeout();
            let realm = self
                .with_engine(move |engine| inspect::realm(engine, timeout))
                .await;
            match realm {
                // A navigation began between the wait and the entry.
                Err(Error::InvalidArgument { .. }) if self.navigation_in_flight() => {}
                other => return other?.map_err(inspect_error),
            }
        }
    }

    /// Releases one remote object by id. Reports whether it was still held.
    pub async fn release_object(&self, id: &str) -> Result<bool> {
        let id = id.to_owned();
        let timeout = self.entry_timeout();
        self.with_engine(move |engine| inspect::release(engine, &id, timeout))
            .await?
            .map_err(inspect_error)
    }

    /// Releases every remote object of `group`.
    pub async fn release_object_group(&self, group: &str) -> Result<()> {
        let group = group.to_owned();
        let timeout = self.entry_timeout();
        self.with_engine(move |engine| inspect::release_group(engine, &group, timeout))
            .await?
            .map_err(inspect_error)
    }

    /// How many remote objects the page's realm holds.
    pub async fn live_remote_objects(&self) -> Result<usize> {
        let timeout = self.entry_timeout();
        self.with_engine(move |engine| inspect::live_handles(engine, timeout))
            .await?
            .map_err(inspect_error)
    }

    /// Installs a global function `name` in the main world, and in every
    /// document the page loads afterwards, that reports its one string argument
    /// as [`crate::PageEvent::BindingCalled`].
    pub async fn add_binding(&self, name: &str) -> Result<()> {
        if !is_identifier(name) {
            return Err(Error::invalid(format!(
                "{name:?} is not a valid binding name"
            )));
        }
        {
            let mut bindings = self.inner.bindings.lock();
            if !bindings.iter().any(|existing| existing == name) {
                bindings.push(name.to_owned());
            }
        }
        if !self.inner.state.lock().engine_held {
            // The engine built for the next script or evaluation installs it.
            return Ok(());
        }
        let name = name.to_owned();
        let timeout = self.entry_timeout();
        self.with_engine(move |engine| inspect::bind(engine, &name, timeout))
            .await?
            .map_err(inspect_error)
    }

    /// Removes a binding installed by [`Page::add_binding`].
    pub async fn remove_binding(&self, name: &str) -> Result<()> {
        self.inner
            .bindings
            .lock()
            .retain(|existing| existing != name);
        if !self.inner.state.lock().engine_held {
            return Ok(());
        }
        let name = name.to_owned();
        let timeout = self.entry_timeout();
        self.with_engine(move |engine| inspect::unbind(engine, &name, timeout))
            .await?
            .map_err(inspect_error)
    }

    fn entry_timeout(&self) -> Duration {
        self.inner.config.budgets.evaluate_time
    }

    async fn run(&self, job: Job, options: EvalOptions) -> Result<Evaluation> {
        if let World::Isolated(name) = &options.world {
            return Err(Error::unsupported(format!(
                "isolated world {name:?}: the engines run one world per realm"
            )));
        }
        let timeout = options.timeout.unwrap_or_else(|| self.entry_timeout());
        let ticket = options.await_promise.then(next_ticket);
        let settings = Settings {
            by_value: options.return_by_value,
            preview: options.generate_preview,
            group: options.object_group,
            await_ticket: ticket,
        };
        let entry = self
            .with_engine_awaiting(move |engine, awaits| {
                let waiter = ticket.map(|ticket| {
                    let (sender, receiver) = oneshot::channel();
                    // Registered before the entry: a promise that is already
                    // settled reports during the entry itself.
                    awaits.register(ticket, sender);
                    receiver
                });
                let entered = match job {
                    Job::Expression(expression) => {
                        inspect::evaluate(engine, &expression, &settings, timeout)
                    }
                    Job::Call {
                        target,
                        function,
                        arguments,
                    } => inspect::call_function(
                        engine,
                        target.as_deref(),
                        &function,
                        &arguments,
                        &settings,
                        timeout,
                    ),
                };
                match (entered, waiter) {
                    (Ok(inspect::Entered::Pending), Some(waiter)) => Ok(Entry::Waiting(waiter)),
                    (Ok(inspect::Entered::Done(completion)), _) => {
                        if let Some(ticket) = ticket {
                            awaits.cancel(ticket);
                        }
                        Ok(Entry::Done(completion))
                    }
                    (Ok(inspect::Entered::Pending), None) => Err(InspectError {
                        kind: InspectErrorKind::Internal,
                        message: "a promise was awaited that nobody asked to await".to_owned(),
                    }),
                    (Err(error), _) => {
                        if let Some(ticket) = ticket {
                            awaits.cancel(ticket);
                        }
                        Err(error)
                    }
                }
            })
            .await?
            .map_err(inspect_error)?;
        let completion = match entry {
            Entry::Done(completion) => completion,
            Entry::Waiting(receiver) => {
                let ticket = ticket.expect("a waiter exists only for an awaited evaluation");
                let _guard = WaiterGuard {
                    inner: Arc::clone(&self.inner),
                    ticket,
                };
                self.wait(receiver, timeout).await?
            }
        };
        Ok(Evaluation { completion })
    }

    async fn wait(
        &self,
        receiver: oneshot::Receiver<Settlement>,
        timeout: Duration,
    ) -> Result<Completion> {
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(settlement)) => settlement.map_err(inspect_error),
            Ok(Err(_)) => {
                if self.inner.state.lock().closed {
                    Err(Error::Closed)
                } else {
                    Err(context_destroyed())
                }
            }
            Err(_) => Err(Error::timeout(
                "awaiting a promise",
                timeout.as_millis() as u64,
            )),
        }
    }
}

fn context_destroyed() -> Error {
    Error::Other {
        code: "execution_context_destroyed".to_owned(),
        message: "the execution context was destroyed by a navigation".to_owned(),
    }
}

fn inspect_error(error: InspectError) -> Error {
    let code = match error.kind {
        InspectErrorKind::StaleHandle => return context_destroyed(),
        InspectErrorKind::UnknownHandle | InspectErrorKind::InvalidArgument => {
            return Error::InvalidArgument {
                reason: error.message,
            };
        }
        InspectErrorKind::HandleLimit => "handle_limit",
        InspectErrorKind::NotSerializable => "not_serializable",
        InspectErrorKind::Terminated => "script_terminated",
        _ => "internal",
    };
    Error::Other {
        code: code.to_owned(),
        message: error.message,
    }
}

/// A name `window[name] = ...` can introduce without surprises: an ASCII
/// identifier.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_' || first == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}
