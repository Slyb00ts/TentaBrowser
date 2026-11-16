//! Stylesheets load when their element enters the document (CSS1,
//! `docs/dev/p3-plan.md`), not when a screenshot asks for them.
//!
//! HTML's "fetch and process the linked resource" starts when a
//! `<link rel=stylesheet>` is inserted, by the parser or by script, and three
//! things hang on its outcome:
//!
//! * the document's `load` waits for it (an alternate sheet does not);
//! * a link the *parser* inserted blocks script execution until it has loaded
//!   or failed ("a style sheet that is blocking scripts"), so a script that
//!   reads styles or layout sees the sheet; a link a script inserted does not;
//! * the element receives a `load` or an `error` event.
//!
//! One [`SheetLoader`] lives next to the render session of a navigation. It
//! fills the same [`StyleState`] the session's sources are assembled from, so a
//! screenshot or a geometry query finds every sheet already fetched and
//! requests nothing. Fetches run as page-thread tasks, one per distinct sheet
//! however many elements name it, through the code the screenshot path uses
//! (`styles::fetch_root`), so a request is built, budgeted, decoded and
//! reported to the network host exactly as before.
//!
//! The page loops call [`SheetLoader::sweep`] on every turn (it costs one
//! integer compare when the document did not change), and the parse calls it
//! at each script pause. A sweep run at a pause during the parse marks the
//! links it finds as parser-inserted; the sweep that follows the script marks
//! the ones the script inserted. That is the one inexactness: a link the parser
//! inserted is found at the next pause or at the end of the parse, not the
//! moment it is tokenised, so its fetch starts a little later than a browser's
//! preload scanner would start it.
//!
//! Events are delivered by the page loop (`image_loader::pump`) through the
//! engine's image-event path, which fires a trusted `load` or `error` at an
//! element, and never from inside a fetch task.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use tenta_core::{NodeRef, RequestId, Tier};
use tenta_dom::Document as DomDocument;
use tenta_js::image::{ImageEvent, ImageEventKind};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use url::Url;

use super::{PageInner, SharedDocument};
use crate::event::PageEvent;
use crate::fetch_policy::FetchPolicy;
use crate::render;
use crate::settle::{ImageDelay, Origin, RequestClass, SettleTracker};
use crate::styles::{
    Env, SheetElement, SheetSource, StyleContext, StyleState, fetch_root, load_imports,
    sheet_elements,
};
use crate::task::{catch_unwind_future, panic_message};

/// How long one sheet may take from the moment its fetch starts.
const SHEET_WAIT: Duration = Duration::from_secs(30);

/// Aborts its task when dropped, so a task cannot outlive what owns it.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Where one element's sheet stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Its fetch (and its imports) are in flight.
    Waiting,
    /// It ended; `ok` false is a failed link.
    Done { ok: bool },
}

struct Entry {
    source: SheetSource,
    /// Tells a stale event from a current one: it changes whenever the element
    /// starts a new load.
    serial: u64,
    phase: Phase,
    /// Holds script execution back while the sheet loads.
    blocking: bool,
    /// Only a `<link>` fires `load` and `error`.
    fires: bool,
    /// Holds the document's `load` back while the sheet loads.
    delay: Option<ImageDelay>,
}

/// One fetch of one sheet, shared by every element that names it.
struct Flight {
    request: RequestId,
    tracker: Rc<RefCell<SettleTracker>>,
    /// The fetch task. Taken (and so detached) by the task itself when it ends,
    /// so finishing a flight never aborts the task that is finishing it.
    task: Option<JoinHandle<()>>,
    _cap: AbortOnDrop,
}

impl Drop for Flight {
    /// A request the loader forgets (the navigation replaced it) is over as far
    /// as the settle machine is concerned.
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if let Ok(mut tracker) = self.tracker.try_borrow_mut() {
            tracker.note_request_done(self.request);
        }
    }
}

