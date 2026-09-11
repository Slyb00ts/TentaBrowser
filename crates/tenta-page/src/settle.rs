//! The settle machine (`docs/spec/02-architecture.md` §5.1, normative).
//!
//! Settle answers "has the work that loading this page (or the last action)
//! set in motion finished". It is defined over **task provenance**, not over
//! activity, so it terminates on pages that hold sockets open, run animation
//! loops and talk to workers forever.
//!
//! The machine is a pure state machine: it never sleeps and never polls. The
//! page feeds it events ([`SettleTracker::note_request_started`],
//! [`SettleTracker::note_load_fired`], …) and asks
//! [`SettleTracker::evaluate`] for the state whenever a source changed or a
//! timer the machine named through [`SettleTracker::next_wakeup`] expired.
//! Time comes from an injected [`Clock`], so tests drive it by hand.
//!
//! At Tier 1 only the navigation, the document request and the parser feed
//! it; the other sources exist because Tier 2 (P2) drives the same machine.

use std::sync::Arc;
use std::time::{Duration, Instant};

use hashbrown::HashMap;
use tenta_core::RequestId;
use tokio::sync::Notify;

use crate::clock::Clock;

// `Origin` and `RequestClass` are defined in `tenta-core` so that the crates
// below this one - `tenta-js` tagging the tasks it queues, `tenta-net`
// classifying the requests it issues - can name them without depending on
// `tenta-page`. They are re-exported here because the settle machine is what
// gives them meaning.
pub use tenta_core::provenance::{Origin, RequestClass, TimerRule};

/// The tunables of the machine. The library and MCP differ only in patience.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettleConfig {
    /// How long `Quiet` tolerates tracked work before reporting `busy`.
    pub busy_cap: Duration,
    /// After this long without a complete body, a foreground request other
    /// than the document and parser-inserted subresources becomes background
    /// for the current settle call.
    pub foreground_request_max: Duration,
    /// Longest one-shot timer delay that still inherits `Tracked`.
    pub tracked_timer_max: Duration,
    /// Deepest chain of one-shot timers that still inherits `Tracked`.
    pub tracked_timer_depth: u32,
    /// How often `Quiet` may fall back to `Draining` before the machine gives
    /// up and reports `busy`.
    pub max_reentries: u32,
    /// The window Chromium's `networkIdle` and `networkAlmostIdle` use.
    pub network_idle_window: Duration,
    /// In-flight requests `networkAlmostIdle` tolerates.
    pub network_almost_idle_max: usize,
}

impl Default for SettleConfig {
    fn default() -> Self {
        Self::library()
    }
}

impl SettleConfig {
    /// The timer tunables on their own, in the form the event loop applies
    /// them at schedule time. `tenta-js` cannot see `SettleConfig`, so the page
    /// hands it this instead.
    pub const fn timer_rule(&self) -> TimerRule {
        TimerRule {
            max_delay: self.tracked_timer_max,
            max_depth: self.tracked_timer_depth,
        }
    }

    /// The library defaults (`docs/spec/02-architecture.md` §5.1).
    pub const fn library() -> Self {
        Self {
            busy_cap: Duration::from_secs(1),
            foreground_request_max: Duration::from_secs(5),
            tracked_timer_max: Duration::from_millis(32),
            tracked_timer_depth: 8,
            max_reentries: 16,
            network_idle_window: Duration::from_millis(500),
            network_almost_idle_max: 2,
        }
    }

    /// MCP is less patient than the library.
    pub const fn mcp() -> Self {
        Self {
            busy_cap: Duration::from_millis(500),
            foreground_request_max: Duration::from_secs(2),
            ..Self::library()
        }
    }
}

/// What a caller waits for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Settle {
    /// The `load` event fired.
    #[default]
    Load,
    /// The `DOMContentLoaded` event fired.
    DomContent,
    /// The state machine of `docs/spec/02-architecture.md` §5.1.
    NetworkIdle,
    /// `Load` and a selector matches.
    Selector(String),
    /// `Load` and the text appears in the document.
    Text(String),
    /// `NetworkIdle` and no DOM mutation from any origin for `quiet`. An
    /// explicit opt-in: it ends in `Timeout` on a page that mutates forever.
    VisuallyStable { quiet: Duration },
}

impl Settle {
    /// `VisuallyStable` with the default 50 ms quiet window.
    pub const fn visually_stable() -> Self {
        Settle::VisuallyStable {
            quiet: Duration::from_millis(50),
        }
    }

    /// Whether the page must evaluate a document condition for this variant.
    pub const fn needs_condition(&self) -> bool {
        matches!(self, Settle::Selector(_) | Settle::Text(_))
    }
}

