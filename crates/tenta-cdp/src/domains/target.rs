//! The `Target` domain: discovery, auto-attach, browser contexts and the
//! lifecycle of page targets.
//!
//! # Pausing a new target
//!
//! A session that auto-attaches with `waitForDebuggerOnStart` expects a new
//! page to wait until it sends `Runtime.runIfWaitingForDebugger`. A page made
//! by `Target.createTarget` has no script before its first navigation, so
//! "waiting" means one thing here: the navigation to the requested URL has
//! not started. Every waiting attachment holds the navigation back; it starts
//! when the last hold is released, by the session's `runIfWaitingForDebugger`
//! or by the session going away (a debugger that detaches resumes the page,
//! as in Chromium). Pages made any other way (`/json/new`, another embedder
//! of the backend) already navigate on their own and are reported as not
//! waiting.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};
use tenta_browser::{BrowserContext, ContextOptions, Goto, Page, PageEvent};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::broadcast::error::RecvError;

use crate::backend::{Backend, Target, TargetEvent, lock};
use crate::connection::{Connection, SessionIdKind};
use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry, parse_params};
use crate::error::CdpError;
use crate::session::Session;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(TargetDomain);
}

struct TargetDomain;

/// State the domains of one engine share. It lives in the engine's extension
/// slot, so every connection and every domain module sees the same instance.
#[derive(Default)]
pub(super) struct Shared {
    /// Held while a target is created and its pause is registered, so a
    /// connection that hears about the target from the backend's broadcast
    /// cannot look the pause up before it exists.
    creation: AsyncMutex<()>,
    pending: Mutex<HashMap<String, Arc<Pending>>>,
    /// The validated `Browser.setDownloadBehavior` parameters per context.
    /// Downloads are a later phase's policy; this is where it reads them.
    pub(super) downloads: Mutex<HashMap<String, Value>>,
}

/// `Security.setIgnoreCertificateErrors` of one connection. A context's TLS
/// settings are fixed when it is built, so the flag applies to the contexts
/// that connection creates afterwards, and to nobody else's.
#[derive(Default)]
pub(super) struct IgnoreCertificateErrors(pub(super) AtomicBool);

fn context_options(
    base: &ContextOptions,
    proxy_server: Option<String>,
    ignore_certificate_errors: bool,
) -> ContextOptions {
    let mut options = base.clone();
    // A new context must not share the default one's disk cache or its TLS
    // session cache: that would link them.
    options.cache_dir = None;
    options.persistence_id = None;
    if let Some(proxy) = proxy_server.filter(|proxy| !proxy.is_empty()) {
        options.proxy = Some(if proxy.contains("://") {
            proxy
        } else {
            format!("http://{proxy}")
        });
    }
    options.ignore_https_errors |= ignore_certificate_errors;
    options
}

pub(super) fn shared(cx: &CommandContext) -> Result<Arc<Shared>, CdpError> {
    cx.backend()
        .engine()
        .extension(|_| Ok(Shared::default()))
        .map_err(CdpError::from)
}

/// Resolves the `browserContextId` parameter; absent means the default
/// context.
pub(super) fn find_context(
    backend: &Backend,
    requested: Option<&str>,
) -> Result<(String, BrowserContext), CdpError> {
    let id = requested.unwrap_or_else(|| backend.default_context_id());
    let context = backend.context(id).ok_or_else(|| {
        CdpError::invalid_params(format!("Failed to find browser context with id {id}"))
    })?;
    Ok((id.to_owned(), context))
}

// ---- the pause of a new target ------------------------------------------

struct Pending {
    id: String,
    page: Page,
    /// The runtime of the command that created the target: a hold can be
    /// dropped on any thread, including one outside the runtime.
    runtime: tokio::runtime::Handle,
    state: Mutex<PendingState>,
}

struct PendingState {
    /// The creator and every waiting attachment; the navigation starts when
    /// the count reaches zero.
    holds: usize,
    url: Option<String>,
}

/// One reason the navigation of a new target has not started. Dropping it
/// releases the reason.
struct Hold {
    pending: Arc<Pending>,
    shared: Arc<Shared>,
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.shared.release(&self.pending);
    }
}