#[derive(Default)]
struct State {
    /// The tree the elements below belong to.
    document_id: Option<u64>,
    seen_generation: Option<u64>,
    wake: Option<Arc<Notify>>,
    entries: HashMap<NodeRef, Entry>,
    flights: HashMap<SheetSource, Flight>,
    /// Loads that ended, for the engine to report, with the entry's serial.
    events: Vec<(ImageEvent, u64)>,
    serial: u64,
}

/// What a sweep reads.
pub(crate) struct SheetSweep<'a> {
    pub doc: &'a DomDocument,
    pub tracker: &'a Rc<RefCell<SettleTracker>>,
    /// The parser is paused on a script (or has just ended): the sheets found
    /// now are the parser's, and block scripts.
    pub parser_inserted: bool,
    /// The encoding the parser has chosen so far, which the document records
    /// only at the end of the parse.
    pub encoding: Option<&'static tenta_html::Encoding>,
    /// The viewport in CSS pixels, which `media` is judged against.
    pub viewport: (f32, f32),
}

/// The stylesheet loader of one navigation's render session.
pub(crate) struct SheetLoader {
    state: RefCell<State>,
    style: Rc<RefCell<StyleState>>,
    /// Woken whenever a flight ends.
    idle: Rc<Notify>,
}

impl SheetLoader {
    pub(crate) fn new(style: Rc<RefCell<StyleState>>) -> Rc<SheetLoader> {
        Rc::new(SheetLoader {
            state: RefCell::new(State::default()),
            style,
            idle: Rc::new(Notify::new()),
        })
    }

    /// The fetched sheets, shared with the render session's sources.
    pub(crate) fn style(&self) -> Rc<RefCell<StyleState>> {
        Rc::clone(&self.style)
    }

    /// Records that this loader serves the tree `document_id` names, or says it
    /// serves another. The first caller claims it.
    pub(crate) fn claim(&self, document_id: u64) -> bool {
        *self
            .state
            .borrow_mut()
            .document_id
            .get_or_insert(document_id)
            == document_id
    }

    /// Whether no sheet is loading.
    pub(crate) fn is_idle(&self) -> bool {
        self.state.borrow().flights.is_empty()
    }

    /// Resolves once no sheet is loading.
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

    /// Whether a parser-inserted sheet that blocks scripts is still loading.
    pub(crate) fn blocking_pending(&self) -> bool {
        self.state
            .borrow()
            .entries
            .values()
            .any(|entry| entry.blocking && entry.phase == Phase::Waiting)
    }

    /// Resolves once no blocking sheet is loading.
    pub(crate) async fn wait_unblocked(&self) {
        loop {
            let idle = self.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if !self.blocking_pending() {
                return;
            }
            idle.await;
        }
    }

    /// Up to `limit` of the events queued, oldest first, each with the serial of
    /// the load it reports.
    pub(crate) fn take_events(&self, limit: usize) -> Vec<(ImageEvent, u64)> {
        let mut state = self.state.borrow_mut();
        let at = limit.min(state.events.len());
        let rest = state.events.split_off(at);
        std::mem::replace(&mut state.events, rest)
    }

    /// Whether events are waiting for a later turn.
    pub(crate) fn has_events(&self) -> bool {
        !self.state.borrow().events.is_empty()
    }

    /// Whether `event` still describes the element's current load: a load for a
    /// `href` the element has since left is not reported.
    pub(crate) fn event_is_current(
        &self,
        doc: &DomDocument,
        event: &ImageEvent,
        serial: u64,
    ) -> bool {
        doc.resolve(event.node).is_ok()
            && self
                .state
                .borrow()
                .entries
                .get(&event.node)
                .is_some_and(|entry| entry.serial == serial)
    }