/// What was still outstanding when a settle timed out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PendingSources {
    pub parser_running: bool,
    pub pending_scripts: u32,
    /// In-flight foreground requests that had not been reclassified.
    pub foreground_requests: u32,
    pub navigation_pending: bool,
    pub service_worker_fetches: u32,
    pub tracked_tasks: u32,
    /// Whether the `Selector`/`Text` condition had been met.
    pub condition_met: bool,
}

impl PendingSources {
    /// Whether anything at all was outstanding.
    pub fn is_empty(&self) -> bool {
        !self.parser_running
            && self.pending_scripts == 0
            && self.foreground_requests == 0
            && !self.navigation_pending
            && self.service_worker_fetches == 0
            && self.tracked_tasks == 0
    }
}

/// The state of one settle call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettleState {
    /// Waiting for the event the `wait_until` variant names.
    Loading,
    /// `load` has fired and network sources are still open.
    Draining,
    /// Nothing is on the network; tracked tasks are still running.
    Quiet { since: Instant },
    /// Done. `busy` means the load chain never ended by itself.
    Settled { busy: bool },
    /// The deadline expired.
    Timeout { pending: PendingSources },
}

impl SettleState {
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            SettleState::Settled { .. } | SettleState::Timeout { .. }
        )
    }

    pub fn is_settled(&self) -> bool {
        matches!(self, SettleState::Settled { .. })
    }
}

/// The provenance a timer scheduled from `parent` inherits
/// (`docs/spec/02-architecture.md` §5.1).
///
/// A timer is a deferral (tracked) only if it is one-shot, short and not too
/// deep in a chain of timers; otherwise it is activity (background). Used by
/// the Tier 2 event loop; exposed here because the rule belongs to settle.
pub fn timer_origin(
    parent: Origin,
    repeating: bool,
    delay: Duration,
    depth: u32,
    config: &SettleConfig,
) -> Origin {
    config
        .timer_rule()
        .origin_for(parent, repeating, delay, depth)
}

#[derive(Clone, Copy, Debug)]
struct RequestRecord {
    class: RequestClass,
    origin: Origin,
    started: Instant,
    /// The parser created this request, so it is never reclassified.
    parser_inserted: bool,
    reclassified: bool,
}

impl RequestRecord {
    fn holds_settle(&self) -> bool {
        self.class.is_foreground() && self.origin == Origin::Tracked && !self.reclassified
    }

    fn exempt_from_reclassification(&self) -> bool {
        self.parser_inserted || self.class == RequestClass::Document
    }
}

/// The settle machine of one page.
pub struct SettleTracker {
    config: SettleConfig,
    wait_until: Settle,
    clock: Arc<dyn Clock>,
    /// Signals whenever a state-changing call is made.
    notify: Arc<Notify>,

    started: Instant,
    deadline: Option<Instant>,
    latched: Option<SettleState>,

    load_fired: bool,
    domcontentloaded_fired: bool,
    condition_met: bool,

    parser_running: bool,
    pending_scripts: u32,
    navigation_pending: bool,
    service_worker_fetches: u32,
    tracked_tasks: u32,
    background_tasks: u32,

    requests: HashMap<RequestId, RequestRecord>,
    reclassified_any: bool,

    quiet_since: Option<Instant>,
    reentries: u32,
    last_mutation: Option<Instant>,

    /// In-flight requests that count for CDP's idle signals.
    idle_counted: usize,
    zero_since: Option<Instant>,
    almost_since: Option<Instant>,
}

impl std::fmt::Debug for SettleTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettleTracker")
            .field("wait_until", &self.wait_until)
            .field("load_fired", &self.load_fired)
            .field("parser_running", &self.parser_running)
            .field("in_flight", &self.requests.len())
            .field("tracked_tasks", &self.tracked_tasks)
            .field("reentries", &self.reentries)
            .finish_non_exhaustive()
    }
}

impl SettleTracker {
    pub fn new(config: SettleConfig, wait_until: Settle, clock: Arc<dyn Clock>) -> Self {
        let started = clock.now();
        Self {
            config,
            wait_until,
            clock,
            notify: Arc::new(Notify::new()),
            started,
            deadline: None,
            latched: None,
            load_fired: false,
            domcontentloaded_fired: false,
            condition_met: false,
            parser_running: false,
            pending_scripts: 0,
            navigation_pending: false,
            service_worker_fetches: 0,
            tracked_tasks: 0,
            background_tasks: 0,
            requests: HashMap::new(),
            reclassified_any: false,
            quiet_since: None,
            reentries: 0,
            last_mutation: None,
            idle_counted: 0,
            zero_since: Some(started),
            almost_since: Some(started),
        }
    }