impl Shared {
    /// Registers a pause for `target`; the returned hold is the creator's.
    fn begin(self: &Arc<Self>, target: &Target, url: Option<String>) -> Hold {
        let pending = Arc::new(Pending {
            id: target.id().to_owned(),
            page: target.page().clone(),
            runtime: tokio::runtime::Handle::current(),
            state: Mutex::new(PendingState { holds: 1, url }),
        });
        lock(&self.pending).insert(pending.id.clone(), Arc::clone(&pending));
        Hold {
            pending,
            shared: Arc::clone(self),
        }
    }

    /// A further hold, unless the target is not paused (any more).
    fn hold(self: &Arc<Self>, target_id: &str) -> Option<Hold> {
        let pending = lock(&self.pending).get(target_id).cloned()?;
        {
            let mut state = lock(&pending.state);
            if state.holds == 0 {
                return None;
            }
            state.holds += 1;
        }
        Some(Hold {
            pending,
            shared: Arc::clone(self),
        })
    }

    fn release(&self, pending: &Pending) {
        let url = {
            let mut state = lock(&pending.state);
            state.holds -= 1;
            if state.holds > 0 {
                return;
            }
            state.url.take()
        };
        lock(&self.pending).remove(&pending.id);
        if let Some(url) = url {
            start_navigation(&pending.runtime, pending.page.clone(), url);
        }
    }

    pub(super) fn forget_context(&self, id: &str) {
        lock(&self.downloads).remove(id);
    }
}

fn start_navigation(runtime: &tokio::runtime::Handle, page: Page, url: String) {
    runtime.spawn(async move {
        // A failed navigation surfaces as page events and an error document,
        // not as a failure of the command that opened the page.
        let _ = page.goto(&url, Goto::default()).await;
    });
}

/// The hold a waiting attachment keeps for its session.
#[derive(Default)]
struct Waiting(Mutex<Option<Hold>>);

/// `Runtime.runIfWaitingForDebugger`: lets a paused target start. Succeeds
/// when the session is not waiting.
pub(super) fn resume(session: &Session) {
    let hold = lock(&session.state::<Waiting>().0).take();
    drop(hold);
}

// ---- what a session watches ---------------------------------------------

#[derive(Clone, Deserialize)]
struct FilterEntry {
    r#type: Option<String>,
    exclude: Option<bool>,
}

/// A `TargetFilter`: the first entry whose type matches decides, an entry
/// without a type matches everything, and no match excludes.
#[derive(Clone)]
struct Filter(Vec<FilterEntry>);

impl Default for Filter {
    /// What Chromium applies when a command sends no filter.
    fn default() -> Self {
        let entry = |kind: Option<&str>, exclude| FilterEntry {
            r#type: kind.map(str::to_owned),
            exclude,
        };
        Self(vec![
            entry(Some("browser"), Some(true)),
            entry(Some("tab"), Some(true)),
            entry(None, None),
        ])
    }
}

impl Filter {
    fn from_param(filter: Option<Vec<FilterEntry>>) -> Self {
        filter.map_or_else(Self::default, Self)
    }

    fn allows(&self, kind: &str) -> bool {
        self.0
            .iter()
            .find(|entry| entry.r#type.as_deref().is_none_or(|t| t == kind))
            .is_some_and(|entry| entry.exclude != Some(true))
    }
}

#[derive(Default)]
struct Watch {
    inner: AsyncMutex<WatchState>,
    subscribed: AtomicBool,
}

#[derive(Default)]
struct WatchState {
    discover: bool,
    discover_filter: Filter,
    auto_attach_filter: Filter,
    /// `Some(wait_for_debugger_on_start)` while auto-attach is on.
    auto_attach: Option<bool>,
    /// Targets announced with `targetCreated`.
    announced: HashSet<String>,
    /// Targets with a `targetInfoChanged` watcher.
    watched: HashSet<String>,
    /// Targets this session auto-attached.
    attached: HashSet<String>,
}

pub(super) fn target_info(target: &Target) -> Value {
    target_info_at(target, target.url())
}

/// The info of a target that is at `url`. A navigation event is sent before
/// the page records the navigation, so the event's URL is the one to report.
fn target_info_at(target: &Target, url: String) -> Value {
    // Scripted pages keep their document inside the engine, so the title is
    // known only for pages that parsed one; Chromium shows the URL for a page
    // without a title as well.
    let title = target
        .page()
        .document()
        .and_then(|document| document.metadata().title)
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| url.clone());
    json!({
        "targetId": target.id(),
        "type": "page",
        "title": title,
        "url": url,
        "attached": target.is_attached(),
        "canAccessOpener": false,
        "browserContextId": target.context_id(),
    })
}

