//! The target's side of its page's network traffic: the sink the page pushes
//! events into, the sessions that listen, the bodies kept for
//! `Network.getResponseBody`, and the requests held for `Fetch`.
//!
//! The page calls the sink from its own threads and must never wait on a
//! client, so everything here only queues: a session's task drains its queue
//! and does the (awaiting) writing.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tenta_browser::network::{
    BoxFuture, Bytes, Modifications, NetworkEvent, NetworkSink, RequestInfo, RequestKey,
    Resolution, ResourceType,
};
use tokio::sync::{Notify, oneshot};

use crate::backend::lock;

/// The most bytes of response bodies and request bodies one target keeps. The
/// least recently read goes first.
pub(crate) const BODY_BUDGET: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    Response,
    Post,
}

struct Entry {
    bytes: Bytes,
    /// Whether the bytes are text a client may be given as a string.
    text: bool,
    tick: u64,
}

/// Bodies by request, bounded by bytes, evicting the least recently used.
pub(crate) struct BodyStore {
    budget: usize,
    used: usize,
    tick: u64,
    entries: HashMap<(Kind, u64), Entry>,
    order: BTreeMap<u64, (Kind, u64)>,
}

impl BodyStore {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            used: 0,
            tick: 0,
            entries: HashMap::new(),
            order: BTreeMap::new(),
        }
    }

    fn insert(&mut self, kind: Kind, id: u64, bytes: Bytes, text: bool) {
        if bytes.len() > self.budget {
            return;
        }
        self.remove(kind, id);
        self.tick += 1;
        self.used += bytes.len();
        self.order.insert(self.tick, (kind, id));
        self.entries.insert(
            (kind, id),
            Entry {
                bytes,
                text,
                tick: self.tick,
            },
        );
        while self.used > self.budget {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.used -= entry.bytes.len();
            }
        }
    }

    fn remove(&mut self, kind: Kind, id: u64) {
        if let Some(entry) = self.entries.remove(&(kind, id)) {
            self.used -= entry.bytes.len();
            self.order.remove(&entry.tick);
        }
    }

    fn get(&mut self, kind: Kind, id: u64) -> Option<(Bytes, bool)> {
        self.tick += 1;
        let entry = self.entries.get_mut(&(kind, id))?;
        self.order.remove(&entry.tick);
        entry.tick = self.tick;
        self.order.insert(self.tick, (kind, id));
        Some((entry.bytes.clone(), entry.text))
    }
}

/// Set on the store key of a document request, which clients name by its
/// loader id rather than by a request number.
pub(crate) const DOCUMENT_BIT: u64 = 1 << 63;

fn store_key(key: &RequestKey) -> u64 {
    if key.resource_type == ResourceType::Document {
        DOCUMENT_BIT | key.loader
    } else {
        key.id
    }
}

/// What a session's task is told, in order.
pub(crate) enum Outgoing {
    Event(NetworkEvent),
    Paused {
        fetch_id: String,
        request: Box<RequestInfo>,
    },
}

/// Which requests a `Fetch.enable` holds.
#[derive(Clone, Debug, Default)]
pub(crate) struct Gate {
    /// `urlPattern` and `resourceType` of each pattern; no pattern holds all.
    pub patterns: Vec<(String, Option<String>)>,
}

impl Gate {
    fn matches(&self, url: &str, resource_type: &str) -> bool {
        self.patterns.is_empty()
            || self.patterns.iter().any(|(pattern, kind)| {
                kind.as_deref()
                    .is_none_or(|kind| kind.eq_ignore_ascii_case(resource_type))
                    && wildcard(pattern, url)
            })
    }
}

