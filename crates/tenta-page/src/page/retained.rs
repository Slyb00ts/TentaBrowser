//! The engine a page keeps after its navigation settles
//! (`docs/spec/02-architecture.md` §3, §5.1).
//!
//! A navigation that built an engine does not release it when it settles: the
//! document, its timers and its pending completions stay bound to one realm,
//! so later host entries and the page's own deferred work run where the page's
//! scripts ran. While the page keeps an engine the tree is the engine's
//! (`PageState::engine_held`), and everything that touches either lives on the
//! page thread.
//!
//! # Who drives the engine
//!
//! Exactly one party turns a retained runtime at a time. Between settle calls
//! it is the pump: a task that runs a turn, then parks until the engine's next
//! deadline or until the settle tracker signals new work, so an idle page costs
//! nothing. While a settle call runs, that call's loop turns the engine itself
//! and the pump is stopped, because two waiters on the tracker's notification
//! would each steal the other's wake.
//!
//! # When the engine ends
//!
//! The next navigation commits (its response head arrives, or an error page
//! replaces the document), the page closes, the page hands its document out, or
//! the page crashes. Each drops the runtime on the page thread, engine first.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use tenta_core::{Error, Result, Tier};
use tenta_dom::{Document as DomDocument, SharedDocument};
use tenta_js::engine::PageEngine;
use tokio::sync::{Notify, oneshot};

use super::{
    MINIMUM_PARK, Page, PageInner, ScriptBudget, TIMERS_PER_TURN, TreeHandle, Weak,
    native_memory_budget_error, settle_loop,
};
use crate::runtime::{self, AwaitTable, PageRuntime, Pump};
use crate::settle::{Settle, SettleState};
use crate::task::{catch_unwind_future, catch_unwind_task, panic_message};

impl Page {
    /// Fails unless this page may run script.
    pub(super) fn require_javascript(&self) -> Result<()> {
        if self.inner.config.max_tier < Tier::Js {
            return Err(Error::TierDenied {
                needed: Tier::Js,
                ceiling: self.inner.config.max_tier,
            });
        }
        if !self.inner.config.javascript {
            return Err(Error::unsupported(
                "this page was created with JavaScript disabled",
            ));
        }
        Ok(())
    }

    /// Runs `entry` inside the page's engine, on the page thread, and returns
    /// what it returned.
    ///
    /// The entry runs under the page's per-entry deadline and script budget, with
    /// the engine's native-failure check afterwards, exactly like the entries
    /// the page makes itself; a runaway entry is terminated and reported as a
    /// [`crate::PageEvent::ScriptTerminated`]. A page that has a document but
    /// no engine, because none of its scripts needed one, gets an engine bound
    /// to that document first. Work the entry schedules wakes the page's driver,
    /// so timers and completions it starts run without anyone asking.
    ///
    /// Work that should hold a later [`Page::wait_for_settle`] open has to be
    /// started by an entry the engine tags as tracked (`PageEngine::run_script`
    /// does); an entry such as `PageEngine::evaluate` starts background work.
    ///
    /// Fails with [`Error::Closed`] on a closed page, [`Error::Crashed`] when the
    /// entry crashed it, and [`Error::InvalidArgument`] when there is no
    /// document yet or a navigation is replacing it. Dropping the returned
    /// future does not cancel an entry already queued.
    pub async fn with_engine<R, F>(&self, entry: F) -> Result<R>
    where
        F: FnOnce(&mut dyn PageEngine) -> R + Send + 'static,
        R: Send + 'static,
    {
        self.with_runtime(move |runtime| runtime.run_in_engine(entry))
            .await
    }

    /// [`Page::with_engine`] for an entry that may leave an evaluation waiting
    /// on a promise of the engine's realm.
    pub(super) async fn with_engine_awaiting<R, F>(&self, entry: F) -> Result<R>
    where
        F: FnOnce(&mut dyn PageEngine, &AwaitTable) -> R + Send + 'static,
        R: Send + 'static,
    {
        self.with_runtime(move |runtime| runtime.run_in_engine_awaiting(entry))
            .await
    }

