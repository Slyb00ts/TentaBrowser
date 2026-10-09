//! Images load when they enter the document (IM4,
//! `docs/dev/p3-plan.md`), not when a screenshot asks for them.
//!
//! HTML's "update the image data" starts a fetch when an `<img>` is inserted or
//! its `src`, `srcset` or `sizes` change, connected or not, and the document's
//! `load` event waits for the fetches that began before it. The render
//! session (`tenta-render`) still owns the pixels and the screenshot still
//! reads them from it, so this module is the other consumer of the IM2
//! machinery: it asks the session what an element selects, fetches and decodes
//! through the same [`drive`] loop a screenshot uses, installs the result in
//! the session, and tells the element's engine what happened.
//!
//! One [`ImageLoader`] lives next to the render session of a navigation. The
//! page loops call [`pump`] on every turn: it notices new and changed image
//! elements (the document's mutation generation, plus the elements script
//! reported through [`PageImageHost`]), starts what is missing, and hands the
//! loads that ended to the engine as `load` and `error` events. The events are
//! delivered by the page loop and never from inside the fetch task, so a handler
//! always runs as its own bounded engine entry.
//!
//! What is deliberately not here: CSS background images (still fetched when a
//! capture lays the page out), `loading=lazy` images (left to the capture, which
//! knows where the box is; they neither delay `load` nor fire events), and
//! re-selecting a `srcset` when the viewport changes.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use tenta_core::atom::atoms;
use tenta_core::{NodeId, NodeRef, PageId, RequestId, Tier};
use tenta_dom::Document as DomDocument;
use tenta_js::image::{ImageEvent, ImageEventKind, ImageHost, ImageSnapshot};
use tenta_render::{ElementImage, ImageKey, ImageStatus, ImageWant, RenderSession, WantOrigin};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;
use url::Url;

use super::{PageInner, TreeHandle, with_document};
use crate::clock::Clock;
use crate::event::PageEvent;
use crate::images::{ImageContext, Loaded, drive};
use crate::render::{self, Sheets};
use crate::runtime;
use crate::settle::{ImageDelay, Origin, RequestClass, SettleTracker};
use crate::task::{catch_unwind_future, panic_message};

/// How long one image may take from the moment it is admitted: the wait a
/// screenshot gives all of its images together.
const IMAGE_WAIT: Duration = Duration::from_secs(20);

/// Image events delivered to the engine in one turn of a page loop.
const EVENTS_PER_TURN: usize = 64;

/// Tracked elements past which the ones the engine has reclaimed are forgotten.
const FORGET_ABOVE: usize = 2048;

/// Requests in flight at once from one page's loader. An element that would
/// start another gets an `error` event, so a script that makes thousands of
/// images cannot grow the loader (or the browser's request queue) without
/// bound.
const MAX_PENDING_LOADS: usize = 512;

/// Keys whose origin flag is remembered before the table starts over.
const FORGET_ORIGINS_ABOVE: usize = 4096;

/// Aborts its task when dropped, so a task cannot outlive what owns it.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Where one element's image stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ElementState {
    /// A fetch for its key is in flight.
    Waiting,
    /// `loading=lazy` and connected: nothing is requested until a capture
    /// finds the box near the viewport.
    Deferred,
    /// The load ended. `ok` false is a broken image.
    Settled { ok: bool },
}

struct Element {
    /// What the element selected; `None` when it selects nothing.
    image: Option<ElementImage>,
    state: ElementState,
}

/// One request for one key, shared by every element that wants it.
struct Flight {
    request: RequestId,
    tracker: Rc<RefCell<SettleTracker>>,
    /// Holds the document's `load` back while the image loads.
    delay: Option<ImageDelay>,
    waiters: Vec<NodeRef>,
    _cap: AbortOnDrop,
}

impl Drop for Flight {
    /// A request the loader forgets (the navigation replaced it) is over as far
    /// as the settle machine is concerned.
    fn drop(&mut self) {
        drop(self.delay.take());
        if let Ok(mut tracker) = self.tracker.try_borrow_mut() {
            tracker.note_request_done(self.request);
        }
    }
}

