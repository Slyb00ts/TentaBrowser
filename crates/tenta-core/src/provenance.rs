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