    async fn with_runtime<R, F>(&self, entry: F) -> Result<R>
    where
        F: FnOnce(&mut PageRuntime) -> R + Send + 'static,
        R: Send + 'static,
    {
        self.require_javascript()?;
        let inner = Arc::clone(&self.inner);
        let (reply, answer) = oneshot::channel();
        self.inner.config.thread.post_task(Box::new(move || {
            let _ = reply.send(enter_engine(&inner, entry));
        }))?;
        answer.await.map_err(|_| Error::Closed)?
    }

    /// Waits until `wait_until` holds for the work the page has in motion now.
    ///
    /// The settle machine a navigation used keeps accounting for what the page
    /// started afterwards, so this sees timers and requests begun after `load`.
    /// A page with no engine has nothing in motion and settles at once. A
    /// deadline that expires is reported as [`SettleState::Timeout`] rather than
    /// an error, like a navigation's. A newer `goto` or a close ends the wait
    /// with an aborted-navigation error.
    pub async fn wait_for_settle(
        &self,
        wait_until: Settle,
        timeout: Duration,
    ) -> Result<SettleState> {
        let inner = Arc::clone(&self.inner);
        let (reply, answer) = oneshot::channel::<Result<SettleState>>();
        self.inner.config.thread.post_job(Box::new(move || {
            Box::pin(async move {
                let mut reply = reply;
                // The caller dropping its future ends the wait: nothing reads
                // the answer, and the page's driver should not stay stopped for it.
                let settled = tokio::select! {
                    biased;
                    () = reply.closed() => return,
                    settled = catch_unwind_future(settle_retained(&inner, wait_until, timeout)) => settled,
                };
                let outcome = settled.unwrap_or_else(|payload| {
                    let reason = panic_message(&payload);
                    crash(&inner, &reason);
                    Err(Error::Crashed { reason })
                });
                let _ = reply.send(outcome);
            })
        }))?;
        answer.await.map_err(|_| Error::Closed)?
    }
}

/// Ends the engine the page keeps and moves its tree back into the page, on the
/// page thread. Nothing happens when the page keeps no engine.
pub(super) async fn release_engine(inner: &Arc<PageInner>) -> Result<()> {
    if !inner.state.lock().engine_held {
        return Ok(());
    }
    let task_inner = Arc::clone(inner);
    let (reply, answer) = oneshot::channel();
    inner.config.thread.post_task(Box::new(move || {
        if let Err(reason) = catch_unwind_task(|| release(&task_inner)) {
            crash(&task_inner, &reason);
        }
        let _ = reply.send(());
    }))?;
    answer.await.map_err(|_| Error::Closed)
}

/// The synchronous half of [`release_engine`], also what a committing
/// navigation does to the document it replaces.
///
/// The tree goes back into the page rather than away, so a navigation that is
/// dropped after its response head still leaves the previous document readable.
pub(super) fn release(inner: &PageInner) {
    if !inner.state.lock().engine_held {
        return;
    }
    // The state lock is not held across the drop: an engine tearing down may
    // report to the page.
    let document =
        runtime::take_runtime(inner.config.id).and_then(runtime::PageRuntime::into_document);
    let mut state = inner.state.lock();
    state.engine_held = false;
    if state.crashed.is_none() {
        if document.is_none() {
            tracing::error!(
                page = ?inner.config.id,
                "the document was still shared when its engine ended; it is lost"
            );
        }
        state.document = document;
    }
}

/// Drops everything the page holds on the page thread: its engine and its
/// document. The page thread's own task wrapper only logs a panic, which would
/// leave a page that crashed looking merely closed, so the failure is recorded
/// on the page here.
pub(super) fn teardown(inner: &Arc<PageInner>) {
    let outcome = catch_unwind_task(|| {
        #[cfg(feature = "render")]
        crate::render::release(inner.config.id);
        drop(runtime::take_runtime(inner.config.id));
        let document = {
            let mut state = inner.state.lock();
            state.engine_held = false;
            state.document.take()
        };
        drop(document);
    });
    if let Err(reason) = outcome {
        inner.mark_crashed(&reason);
    }
}

/// [`teardown`] without waiting, for a close that has no await to wait with.
pub(super) fn teardown_later(inner: &Arc<PageInner>) {
    let task_inner = Arc::clone(inner);
    // A thread that is gone took the engine with it.
    let _ = inner
        .config
        .thread
        .post_task(Box::new(move || teardown(&task_inner)));
}