#[derive(Default)]
struct State {
    /// The tree the elements below belong to (`Document::document_id`, which
    /// survives the tree moving between the page and its engine); a snapshot
    /// asked about another tree is not ours to answer.
    document_id: Option<u64>,
    document_url: Option<Url>,
    elements: HashMap<NodeRef, Element>,
    flights: HashMap<ImageKey, Flight>,
    seen_generation: Option<u64>,
    /// Loads that ended, for the engine to report.
    ///
    /// Each carries the key the load was for, so one that no longer matches
    /// what the element selects (script changed `src` meanwhile) is dropped at
    /// delivery instead of reporting a load the element never made.
    events: Vec<(ImageEvent, Option<ImageKey>)>,
    /// Fetches asked for and not yet taken by the runner.
    queue: Vec<ImageWant>,
    /// Whether each fetched key was same-origin with the document (data: URLs
    /// are clean). Canvas `drawImage` will taint on a `false`.
    origin_clean: HashMap<ImageKey, bool>,
    runner: Option<AbortOnDrop>,
    /// Wakes the page loop when events are queued off its turn.
    wake: Option<Arc<Notify>>,
}

impl State {
    /// The engine is to let go of `node`: nothing is loading for it.
    fn push_release(&mut self, node: NodeRef) {
        self.events.push((
            ImageEvent {
                node,
                kind: ImageEventKind::Release,
            },
            None,
        ));
    }

    /// `node`'s load ended. An element that fires no events (a video poster)
    /// still gets a release, for the pin script's write may have taken.
    fn push_end(&mut self, node: NodeRef, image: &ElementImage, ok: bool, touched: bool) {
        if image.fires_events {
            let kind = if ok {
                ImageEventKind::Load
            } else {
                ImageEventKind::Error
            };
            self.events
                .push((ImageEvent { node, kind }, Some(image.key.clone())));
        } else if touched {
            self.push_release(node);
        }
    }
}

/// The image loader of one navigation's render session.
pub(crate) struct ImageLoader {
    state: RefCell<State>,
    session: Weak<RefCell<RenderSession>>,
    /// Wakes the runner when wants arrive while it waits on a fetch.
    arrived: Rc<Notify>,
    /// Signalled when the runner has nothing left to do.
    idle: Rc<Notify>,
    running: Rc<Cell<bool>>,
}

/// What a sweep reads.
pub(crate) struct SweepInput<'a> {
    pub doc: &'a DomDocument,
    /// Elements script touched since the last sweep.
    pub dirty: Vec<NodeId>,
    pub tracker: &'a Rc<RefCell<SettleTracker>>,
    pub session: &'a Rc<RefCell<RenderSession>>,
    pub sheets: &'a Rc<Mutex<Sheets>>,
    /// The page's clock, which the load-delay cap is counted on.
    pub clock: Arc<dyn Clock>,
}

impl ImageLoader {
    pub(crate) fn new(session: Weak<RefCell<RenderSession>>) -> Rc<ImageLoader> {
        Rc::new(ImageLoader {
            state: RefCell::new(State::default()),
            session,
            arrived: Rc::new(Notify::new()),
            idle: Rc::new(Notify::new()),
            running: Rc::new(Cell::new(false)),
        })
    }

    /// Whether nothing is queued and no fetch runs.
    pub(crate) fn is_idle(&self) -> bool {
        !self.running.get() && self.state.borrow().queue.is_empty()
    }

    /// Resolves once the loader has nothing queued and no fetch running.
    pub(crate) async fn wait_idle(&self) {
        loop {
            let idle = self.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self.is_idle() {
                return;
            }
            idle.await;
        }
    }

    /// Whether the image of `key` came from the document's own origin. `None`
    /// when this loader never fetched it. The reader is canvas `drawImage`,
    /// which taints on a cross-origin image and does not exist yet.
    #[allow(dead_code)]
    pub(crate) fn origin_clean(&self, key: &ImageKey) -> Option<bool> {
        self.state.borrow().origin_clean.get(key).copied()
    }