fn browser_info(backend: &Backend) -> Value {
    json!({
        "targetId": backend.browser_id(),
        "type": "browser",
        "title": "",
        "url": "",
        "attached": true,
        "canAccessOpener": false,
    })
}

/// The connection's own session followed by its attached ones.
fn sessions_of(connection: &Connection) -> Vec<Arc<Session>> {
    let mut all = vec![Arc::clone(connection.root())];
    all.extend(connection.sessions());
    all
}

/// Starts the background task that feeds `session` with the backend's target
/// events. The subscription is taken before this returns, so a caller that
/// lists the existing targets afterwards misses nothing; the sets in
/// [`WatchState`] absorb the overlap.
fn ensure_subscribed(connection: &Arc<Connection>, shared: &Arc<Shared>, session: &Arc<Session>) {
    if session
        .state::<Watch>()
        .subscribed
        .swap(true, Ordering::AcqRel)
    {
        return;
    }
    let mut events = connection.backend().subscribe();
    let (connection, shared, owner) = (
        Arc::clone(connection),
        Arc::clone(shared),
        Arc::clone(session),
    );
    session.spawn(async move {
        loop {
            match events.recv().await {
                Ok(TargetEvent::Created(target)) => {
                    drop(shared.creation.lock().await);
                    on_created(&connection, &shared, &owner, &target).await;
                }
                Ok(TargetEvent::Destroyed(target)) => on_destroyed(&owner, target.id()).await,
                // Events were lost: compare the whole table instead.
                Err(RecvError::Lagged(_)) => resync(&connection, &shared, &owner).await,
                Err(RecvError::Closed) => break,
            }
        }
    });
}

async fn resync(connection: &Arc<Connection>, shared: &Arc<Shared>, session: &Arc<Session>) {
    let live = connection.backend().targets();
    for target in &live {
        on_created(connection, shared, session, target).await;
    }
    let live: HashSet<&str> = live.iter().map(|target| target.id()).collect();
    let gone: Vec<String> = {
        let watch = session.state::<Watch>();
        let state = watch.inner.lock().await;
        state
            .announced
            .iter()
            .chain(&state.attached)
            .filter(|id| {
                id.as_str() != connection.backend().browser_id() && !live.contains(id.as_str())
            })
            .cloned()
            .collect()
    };
    for id in gone {
        on_destroyed(session, &id).await;
    }
}

async fn on_created(
    connection: &Arc<Connection>,
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    target: &Arc<Target>,
) {
    let watch = session.state::<Watch>();
    let mut state = watch.inner.lock().await;
    if session.is_closed() {
        return;
    }
    announce(session, &mut state, target).await;
    attach_automatically(connection, shared, session, &mut state, target).await;
}

async fn on_destroyed(session: &Arc<Session>, target_id: &str) {
    let watch = session.state::<Watch>();
    let mut state = watch.inner.lock().await;
    state.attached.remove(target_id);
    state.watched.remove(target_id);
    if state.announced.remove(target_id) && state.discover && !session.is_closed() {
        session
            .emit("Target.targetDestroyed", json!({ "targetId": target_id }))
            .await;
    }
}

async fn announce(session: &Arc<Session>, state: &mut WatchState, target: &Arc<Target>) {
    if !state.discover
        || !state.discover_filter.allows("page")
        || !state.announced.insert(target.id().to_owned())
    {
        return;
    }
    session
        .emit(
            "Target.targetCreated",
            json!({ "targetInfo": target_info(target) }),
        )
        .await;
    if state.watched.insert(target.id().to_owned()) {
        watch_navigations(session, target);
    }
}

/// Reports a navigation of the target's main frame as `targetInfoChanged`.
fn watch_navigations(session: &Arc<Session>, target: &Arc<Target>) {
    let (owner, target) = (Arc::clone(session), Arc::clone(target));
    // Subscribed before the task runs: the navigation of a new target starts
    // right after it is announced, and an event sent before the task's first
    // poll would be lost.
    let mut events = target.page().events();
    session.spawn(async move {
        let main_frame = target.page().main_frame();
        while let Some(event) = events.next().await {
            let PageEvent::FrameNavigated { frame, url } = event else {
                continue;
            };
            if frame != main_frame {
                continue;
            }
            let watch = owner.state::<Watch>();
            let state = watch.inner.lock().await;
            if state.discover && state.announced.contains(target.id()) {
                owner
                    .emit(
                        "Target.targetInfoChanged",
                        json!({ "targetInfo": target_info_at(&target, url.to_string()) }),
                    )
                    .await;
            }
        }
    });
}