/// Marks the page crashed and discards its engine.
fn crash(inner: &Arc<PageInner>, reason: &str) {
    inner.mark_crashed(reason);
    discard_crashed_engine(inner);
}

/// Drops the engine of a page that has crashed. The page thread is the only
/// place it may drop, and never from inside one of its own entries.
fn discard_crashed_engine(inner: &Arc<PageInner>) {
    if inner.crash_reason().is_none() {
        return;
    }
    teardown(inner);
}

/// Starts the task that runs the retained runtime bound to `tree` between
/// settle calls. A no-op when that runtime is gone.
pub(super) fn start_pump(inner: &Arc<PageInner>, tree: Weak<RefCell<DomDocument>>) {
    let page = inner.config.id;
    let Some(document) = tree.upgrade() else {
        return;
    };
    let notify = runtime::with_runtime_for(page, &document, |runtime| {
        runtime.tracker().borrow().notify_handle()
    });
    // Not held past the lookup: a strong handle in this scope would outlive the
    // runtime and keep its tree from moving out.
    drop(document);
    let Some(notify) = notify else {
        return;
    };
    let task_inner = Arc::clone(inner);
    let task = tokio::task::spawn_local(async move {
        if let Err(payload) = catch_unwind_future(drive(&task_inner, tree, notify)).await {
            let reason = panic_message(&payload);
            tracing::error!(page = ?task_inner.config.id, reason, "the page's engine driver panicked");
            crash(&task_inner, &reason);
        }
    });
    runtime::with_runtime(page, |runtime| runtime.set_pump(Pump::new(task)));
}

/// Turns the engine whenever it has work and sleeps otherwise.
async fn drive(inner: &Arc<PageInner>, tree: Weak<RefCell<DomDocument>>, notify: Arc<Notify>) {
    let page = inner.config.id;
    loop {
        // Registered before the turn, so a signal raised while the engine runs
        // is not lost to the park below.
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let wakeup = tree.upgrade().and_then(|document| {
            let wakeup =
                runtime::with_runtime_for(page, &document, |runtime| runtime.turn(TIMERS_PER_TURN));
            refresh_title(inner, &document);
            wakeup
        });
        let Some(wakeup) = wakeup else {
            return;
        };
        if inner.crash_reason().is_some() {
            discard_crashed_engine(inner);
            return;
        }
        match wakeup {
            Some(delay) => {
                tokio::select! {
                    () = &mut notified => {}
                    () = tokio::time::sleep(delay.max(MINIMUM_PARK)) => {}
                }
            }
            None => notified.await,
        }
    }
}

/// Re-reads the title the page serves other threads, if the tree changed.
fn refresh_title(inner: &PageInner, document: &SharedDocument) {
    inner.state.lock().cache_title(&document.borrow());
}

/// The page-thread half of [`Page::with_engine`].
fn enter_engine<R>(inner: &Arc<PageInner>, entry: impl FnOnce(&mut PageRuntime) -> R) -> Result<R> {
    {
        let state = inner.state.lock();
        if state.closed {
            return Err(Error::Closed);
        }
        if let Some(reason) = &state.crashed {
            return Err(Error::Crashed {
                reason: reason.clone(),
            });
        }
    }
    ensure_engine(inner)?;
    let page = inner.config.id;
    let ran = catch_unwind(AssertUnwindSafe(|| {
        runtime::with_runtime(page, |runtime| {
            let value = entry(runtime);
            refresh_title(inner, runtime.document());
            value
        })
    }));
    let value = match ran {
        Ok(value) => value,
        Err(payload) => {
            let reason = panic_message(&payload);
            crash(inner, &reason);
            return Err(Error::Crashed { reason });
        }
    };
    if let Some(reason) = inner.crash_reason() {
        discard_crashed_engine(inner);
        return Err(inner
            .native_memory_crash()
            .map(native_memory_budget_error)
            .unwrap_or(Error::Crashed { reason }));
    }
    value.ok_or(Error::Closed)
}

