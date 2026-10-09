//! Execution contexts and the events a session hears while `Runtime` is
//! enabled.
//!
//! # Context ids
//!
//! A page has one script realm at a time, numbered by the engine, and the
//! realm ends with its document. The protocol's `executionContextId` is not
//! that number: the target keeps a counter and a table from each id to the
//! `(realm, world)` it stands for (`ContextIds`), so ids stay small and
//! stable for a client, and isolated worlds can add contexts without changing
//! any id handed out. An id the table no longer holds, or one whose realm is
//! not the page's current one, is stale. Only the main world exists today.
//!
//! The realm is announced when `Runtime.enable` arrives, when a navigation of
//! the main frame commits, and, because a document's scripts run before its
//! navigation commits, when an event shows up from a realm that has not been
//! announced. Messages the page logged before `enable` are replayed from the
//! target's ring, so a late client sees them as Chromium's does.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tenta_browser::{PageEvent, PageEvents};
use tokio::sync::Mutex as AsyncMutex;

use super::remote;
use crate::backend::{Target, lock};
use crate::page_runtime::World;
use crate::session::Session;

/// What one session knows about `Runtime`.
#[derive(Default)]
pub(super) struct RuntimeState {
    enabled: AtomicBool,
    watching: AtomicBool,
    /// The context the client has been told about, as `(realm, id)`. Held
    /// across the engine query that decides the next one, so announcements
    /// cannot cross.
    announced: AsyncMutex<Option<(u64, u64)>>,
    /// Where this session is in the target's console ring; held while
    /// messages are forwarded so none is sent twice or out of order.
    console: AsyncMutex<u64>,
    bindings: Mutex<HashSet<String>>,
    exceptions: AtomicU64,
    /// The origin of the document the main frame last committed, for a context
    /// announced before the page records the navigation.
    frame_origin: Mutex<Option<String>>,
    /// A realm built before its navigation committed, announced at the commit
    /// so its context carries the document's origin.
    unannounced_realm: Mutex<Option<u64>>,
    /// The utility-world contexts the client has been told about, as `(id,
    /// uniqueId)`. Changed only under the `announced` lock.
    utility_announced: Mutex<Vec<(u64, String)>>,
}

impl RuntimeState {
    pub(super) fn add_binding(&self, name: &str) {
        lock(&self.bindings).insert(name.to_owned());
    }

    pub(super) fn remove_binding(&self, name: &str) {
        lock(&self.bindings).remove(name);
    }

    pub(super) fn next_exception_id(&self) -> u64 {
        self.exceptions.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }
}

fn context_description(
    session: &Session,
    target: &Target,
    realm: u64,
    id: u64,
    world: World,
    name: &str,
) -> Value {
    // `about:blank` has the opaque origin Chromium spells "://".
    let committed = lock(&session.state::<RuntimeState>().frame_origin).clone();
    let origin = committed.unwrap_or_else(|| {
        target.page().navigation().map_or_else(
            || "://".to_owned(),
            |navigation| navigation.final_url.origin().ascii_serialization(),
        )
    });
    let main = world == World::Main;
    json!({
        "id": id,
        "origin": origin,
        "name": name,
        "uniqueId": target.contexts().unique_id(realm, world),
        "auxData": {
            "isDefault": main,
            "type": if main { "default" } else { "isolated" },
            "frameId": target.id(),
        },
    })
}

/// Tells the client its current context is gone, if it was told of one.
async fn retire(session: &Session, target: &Target, announced: &mut Option<(u64, u64)>) {
    let Some((realm, id)) = announced.take() else {
        return;
    };
    let utility = std::mem::take(&mut *lock(
        &session.state::<RuntimeState>().utility_announced,
    ));
    for (id, unique) in utility {
        session
            .emit(
                "Runtime.executionContextDestroyed",
                json!({ "executionContextId": id, "executionContextUniqueId": unique }),
            )
            .await;
    }
    session
        .emit(
            "Runtime.executionContextDestroyed",
            json!({
                "executionContextId": id,
                "executionContextUniqueId": target.contexts().unique_id(realm, World::Main),
            }),
        )
        .await;
    session
        .emit("Runtime.executionContextsCleared", json!({}))
        .await;
}

