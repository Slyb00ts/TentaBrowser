//! State shared by every connection: the engine, browser contexts, targets.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};
use tenta_browser::{BrowserContext, ContextOptions, Engine, Goto, Page, PageOptions};
use tenta_core::{Result, Tier};
use tokio::sync::{broadcast, watch};

use crate::error::CdpError;
use crate::ids::random_id;
use crate::options::CdpOptions;
use crate::page_runtime::{ConsoleRing, ContextIds};

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A handler that panicked must not take every other connection with it.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Live counters, observable through [`crate::CdpServer::stats`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Open client connections, HTTP and WebSocket.
    pub connections: usize,
    /// Attached target sessions across all connections.
    pub sessions: usize,
}

/// One debuggable page.
pub struct Target {
    id: String,
    context_id: String,
    page: Page,
    attached: AtomicUsize,
    contexts: ContextIds,
    console: Arc<ConsoleRing>,
    overrides: Mutex<crate::overrides::ViewportOverrides>,
}

impl Target {
    pub(crate) fn viewport_overrides(&self) -> MutexGuard<'_, crate::overrides::ViewportOverrides> {
        lock(&self.overrides)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn context_id(&self) -> &str {
        &self.context_id
    }

    pub fn page(&self) -> &Page {
        &self.page
    }

    pub(crate) fn contexts(&self) -> &ContextIds {
        &self.contexts
    }

    pub(crate) fn console(&self) -> &Arc<ConsoleRing> {
        &self.console
    }

    pub fn url(&self) -> String {
        self.page
            .navigation()
            .map_or_else(|| "about:blank".to_owned(), |nav| nav.final_url.to_string())
    }

    /// Whether at least one session is attached.
    pub fn is_attached(&self) -> bool {
        self.attached.load(Ordering::Acquire) > 0
    }