    /// Up to `limit` of the events queued, oldest first, each with the key its
    /// load was for. Delivery re-checks [`Self::event_is_current`] one event at a time,
    /// because a handler can change another element's `src` before that
    /// element's event comes up.
    pub(crate) fn take_events(&self, limit: usize) -> Vec<(ImageEvent, Option<ImageKey>)> {
        let mut state = self.state.borrow_mut();
        let at = limit.min(state.events.len());
        let rest = state.events.split_off(at);
        std::mem::replace(&mut state.events, rest)
    }

    /// Whether events are waiting for a later turn.
    pub(crate) fn has_events(&self) -> bool {
        !self.state.borrow().events.is_empty()
    }

    /// Whether `event` still describes what its element selects: a load for a
    /// source the element has since left is not reported.
    pub(crate) fn event_is_current(
        &self,
        doc: &DomDocument,
        event: &ImageEvent,
        key: &Option<ImageKey>,
    ) -> bool {
        let Some(key) = key else {
            return true;
        };
        let Ok(node) = doc.resolve(event.node) else {
            return false;
        };
        let Some(session) = self.session.upgrade() else {
            return false;
        };
        let Ok(session) = session.try_borrow() else {
            return true;
        };
        let about_blank = || Url::parse("about:blank").expect("a constant URL");
        let base = doc
            .base_url()
            .or(doc.url())
            .cloned()
            .unwrap_or_else(about_blank);
        session
            .element_image(doc, node, &base)
            .is_some_and(|image| &image.key == key)
    }

    /// A capture installed images the loader had left alone (`loading=lazy`):
    /// the elements that were waiting for them are complete now, and report it.
    pub(crate) fn rescan_deferred(&self) {
        let Some(session) = self.session.upgrade() else {
            return;
        };
        let Ok(session) = session.try_borrow() else {
            return;
        };
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let mut ended = Vec::new();
        for (node, element) in &mut state.elements {
            let (ElementState::Deferred, Some(image)) = (element.state, &element.image) else {
                continue;
            };
            let ok = match session.image_status(&image.key, image.density) {
                ImageStatus::Loaded { .. } => true,
                ImageStatus::Failed => false,
                ImageStatus::Unknown => continue,
            };
            element.state = ElementState::Settled { ok };
            if image.fires_events {
                ended.push((*node, image.key.clone(), ok));
            }
        }
        for (node, key, ok) in ended {
            let kind = if ok {
                ImageEventKind::Load
            } else {
                ImageEventKind::Error
            };
            state.events.push((ImageEvent { node, kind }, Some(key)));
        }
        if let Some(wake) = &state.wake
            && !state.events.is_empty()
        {
            wake.notify_one();
        }
    }

    fn take_queue(&self) -> Vec<ImageWant> {
        std::mem::take(&mut self.state.borrow_mut().queue)
    }