/// Announces `realm`, retiring the announced context first; returns its id.
async fn announce(
    session: &Session,
    target: &Target,
    announced: &mut Option<(u64, u64)>,
    realm: u64,
) -> u64 {
    if let Some((current, id)) = *announced
        && current == realm
    {
        return id;
    }
    retire(session, target, announced).await;
    let id = target.contexts().id_for(realm, World::Main);
    *announced = Some((realm, id));
    session
        .emit(
            "Runtime.executionContextCreated",
            json!({ "context": context_description(session, target, realm, id, World::Main, "") }),
        )
        .await;
    announce_utility_worlds(session, target, realm).await;
    id
}

/// Announces the contexts of the utility worlds of `realm` that the client has
/// not been told about. Every new document gets its worlds again, as in
/// Chromium, where a world outlives the documents it was created for.
async fn announce_utility_worlds(session: &Session, target: &Target, realm: u64) {
    let state = session.state::<RuntimeState>();
    for (world, name) in target.contexts().utility_worlds() {
        let id = target.contexts().id_for(realm, world);
        if lock(&state.utility_announced)
            .iter()
            .any(|(known, _)| *known == id)
        {
            continue;
        }
        let unique = target.contexts().unique_id(realm, world);
        lock(&state.utility_announced).push((id, unique));
        session
            .emit(
                "Runtime.executionContextCreated",
                json!({ "context": context_description(session, target, realm, id, world, &name) }),
            )
            .await;
    }
}

/// The context of the utility world `name` in the page's current realm, for
/// `Page.createIsolatedWorld`. Announced to the session if it listens.
pub(in crate::domains) async fn utility_context(
    session: &Session,
    target: &Target,
    name: &str,
) -> Result<u64, tenta_core::Error> {
    let realm = target.page().execution_context().await?;
    let world = target.contexts().utility_world(name);
    let state = session.state::<RuntimeState>();
    let announced = state.announced.lock().await;
    if state.is_enabled() && announced.is_some_and(|(current, _)| current == realm) {
        announce_utility_worlds(session, target, realm).await;
    }
    Ok(target.contexts().id_for(realm, world))
}

/// Announces the page's current realm, building the engine of a document that
/// has none, or retires the announced one when the page has no realm. Waits
/// for a navigation in flight before it takes the announcement lock, so a
/// `disable` or an event never queues behind a slow page.
pub(super) async fn refresh(session: &Session, target: &Target) {
    target.page().navigation_settled().await;
    let state = session.state::<RuntimeState>();
    let mut announced = state.announced.lock().await;
    if !state.is_enabled() {
        return;
    }
    match target.page().execution_context().await {
        Ok(realm) => {
            announce(session, target, &mut announced, realm).await;
        }
        Err(_) => retire(session, target, &mut announced).await,
    }
}

/// Announces `realm` when an event from it arrives first and returns the id
/// to report it under. Realm numbers only grow, so an event of a realm older
/// than the announced one is a straggler from a document that is gone: `None`.
async fn reveal(session: &Session, target: &Target, realm: u64) -> Option<u64> {
    let state = session.state::<RuntimeState>();
    let mut announced = state.announced.lock().await;
    if !state.is_enabled() || announced.is_some_and(|(current, _)| realm < current) {
        return None;
    }
    Some(announce(session, target, &mut announced, realm).await)
}

/// Enabling never waits for a navigation: the context of a document that is
/// still being installed is announced by a task, or by the navigation's own
/// commit, and the command returns at once.
pub(super) async fn enable(session: &Arc<Session>, target: &Arc<Target>) {
    let state = session.state::<RuntimeState>();
    if state.enabled.swap(true, Ordering::AcqRel) {
        return;
    }
    // From the oldest message the page still holds: what it logged before this
    // command is owed to the client, as with Chromium's console agent.
    *state.console.lock().await = target.console().oldest();
    if !state.watching.swap(true, Ordering::AcqRel) {
        // Subscribed before the first announcement: the page's events from
        // here on are all heard by the task.
        let events = target.page().events();
        let (owner, target) = (Arc::clone(session), Arc::clone(target));
        session.spawn(watch(owner, target, events));
    }
    if target.page().navigation_in_flight() {
        let (owner, target) = (Arc::clone(session), Arc::clone(target));
        session.spawn(async move {
            refresh(&owner, &target).await;
            forward_console(&owner, &target).await;
        });
        return;
    }
    refresh(session, target).await;
    forward_console(session, target).await;
}

