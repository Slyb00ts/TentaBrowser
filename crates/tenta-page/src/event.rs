//! The page event stream (`docs/spec/11-library-api.md` §1).
//!
//! Events are broadcast to every subscriber; a subscriber that falls behind
//! loses the oldest events rather than blocking the page thread.

use tenta_core::{FrameId, RequestId, Tier};
use tenta_js::inspect::{BindingCall, ConsoleCall, ExceptionEvent};
use tokio::sync::broadcast;
use url::Url;

/// A milestone of a committed document's load, in the order Chromium reports
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LoadStage {
    DomContentLoaded,
    Load,
    /// At most two requests in flight for the settle configuration's idle
    /// window, after `load`.
    NetworkAlmostIdle,
    /// Nothing in flight for the idle window, after `load`.
    NetworkIdle,
}

impl LoadStage {
    /// Every stage, in the order a document reaches them.
    pub const ALL: [LoadStage; 4] = [
        LoadStage::DomContentLoaded,
        LoadStage::Load,
        LoadStage::NetworkAlmostIdle,
        LoadStage::NetworkIdle,
    ];
}

/// What a navigation committed: the response head arrived, or an error
/// document replaced the page.
#[derive(Clone, Debug, PartialEq)]
pub struct Commit {
    /// Identifies the navigation, and with it the document it loads.
    pub navigation_id: u64,
    /// The committed document's URL, after redirects.
    pub url: Url,
    /// The MIME type of the response, lower-cased and without parameters.
    pub mime_type: String,
    /// The HTTP status; 0 for an error document.
    pub status: u16,
    /// The `net::ERR_*` name when the navigation failed and an error document
    /// was committed instead.
    pub error: Option<String>,
    /// The URL the failed navigation was for, when `error` is set.
    pub unreachable_url: Option<Url>,
}

/// Where the page's main frame is in loading its current document.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameState {
    /// The navigation of the document the frame shows; 0 for the initial
    /// `about:blank`.
    pub navigation_id: u64,
    /// `None` for the initial `about:blank` document, which no navigation
    /// committed.
    pub commit: Option<Commit>,
    /// The stages that document has reached, in order.
    pub stages: Vec<LoadStage>,
}

impl FrameState {
    /// The initial document, which is complete from the start.
    pub(crate) fn initial() -> Self {
        Self {
            navigation_id: 0,
            commit: None,
            stages: LoadStage::ALL.to_vec(),
        }
    }
}

/// How many events a slow subscriber may fall behind before it loses the
/// oldest ones.
pub const EVENT_BUFFER: usize = 512;

/// What a page reports while it works.
///
/// Not `Eq`: a console argument or an exception value can be a `NaN`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum PageEvent {
    /// A request left the page.
    Request {
        id: RequestId,
        url: Url,
        method: String,
        resource_type: &'static str,
    },
    /// A response head arrived.
    Response {
        id: RequestId,
        url: Url,
        status: u16,
        from_cache: bool,
    },
    /// A request failed with a `net::ERR_*` name.
    RequestFailed {
        id: RequestId,
        url: Url,
        error: String,
    },
    /// A navigation began, before any request left the page.
    NavigationStarted {
        frame: FrameId,
        navigation_id: u64,
        url: Url,
    },
    /// A navigation's response head, or an error document, replaced the
    /// frame's document. The new document is not installed yet: that is
    /// [`PageEvent::FrameNavigated`].
    NavigationCommitted { frame: FrameId, commit: Commit },
    /// The document of `navigation_id` reached `stage`.
    Lifecycle {
        frame: FrameId,
        navigation_id: u64,
        stage: LoadStage,
    },
    /// The navigation ended, or was stopped, without its document reaching
    /// `load`. Not sent for a navigation a newer one replaced.
    NavigationStopped { frame: FrameId, navigation_id: u64 },
    /// A frame committed a navigation.
    FrameNavigated { frame: FrameId, url: Url },
    /// The page's task panicked, its watchdog fired, or a hard budget was
    /// exceeded (`docs/spec/02-architecture.md` §8).
    Crashed { reason: String },
    /// The page's active tier changed.
    TierChanged { tier: Tier },
    /// The watchdog cut an engine entry short because it outran its deadline.
    /// The page survives; `entry` names the kind of entry that was stopped
    /// (`timers`, `fetch`, `host`, ...). An entry that used its whole deadline is
    /// reported even when the engine's own deadline stopped it first, so an entry
    /// that happened to finish exactly at the deadline is reported too.
    ScriptTerminated { entry: &'static str },
    /// Page script called a `console` method. Only produced while someone
    /// holds an event subscription: describing the arguments is not free.
    Console(ConsoleCall),
    /// An exception no script caught, or a promise rejection nobody handled.
    Exception(ExceptionEvent),
    /// Page script called a function installed by [`crate::Page::add_binding`].
    BindingCalled(BindingCall),
    /// A script realm now exists for the document being installed, numbered as
    /// the remote object ids, console calls and binding calls of that realm are.
    /// Sent when the engine is built, before the document's scripts run, so a
    /// host can announce an execution context before the page's load events
    /// instead of when it first enters the page.
    RealmCreated { realm: u64 },
    /// A stylesheet the render could not load, or a limit that cut an import
    /// tree short. The page renders without it.
    StylesheetFailed { url: Url, reason: String },
    /// An image the render could not fetch or decode. Elements that use it are
    /// laid out as broken images.
    ImageFailed { url: String, reason: String },
}

