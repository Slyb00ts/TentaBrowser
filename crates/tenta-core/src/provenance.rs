//! Task and request provenance (`docs/spec/02-architecture.md` §5.1).
//!
//! The settle machine is defined over *where work came from*, not over whether
//! the page is busy, which is what lets it terminate on pages that hold
//! sockets open and run animation loops forever. The machine itself lives in
//! `tenta-page`, but the tags it is defined over have to be readable from
//! below it in the dependency order (`02` §2): `tenta-js` tags every task it
//! queues, and `tenta-net` classifies every request it issues, and neither may
//! depend on `tenta-page`.

/// Where a task, microtask or request came from.
///
/// `Tracked` is the navigation, the parser, parser-driven script execution
/// and completions of foreground requests issued from a tracked task.
/// `Background` is everything a live page does on its own: animation frames,
/// idle callbacks, intervals, socket and worker messages, media events.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Origin {
    Tracked,
    Background,
}

impl Origin {
    /// Provenance is inherited by the task a task queues.
    pub const fn inherit(self) -> Origin {
        self
    }
}

/// What a request is for. The class decides whether the request can hold a
/// settle open (foreground) or only counts for CDP's `networkIdle`
/// (background).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum RequestClass {
    Document,
    Script,
    Style,
    Font,
    XhrFetch,
    /// An image the current tier's `FetchPolicy` decided to issue.
    Image,
    MediaManifest,
    Beacon,
    Prefetch,
    MediaSegment,
    /// The HTTP upgrade of a WebSocket. The open connection afterwards is not
    /// a request at all.
    WebSocket,
    EventSource,
}

impl RequestClass {
    /// Foreground classes can hold a settle open; background classes cannot.
    pub const fn is_foreground(self) -> bool {
        matches!(
            self,
            RequestClass::Document
                | RequestClass::Script
                | RequestClass::Style
                | RequestClass::Font
                | RequestClass::XhrFetch
                | RequestClass::Image
                | RequestClass::MediaManifest
        )
    }

    /// Chromium's `networkIdle` counts every in-flight request regardless of
    /// origin, except upgraded WebSocket connections and EventSource streams.
    pub const fn counts_for_network_idle(self) -> bool {
        !matches!(self, RequestClass::WebSocket | RequestClass::EventSource)
    }
}

/// When a timer is a *deferral* of tracked work rather than page activity
/// (`docs/spec/02-architecture.md` §5.1).
///
/// The rule belongs to settle, but it is applied by the event loop in
/// `tenta-js` at the moment a timer is scheduled, so it lives down here with
/// the tags it produces. `tenta-page` builds one from its `SettleConfig`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimerRule {
    /// The longest one-shot delay that still counts as a deferral.
    pub max_delay: core::time::Duration,
    /// How deep a chain of timers may go before it is activity, not deferral.
    pub max_depth: u32,
}

impl Default for TimerRule {
    fn default() -> Self {
        Self {
            max_delay: core::time::Duration::from_millis(32),
            max_depth: 8,
        }
    }
}

impl TimerRule {
    /// The provenance a timer scheduled from a `parent` task inherits.
    ///
    /// A timer is tracked only if its parent was tracked and it is one-shot,
    /// short and not too deep in a chain of timers. Everything else - an
    /// interval, a long delay, a deep chain - is what a live page does on its
    /// own, and must never hold a settle open.
    pub fn origin_for(
        self,
        parent: Origin,
        repeating: bool,
        delay: core::time::Duration,
        depth: u32,
    ) -> Origin {
        if parent == Origin::Tracked
            && !repeating
            && delay <= self.max_delay
            && depth <= self.max_depth
        {
            Origin::Tracked
        } else {
            Origin::Background
        }
    }
}
