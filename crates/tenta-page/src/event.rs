//! The page event stream (`docs/spec/11-library-api.md` §1).
//!
//! Events are broadcast to every subscriber; a subscriber that falls behind
//! loses the oldest events rather than blocking the page thread.

use tenta_core::{FrameId, RequestId, Tier};
use tenta_js::inspect::{BindingCall, ConsoleCall, ExceptionEvent};
use tokio::sync::broadcast;
use url::Url;

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