async fn attach_automatically(
    connection: &Arc<Connection>,
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    state: &mut WatchState,
    target: &Arc<Target>,
) {
    let Some(wait_for_debugger) = state.auto_attach else {
        return;
    };
    // Page sessions have no sub-targets to attach to yet, and must not adopt
    // the browser's other pages.
    if session.target().is_some()
        || !state.auto_attach_filter.allows("page")
        || !state.attached.insert(target.id().to_owned())
    {
        return;
    }
    let attached = connection
        .attach(target, session.id(), SessionIdKind::Managed)
        // The managed id is taken when another session of this connection
        // auto-attached the target first.
        .or_else(|_| connection.attach(target, session.id(), SessionIdKind::Explicit));
    let Ok(child) = attached else {
        return;
    };
    let hold = if wait_for_debugger {
        shared.hold(target.id())
    } else {
        None
    };
    let waiting = hold.is_some();
    if let Some(hold) = hold {
        *lock(&child.state::<Waiting>().0) = Some(hold);
    }
    super::inspector::watch_crashes(&child, target);
    session
        .emit(
            "Target.attachedToTarget",
            json!({
                "sessionId": child.id(),
                "targetInfo": target_info(target),
                "waitingForDebugger": waiting,
            }),
        )
        .await;
}

/// Tells this connection's sessions about a new target, before the command
/// that made it answers.
async fn announce_to_connection(
    connection: &Arc<Connection>,
    shared: &Arc<Shared>,
    target: &Arc<Target>,
) {
    for session in sessions_of(connection) {
        on_created(connection, shared, &session, target).await;
    }
}

/// Detaches the sessions of a closed target and reports it destroyed, in that
/// order, before the command that closed it answers.
async fn settle_destroyed(connection: &Arc<Connection>, target: &Target) {
    for session in connection.sessions() {
        if let (Some(id), Some(attached)) = (session.id(), session.target())
            && attached.id() == target.id()
        {
            connection.detach(id).await;
        }
    }
    for session in sessions_of(connection) {
        on_destroyed(&session, target.id()).await;
    }
}

/// Contexts created with `disposeOnDetach` die with the session that made
/// them.
#[derive(Default)]
struct DisposeOnDetach(Mutex<Vec<(Arc<Backend>, String)>>);

impl Drop for DisposeOnDetach {
    fn drop(&mut self) {
        let contexts = std::mem::take(
            self.0
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        if contexts.is_empty() {
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                for (backend, id) in contexts {
                    // The client may have disposed of it already.
                    let _ = backend.dispose_context(&id).await;
                }
            });
        }
    }
}