    /// Looks at the stylesheet elements of `input.doc`, starts the fetches the
    /// new and changed ones need and queues the events for the ones whose sheet
    /// is already known.
    pub(crate) fn sweep(self: &Rc<Self>, inner: &Arc<PageInner>, input: SheetSweep<'_>) {
        let doc = input.doc;
        let generation = doc.mutation_generation();
        {
            let mut state = self.state.borrow_mut();
            state.document_id = Some(doc.document_id());
            state.wake = Some(input.tracker.borrow().notify_handle());
            if state.seen_generation == Some(generation) {
                return;
            }
            state.seen_generation = Some(generation);
        }
        let about_blank = || Url::parse("about:blank").expect("a constant URL");
        let base = doc
            .base_url()
            .or(doc.url())
            .cloned()
            .unwrap_or_else(about_blank);
        let document_url = doc.url().cloned().unwrap_or_else(|| base.clone());
        let elements = sheet_elements(doc, &base, input.viewport);

        // Elements that left the document or changed what they load.
        let mut dropped = Vec::new();
        {
            let mut state = self.state.borrow_mut();
            let current: HashMap<NodeRef, &SheetElement> = elements
                .iter()
                .map(|element| (doc.node_ref(element.node), element))
                .collect();
            let gone: Vec<NodeRef> = state
                .entries
                .iter()
                .filter(|(node, entry)| {
                    current
                        .get(node)
                        .is_none_or(|element| element.source != entry.source)
                })
                .map(|(node, _)| *node)
                .collect();
            for node in gone {
                dropped.extend(state.entries.remove(&node));
            }
        }
        // Dropped outside the borrow: a delay releases into the settle machine.
        drop(dropped);

        let mut started = false;
        for element in elements {
            let node = doc.node_ref(element.node);
            if self.state.borrow().entries.contains_key(&node) {
                continue;
            }
            started |= self.begin(inner, &input, &element, node, doc, &document_url);
        }
        if started {
            self.idle.notify_waiters();
        }
    }