/// The receiving end of [`crate::Page::events`].
#[derive(Debug)]
pub struct PageEvents {
    inner: broadcast::Receiver<PageEvent>,
}

impl PageEvents {
    pub(crate) fn new(inner: broadcast::Receiver<PageEvent>) -> Self {
        Self { inner }
    }

    /// The next event, or `None` once the page is closed. Events lost by a
    /// slow subscriber are skipped, not reported.
    pub async fn next(&mut self) -> Option<PageEvent> {
        loop {
            match self.inner.recv().await {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Like [`PageEvents::next`], but says when events were lost instead of
    /// skipping them silently, for a consumer that can recover state: `Err`
    /// carries how many events were dropped.
    pub async fn next_or_lag(&mut self) -> Option<Result<PageEvent, u64>> {
        match self.inner.recv().await {
            Ok(event) => Some(Ok(event)),
            Err(broadcast::error::RecvError::Lagged(lost)) => Some(Err(lost)),
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }

    /// The next event if one is already buffered.
    pub fn try_next(&mut self) -> Option<PageEvent> {
        loop {
            match self.inner.try_recv() {
                Ok(event) => return Some(event),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => return None,
            }
        }
    }

    /// Every event buffered so far, oldest first.
    pub fn drain(&mut self) -> Vec<PageEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.try_next() {
            events.push(event);
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn events_reach_every_subscriber() {
        let (sender, _) = broadcast::channel(EVENT_BUFFER);
        let mut first = PageEvents::new(sender.subscribe());
        let mut second = PageEvents::new(sender.subscribe());
        let event = PageEvent::Crashed {
            reason: "test".to_owned(),
        };
        sender.send(event.clone()).expect("two subscribers");
        assert_eq!(first.next().await, Some(event.clone()));
        assert_eq!(second.next().await, Some(event));
    }

    #[tokio::test]
    async fn closing_the_sender_ends_the_stream() {
        let (sender, _) = broadcast::channel(4);
        let mut events = PageEvents::new(sender.subscribe());
        drop(sender);
        assert_eq!(events.next().await, None);
        assert!(events.try_next().is_none());
    }

    #[tokio::test]
    async fn a_lagging_subscriber_can_learn_how_much_it_lost() {
        let (sender, _) = broadcast::channel(2);
        let mut events = PageEvents::new(sender.subscribe());
        for index in 0..5u64 {
            let _ = sender.send(PageEvent::Crashed {
                reason: index.to_string(),
            });
        }
        assert!(matches!(events.next_or_lag().await, Some(Err(3))));
        assert!(matches!(
            events.next_or_lag().await,
            Some(Ok(PageEvent::Crashed { .. }))
        ));
    }

    #[test]
    fn a_slow_subscriber_skips_lost_events_instead_of_failing() {
        let (sender, _) = broadcast::channel(2);
        let mut events = PageEvents::new(sender.subscribe());
        for index in 0..5u64 {
            let _ = sender.send(PageEvent::Crashed {
                reason: index.to_string(),
            });
        }
        let drained = events.drain();
        assert_eq!(drained.len(), 2, "the channel holds the two newest events");
        assert_eq!(
            drained[0],
            PageEvent::Crashed {
                reason: "3".to_owned()
            }
        );
    }
}