// ---- commands -----------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTarget {
    url: Option<String>,
    browser_context_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetId {
    target_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetTargetInfo {
    target_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachToTarget {
    target_id: String,
    flatten: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetachFromTarget {
    session_id: String,
}

#[derive(Deserialize)]
struct SetDiscoverTargets {
    discover: bool,
    filter: Option<Vec<FilterEntry>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetAutoAttach {
    auto_attach: bool,
    wait_for_debugger_on_start: bool,
    flatten: Option<bool>,
    filter: Option<Vec<FilterEntry>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBrowserContext {
    dispose_on_detach: Option<bool>,
    proxy_server: Option<String>,
    proxy_bypass_list: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserContextId {
    browser_context_id: String,
}

fn no_such_target() -> CdpError {
    CdpError::invalid_params("No target with given id found")
}

/// The URL to navigate to after creation; `None` leaves the page blank.
fn destination(url: Option<&str>) -> Result<Option<String>, CdpError> {
    match url {
        None | Some("" | "about:blank") => Ok(None),
        Some(url) => {
            tenta_browser::context::parse_url(url)
                .map_err(|error| CdpError::invalid_params(error.to_string()))?;
            Ok(Some(url.to_owned()))
        }
    }
}

impl DomainHandler for TargetDomain {
    fn domain(&self) -> &'static str {
        "Target"
    }

    fn methods(&self) -> &'static [&'static str] {
        &[
            "getTargets",
            "getTargetInfo",
            "createTarget",
            "closeTarget",
            "activateTarget",
            "attachToTarget",
            "attachToBrowserTarget",
            "detachFromTarget",
            "setDiscoverTargets",
            "setAutoAttach",
            "getBrowserContexts",
            "createBrowserContext",
            "disposeBrowserContext",
        ]
    }

    fn handle<'a>(
        &'a self,
        cx: CommandContext,
        method: &'a str,
        params: Value,
    ) -> CommandFuture<'a> {
        Box::pin(async move {
            match method {
                // Pages only: the browser target is announced by discovery
                // and by `getTargetInfo`.
                "getTargets" => {
                    let infos: Vec<Value> = cx
                        .backend()
                        .targets()
                        .iter()
                        .map(|target| target_info(target))
                        .collect();
                    Ok(json!({ "targetInfos": infos }))
                }
                "getTargetInfo" => {
                    let GetTargetInfo { target_id } = parse_params(params)?;
                    let backend = cx.backend();
                    let info = match target_id.as_deref() {
                        None => cx
                            .session()
                            .target()
                            .map_or_else(|| browser_info(backend), |target| target_info(target)),
                        Some(id) if id == backend.browser_id() => browser_info(backend),
                        Some(id) => {
                            let target = backend.target(id).ok_or_else(no_such_target)?;
                            target_info(&target)
                        }
                    };
                    Ok(json!({ "targetInfo": info }))
                }
                "createTarget" => {
                    let CreateTarget {
                        url,
                        browser_context_id,
                    } = parse_params(params)?;
                    let navigate_to = destination(url.as_deref())?;
                    let shared = shared(&cx)?;
                    let (target, creator) = {
                        let _creating = shared.creation.lock().await;
                        // The backend would start navigating at once; the
                        // navigation is ours to start, once nobody waits.
                        let target = cx
                            .backend()
                            .create_target(browser_context_id.as_deref(), None)
                            .await?;
                        let creator = shared.begin(&target, navigate_to);
                        (target, creator)
                    };
                    announce_to_connection(cx.connection(), &shared, &target).await;
                    drop(creator);
                    Ok(json!({ "targetId": target.id() }))
                }
                "closeTarget" => {
                    let TargetId { target_id } = parse_params(params)?;
                    let target = cx.backend().target(&target_id).ok_or_else(no_such_target)?;
                    if !cx.backend().close_target(&target_id).await? {
                        return Err(no_such_target());
                    }
                    settle_destroyed(cx.connection(), &target).await;
                    Ok(json!({ "success": true }))
                }
                // There is no window to raise.
                "activateTarget" => {
                    let TargetId { target_id } = parse_params(params)?;
                    cx.backend().target(&target_id).ok_or_else(no_such_target)?;
                    Ok(json!({}))
                }
                "attachToTarget" => {
                    let AttachToTarget { target_id, flatten } = parse_params(params)?;
                    if flatten != Some(true) {
                        return Err(CdpError::server(
                            "Only flattened sessions are supported; pass flatten: true",
                        ));
                    }
                    let target = cx.backend().target(&target_id).ok_or_else(no_such_target)?;
                    let session = cx.connection().attach(
                        &target,
                        cx.session().id(),
                        SessionIdKind::Explicit,
                    )?;
                    let session_id = session.id().unwrap_or_default().to_owned();
                    super::inspector::watch_crashes(&session, &target);
                    cx.emit(
                        "Target.attachedToTarget",
                        json!({
                            "sessionId": session_id,
                            "targetInfo": target_info(&target),
                            "waitingForDebugger": false,
                        }),
                    )
                    .await;
                    Ok(json!({ "sessionId": session_id }))
                }
                "attachToBrowserTarget" => {
                    let session = cx.connection().attach_browser(cx.session().id())?;
                    Ok(json!({ "sessionId": session.id() }))
                }
                "detachFromTarget" => {
                    let DetachFromTarget { session_id } = parse_params(params)?;
                    cx.connection()
                        .detach(&session_id)
                        .await
                        .ok_or_else(|| CdpError::invalid_params("Session not found"))?;
                    Ok(json!({}))
                }
                "setDiscoverTargets" => {
                    let SetDiscoverTargets { discover, filter } = parse_params(params)?;
                    let shared = shared(&cx)?;
                    ensure_subscribed(cx.connection(), &shared, cx.session());
                    let watch = cx.session().state::<Watch>();
                    let mut state = watch.inner.lock().await;
                    state.discover = discover;
                    state.discover_filter = Filter::from_param(filter);
                    if !discover {
                        state.announced.clear();
                        return Ok(json!({}));
                    }
                    let browser = state.discover_filter.allows("browser")
                        && state.announced.insert(cx.backend().browser_id().to_owned());
                    if browser {
                        cx.emit(
                            "Target.targetCreated",
                            json!({ "targetInfo": browser_info(cx.backend()) }),
                        )
                        .await;
                    }
                    for target in cx.backend().targets() {
                        announce(cx.session(), &mut state, &target).await;
                    }
                    Ok(json!({}))
                }
                "setAutoAttach" => {
                    let SetAutoAttach {
                        auto_attach,
                        wait_for_debugger_on_start,
                        flatten,
                        filter,
                    } = parse_params(params)?;
                    if auto_attach && flatten != Some(true) {
                        return Err(CdpError::server(
                            "Only flattened sessions are supported; pass flatten: true",
                        ));
                    }
                    let shared = shared(&cx)?;
                    ensure_subscribed(cx.connection(), &shared, cx.session());
                    {
                        let watch = cx.session().state::<Watch>();
                        let mut state = watch.inner.lock().await;
                        state.auto_attach = auto_attach.then_some(wait_for_debugger_on_start);
                        state.auto_attach_filter = Filter::from_param(filter);
                    }
                    if auto_attach {
                        for target in cx.backend().targets() {
                            on_created(cx.connection(), &shared, cx.session(), &target).await;
                        }
                    }
                    Ok(json!({}))
                }
                "getBrowserContexts" => {
                    let backend = cx.backend();
                    let mut ids = backend.context_ids();
                    ids.retain(|id| id != backend.default_context_id());
                    ids.sort();
                    Ok(json!({ "browserContextIds": ids }))
                }
                "createBrowserContext" => {
                    let CreateBrowserContext {
                        dispose_on_detach,
                        proxy_server,
                        proxy_bypass_list,
                    } = parse_params(params)?;
                    // The transport's proxy support has no bypass list, and
                    // accepting one would send traffic through the proxy that
                    // the client asked to keep off it.
                    if proxy_bypass_list.is_some_and(|list| !list.is_empty()) {
                        return Err(CdpError::invalid_params("proxyBypassList is not supported"));
                    }
                    let ignore = cx
                        .connection()
                        .root()
                        .state::<IgnoreCertificateErrors>()
                        .0
                        .load(Ordering::Acquire);
                    let options = context_options(
                        &cx.backend().options().default_context,
                        proxy_server,
                        ignore,
                    );
                    let id = cx.backend().create_context(options)?;
                    if dispose_on_detach == Some(true) {
                        lock(&cx.session().state::<DisposeOnDetach>().0)
                            .push((Arc::clone(cx.backend()), id.clone()));
                    }
                    Ok(json!({ "browserContextId": id }))
                }
                "disposeBrowserContext" => {
                    let BrowserContextId { browser_context_id } = parse_params(params)?;
                    let shared = shared(&cx)?;
                    let doomed: Vec<_> = cx
                        .backend()
                        .targets()
                        .into_iter()
                        .filter(|target| target.context_id() == browser_context_id)
                        .collect();
                    cx.backend().dispose_context(&browser_context_id).await?;
                    shared.forget_context(&browser_context_id);
                    for target in doomed {
                        settle_destroyed(cx.connection(), &target).await;
                    }
                    Ok(json!({}))
                }
                other => Err(CdpError::method_not_found(&format!("Target.{other}"))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certificate_errors_are_ignored_only_when_asked() {
        let base = ContextOptions::default();
        assert!(!context_options(&base, None, false).ignore_https_errors);
        assert!(context_options(&base, None, true).ignore_https_errors);
    }

    #[test]
    fn a_new_context_shares_neither_cache_nor_session_cache() {
        let base = ContextOptions {
            cache_dir: Some("/cache".into()),
            persistence_id: Some("shared".to_owned()),
            ..ContextOptions::default()
        };
        let options = context_options(&base, Some("127.0.0.1:3128".to_owned()), false);
        assert!(options.cache_dir.is_none());
        assert!(options.persistence_id.is_none());
        assert_eq!(options.proxy.as_deref(), Some("http://127.0.0.1:3128"));
    }

    #[test]
    fn filters_are_decided_by_the_first_matching_entry() {
        let default = Filter::default();
        assert!(default.allows("page"));
        assert!(!default.allows("browser"));
        assert!(!default.allows("tab"));
        assert!(!Filter(Vec::new()).allows("page"));
    }
}