    /// Starts the load of one new or changed element. Returns whether it began
    /// anything.
    fn begin(
        self: &Rc<Self>,
        inner: &Arc<PageInner>,
        input: &SheetSweep<'_>,
        element: &SheetElement,
        node: NodeRef,
        doc: &DomDocument,
        document_url: &Url,
    ) -> bool {
        let tracker = input.tracker;
        let load_fired = tracker.borrow().load_fired();
        let blocking = input.parser_inserted && !element.alternate && element.media_matches;
        let fires = matches!(element.source, SheetSource::Link(..));
        let delay = (!load_fired && !element.alternate).then(|| ImageDelay::hold(tracker));
        let known = match &element.source {
            SheetSource::Link(url, _) => {
                let mut style = self.style.borrow_mut();
                match style.sheet_state(url) {
                    // A link that enters the document again gets a new try at
                    // a sheet that failed before (a server that was down).
                    Some(false) => {
                        style.forget_failed(url);
                        None
                    }
                    known => known,
                }
            }
            SheetSource::Style(_) => None,
        };
        let mut state = self.state.borrow_mut();
        state.serial += 1;
        let serial = state.serial;
        if let Some(ok) = known {
            // Fetched before (another element named it, or a capture did): the
            // event still comes, on the next turn.
            if fires {
                state.events.push((
                    ImageEvent {
                        node,
                        kind: if ok {
                            ImageEventKind::Load
                        } else {
                            ImageEventKind::Error
                        },
                    },
                    serial,
                ));
            }
            state.entries.insert(
                node,
                Entry {
                    source: element.source.clone(),
                    serial,
                    phase: Phase::Done { ok },
                    blocking: false,
                    fires,
                    delay: None,
                },
            );
            drop(delay);
            if let Some(wake) = &state.wake {
                wake.notify_one();
            }
            return false;
        }
        state.entries.insert(
            node,
            Entry {
                source: element.source.clone(),
                serial,
                phase: Phase::Waiting,
                blocking,
                fires,
                delay,
            },
        );
        if state.flights.contains_key(&element.source) {
            return false;
        }
        let request = crate::page::next_request_id();
        tracker
            .borrow_mut()
            .note_request_started(request, RequestClass::Style, Origin::Tracked);
        let cap = tracker.borrow().config().foreground_request_max;
        let weak = Rc::downgrade(self);
        let clock = Arc::clone(&inner.config.clock);
        let cap_task = tokio::task::spawn_local({
            let (source, weak) = (element.source.clone(), weak.clone());
            async move {
                // Counted on the page's clock, so a test that drives it by hand
                // controls the cap; the sleep only decides how often to look.
                let started = clock.now();
                while clock.now().saturating_duration_since(started) < cap {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                if let Some(loader) = weak.upgrade() {
                    loader.release_delays(&source);
                }
            }
        });
        let task = tokio::task::spawn_local(run_flight(
            weak,
            Arc::clone(inner),
            element.clone(),
            Env::of(doc, input.encoding),
            document_url.clone(),
            Rc::clone(&self.style),
        ));
        state.flights.insert(
            element.source.clone(),
            Flight {
                request,
                tracker: Rc::clone(tracker),
                task: Some(task),
                _cap: AbortOnDrop(cap_task),
            },
        );
        true
    }

    /// `load` waits for a sheet that began before it, but not for ever: a hung
    /// sheet releases the hold after the foreground request limit and goes on
    /// loading as an ordinary request.
    fn release_delays(&self, source: &SheetSource) {
        let released: Vec<ImageDelay> = self
            .state
            .borrow_mut()
            .entries
            .values_mut()
            .filter(|entry| &entry.source == source)
            .filter_map(|entry| entry.delay.take())
            .collect();
        drop(released);
    }

    /// The fetch for `source` ended: settle its waiters, queue their events, and
    /// only then release the holds, so the page loop that the release wakes
    /// finds the events waiting.
    fn finish(&self, source: &SheetSource) {
        let mut released = Vec::new();
        let flight = {
            let mut guard = self.state.borrow_mut();
            let state = &mut *guard;
            let ok = match source {
                SheetSource::Link(url, _) => self.style.borrow().sheet_state(url).unwrap_or(false),
                SheetSource::Style(_) => true,
            };
            for (node, entry) in &mut state.entries {
                if &entry.source != source || entry.phase != Phase::Waiting {
                    continue;
                }
                entry.phase = Phase::Done { ok };
                if entry.fires {
                    let kind = if ok {
                        ImageEventKind::Load
                    } else {
                        ImageEventKind::Error
                    };
                    state
                        .events
                        .push((ImageEvent { node: *node, kind }, entry.serial));
                }
                released.extend(entry.delay.take());
            }
            let mut flight = state.flights.remove(source);
            if let Some(flight) = &mut flight {
                drop(flight.task.take());
            }
            flight
        };
        drop(released);
        drop(flight);
        self.idle.notify_waiters();
        if let Some(wake) = self.state.borrow().wake.clone() {
            wake.notify_one();
        }
    }
}

/// The task of one flight: fetches the sheet and everything it imports, reports
/// what failed, and tells the loader.
async fn run_flight(
    loader: Weak<SheetLoader>,
    inner: Arc<PageInner>,
    element: SheetElement,
    env: Env,
    document_url: Url,
    style: Rc<RefCell<StyleState>>,
) {
    let outcome = catch_unwind_future(async {
        let config = &inner.config;
        let ctx = StyleContext {
            client: &config.client,
            fs_policy: &config.fs_policy,
            budgets: &config.budgets,
            clock: &*config.clock,
            extra_headers: &config.extra_headers,
            net: Some(&config.net),
            document_url: &document_url,
            deadline: config.clock.now() + SHEET_WAIT,
            fetch_policy: FetchPolicy::for_tier(Tier::Render),
            cors: matches!(element.source, SheetSource::Link(_, true)),
        };
        match &element.source {
            SheetSource::Link(url, _) => fetch_root(&style, &ctx, &env, url).await,
            SheetSource::Style(_) => {
                let base = document_url.clone();
                load_imports(
                    &style,
                    &ctx,
                    &env,
                    &base,
                    element.imports.clone(),
                    env.fallback(),
                )
                .await;
            }
        }
        let failures = style.borrow_mut().take_failures();
        for failure in failures {
            inner.emit(PageEvent::StylesheetFailed {
                url: failure.url,
                reason: failure.reason,
            });
        }
    })
    .await;
    if let Err(payload) = outcome {
        let reason = panic_message(&payload);
        tracing::error!(page = ?inner.config.id, reason, "the stylesheet loader panicked");
        inner.mark_crashed(&reason);
    }
    if let Some(loader) = loader.upgrade() {
        loader.finish(&element.source);
    }
}

/// Starts the loads of the stylesheet elements the parser has put in `document`
/// so far (`parser_inserted`), or of the ones a script just inserted.
///
/// The parse calls this with the tree it is building, before and after each
/// script it pauses on. A page whose ceiling is below the render tier loads no
/// stylesheets this way.
pub(super) fn sweep_parsed(
    inner: &Arc<PageInner>,
    tracker: &Rc<RefCell<SettleTracker>>,
    document: &SharedDocument,
    navigation: u64,
    parser_inserted: bool,
    encoding: Option<&'static tenta_html::Encoding>,
) {
    if inner.config.max_tier < Tier::Render {
        return;
    }
    let Ok(doc) = document.try_borrow() else {
        return;
    };
    // A page with no stylesheet to fetch creates no render session for this.
    let viewport = *inner.viewport.lock();
    let sizes = (viewport.width as f32, viewport.height as f32);
    if !render::loader_exists(inner.config.id, navigation) && !has_sheet_elements(&doc, sizes) {
        return;
    }
    let handles = render::handles(
        inner.config.id,
        navigation,
        viewport,
        inner.config.system_fonts,
    );
    handles.sheet_loader.sweep(
        inner,
        SheetSweep {
            doc: &doc,
            tracker,
            parser_inserted,
            encoding,
            viewport: sizes,
        },
    );
}

/// Whether `doc` has a stylesheet element the loader would fetch for.
pub(super) fn has_sheet_elements(doc: &DomDocument, viewport: (f32, f32)) -> bool {
    let about_blank = || Url::parse("about:blank").expect("a constant URL");
    let base = doc
        .base_url()
        .or(doc.url())
        .cloned()
        .unwrap_or_else(about_blank);
    !sheet_elements(doc, &base, viewport).is_empty()
}

/// Waits until no parser-inserted stylesheet that blocks scripts is loading,
/// the navigation is replaced, or its `deadline` passes, then reports the
/// `load` and `error` events of the sheets that ended to `document`'s engine,
/// so a blocked script runs after the events of the sheets it waited for.
///
/// A script that gives up waiting at the deadline runs anyway: the navigation
/// is as good as over and the body loop turns that into its outcome.
pub(super) async fn wait_for_blocking_sheets(
    inner: &Arc<PageInner>,
    tracker: &Rc<RefCell<SettleTracker>>,
    document: &SharedDocument,
    navigation: u64,
    deadline: std::time::Instant,
) {
    let Some(loader) = render::sheet_loader_of(inner.config.id, navigation) else {
        return;
    };
    if loader.blocking_pending() {
        wait_blockers(inner, &loader, navigation, deadline).await;
    }
    if let Some(images) = render::loader_of(inner.config.id) {
        super::image_loader::deliver_events(inner.config.id, tracker, document, &images, &loader);
    }
}

async fn wait_blockers(
    inner: &Arc<PageInner>,
    loader: &SheetLoader,
    navigation: u64,
    deadline: std::time::Instant,
) {
    // Registered before the check, so a bump between the two is not lost.
    let replaced = inner.generation_changed.notified();
    tokio::pin!(replaced);
    replaced.as_mut().enable();
    if inner.state.lock().navigation_generation != navigation {
        return;
    }
    let at = tokio::time::Instant::from_std(deadline);
    tokio::select! {
        biased;
        () = &mut replaced => {}
        () = loader.wait_unblocked() => {}
        () = tokio::time::sleep_until(at) => {
            tracing::warn!(
                page = ?inner.config.id,
                "a stylesheet that blocks scripts did not load before the navigation deadline; \
                 the script runs without it"
            );
        }
    }
}