    pub(crate) fn session_attached(&self) {
        self.attached.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn session_detached(&self) {
        self.attached.fetch_sub(1, Ordering::AcqRel);
    }

    /// The `TargetInfo` object of the protocol.
    pub fn info(&self) -> Value {
        json!({
            "targetId": self.id,
            "type": "page",
            "title": self.url(),
            "url": self.url(),
            "attached": self.is_attached(),
            "canAccessOpener": false,
            "browserContextId": self.context_id,
        })
    }
}

/// What changed in the target list. Connections use it to drop sessions of a
/// target that is gone; `Target.targetCreated` discovery builds on it.
#[derive(Clone)]
pub enum TargetEvent {
    Created(Arc<Target>),
    Destroyed(Arc<Target>),
}

/// The browser behind the protocol. One per server, shared by connections.
pub struct Backend {
    engine: Engine,
    options: CdpOptions,
    browser_id: String,
    default_context: String,
    contexts: Mutex<HashMap<String, BrowserContext>>,
    targets: Mutex<Vec<Arc<Target>>>,
    events: broadcast::Sender<TargetEvent>,
    stats: watch::Sender<Stats>,
}

impl Backend {
    pub(crate) fn new(engine: Engine, options: CdpOptions) -> Result<Arc<Self>> {
        let context = engine.new_context(options.default_context.clone())?;
        let default_context = random_id();
        let contexts = HashMap::from([(default_context.clone(), context)]);
        Ok(Arc::new(Self {
            engine,
            options,
            browser_id: random_id(),
            default_context,
            contexts: Mutex::new(contexts),
            targets: Mutex::new(Vec::new()),
            events: broadcast::channel(256).0,
            stats: watch::channel(Stats::default()).0,
        }))
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn options(&self) -> &CdpOptions {
        &self.options
    }

    /// The identifier in `/devtools/browser/{id}`.
    pub fn browser_id(&self) -> &str {
        &self.browser_id
    }

    pub fn default_context_id(&self) -> &str {
        &self.default_context
    }

    pub fn context(&self, id: &str) -> Option<BrowserContext> {
        lock(&self.contexts).get(id).cloned()
    }

    pub fn context_ids(&self) -> Vec<String> {
        lock(&self.contexts).keys().cloned().collect()
    }

    pub fn create_context(&self, options: ContextOptions) -> Result<String> {
        let context = self.engine.new_context(options)?;
        let id = random_id();
        lock(&self.contexts).insert(id.clone(), context);
        Ok(id)
    }

    /// Closes every target of the context, then the context. The default
    /// context lives as long as the server.
    pub async fn dispose_context(&self, id: &str) -> std::result::Result<(), CdpError> {
        if id == self.default_context {
            return Err(CdpError::server(
                "The default browser context cannot be disposed",
            ));
        }
        let context = lock(&self.contexts)
            .remove(id)
            .ok_or_else(|| CdpError::invalid_params("Failed to find context with id"))?;
        let doomed: Vec<_> = lock(&self.targets)
            .iter()
            .filter(|target| target.context_id == id)
            .cloned()
            .collect();
        for target in doomed {
            self.close_target(target.id()).await?;
        }
        context.close();
        Ok(())
    }

    /// Live targets in creation order. Pages that closed on their own are
    /// dropped here, and announced as destroyed.
    pub fn targets(&self) -> Vec<Arc<Target>> {
        let (live, gone): (Vec<_>, Vec<_>) = lock(&self.targets)
            .iter()
            .cloned()
            .partition(|target| !target.page.is_closed());
        if !gone.is_empty() {
            lock(&self.targets).retain(|target| !target.page.is_closed());
            for target in gone {
                let _ = self.events.send(TargetEvent::Destroyed(target));
            }
        }
        live
    }

    pub fn target(&self, id: &str) -> Option<Arc<Target>> {
        self.targets().into_iter().find(|target| target.id == id)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TargetEvent> {
        self.events.subscribe()
    }

    /// Opens a page in `context_id` (the default context when `None`) and
    /// starts navigating it to `url` without waiting for the result: the
    /// caller learns about the navigation through events, as in Chromium.
    pub async fn create_target(
        &self,
        context_id: Option<&str>,
        url: Option<&str>,
    ) -> std::result::Result<Arc<Target>, CdpError> {
        let context_id = context_id.unwrap_or(&self.default_context);
        let context = self.context(context_id).ok_or_else(|| {
            CdpError::invalid_params(format!(
                "Failed to find browser context with id {context_id}"
            ))
        })?;
        if self.targets().len() >= self.options.max_targets {
            return Err(CdpError::server(format!(
                "Too many targets (limit {})",
                self.options.max_targets
            )));
        }
        let navigate_to = match url {
            None | Some("" | "about:blank") => None,
            Some(url) => {
                tenta_browser::context::parse_url(url)
                    .map_err(|error| CdpError::invalid_params(error.to_string()))?;
                Some(url.to_owned())
            }
        };
        let options = PageOptions {
            javascript: self.engine.max_tier() >= Tier::Js,
            ..PageOptions::default()
        };
        let page = context.new_page(options).await?;
        page.add_init_script(crate::interaction::SOURCE);
        let target = Arc::new(Target {
            id: random_id(),
            context_id: context_id.to_owned(),
            page,
            attached: AtomicUsize::new(0),
            contexts: ContextIds::default(),
            console: Arc::new(ConsoleRing::new()),
            overrides: Mutex::default(),
        });
        // Subscribed before the page does anything: a session that enables
        // Runtime later is owed what the page logged meanwhile. The task holds
        // the ring, not the target, so it ends with the page's event stream.
        let (ring, mut events) = (Arc::clone(&target.console), target.page.events());
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                ring.record(event);
            }
        });
        lock(&self.targets).push(Arc::clone(&target));
        let _ = self.events.send(TargetEvent::Created(Arc::clone(&target)));
        if let Some(url) = navigate_to {
            let page = target.page.clone();
            tokio::spawn(async move {
                // A failed navigation surfaces as page events and an error
                // document, not as a failure of the call that opened the page.
                let _ = page.goto(&url, Goto::default()).await;
            });
        }
        Ok(target)
    }

    /// Closes a target. `Ok(false)` when no such target exists.
    pub async fn close_target(&self, id: &str) -> std::result::Result<bool, CdpError> {
        let removed = {
            let mut targets = lock(&self.targets);
            targets
                .iter()
                .position(|target| target.id == id)
                .map(|index| targets.remove(index))
        };
        let Some(target) = removed else {
            return Ok(false);
        };
        let closed = target.page.close().await;
        let _ = self.events.send(TargetEvent::Destroyed(target));
        closed?;
        Ok(true)
    }

    pub(crate) fn connection_opened(&self) {
        self.stats.send_modify(|stats| stats.connections += 1);
    }

    pub(crate) fn connection_closed(&self) {
        self.stats.send_modify(|stats| stats.connections -= 1);
    }

    pub(crate) fn session_opened(&self) {
        self.stats.send_modify(|stats| stats.sessions += 1);
    }

    pub(crate) fn session_closed(&self) {
        self.stats.send_modify(|stats| stats.sessions -= 1);
    }

    pub(crate) fn stats(&self) -> watch::Receiver<Stats> {
        self.stats.subscribe()
    }
}