    /// Looks at the image elements of `input.doc` that are new or that script
    /// touched, starts the loads they need and queues the events for the ones
    /// already known.
    pub(crate) fn sweep(self: &Rc<Self>, inner: &Arc<PageInner>, input: SweepInput<'_>) {
        let doc = input.doc;
        let generation = doc.mutation_generation();
        let base = {
            let mut state = self.state.borrow_mut();
            state.document_id = Some(doc.document_id());
            state.wake = Some(input.tracker.borrow().notify_handle());
            if state.seen_generation == Some(generation) && input.dirty.is_empty() {
                return;
            }
            state.seen_generation = Some(generation);
            let about_blank = || Url::parse("about:blank").expect("a constant URL");
            let base = doc
                .base_url()
                .or(doc.url())
                .cloned()
                .unwrap_or_else(about_blank);
            state.document_url = Some(doc.url().cloned().unwrap_or_else(|| base.clone()));
            if state.elements.len() > FORGET_ABOVE {
                state.elements.retain(|node, _| doc.resolve(*node).is_ok());
            }
            base
        };
        let mut candidates: Vec<NodeId> = Vec::new();
        {
            let state = self.state.borrow();
            for tag in ["img", "input", "video"] {
                candidates.extend(
                    doc.get_elements_by_tag_name(tag)
                        .into_iter()
                        .filter(|node| tenta_render::is_image_element(doc, *node))
                        .filter(|node| !state.elements.contains_key(&doc.node_ref(*node))),
                );
            }
        }
        let dirty: HashSet<NodeId> = input
            .dirty
            .iter()
            .copied()
            .filter(|node| doc.is_live(*node))
            .collect();
        for &node in &dirty {
            if doc.tag(node) == Some(atoms::SOURCE) {
                // A `<picture>`'s sources choose for the images after them;
                // the source itself has nothing to load.
                let parent = doc.parent(node);
                if parent != NodeId::NONE {
                    candidates.extend(
                        doc.children(parent)
                            .filter(|child| doc.tag(*child) == Some(atoms::IMG)),
                    );
                }
                self.state.borrow_mut().push_release(doc.node_ref(node));
            } else {
                candidates.push(node);
            }
        }
        let mut seen = HashSet::new();
        candidates.retain(|node| seen.insert(*node));
        let mut started = false;
        for node in candidates {
            started |= self.update(&input, node, &base, dirty.contains(&node));
        }
        if started {
            self.ensure_runner(inner, input.session, input.sheets);
        }
    }

    /// HTML's "update the image data" for one element, as far as it matters
    /// here. `touched` is whether script reported the element, which means the
    /// engine pinned it and waits for one event about it. Returns whether a
    /// fetch was queued.
    fn update(
        self: &Rc<Self>,
        input: &SweepInput<'_>,
        node: NodeId,
        base: &Url,
        touched: bool,
    ) -> bool {
        let doc = input.doc;
        let nref = doc.node_ref(node);
        let current = input.session.borrow().element_image(doc, node, base);
        let is_img = doc.tag(node) == Some(atoms::IMG);
        let mut guard = self.state.borrow_mut();
        let state = &mut *guard;
        let previous = state.elements.get(&nref);
        if current.is_none() && previous.is_none() && !is_img {
            if touched {
                state.push_release(nref);
            }
            return false;
        }
        let same = previous.is_some_and(|previous| {
            previous.image.as_ref().map(|image| &image.key) == current.as_ref().map(|c| &c.key)
        });
        if same {
            if touched {
                state.push_release(nref);
            }
            return false;
        }
        // The element no longer waits for the key it had.
        if let Some(Element {
            image: Some(old),
            state: ElementState::Waiting,
        }) = previous
        {
            let key = old.key.clone();
            if let Some(flight) = state.flights.get_mut(&key) {
                flight.waiters.retain(|waiter| *waiter != nref);
            }
        }
        let Some(image) = current else {
            state.elements.insert(
                nref,
                Element {
                    image: None,
                    state: ElementState::Settled { ok: true },
                },
            );
            if touched {
                state.push_release(nref);
            }
            return false;
        };
        let status = input
            .session
            .borrow()
            .image_status(&image.key, image.density);
        let fires = image.fires_events;
        let mut queued = false;
        let element_state = match status {
            ImageStatus::Loaded { .. } => {
                state.push_end(nref, &image, true, touched);
                ElementState::Settled { ok: true }
            }
            ImageStatus::Failed => {
                state.push_end(nref, &image, false, touched);
                ElementState::Settled { ok: false }
            }
            ImageStatus::Unknown if image.lazy && doc.is_connected(node) => {
                if touched {
                    state.push_release(nref);
                }
                ElementState::Deferred
            }
            ImageStatus::Unknown
                if !state.flights.contains_key(&image.key)
                    && state.flights.len() >= MAX_PENDING_LOADS =>
            {
                // Too many requests at once from this page: this one fails.
                state.push_end(nref, &image, false, touched);
                ElementState::Settled { ok: false }
            }
            ImageStatus::Unknown => {
                queued = self.join_flight(state, input, nref, &image);
                ElementState::Waiting
            }
        };
        let _ = fires;
        state.elements.insert(
            nref,
            Element {
                image: Some(image),
                state: element_state,
            },
        );
        queued
    }

