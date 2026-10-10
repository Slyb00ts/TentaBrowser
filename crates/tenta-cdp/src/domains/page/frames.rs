//! The main frame as a client sees it: the `Frame` object, the loader ids, and
//! the events that follow a navigation from its start to its idle network.
//!
//! The page reports the milestones of every navigation (`PageEvent`); this
//! module turns them into the protocol's events for one session. A session
//! can join at any moment, so what it is told is decided per document: an
//! event of a document is sent at most once to a session, whichever of the
//! live stream and the replay at `setLifecycleEventsEnabled` reaches it first.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde_json::{Value, json};
use tenta_browser::{Commit, LoadStage, PageEvent};
use tokio::sync::Mutex;

use crate::backend::Target;
use crate::session::Session;

/// What one session has been told, keyed by document and event.
type Sent = HashSet<(u64, &'static str)>;

/// A session's Page domain state.
#[derive(Default)]
pub(super) struct FrameEvents {
    enabled: AtomicBool,
    lifecycle: AtomicBool,
    watching: AtomicBool,
    sent: Mutex<Sent>,
}

/// Seconds on a clock that only moves forward, the unit of the protocol's
/// `MonotonicTime`.
fn monotonic_seconds() -> f64 {
    crate::clock::seconds_at(Instant::now())
}

/// The loader id of the document `navigation_id` loaded. Chromium's are 32
/// hex digits; clients only compare them.
pub(super) fn loader_id(target: &Target, navigation_id: u64) -> String {
    target.loader_id(navigation_id)
}

/// The origin Chromium reports for a document: opaque origins, such as
/// `about:blank`'s, are spelled `://`.
fn security_origin(url: &url::Url) -> String {
    let origin = url.origin();
    if origin.is_tuple() {
        origin.ascii_serialization()
    } else {
        "://".to_owned()
    }
}

/// The protocol's `Frame` for the document the frame shows.
pub(super) fn frame_json(target: &Target) -> Value {
    let state = target.page().frame_state();
    let Some(commit) = state.commit else {
        return json!({
            "id": target.id(),
            "loaderId": loader_id(target, state.navigation_id),
            "url": "about:blank",
            "domainAndRegistry": "",
            "securityOrigin": "://",
            "mimeType": "text/html",
            "secureContextType": "InsecureScheme",
            "crossOriginIsolatedContextType": "NotIsolated",
            "gatedAPIFeatures": [],
        });
    };
    commit_frame(target, &commit)
}

fn commit_frame(target: &Target, commit: &Commit) -> Value {
    let mut url = commit.url.clone();
    let fragment = url.fragment().map(str::to_owned);
    url.set_fragment(None);
    let mut frame = json!({
        "id": target.id(),
        "loaderId": loader_id(target, commit.navigation_id),
        "url": url.as_str(),
        "domainAndRegistry": "",
        "securityOrigin": security_origin(&commit.url),
        "mimeType": commit.mime_type,
        "secureContextType": "InsecureScheme",
        "crossOriginIsolatedContextType": "NotIsolated",
        "gatedAPIFeatures": [],
    });
    if let Some(fragment) = fragment {
        frame["urlFragment"] = json!(format!("#{fragment}"));
    }
    if let Some(unreachable) = &commit.unreachable_url {
        frame["unreachableUrl"] = json!(unreachable.as_str());
    }
    frame
}

/// Events decided under the `sent` lock and emitted after it is released, so a
/// slow client never holds the lock.
type Outbox = Vec<(&'static str, Value)>;

/// Starts telling `session` about the frame's loading. Idempotent.
pub(super) fn enable(session: &Arc<Session>, target: &Arc<Target>) {
    let state = session.state::<FrameEvents>();
    let was_enabled = state.enabled.swap(true, Ordering::AcqRel);
    if !state.watching.swap(true, Ordering::AcqRel) {
        // Subscribed here, not in the task, so no event after this command is lost.
        let mut events = target.page().events();
        let (task_session, target) = (Arc::clone(session), Arc::clone(target));
        session.spawn(async move {
            let main_frame = target.page().main_frame();
            while let Some(delivery) = events.next_or_lag().await {
                match delivery {
                    Ok(event) => {
                        let relevant = match &event {
                            PageEvent::NavigationStarted { frame, .. }
                            | PageEvent::NavigationCommitted { frame, .. }
                            | PageEvent::NavigationStopped { frame, .. }
                            | PageEvent::Lifecycle { frame, .. } => *frame == main_frame,
                            _ => false,
                        };
                        if relevant {
                            tell_about(&task_session, &target, event).await;
                        }
                    }
                    // Events were lost, so what the frame has done is read from
                    // its state instead.
                    Err(_) => replay(&task_session, &target, true).await,
                }
            }
        });
    }
    // Lifecycle events asked for before the domain was enabled are owed now.
    if !was_enabled && state.lifecycle.load(Ordering::Acquire) {
        let (task_session, target) = (Arc::clone(session), Arc::clone(target));
        session.spawn(async move { replay(&task_session, &target, false).await });
    }
}

pub(super) fn disable(session: &Session) {
    session
        .state::<FrameEvents>()
        .enabled
        .store(false, Ordering::Release);
}

/// Turns the lifecycle events on or off. Turning them on replays what the
/// current document has reached, as Chromium does, so a client that asks
/// after the load still hears `load`. Before `Page.enable` the choice is kept
/// and the replay happens when the domain is enabled.
pub(super) async fn set_lifecycle(session: &Session, target: &Target, on: bool) {
    let state = session.state::<FrameEvents>();
    let was_on = state.lifecycle.swap(on, Ordering::AcqRel);
    if on && !was_on && state.enabled.load(Ordering::Acquire) {
        replay(session, target, false).await;
    }
}

/// Tells the session what the frame's current document has already done. With
/// `everything` the navigation events are included, for a session that lost
/// some; without, only the lifecycle events a new subscriber is owed.
async fn replay(session: &Session, target: &Target, everything: bool) {
    let state = session.state::<FrameEvents>();
    if !state.enabled.load(Ordering::Acquire) {
        return;
    }
    let frame = target.page().frame_state();
    let document = frame.navigation_id;
    let mut out = Outbox::new();
    {
        let mut sent = state.sent.lock().await;
        lifecycle(session, target, &mut sent, &mut out, document, "init");
        if let Some(commit) = &frame.commit {
            if everything {
                tell(
                    &mut sent,
                    &mut out,
                    document,
                    "Page.frameNavigated",
                    json!({ "frame": commit_frame(target, commit), "type": "Navigation" }),
                );
            }
            lifecycle(session, target, &mut sent, &mut out, document, "commit");
        }
        for stage in frame.stages {
            reached(
                session, target, &mut sent, &mut out, document, stage, everything,
            );
        }
    }
    flush(session, out).await;
}

fn stage_name(stage: LoadStage) -> &'static str {
    match stage {
        LoadStage::DomContentLoaded => "DOMContentLoaded",
        LoadStage::Load => "load",
        LoadStage::NetworkAlmostIdle => "networkAlmostIdle",
        LoadStage::NetworkIdle => "networkIdle",
    }
}

/// Queues `method` unless this session already heard it for `document`.
fn tell(sent: &mut Sent, out: &mut Outbox, document: u64, method: &'static str, params: Value) {
    if sent.insert((document, method)) {
        out.push((method, params));
    }
}

/// Queues `Page.lifecycleEvent` `name` when lifecycle events are on. Lifecycle
/// names never collide with method names, so they share the dedup key space.
fn lifecycle(
    session: &Session,
    target: &Target,
    sent: &mut Sent,
    out: &mut Outbox,
    document: u64,
    name: &'static str,
) {
    if !session
        .state::<FrameEvents>()
        .lifecycle
        .load(Ordering::Acquire)
    {
        return;
    }
    if sent.insert((document, name)) {
        out.push((
            "Page.lifecycleEvent",
            json!({
                "frameId": target.id(),
                "loaderId": loader_id(target, document),
                "name": name,
                "timestamp": monotonic_seconds(),
            }),
        ));
    }
}

/// Queues everything `stage` means for a client. `navigation_events` is false
/// for a lifecycle-only replay.
fn reached(
    session: &Session,
    target: &Target,
    sent: &mut Sent,
    out: &mut Outbox,
    document: u64,
    stage: LoadStage,
    navigation_events: bool,
) {
    lifecycle(session, target, sent, out, document, stage_name(stage));
    if !navigation_events {
        return;
    }
    let timestamp = json!({ "timestamp": monotonic_seconds() });
    match stage {
        LoadStage::DomContentLoaded => {
            tell(sent, out, document, "Page.domContentEventFired", timestamp);
        }
        LoadStage::Load => {
            tell(sent, out, document, "Page.loadEventFired", timestamp);
            tell(
                sent,
                out,
                document,
                "Page.frameStoppedLoading",
                json!({ "frameId": target.id() }),
            );
        }
        LoadStage::NetworkAlmostIdle | LoadStage::NetworkIdle => {}
    }
}

async fn flush(session: &Session, out: Outbox) {
    for (method, params) in out {
        session.emit(method, params).await;
    }
}

async fn tell_about(session: &Session, target: &Target, event: PageEvent) {
    let state = session.state::<FrameEvents>();
    if !state.enabled.load(Ordering::Acquire) {
        return;
    }
    let mut out = Outbox::new();
    {
        let mut sent = state.sent.lock().await;
        match event {
            PageEvent::NavigationStarted { navigation_id, .. } => {
                sent.retain(|(document, _)| *document >= navigation_id);
                tell(
                    &mut sent,
                    &mut out,
                    navigation_id,
                    "Page.frameStartedLoading",
                    json!({ "frameId": target.id() }),
                );
                lifecycle(session, target, &mut sent, &mut out, navigation_id, "init");
            }
            PageEvent::NavigationCommitted { commit, .. } => {
                let document = commit.navigation_id;
                sent.retain(|(kept, _)| *kept >= document);
                // A session that joined after the navigation started missed `init`.
                lifecycle(session, target, &mut sent, &mut out, document, "init");
                tell(
                    &mut sent,
                    &mut out,
                    document,
                    "Page.frameNavigated",
                    json!({ "frame": commit_frame(target, &commit), "type": "Navigation" }),
                );
                lifecycle(session, target, &mut sent, &mut out, document, "commit");
            }
            PageEvent::Lifecycle {
                navigation_id,
                stage,
                ..
            } => reached(
                session,
                target,
                &mut sent,
                &mut out,
                navigation_id,
                stage,
                true,
            ),
            PageEvent::NavigationStopped { navigation_id, .. } => tell(
                &mut sent,
                &mut out,
                navigation_id,
                "Page.frameStoppedLoading",
                json!({ "frameId": target.id() }),
            ),
            _ => {}
        }
    }
    flush(session, out).await;
}