/// `*` matches any run and `?` any one character, as the protocol's
/// `urlPattern` does.
pub(crate) fn wildcard(pattern: &str, text: &str) -> bool {
    let (pattern, text): (Vec<char>, Vec<char>) =
        (pattern.chars().collect(), text.chars().collect());
    let (mut p, mut t) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(star) = star {
            p = star + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

/// Events one session may have queued before `dataReceived` events are shed.
/// Everything else a request does is kept: it is bounded by the requests in
/// flight, and a client that misses a `loadingFinished` waits for it forever.
pub(crate) const QUEUE_CAP: usize = 8192;

/// One session's subscription to the target's traffic.
pub(crate) struct Subscriber {
    queue: Mutex<VecDeque<Outgoing>>,
    wake: Notify,
    closed: AtomicBool,
    events: AtomicBool,
    gate: Mutex<Option<Gate>>,
}

impl Subscriber {
    pub(crate) fn set_events(&self, on: bool) {
        self.events.store(on, Ordering::Release);
    }

    pub(crate) fn set_gate(&self, gate: Option<Gate>) {
        *lock(&self.gate) = gate;
    }

    pub(crate) fn has_gate(&self) -> bool {
        lock(&self.gate).is_some()
    }

    /// Queues `outgoing` for the session's task. Consecutive `dataReceived`
    /// events of one request merge, and past [`QUEUE_CAP`] the oldest data
    /// event goes. `false` once the session's task has ended.
    fn push(&self, outgoing: Outgoing) -> bool {
        if self.closed.load(Ordering::Acquire) {
            return false;
        }
        let mut queue = lock(&self.queue);
        if let Outgoing::Event(NetworkEvent::Data { key, length, at }) = &outgoing
            && let Some(Outgoing::Event(NetworkEvent::Data {
                key: last_key,
                length: last_length,
                at: last_at,
            })) = queue.back_mut()
            && last_key == key
        {
            *last_length += length;
            *last_at = *at;
            return true;
        }
        if queue.len() >= QUEUE_CAP
            && let Some(oldest) = queue
                .iter()
                .position(|queued| matches!(queued, Outgoing::Event(NetworkEvent::Data { .. })))
        {
            queue.remove(oldest);
        }
        queue.push_back(outgoing);
        drop(queue);
        self.wake.notify_one();
        true
    }

    /// The next thing to tell the session.
    pub(crate) async fn next(&self) -> Outgoing {
        loop {
            if let Some(outgoing) = lock(&self.queue).pop_front() {
                return outgoing;
            }
            self.wake.notified().await;
        }
    }

    /// The session's task has ended.
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        lock(&self.queue).clear();
    }

    #[cfg(test)]
    fn queued(&self) -> usize {
        lock(&self.queue).len()
    }
}

/// The sink of one target.
#[derive(Default)]
pub(crate) struct NetworkBridge {
    subscribers: Mutex<Vec<Arc<Subscriber>>>,
    store: Mutex<Option<BodyStore>>,
    /// The content type of each response whose body may still arrive.
    mimes: Mutex<HashMap<u64, String>>,
    /// Held requests by id, with the subscriber that holds each.
    pending: Mutex<HashMap<String, (usize, oneshot::Sender<Resolution>)>>,
    held: AtomicU64,
}

/// Whether a content type is text a client may be handed as a string.
fn is_text(mime: &str) -> bool {
    mime.starts_with("text/")
        || mime.ends_with("json")
        || mime.ends_with("xml")
        || mime.ends_with("javascript")
        || mime == "application/x-www-form-urlencoded"
}

impl NetworkBridge {
    pub(crate) fn subscribe(&self) -> Arc<Subscriber> {
        let subscriber = Arc::new(Subscriber {
            queue: Mutex::new(VecDeque::new()),
            wake: Notify::new(),
            closed: AtomicBool::new(false),
            events: AtomicBool::new(false),
            gate: Mutex::new(None),
        });
        lock(&self.subscribers).push(Arc::clone(&subscriber));
        subscriber
    }

    /// Removes the subscription, and lets every request it held go on.
    pub(crate) fn unsubscribe(&self, subscriber: &Arc<Subscriber>) {
        subscriber.set_gate(None);
        self.release(subscriber);
        subscriber.close();
        lock(&self.subscribers).retain(|known| !Arc::ptr_eq(known, subscriber));
    }

    pub(crate) fn wants_events(&self) -> bool {
        lock(&self.subscribers)
            .iter()
            .any(|subscriber| subscriber.events.load(Ordering::Acquire))
    }

    pub(crate) fn is_intercepting(&self) -> bool {
        lock(&self.subscribers)
            .iter()
            .any(|subscriber| subscriber.has_gate())
    }

    /// Whether some session other than `except` already holds requests.
    pub(crate) fn gate_taken_by_other(&self, except: &Arc<Subscriber>) -> bool {
        lock(&self.subscribers)
            .iter()
            .any(|subscriber| !Arc::ptr_eq(subscriber, except) && subscriber.has_gate())
    }

    /// The body of response `id` and whether it is text.
    pub(crate) fn response_body(&self, id: u64) -> Option<(Bytes, bool)> {
        lock(&self.store).as_mut()?.get(Kind::Response, id)
    }

    pub(crate) fn post_data(&self, id: u64) -> Option<Bytes> {
        lock(&self.store)
            .as_mut()?
            .get(Kind::Post, id)
            .map(|(bytes, _)| bytes)
    }

    fn keep(&self, kind: Kind, id: u64, bytes: Bytes, text: bool) {
        lock(&self.store)
            .get_or_insert_with(|| BodyStore::new(BODY_BUDGET))
            .insert(kind, id, bytes, text);
    }

    fn record(&self, event: &NetworkEvent) {
        match event {
            NetworkEvent::Request(request) => {
                if let Some(post) = &request.post_data {
                    let key = RequestKey {
                        id: request.id,
                        loader: request.loader,
                        resource_type: request.resource_type,
                    };
                    self.keep(Kind::Post, store_key(&key), post.clone(), true);
                }
            }
            NetworkEvent::Response(response) => {
                let key = RequestKey {
                    id: response.id,
                    loader: response.loader,
                    resource_type: response.resource_type,
                };
                lock(&self.mimes).insert(store_key(&key), response.mime_type.clone());
            }
            NetworkEvent::Body { key, body } => {
                let stored = store_key(key);
                let mime = lock(&self.mimes).get(&stored).cloned().unwrap_or_default();
                let text = is_text(&mime) && std::str::from_utf8(body).is_ok();
                self.keep(Kind::Response, stored, body.clone(), text);
            }
            NetworkEvent::Finished { key, .. } | NetworkEvent::Failed { key, .. } => {
                lock(&self.mimes).remove(&store_key(key));
            }
            _ => {}
        }
    }

    /// Releases the request held under `fetch_id`. `false` when none is.
    pub(crate) fn resolve(&self, fetch_id: &str, resolution: Resolution) -> bool {
        match lock(&self.pending).remove(fetch_id) {
            Some((_, waiting)) => waiting.send(resolution).is_ok(),
            None => false,
        }
    }

    /// Lets every request `subscriber` holds go on unchanged.
    pub(crate) fn release(&self, subscriber: &Arc<Subscriber>) {
        let owner = Arc::as_ptr(subscriber) as usize;
        lock(&self.pending).retain(|_, (holder, _)| *holder != owner);
    }
}

impl NetworkSink for NetworkBridge {
    fn event(&self, event: NetworkEvent) {
        self.record(&event);
        if matches!(event, NetworkEvent::Body { .. }) {
            return;
        }
        let mut subscribers = lock(&self.subscribers);
        subscribers.retain(|subscriber| {
            !subscriber.events.load(Ordering::Acquire)
                || subscriber.push(Outgoing::Event(event.clone()))
        });
    }

    fn pause<'a>(&'a self, request: &'a RequestInfo) -> BoxFuture<'a, Resolution> {
        Box::pin(async move {
            let unchanged = || Resolution::Continue(Modifications::default());
            let resource_type = super::events::resource_type(request.resource_type);
            // Looking for the holder, registering the request and queueing it
            // happen under the holder's gate: a `Fetch.disable` that lands
            // first leaves nothing to hold, and one that lands after finds the
            // entry and releases it.
            let (fetch_id, decision) = {
                let subscribers = lock(&self.subscribers);
                let mut held = None;
                for subscriber in subscribers.iter() {
                    let gate = lock(&subscriber.gate);
                    if !gate
                        .as_ref()
                        .is_some_and(|gate| gate.matches(request.url.as_str(), resource_type))
                    {
                        continue;
                    }
                    let fetch_id = format!(
                        "interception-job-{}.0",
                        self.held.fetch_add(1, Ordering::Relaxed) + 1
                    );
                    let (decide, decision) = oneshot::channel();
                    let owner = Arc::as_ptr(subscriber) as usize;
                    lock(&self.pending).insert(fetch_id.clone(), (owner, decide));
                    let queued = subscriber.push(Outgoing::Paused {
                        fetch_id: fetch_id.clone(),
                        request: Box::new(request.clone()),
                    });
                    if !queued {
                        lock(&self.pending).remove(&fetch_id);
                        return unchanged();
                    }
                    held = Some((fetch_id, decision));
                    break;
                }
                match held {
                    Some(held) => held,
                    None => return unchanged(),
                }
            };
            // A request abandoned while it waits (its navigation was replaced,
            // its page closed) must not leave its entry behind.
            let _forget = Forget {
                bridge: self,
                fetch_id: &fetch_id,
            };
            decision.await.unwrap_or_else(|_| unchanged())
        })
    }
}

struct Forget<'a> {
    bridge: &'a NetworkBridge,
    fetch_id: &'a str,
}