    /// Makes `nref` wait for `image`'s key, starting the request when none is
    /// in flight. Returns whether a request was queued.
    fn join_flight(
        self: &Rc<Self>,
        state: &mut State,
        input: &SweepInput<'_>,
        nref: NodeRef,
        image: &ElementImage,
    ) -> bool {
        if let Some(flight) = state.flights.get_mut(&image.key) {
            flight.waiters.push(nref);
            return false;
        }
        let tracker = input.tracker;
        let request = crate::page::next_request_id();
        let cap = tracker.borrow().config().foreground_request_max;
        // `load` waits for an image that began before it, but not for ever: a
        // hung image releases the hold after the foreground request limit and
        // goes on loading as an ordinary request.
        let load_fired = tracker.borrow().load_fired();
        let delay = (!load_fired).then(|| ImageDelay::hold(tracker));
        tracker
            .borrow_mut()
            .note_request_started(request, RequestClass::Image, Origin::Tracked);
        let weak = Rc::downgrade(self);
        let key = image.key.clone();
        let clock = Arc::clone(&input.clock);
        let cap_task = tokio::task::spawn_local({
            let key = key.clone();
            async move {
                // Counted on the page's clock, so a test that drives it by hand
                // controls the cap; the sleep only decides how often to look.
                let started = clock.now();
                while clock.now().saturating_duration_since(started) < cap {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                if let Some(loader) = weak.upgrade()
                    && let Some(flight) = loader.state.borrow_mut().flights.get_mut(&key)
                {
                    flight.delay = None;
                }
            }
        });
        if state.origin_clean.len() > FORGET_ORIGINS_ABOVE {
            state.origin_clean.clear();
        }
        let clean = is_origin_clean(&image.url, state.document_url.as_ref());
        state.origin_clean.insert(key.clone(), clean);
        state.flights.insert(
            key,
            Flight {
                request,
                tracker: Rc::clone(tracker),
                delay,
                waiters: vec![nref],
                _cap: AbortOnDrop(cap_task),
            },
        );
        state.queue.push(ImageWant {
            url: image.url.clone(),
            key: image.key.clone(),
            origin: WantOrigin::Element,
            rerender: None,
        });
        true
    }

    fn ensure_runner(
        self: &Rc<Self>,
        inner: &Arc<PageInner>,
        session: &Rc<RefCell<RenderSession>>,
        sheets: &Rc<Mutex<Sheets>>,
    ) {
        if self.running.get() {
            self.arrived.notify_one();
            return;
        }
        self.running.set(true);
        let weak = Rc::downgrade(self);
        let (arrived, idle, running) = (
            Rc::clone(&self.arrived),
            Rc::clone(&self.idle),
            Rc::clone(&self.running),
        );
        let (inner, session, sheets) = (Arc::clone(inner), Rc::clone(session), Rc::clone(sheets));
        let task = tokio::task::spawn_local(async move {
            let outcome = catch_unwind_future(run(weak, &inner, &session, &sheets, &arrived)).await;
            running.set(false);
            idle.notify_waiters();
            if let Err(payload) = outcome {
                let reason = panic_message(&payload);
                tracing::error!(page = ?inner.config.id, reason, "the image loader panicked");
                inner.mark_crashed(&reason);
            }
        });
        self.state.borrow_mut().runner = Some(AbortOnDrop(task));
    }

    /// Hands one outcome to the session and to the elements waiting for it.
    fn complete(&self, inner: &PageInner, session: &RefCell<RenderSession>, loaded: Loaded) {
        let key = loaded.want.key.clone();
        let installed = render::install_images(session, vec![loaded]);
        let ok = installed.failures.is_empty();
        for (want, reason) in installed.failures {
            inner.emit(PageEvent::ImageFailed {
                url: want.url.as_str().chars().take(256).collect(),
                reason,
            });
        }
        self.finish(&key, ok);
    }

    /// The fetch for `key` ended: settle its waiters, queue their events, and
    /// only then release the load hold and the request, so the page loop that
    /// the release wakes finds the events waiting.
    fn finish(&self, key: &ImageKey, ok: bool) {
        let flight = {
            let mut guard = self.state.borrow_mut();
            let state = &mut *guard;
            let flight = state.flights.remove(key);
            if let Some(flight) = &flight {
                for waiter in &flight.waiters {
                    let Some(element) = state.elements.get_mut(waiter) else {
                        continue;
                    };
                    let Some(image) = element.image.as_ref() else {
                        continue;
                    };
                    if element.state != ElementState::Waiting || &image.key != key {
                        continue;
                    }
                    element.state = ElementState::Settled { ok };
                    let image = image.clone();
                    state.push_end(*waiter, &image, ok, true);
                }
            }
            flight
        };
        // Dropping the flight releases the load hold and the request, after
        // the events above are queued, so the page loop that wakes finds them.
        drop(flight);
        if let Some(wake) = self.state.borrow().wake.clone() {
            wake.notify_one();
        }
    }

    /// What script reads from `node`, or `None` when this loader does not know
    /// the tree `document` names.
    fn snapshot(
        &self,
        document: &Weak<RefCell<DomDocument>>,
        node: NodeId,
    ) -> Option<ImageSnapshot> {
        let state = self.state.try_borrow().ok()?;
        let shared = document.upgrade()?;
        let doc = shared.try_borrow().ok()?;
        if state.document_id != Some(doc.document_id()) || !doc.is_live(node) {
            return None;
        }
        let session = self.session.upgrade()?;
        let session = session.try_borrow().ok()?;
        let base = state.document_url.clone()?;
        let base = doc.base_url().cloned().unwrap_or(base);
        let Some(current) = session.element_image(&doc, node, &base) else {
            // Nothing to load.
            return Some(ImageSnapshot {
                complete: true,
                ..ImageSnapshot::default()
            });
        };
        let pending = ImageSnapshot {
            complete: false,
            current_src: current.url.to_string(),
            ..ImageSnapshot::default()
        };
        let tracked = state.elements.get(&doc.node_ref(node));
        let Some(tracked) = tracked.filter(|t| {
            t.image
                .as_ref()
                .is_some_and(|image| image.key == current.key)
        }) else {
            // Changed since the loader last looked, or not yet seen.
            return Some(pending);
        };
        Some(match session.image_status(&current.key, current.density) {
            ImageStatus::Loaded { width, height } => ImageSnapshot {
                complete: true,
                natural_width: width,
                natural_height: height,
                current_src: pending.current_src,
                broken: false,
            },
            ImageStatus::Failed => ImageSnapshot {
                complete: true,
                broken: true,
                ..pending
            },
            ImageStatus::Unknown => match tracked.state {
                // A failure the session does not remember (a timeout).
                ElementState::Settled { ok: false } => ImageSnapshot {
                    complete: true,
                    broken: true,
                    ..pending
                },
                _ => pending,
            },
        })
    }
}

/// Whether an image fetched from `image` taints nothing in a document at
/// `document`: it is the document's own origin, or a `data:` URL. CORS-mode
/// requests (`crossorigin`) are not implemented, so a cross-origin image is
/// never clean.
fn is_origin_clean(image: &Url, document: Option<&Url>) -> bool {
    if image.scheme() == "data" {
        return true;
    }
    document.is_some_and(|document| image.origin() == document.origin())
}

/// The runner: holds the page's image ledger for as long as there is work, so
/// a screenshot (which takes the same lock) waits for the images in flight
/// rather than fetching them twice.
async fn run(
    weak: Weak<ImageLoader>,
    inner: &Arc<PageInner>,
    session: &Rc<RefCell<RenderSession>>,
    sheets: &Rc<Mutex<Sheets>>,
    arrived: &Rc<Notify>,
) {
    let mut sheets = sheets.lock().await;
    let Some(document_url) = weak
        .upgrade()
        .and_then(|loader| loader.state.borrow().document_url.clone())
    else {
        return;
    };
    let decode = session.borrow().decode_settings();
    let config = &inner.config;
    let ctx = ImageContext {
        client: &config.client,
        fs_policy: &config.fs_policy,
        budgets: &config.budgets,
        clock: &*config.clock,
        extra_headers: &config.extra_headers,
        document_url: &document_url,
        deadline: config.clock.now() + IMAGE_WAIT,
        rolling: Some(IMAGE_WAIT),
    };
    drive(
        decode,
        &ctx,
        &mut sheets.images,
        Some(arrived),
        || weak.upgrade().map(|l| l.take_queue()).unwrap_or_default(),
        |_, loaded| {
            if let Some(loader) = weak.upgrade() {
                loader.complete(inner, session, loaded);
            }
        },
    )
    .await;
}

/// The page's answers about image elements, handed to the engine.
///
/// It belongs to the runtime, which exists before any render session does, so
/// it keeps what script reports (`attribute_changed`) until the next pump and
/// asks the loader of the page for the rest. A host that is not `enabled` (the
/// page cannot reach the render tier) loads nothing: every image is complete,
/// nothing is pinned, and `decode()` rejects.
pub(crate) struct PageImageHost {
    page: PageId,
    document: Weak<RefCell<DomDocument>>,
    enabled: bool,
    dirty: RefCell<Vec<NodeId>>,
    queued: RefCell<HashSet<NodeId>>,
    /// The document generation the last "does the page have an image at all"
    /// probe looked at.
    probed: Cell<Option<u64>>,
    wake: Arc<Notify>,
}

impl PageImageHost {
    pub(crate) fn new(
        page: PageId,
        document: Weak<RefCell<DomDocument>>,
        wake: Arc<Notify>,
        enabled: bool,
    ) -> PageImageHost {
        PageImageHost {
            page,
            document,
            enabled,
            dirty: RefCell::new(Vec::new()),
            queued: RefCell::new(HashSet::new()),
            probed: Cell::new(None),
            wake,
        }
    }