    /// Arms the deadline of this settle call.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.deadline = Some(self.started + timeout);
        self
    }

    pub fn set_deadline(&mut self, at: Instant) {
        self.deadline = Some(at);
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub fn config(&self) -> &SettleConfig {
        &self.config
    }

    pub fn wait_until(&self) -> &Settle {
        &self.wait_until
    }

    pub fn now(&self) -> Instant {
        self.clock.now()
    }

    /// Gets a cloned handle to the state-change notification.
    pub fn notify_handle(&self) -> Arc<Notify> {
        Arc::clone(&self.notify)
    }

    pub fn load_fired(&self) -> bool {
        self.load_fired
    }

    pub fn domcontentloaded_fired(&self) -> bool {
        self.domcontentloaded_fired
    }

    // ----------------------------------------------------------- the sources

    /// A request left `send()`. `origin` is the provenance of the task that
    /// issued it.
    pub fn note_request_started(&mut self, id: RequestId, class: RequestClass, origin: Origin) {
        self.insert_request(id, class, origin, false);
        self.signal_state_change();
    }

    /// A request the parser itself created (the document's scripts and
    /// stylesheets). It is tracked and never reclassified.
    pub fn note_parser_request_started(&mut self, id: RequestId, class: RequestClass) {
        self.insert_request(id, class, Origin::Tracked, true);
        self.signal_state_change();
    }

    fn insert_request(
        &mut self,
        id: RequestId,
        class: RequestClass,
        origin: Origin,
        parser_inserted: bool,
    ) {
        // Make insertion idempotent: if the id is already present, replace it
        // (but never double-count network_idle).
        let now = self.clock.now();
        let record = RequestRecord {
            class,
            origin,
            started: now,
            parser_inserted,
            reclassified: false,
        };

        // If the id already exists, this is a duplicate: tolerate it in both debug and
        // release builds and skip the idle counting increment to stay idempotent (N-R2-3).
        if let Some(_old_record) = self.requests.get(&id) {
            // Log at debug level for troubleshooting, but don't panic
            tracing::debug!(
                id = ?id,
                class = ?class,
                "duplicate RequestId; replacing old record without recounting idle"
            );
            // Replace the old record but don't touch idle_counted
            self.requests.insert(id, record);
            return;
        }

        if class.counts_for_network_idle() {
            self.idle_counted += 1;
            self.update_idle_windows(now);
        }
        if record.holds_settle() && self.quiet_since.is_some() {
            // A tracked task issued a foreground request while the machine was
            // quiet: the load chain continued. Bounded re-entry keeps a page
            // that chains requests forever from waiting out the deadline.
            if self.reentries >= self.config.max_reentries {
                self.latched = Some(SettleState::Settled { busy: true });
            } else {
                self.reentries += 1;
                self.quiet_since = None;
            }
        }
        self.requests.insert(id, record);
    }

    /// The body was fully delivered, the request failed, or it was aborted.
    pub fn note_request_done(&mut self, id: RequestId) {
        let now = self.clock.now();
        if let Some(record) = self.requests.remove(&id)
            && record.class.counts_for_network_idle()
        {
            self.idle_counted = self.idle_counted.saturating_sub(1);
            self.update_idle_windows(now);
        }
        self.signal_state_change();
    }

    /// The navigation's parser, or a `document.write()` from a tracked task.
    pub fn note_parser_running(&mut self, running: bool) {
        self.parser_running = running;
        if running {
            self.quiet_since = None;
        }
        self.signal_state_change();
    }

    /// Scripts awaiting fetch or execution whose insertion was tracked.
    pub fn note_pending_scripts(&mut self, count: u32) {
        self.pending_scripts = count;
        if count > 0 {
            self.quiet_since = None;
        }
        self.signal_state_change();
    }

    pub fn note_load_fired(&mut self) {
        self.load_fired = true;
        self.signal_state_change();
    }

    pub fn note_domcontentloaded(&mut self) {
        self.domcontentloaded_fired = true;
        self.signal_state_change();
    }

    /// A task or microtask was queued.
    pub fn note_task_queued(&mut self, origin: Origin) {
        match origin {
            Origin::Tracked => self.tracked_tasks += 1,
            Origin::Background => self.background_tasks += 1,
        }
        self.signal_state_change();
    }

    /// A task or microtask finished.
    pub fn note_task_finished(&mut self, origin: Origin) {
        match origin {
            Origin::Tracked => self.tracked_tasks = self.tracked_tasks.saturating_sub(1),
            Origin::Background => self.background_tasks = self.background_tasks.saturating_sub(1),
        }
        self.signal_state_change();
    }

    /// A navigation was started but has not committed yet.
    pub fn note_navigation_started(&mut self) {
        self.navigation_pending = true;
        self.quiet_since = None;
        self.signal_state_change();
    }

    /// A navigation committed: `load`, `DOMContentLoaded` and the re-entry
    /// count start again for the new document.
    pub fn note_navigation_commit(&mut self) {
        self.navigation_pending = false;
        self.load_fired = false;
        self.domcontentloaded_fired = false;
        self.condition_met = false;
        self.reentries = 0;
        self.quiet_since = None;
        self.latched = None;
        self.reclassified_any = false;
        self.last_mutation = None;
        // Clear the request table so previous navigation's requests don't bleed
        // into the new navigation's idle tracking (M-4).
        self.requests.clear();
        self.idle_counted = 0;
        // Reset the idle windows so they start fresh for this navigation (M-R2-6).
        self.zero_since = None;
        self.almost_since = None;
        self.signal_state_change();
    }

    /// A Service Worker `fetch` handler is deciding a request.
    pub fn note_service_worker_fetches(&mut self, count: u32) {
        self.service_worker_fetches = count;
        if count > 0 {
            self.quiet_since = None;
        }
        self.signal_state_change();
    }

    /// The DOM changed. Only `VisuallyStable` cares, and it cares about every
    /// origin.
    pub fn note_dom_mutation(&mut self) {
        self.last_mutation = Some(self.clock.now());
        self.signal_state_change();
    }

    /// The result of the `Selector`/`Text` predicate the page evaluated.
    pub fn note_condition_met(&mut self, met: bool) {
        self.condition_met = met;
        self.signal_state_change();
    }

    // ------------------------------------------------------------- the state

    /// The state at `now`. Once the machine reports `Settled` or `Timeout` it
    /// keeps reporting it: one settle call ends once.
    pub fn evaluate(&mut self, now: Instant) -> SettleState {
        if let Some(state) = self.latched {
            return state;
        }
        self.reclassify(now);
        let state = self.machine(now);
        if state.is_settled() {
            self.latched = Some(state);
            return state;
        }
        if let Some(deadline) = self.deadline
            && now >= deadline
        {
            let state = SettleState::Timeout {
                pending: self.pending(),
            };
            self.latched = Some(state);
            return state;
        }
        state
    }

    /// The state at the tracker's own clock.
    pub fn evaluate_now(&mut self) -> SettleState {
        let now = self.clock.now();
        self.evaluate(now)
    }

    /// The earliest instant at which `evaluate` could report something new,
    /// so the page can sleep instead of poll. `None` means only an event can
    /// change the state.
    pub fn next_wakeup(&self, now: Instant) -> Option<Instant> {
        if self.latched.is_some() {
            return None;
        }
        let mut wakeup = self.deadline;
        let mut consider = |instant: Instant| {
            wakeup = Some(match wakeup {
                Some(current) if current <= instant => current,
                _ => instant,
            });
        };
        if let Some(quiet_since) = self.quiet_since
            && self.tracked_tasks > 0
        {
            consider(quiet_since + self.config.busy_cap);
        }
        for record in self.requests.values() {
            if record.holds_settle() && !record.exempt_from_reclassification() {
                consider(record.started + self.config.foreground_request_max);
            }
        }
        if let Settle::VisuallyStable { quiet } = &self.wait_until {
            consider(self.last_mutation.unwrap_or(self.started) + *quiet);
        }
        // Only schedule network_idle_window wakeup for waits that use it (NetworkIdle)
        if matches!(&self.wait_until, Settle::NetworkIdle) {
            let idle_window = self.config.network_idle_window;
            if let Some(since) = self.zero_since
                && since + idle_window > now
            {
                consider(since + idle_window);
            }
        }
        wakeup
    }

    /// What is still outstanding right now.
    pub fn pending(&self) -> PendingSources {
        PendingSources {
            parser_running: self.parser_running,
            pending_scripts: self.pending_scripts,
            foreground_requests: self
                .requests
                .values()
                .filter(|record| record.holds_settle())
                .count() as u32,
            navigation_pending: self.navigation_pending,
            service_worker_fetches: self.service_worker_fetches,
            tracked_tasks: self.tracked_tasks,
            condition_met: self.condition_met,
        }
    }

    /// Chromium's `networkIdle`: nothing in flight for the idle window.
    pub fn network_idle(&self, now: Instant) -> bool {
        self.zero_since.is_some_and(|since| {
            now.saturating_duration_since(since) >= self.config.network_idle_window
        })
    }

    /// Chromium's `networkAlmostIdle`: at most two in flight for the window.
    pub fn network_almost_idle(&self, now: Instant) -> bool {
        self.almost_since.is_some_and(|since| {
            now.saturating_duration_since(since) >= self.config.network_idle_window
        })
    }

    /// In-flight requests that count for the CDP idle signals.
    pub fn in_flight_for_idle(&self) -> usize {
        self.idle_counted
    }

    /// How many times `Quiet` fell back to `Draining` in this settle call.
    pub fn reentries(&self) -> u32 {
        self.reentries
    }

    // ------------------------------------------------------------- internals

    fn update_idle_windows(&mut self, now: Instant) {
        if self.idle_counted == 0 {
            self.zero_since.get_or_insert(now);
        } else {
            self.zero_since = None;
        }
        if self.idle_counted <= self.config.network_almost_idle_max {
            self.almost_since.get_or_insert(now);
        } else {
            self.almost_since = None;
        }
    }

    /// A foreground request whose body is still streaming after
    /// `foreground_request_max` becomes background for this settle call.
    fn reclassify(&mut self, now: Instant) {
        let limit = self.config.foreground_request_max;
        for record in self.requests.values_mut() {
            if record.reclassified
                || !record.holds_settle()
                || record.exempt_from_reclassification()
            {
                continue;
            }
            if now.saturating_duration_since(record.started) >= limit {
                record.reclassified = true;
                self.reclassified_any = true;
            }
        }
    }

    fn network_sources_empty(&self) -> bool {
        !self.parser_running
            && self.pending_scripts == 0
            && !self.navigation_pending
            && self.service_worker_fetches == 0
            && !self.requests.values().any(RequestRecord::holds_settle)
    }

    /// `busy` means the load chain never ended by itself: either a tracked
    /// chain ran past `busy_cap`, or a foreground request was reclassified
    /// because it never delivered its body.
    fn busy_from_reclassification(&self) -> bool {
        self.reclassified_any
    }

    fn machine(&mut self, now: Instant) -> SettleState {
        match self.wait_until.clone() {
            Settle::Load => self.event_state(self.load_fired),
            Settle::DomContent => self.event_state(self.domcontentloaded_fired),
            Settle::Selector(_) | Settle::Text(_) => {
                self.event_state(self.load_fired && self.condition_met)
            }
            Settle::NetworkIdle => self.network_idle_machine(now),
            Settle::VisuallyStable { quiet } => {
                let state = self.network_idle_machine(now);
                match state {
                    SettleState::Settled { .. } if !self.stable_for(quiet, now) => {
                        SettleState::Quiet {
                            since: self.quiet_since.unwrap_or(now),
                        }
                    }
                    other => other,
                }
            }
        }
    }

    fn event_state(&self, reached: bool) -> SettleState {
        if reached {
            SettleState::Settled {
                busy: self.busy_from_reclassification(),
            }
        } else {
            SettleState::Loading
        }
    }

    fn network_idle_machine(&mut self, now: Instant) -> SettleState {
        if !self.load_fired {
            return SettleState::Loading;
        }
        if !self.network_sources_empty() {
            self.quiet_since = None;
            return SettleState::Draining;
        }
        let since = *self.quiet_since.get_or_insert(now);
        if self.tracked_tasks == 0 {
            return SettleState::Settled {
                busy: self.busy_from_reclassification(),
            };
        }
        if now.saturating_duration_since(since) >= self.config.busy_cap {
            return SettleState::Settled { busy: true };
        }
        SettleState::Quiet { since }
    }

    fn stable_for(&self, quiet: Duration, now: Instant) -> bool {
        let last = self.last_mutation.unwrap_or(self.started);
        now.saturating_duration_since(last) >= quiet
    }

    /// Signal that a state change has occurred.
    fn signal_state_change(&self) {
        self.notify.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;

    fn tracker(wait_until: Settle) -> (SettleTracker, ManualClock) {
        let clock = ManualClock::new();
        let tracker = SettleTracker::new(
            SettleConfig::library(),
            wait_until,
            Arc::new(clock.clone()) as Arc<dyn Clock>,
        );
        (tracker, clock)
    }

    /// The `S-static-text` settle test of `docs/spec/15-testing-and-benchmarks.md`
    /// §1: a static page settles the instant `load` fires, with `busy: false`.
    #[test]
    fn a_static_page_settles_the_instant_load_fires() {
        let (mut tracker, clock) = tracker(Settle::NetworkIdle);
        tracker.note_navigation_started();
        assert_eq!(tracker.evaluate_now(), SettleState::Loading);

        tracker.note_navigation_commit();
        tracker.note_request_started(RequestId(1), RequestClass::Document, Origin::Tracked);
        tracker.note_parser_running(true);
        assert_eq!(tracker.evaluate_now(), SettleState::Loading);

        tracker.note_request_done(RequestId(1));
        tracker.note_parser_running(false);
        tracker.note_domcontentloaded();
        assert_eq!(
            tracker.evaluate_now(),
            SettleState::Loading,
            "DOMContentLoaded is not load"
        );

        tracker.note_load_fired();
        // No clock advance at all: the machine settles in the same instant.
        let settled_at = clock.now();
        assert_eq!(
            tracker.evaluate(settled_at),
            SettleState::Settled { busy: false }
        );
        assert!(tracker.pending().is_empty());
    }

    #[test]
    fn a_settle_started_after_load_begins_in_draining() {
        let (mut tracker, _clock) = tracker(Settle::NetworkIdle);
        tracker.note_load_fired();
        tracker.note_request_started(RequestId(1), RequestClass::XhrFetch, Origin::Tracked);
        assert_eq!(tracker.evaluate_now(), SettleState::Draining);
    }

    #[test]
    fn background_sources_never_hold_a_settle_open() {
        let (mut tracker, _clock) = tracker(Settle::NetworkIdle);
        tracker.note_load_fired();
        // A WebSocket, an EventSource and a beacon from background tasks.
        tracker.note_request_started(RequestId(1), RequestClass::WebSocket, Origin::Background);
        tracker.note_request_started(RequestId(2), RequestClass::EventSource, Origin::Background);
        tracker.note_request_started(RequestId(3), RequestClass::Beacon, Origin::Background);
        // An rAF loop and a setInterval handler.
        tracker.note_task_queued(Origin::Background);
        assert_eq!(
            tracker.evaluate_now(),
            SettleState::Settled { busy: false },
            "a page with sockets and an animation loop still settles"
        );
    }

    /// A foreground request from a tracked task that never delivers its body
    /// becomes background after `foreground_request_max`, and the settle ends
    /// `busy` because the load chain never finished on its own.
    #[test]
    fn a_hanging_foreground_request_is_reclassified_and_settles_busy() {
        let (mut tracker, clock) = tracker(Settle::NetworkIdle);
        tracker.note_load_fired();
        tracker.note_request_started(RequestId(7), RequestClass::XhrFetch, Origin::Tracked);
        assert_eq!(tracker.evaluate_now(), SettleState::Draining);

        clock.advance(Duration::from_secs(4));
        assert_eq!(
            tracker.evaluate_now(),
            SettleState::Draining,
            "still inside foreground_request_max"
        );
        let wakeup = tracker.next_wakeup(clock.now()).expect("a wakeup is named");

        clock.advance(Duration::from_secs(1));
        assert!(clock.now() >= wakeup);
        assert_eq!(tracker.evaluate_now(), SettleState::Settled { busy: true });
        assert_eq!(tracker.in_flight_for_idle(), 1, "CDP still counts it");
    }

    #[test]
    fn the_document_request_is_never_reclassified() {
        let (mut tracker, clock) = tracker(Settle::NetworkIdle);
        tracker.note_load_fired();
        tracker.note_request_started(RequestId(1), RequestClass::Document, Origin::Tracked);
        tracker.note_parser_request_started(RequestId(2), RequestClass::Script);
        clock.advance(Duration::from_secs(30));
        assert_eq!(
            tracker.evaluate_now(),
            SettleState::Draining,
            "the document and parser-inserted scripts hold the settle open"
        );
    }

    /// Sixteen re-entries are allowed; the seventeenth ends the settle busy.
    #[test]
    fn the_reentry_cap_ends_a_chain_that_never_stops() {
        let (mut tracker, clock) = tracker(Settle::NetworkIdle);
        tracker.note_load_fired();

        for round in 0..16u64 {
            // The completion of the previous request queued a tracked task,
            // so the machine is quiet but not settled.
            tracker.note_task_queued(Origin::Tracked);
            assert!(
                matches!(tracker.evaluate_now(), SettleState::Quiet { .. }),
                "round {round} must be quiet with a tracked task pending"
            );
            tracker.note_request_started(RequestId(round), RequestClass::XhrFetch, Origin::Tracked);
            tracker.note_task_finished(Origin::Tracked);
            assert_eq!(tracker.evaluate_now(), SettleState::Draining);
            tracker.note_request_done(RequestId(round));
            clock.advance(Duration::from_millis(1));
        }
        assert_eq!(tracker.reentries(), 16);

        tracker.note_task_queued(Origin::Tracked);
        assert!(matches!(tracker.evaluate_now(), SettleState::Quiet { .. }));
        tracker.note_request_started(RequestId(99), RequestClass::XhrFetch, Origin::Tracked);
        assert_eq!(
            tracker.evaluate_now(),
            SettleState::Settled { busy: true },
            "the seventeenth re-entry gives up"
        );
    }

    #[test]
    fn a_tracked_chain_that_loops_forever_settles_at_the_busy_cap() {
        let (mut tracker, clock) = tracker(Settle::NetworkIdle);
        tracker.note_load_fired();
        tracker.note_task_queued(Origin::Tracked);
        let quiet = tracker.evaluate_now();
        assert!(matches!(quiet, SettleState::Quiet { .. }));

        clock.advance(Duration::from_millis(999));
        assert!(matches!(tracker.evaluate_now(), SettleState::Quiet { .. }));
        clock.advance(Duration::from_millis(1));
        assert_eq!(tracker.evaluate_now(), SettleState::Settled { busy: true });
    }

    #[test]
    fn the_deadline_reports_what_was_still_pending() {
        let clock = ManualClock::new();
        let mut tracker = SettleTracker::new(
            SettleConfig::library(),
            Settle::NetworkIdle,
            Arc::new(clock.clone()) as Arc<dyn Clock>,
        )
        .with_timeout(Duration::from_secs(2));
        tracker.note_load_fired();
        tracker.note_parser_running(true);
        tracker.note_request_started(RequestId(1), RequestClass::Document, Origin::Tracked);
        tracker.note_task_queued(Origin::Tracked);

        clock.advance(Duration::from_secs(2));
        let state = tracker.evaluate_now();
        let SettleState::Timeout { pending } = state else {
            panic!("expected a timeout, got {state:?}");
        };
        assert!(pending.parser_running);
        assert_eq!(pending.foreground_requests, 1);
        assert_eq!(pending.tracked_tasks, 1);
        assert!(!pending.is_empty());
        assert_eq!(
            tracker.evaluate_now(),
            state,
            "the outcome of a settle call is latched"
        );
    }

    #[test]
    fn visually_stable_waits_out_a_spinner_that_stops() {
        let (mut tracker, clock) = tracker(Settle::visually_stable());
        tracker.note_load_fired();
        for _ in 0..5 {
            tracker.note_dom_mutation();
            clock.advance(Duration::from_millis(16));
            assert!(matches!(tracker.evaluate_now(), SettleState::Quiet { .. }));
        }
        clock.advance(Duration::from_millis(50));
        assert_eq!(tracker.evaluate_now(), SettleState::Settled { busy: false });
    }

    #[test]
    fn visually_stable_times_out_on_a_page_that_mutates_forever() {
        let clock = ManualClock::new();
        let mut tracker = SettleTracker::new(
            SettleConfig::library(),
            Settle::visually_stable(),
            Arc::new(clock.clone()) as Arc<dyn Clock>,
        )
        .with_timeout(Duration::from_secs(1));
        tracker.note_load_fired();
        for _ in 0..100 {
            tracker.note_dom_mutation();
            clock.advance(Duration::from_millis(16));
            let state = tracker.evaluate_now();
            if let SettleState::Timeout { pending } = state {
                assert!(
                    pending.is_empty(),
                    "nothing was pending, the page just moves"
                );
                return;
            }
            assert!(matches!(state, SettleState::Quiet { .. }));
        }
        panic!("a page that mutates every frame must end in Timeout");
    }

    #[test]
    fn wait_until_load_ignores_the_network() {
        let (mut tracker, _clock) = tracker(Settle::Load);
        tracker.note_request_started(RequestId(1), RequestClass::XhrFetch, Origin::Tracked);
        assert_eq!(tracker.evaluate_now(), SettleState::Loading);
        tracker.note_load_fired();
        assert_eq!(tracker.evaluate_now(), SettleState::Settled { busy: false });
    }

    #[test]
    fn wait_until_domcontent_settles_before_load() {
        let (mut tracker, _clock) = tracker(Settle::DomContent);
        tracker.note_domcontentloaded();
        assert_eq!(tracker.evaluate_now(), SettleState::Settled { busy: false });
    }

    #[test]
    fn selector_needs_both_load_and_the_condition() {
        let (mut tracker, _clock) = tracker(Settle::Selector("#app".to_owned()));
        assert!(tracker.wait_until().needs_condition());
        tracker.note_condition_met(true);
        assert_eq!(
            tracker.evaluate_now(),
            SettleState::Loading,
            "the condition alone is not enough"
        );
        tracker.note_load_fired();
        assert_eq!(tracker.evaluate_now(), SettleState::Settled { busy: false });
    }

    #[test]
    fn network_idle_and_almost_idle_follow_chromiums_definition() {
        let (mut tracker, clock) = tracker(Settle::NetworkIdle);
        for id in 0..3u64 {
            tracker.note_request_started(RequestId(id), RequestClass::Image, Origin::Background);
        }
        clock.advance(Duration::from_millis(600));
        assert!(!tracker.network_idle(clock.now()));
        assert!(!tracker.network_almost_idle(clock.now()));

        tracker.note_request_done(RequestId(0));
        clock.advance(Duration::from_millis(500));
        assert!(
            tracker.network_almost_idle(clock.now()),
            "two in flight for 500 ms is almost idle"
        );
        assert!(!tracker.network_idle(clock.now()));

        tracker.note_request_done(RequestId(1));
        tracker.note_request_done(RequestId(2));
        clock.advance(Duration::from_millis(499));
        assert!(!tracker.network_idle(clock.now()));
        clock.advance(Duration::from_millis(1));
        assert!(tracker.network_idle(clock.now()));

        // An open WebSocket does not stop networkIdle.
        tracker.note_request_started(RequestId(9), RequestClass::WebSocket, Origin::Background);
        clock.advance(Duration::from_millis(500));
        assert!(tracker.network_idle(clock.now()));
    }

    #[test]
    fn a_navigation_commit_resets_the_settle_call() {
        let (mut tracker, _clock) = tracker(Settle::NetworkIdle);
        tracker.note_load_fired();
        assert_eq!(tracker.evaluate_now(), SettleState::Settled { busy: false });
        tracker.note_navigation_commit();
        assert_eq!(
            tracker.evaluate_now(),
            SettleState::Loading,
            "the new document starts over"
        );
    }

    #[test]
    fn timer_provenance_follows_the_deferral_rule() {
        let config = SettleConfig::library();
        let short = Duration::from_millis(4);
        assert_eq!(
            timer_origin(Origin::Tracked, false, short, 0, &config),
            Origin::Tracked,
            "setTimeout(f, 4) from the load chain is a deferral"
        );
        assert_eq!(
            timer_origin(Origin::Tracked, true, short, 0, &config),
            Origin::Background,
            "setInterval is activity"
        );
        assert_eq!(
            timer_origin(
                Origin::Tracked,
                false,
                Duration::from_millis(100),
                0,
                &config
            ),
            Origin::Background,
            "a long timer is activity"
        );
        assert_eq!(
            timer_origin(Origin::Tracked, false, short, 9, &config),
            Origin::Background,
            "a chain deeper than tracked_timer_depth is activity"
        );
        assert_eq!(
            timer_origin(Origin::Background, false, short, 0, &config),
            Origin::Background,
            "provenance is inherited, never promoted"
        );
    }

    #[test]
    fn request_classes_split_foreground_from_background() {
        assert!(RequestClass::Document.is_foreground());
        assert!(RequestClass::XhrFetch.is_foreground());
        assert!(!RequestClass::Prefetch.is_foreground());
        assert!(!RequestClass::MediaSegment.is_foreground());
        assert!(RequestClass::Prefetch.counts_for_network_idle());
        assert!(!RequestClass::WebSocket.counts_for_network_idle());
        assert!(!RequestClass::EventSource.counts_for_network_idle());
        assert_eq!(Origin::Tracked.inherit(), Origin::Tracked);
    }

    #[test]
    fn mcp_is_less_patient_than_the_library() {
        assert!(SettleConfig::mcp().busy_cap < SettleConfig::library().busy_cap);
        assert!(
            SettleConfig::mcp().foreground_request_max
                < SettleConfig::library().foreground_request_max
        );
        assert_eq!(SettleConfig::default(), SettleConfig::library());
        assert_eq!(Settle::default(), Settle::Load);
    }

    /// The settle machine reports Settled immediately when the last tracked
    /// source finishes, with no polling delay (using Tokio's Notify to wake).
    #[test]
    fn settle_transitions_immediately_when_last_tracked_source_finishes() {
        // A page that waits for NetworkIdle (load fired + network empty)
        let (mut tracker, _clock) = tracker(Settle::NetworkIdle);
        tracker.note_load_fired();

        // One foreground request from a tracked task
        tracker.note_request_started(RequestId(1), RequestClass::XhrFetch, Origin::Tracked);
        assert_eq!(tracker.evaluate_now(), SettleState::Draining);

        // Request completes: network now empty, so machine settles
        tracker.note_request_done(RequestId(1));
        assert_eq!(
            tracker.evaluate_now(),
            SettleState::Settled { busy: false },
            "settled immediately when last foreground request finished, no polling"
        );
    }
}