pub(super) async fn disable(session: &Session) {
    let state = session.state::<RuntimeState>();
    let mut announced = state.announced.lock().await;
    if !state.enabled.swap(false, Ordering::AcqRel) {
        return;
    }
    announced.take();
    lock(&state.utility_announced).clear();
    session
        .emit("Runtime.executionContextsCleared", json!({}))
        .await;
}

/// Sends the console calls and exceptions this session has not yet seen.
async fn forward_console(session: &Session, target: &Target) {
    let state = session.state::<RuntimeState>();
    let mut cursor = state.console.lock().await;
    let (events, next) = target.console().since(*cursor);
    *cursor = next;
    for event in events {
        if !state.is_enabled() {
            return;
        }
        match event {
            PageEvent::Console(call) => {
                if let Some(id) = reveal(session, target, call.context).await {
                    session
                        .emit("Runtime.consoleAPICalled", remote::console_event(&call, id))
                        .await;
                }
            }
            PageEvent::Exception(event) => {
                if let Some(id) = reveal(session, target, event.context).await {
                    let exception_id = state.next_exception_id();
                    session
                        .emit(
                            "Runtime.exceptionThrown",
                            json!({
                                "timestamp": event.timestamp_ms,
                                "exceptionDetails":
                                    remote::exception_details(&event.details, exception_id, id),
                            }),
                        )
                        .await;
                }
            }
            _ => {}
        }
    }
}

async fn watch(session: Arc<Session>, target: Arc<Target>, mut events: PageEvents) {
    let main_frame = target.page().main_frame();
    let mut logged = target.console().subscribe();
    loop {
        tokio::select! {
            event = events.next() => {
                let Some(event) = event else {
                    return;
                };
                match event {
                    PageEvent::FrameNavigated { frame, url } if frame == main_frame => {
                        let state = session.state::<RuntimeState>();
                        *lock(&state.frame_origin) = Some(url.origin().ascii_serialization());
                        // The realm of this document was built while it
                        // parsed, before this commit: announce it now, with
                        // the origin, ahead of the page's load events.
                        let held = lock(&state.unannounced_realm).take();
                        if let Some(realm) = held {
                            reveal(&session, &target, realm).await;
                        }
                        // Not awaited here: the loop must go on hearing the
                        // page, and a refresh waits for the navigation to end.
                        let (owner, target) = (Arc::clone(&session), Arc::clone(&target));
                        session.spawn(async move { refresh(&owner, &target).await });
                    }
                    // A navigation that ends before it commits leaves a realm
                    // that will never be announced at a commit: drop it, so a
                    // later commit does not announce a document that is gone.
                    PageEvent::NavigationStopped { frame, .. } if frame == main_frame => {
                        lock(&session.state::<RuntimeState>().unannounced_realm).take();
                    }
                    // The engine of a new document exists. Announce its
                    // context as Chromium does, at commit, instead of when
                    // the navigation has settled and a client has already
                    // looked.
                    PageEvent::RealmCreated { realm } => {
                        if target.page().navigation_in_flight() {
                            *lock(&session.state::<RuntimeState>().unannounced_realm) =
                                Some(realm);
                        } else {
                            reveal(&session, &target, realm).await;
                        }
                    }
                    PageEvent::BindingCalled(call) => {
                        let state = session.state::<RuntimeState>();
                        let wanted = lock(&state.bindings).contains(&call.name);
                        if wanted && let Some(id) = reveal(&session, &target, call.context).await {
                            session
                                .emit(
                                    "Runtime.bindingCalled",
                                    json!({
                                        "name": call.name,
                                        "payload": call.payload,
                                        "executionContextId": id,
                                    }),
                                )
                                .await;
                        }
                    }
                    _ => {}
                }
            }
            changed = logged.changed() => {
                if changed.is_err() {
                    return;
                }
                forward_console(&session, &target).await;
            }
        }
    }
}