    /// The elements script touched since the last call, each once.
    pub(crate) fn take_dirty(&self) -> Vec<NodeId> {
        self.queued.borrow_mut().clear();
        std::mem::take(&mut self.dirty.borrow_mut())
    }

    /// Whether the document changed since the last probe, and notes `generation`
    /// as probed.
    pub(crate) fn changed_since_probe(&self, generation: u64) -> bool {
        self.probed.replace(Some(generation)) != Some(generation)
    }
}

impl ImageHost for PageImageHost {
    fn snapshot(&self, node: NodeId) -> ImageSnapshot {
        if self.enabled
            && let Some(loader) = render::loader_of(self.page)
            && let Some(snapshot) = loader.snapshot(&self.document, node)
        {
            return snapshot;
        }
        // No loader yet: script just created the element or set its source
        // before the page looked. Loading is pending exactly when a source is
        // set (and the page loads images at all).
        let has_source = self.enabled
            && self.document.upgrade().is_some_and(|shared| {
                shared.try_borrow().is_ok_and(|doc| {
                    ["src", "srcset"].iter().any(|name| {
                        doc.attribute_str(node, name)
                            .is_some_and(|value| !value.trim().is_empty())
                    })
                })
            });
        ImageSnapshot {
            complete: !has_source,
            ..ImageSnapshot::default()
        }
    }