/// Binds an engine to the page's loaded document when the page has none.
fn ensure_engine(inner: &Arc<PageInner>) -> Result<()> {
    let page = inner.config.id;
    let document = {
        let mut state = inner.state.lock();
        if state.engine_held {
            return Ok(());
        }
        // The document is about to be replaced, and the navigation builds the
        // engine for its own.
        if state.navigations_in_flight > 0 {
            return Err(Error::invalid(
                "a navigation is replacing the document; its engine is not available yet",
            ));
        }
        // The error page is the browser's own, not the site's, and has no realm
        // for a host to run script in.
        if state
            .navigation
            .as_ref()
            .is_some_and(|navigation| navigation.error.is_some())
        {
            return Err(Error::invalid(
                "the page shows an error page; it has no script realm",
            ));
        }
        state
            .document
            .take()
            .ok_or_else(|| Error::invalid("the page has no document yet"))?
    };
    let shared = Rc::new(RefCell::new(document));
    let adopted = runtime::adopt_loaded_document(
        page,
        Rc::clone(&shared),
        &inner.config,
        Arc::clone(inner) as Arc<dyn ScriptBudget>,
    );
    if let Err(reason) = adopted {
        // The factory refused, so nothing else should hold the tree; if
        // something does, the tree is moved out of the shared cell anyway, so
        // a refused engine never costs the page its document.
        let document = match Rc::try_unwrap(shared) {
            Ok(cell) => cell.into_inner(),
            Err(shared) => std::mem::replace(&mut *shared.borrow_mut(), DomDocument::new()),
        };
        inner.state.lock().document = Some(document);
        return Err(Error::unsupported(reason));
    }
    inner.state.lock().engine_held = true;
    refresh_title(inner, &shared);
    start_pump(inner, Rc::downgrade(&shared));
    Ok(())
}

/// The page-thread half of [`Page::wait_for_settle`].
async fn settle_retained(
    inner: &Arc<PageInner>,
    wait_until: Settle,
    timeout: Duration,
) -> Result<SettleState> {
    let page = inner.config.id;
    let deadline = inner.config.clock.now() + timeout;
    // One settle call over the engine at a time: a second waiter queues here
    // instead of stopping the first one's driver or restarting its tracker. The
    // guard is declared first so it is released last, after the driver is back.
    let Ok(_gate) = tokio::time::timeout(timeout, inner.settle_gate.lock()).await else {
        return Ok(SettleState::Timeout {
            pending: Default::default(),
        });
    };
    let generation = {
        let state = inner.state.lock();
        if state.closed {
            return Err(Error::Closed);
        }
        if let Some(reason) = &state.crashed {
            return Err(Error::Crashed {
                reason: reason.clone(),
            });
        }
        if !state.engine_held {
            // A navigation in flight has work in motion this call cannot see.
            if state.navigations_in_flight > 0 {
                return Err(Error::invalid(
                    "a navigation is in progress; wait for its goto instead",
                ));
            }
            return if state.document.is_some() {
                Ok(SettleState::Settled { busy: false })
            } else {
                Err(Error::invalid("the page has no document yet"))
            };
        }
        state.navigation_generation
    };
    // Time spent queued behind another call counts against this call's deadline.
    let remaining = deadline.saturating_duration_since(inner.config.clock.now());
    let timeout_millis = timeout.as_millis() as u64;
    let (tracker, tree) = runtime::with_runtime(page, |runtime| {
        runtime.stop_pump();
        (
            Rc::clone(runtime.tracker()),
            Rc::downgrade(runtime.document()),
        )
    })
    .ok_or(Error::Closed)?;
    let _driver = DriverResume {
        inner: Arc::clone(inner),
        tree: tree.clone(),
    };
    tracker.borrow_mut().begin_settle(wait_until, remaining);
    let handle: TreeHandle = Some(tree);
    let outcome = settle_loop(
        inner,
        &tracker,
        &handle,
        deadline,
        timeout_millis,
        false,
        generation,
    )
    .await;
    runtime::with_runtime(page, |runtime| refresh_title(inner, runtime.document()));
    discard_crashed_engine(inner);
    match outcome {
        Err(Error::Timeout { .. }) => Ok(SettleState::Timeout {
            pending: tracker.borrow().pending(),
        }),
        other => other,
    }
}

/// Hands the engine back to the page's driver when a settle call ends, however
/// it ends.
struct DriverResume {
    inner: Arc<PageInner>,
    tree: Weak<RefCell<DomDocument>>,
}

impl Drop for DriverResume {
    fn drop(&mut self) {
        start_pump(&self.inner, self.tree.clone());
    }
}