impl Drop for Forget<'_> {
    fn drop(&mut self) {
        lock(&self.bridge.pending).remove(self.fetch_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards_match_like_the_protocol_patterns() {
        assert!(wildcard("*", "http://a.test/x"));
        assert!(wildcard("http://a.test/*.png", "http://a.test/i/b.png"));
        assert!(!wildcard("http://a.test/*.png", "http://a.test/i/b.gif"));
        assert!(wildcard("http://a.test/?", "http://a.test/x"));
        assert!(!wildcard("http://a.test/?", "http://a.test/xy"));
        assert!(wildcard("*a*b*", "xxaxxbxx"));
    }

    #[test]
    fn the_least_recently_read_body_goes_first() {
        let mut store = BodyStore::new(10);
        store.insert(Kind::Response, 1, Bytes::from_static(b"1234"), true);
        store.insert(Kind::Response, 2, Bytes::from_static(b"5678"), true);
        assert!(store.get(Kind::Response, 1).is_some());
        store.insert(Kind::Response, 3, Bytes::from_static(b"abcd"), true);
        assert!(
            store.get(Kind::Response, 2).is_none(),
            "2 was the oldest read"
        );
        assert!(store.get(Kind::Response, 1).is_some());
        assert!(store.get(Kind::Response, 3).is_some());
        assert!(store.used <= 10);
    }

    fn data(id: u64, length: u64) -> NetworkEvent {
        NetworkEvent::Data {
            key: RequestKey {
                id,
                loader: 1,
                resource_type: ResourceType::Script,
            },
            length,
            at: std::time::Instant::now(),
        }
    }

    fn request(url: &str) -> RequestInfo {
        RequestInfo {
            id: 1,
            loader: 1,
            frame: tenta_core::FrameId(1),
            resource_type: ResourceType::Fetch,
            url: url::Url::parse(url).unwrap(),
            method: "GET".to_owned(),
            headers: Vec::new(),
            post_data: None,
            document_url: url::Url::parse("http://a.test/").unwrap(),
            initiator: tenta_browser::network::Initiator::Other,
            redirect: None,
            user_gesture: false,
            at: std::time::Instant::now(),
        }
    }

    #[test]
    fn consecutive_data_events_of_one_request_merge() {
        let bridge = NetworkBridge::default();
        let subscriber = bridge.subscribe();
        subscriber.set_events(true);
        for _ in 0..10 {
            bridge.event(data(1, 100));
        }
        bridge.event(data(2, 5));
        bridge.event(data(1, 7));
        assert_eq!(subscriber.queued(), 3);
        match lock(&subscriber.queue).front() {
            Some(Outgoing::Event(NetworkEvent::Data { length, .. })) => assert_eq!(*length, 1000),
            _ => panic!("a merged data event first"),
        }
    }

    #[test]
    fn past_the_cap_data_goes_first_and_the_rest_stays() {
        let bridge = NetworkBridge::default();
        let subscriber = bridge.subscribe();
        subscriber.set_events(true);
        for n in 0..QUEUE_CAP as u64 {
            // Alternating requests defeat merging.
            bridge.event(data(n % 2, 1));
        }
        let finished = NetworkEvent::Finished {
            key: RequestKey {
                id: 0,
                loader: 1,
                resource_type: ResourceType::Script,
            },
            encoded_length: 0,
            at: std::time::Instant::now(),
        };
        bridge.event(finished);
        bridge.event(data(1, 1));
        assert_eq!(subscriber.queued(), QUEUE_CAP);
        assert!(
            lock(&subscriber.queue)
                .iter()
                .any(|queued| matches!(queued, Outgoing::Event(NetworkEvent::Finished { .. })))
        );
    }

    #[tokio::test]
    async fn disabling_releases_a_request_that_is_waiting() {
        let bridge = Arc::new(NetworkBridge::default());
        let subscriber = bridge.subscribe();
        subscriber.set_gate(Some(Gate::default()));
        let info = request("http://a.test/x");
        let waiting = {
            let bridge = Arc::clone(&bridge);
            tokio::spawn(async move { bridge.pause(&info).await })
        };
        // It is queued for the session, which is how it is known to wait.
        assert!(matches!(subscriber.next().await, Outgoing::Paused { .. }));
        subscriber.set_gate(None);
        bridge.release(&subscriber);
        assert!(matches!(waiting.await.unwrap(), Resolution::Continue(_)));
        assert!(lock(&bridge.pending).is_empty());
    }

    #[tokio::test]
    async fn dropping_the_subscription_releases_it_too() {
        let bridge = Arc::new(NetworkBridge::default());
        let subscriber = bridge.subscribe();
        subscriber.set_gate(Some(Gate::default()));
        let info = request("http://a.test/x");
        let waiting = {
            let bridge = Arc::clone(&bridge);
            tokio::spawn(async move { bridge.pause(&info).await })
        };
        assert!(matches!(subscriber.next().await, Outgoing::Paused { .. }));
        bridge.unsubscribe(&subscriber);
        assert!(matches!(waiting.await.unwrap(), Resolution::Continue(_)));
        // And once nobody holds requests, nothing is held.
        let again = bridge.pause(&request("http://a.test/y")).await;
        assert!(matches!(again, Resolution::Continue(_)));
    }

    #[test]
    fn a_body_over_the_budget_is_not_kept() {
        let mut store = BodyStore::new(3);
        store.insert(Kind::Response, 1, Bytes::from_static(b"1234"), true);
        assert!(store.get(Kind::Response, 1).is_none());
    }
}