    fn attribute_changed(&self, node: NodeId) -> bool {
        if !self.enabled {
            return false;
        }
        if self.queued.borrow_mut().insert(node) {
            self.dirty.borrow_mut().push(node);
        }
        self.wake.notify_one();
        true
    }
}

/// One turn of image work for the document `tree` names: notices new and
/// changed image elements, starts their loads, and delivers the `load` and
/// `error` events of the loads that ended to the engine.
///
/// `navigation` is the generation of the document, which keys the page's
/// render session. A page that cannot reach the render tier loads no images.
pub(super) fn pump(
    inner: &Arc<PageInner>,
    tracker: &Rc<RefCell<SettleTracker>>,
    tree: &TreeHandle,
    navigation: u64,
) {
    if inner.config.max_tier < Tier::Render {
        return;
    }
    {
        let state = inner.state.lock();
        if state.closed || state.crashed.is_some() {
            return;
        }
    }
    let page = inner.config.id;
    let shared = tree.as_ref().and_then(Weak::upgrade);
    let dirty = shared
        .as_ref()
        .and_then(|document| {
            runtime::with_runtime_for(page, document, |runtime| runtime.take_image_dirty())
        })
        .unwrap_or_default();

    // A page whose document has no image element never creates a render
    // session for this, and a quiet one is not rescanned: the probe looks again
    // only when the document changed.
    let known = render::loader_exists(page, navigation);
    if !known && dirty.is_empty() {
        let changed = shared
            .as_ref()
            .and_then(|document| {
                let generation = document.borrow().mutation_generation();
                runtime::with_runtime_for(page, document, |runtime| runtime.image_probe(generation))
            })
            .unwrap_or(true);
        if !changed {
            return;
        }
        let any = with_document(inner, tree, |doc| {
            ["img", "video", "input"].iter().any(|tag| {
                doc.get_elements_by_tag_name(tag)
                    .into_iter()
                    .any(|node| tenta_render::is_image_element(doc, node))
            })
        })
        .unwrap_or(false);
        if !any {
            return;
        }
    }
    let viewport = *inner.viewport.lock();
    let handles = render::handles(page, navigation, viewport, inner.config.system_fonts);
    with_document(inner, tree, |doc| {
        handles.loader.sweep(
            inner,
            SweepInput {
                doc,
                dirty,
                tracker,
                session: &handles.session,
                sheets: &handles.sheets,
                clock: Arc::clone(&inner.config.clock),
            },
        );
    });
    // A flood of loads is delivered over several turns, so timers and the rest
    // of the page keep running between batches.
    let events = handles.loader.take_events(EVENTS_PER_TURN);
    if events.is_empty() {
        return;
    }
    let Some(document) = shared else {
        return;
    };
    let keys: Vec<Option<ImageKey>> = events.iter().map(|(_, key)| key.clone()).collect();
    let events: Vec<ImageEvent> = events.into_iter().map(|(event, _)| event).collect();
    let loader = &handles.loader;
    let mut current = |index: usize, event: &ImageEvent| {
        document.try_borrow().map_or(true, |doc| {
            loader.event_is_current(&doc, event, &keys[index])
        })
    };
    runtime::with_runtime_for(page, &document, |runtime| {
        runtime.dispatch_image_events(events, &mut current);
    });
    if loader.has_events() {
        tracker.borrow().notify_handle().notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("a URL")
    }

    #[test]
    fn only_the_documents_own_origin_and_data_urls_are_clean() {
        let document = url("https://site.test/page.html");
        assert!(is_origin_clean(
            &url("https://site.test/a.png"),
            Some(&document)
        ));
        assert!(is_origin_clean(
            &url("data:image/png;base64,AAAA"),
            Some(&document)
        ));
        assert!(is_origin_clean(&url("data:image/png;base64,AAAA"), None));
        assert!(!is_origin_clean(
            &url("https://cdn.test/a.png"),
            Some(&document)
        ));
        assert!(!is_origin_clean(
            &url("http://site.test/a.png"),
            Some(&document)
        ));
        assert!(!is_origin_clean(
            &url("https://site.test:8443/a.png"),
            Some(&document)
        ));
        assert!(!is_origin_clean(&url("https://site.test/a.png"), None));
    }
}
