//! What every session of one page shares about its scripts: the numbering of
//! execution contexts and the console messages produced so far.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use tenta_browser::PageEvent;
use tokio::sync::watch;

use crate::backend::lock;

/// The console messages Chromium's agent keeps for a late `Runtime.enable`.
const CONSOLE_CAPACITY: usize = 1000;

/// A script world of a page. The main world is the only one today; isolated
/// worlds add variants and keep every id they have handed out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum World {
    Main,
}

impl World {
    fn name(self) -> &'static str {
        match self {
            World::Main => "main",
        }
    }

    fn parse(name: &str) -> Option<World> {
        (name == World::Main.name()).then_some(World::Main)
    }
}

const UNIQUE_PREFIX: &str = "tenta-";

/// `executionContextId`s of one page: a counter, mapped to the realm and
/// world each number stands for. A client's id keeps meaning the same
/// context after the page has moved on, which is how a stale id is told from
/// a live one without the id depending on how the engine numbers realms.
#[derive(Default)]
pub(crate) struct ContextIds {
    table: Mutex<Table>,
}

#[derive(Default)]
struct Table {
    last: u64,
    by_context: HashMap<(u64, World), u64>,
    by_id: HashMap<u64, (u64, World)>,
}

impl ContextIds {
    /// The id of `realm` in `world`, allocated on first sight. Realm numbers
    /// grow, so a newer realm retires the ids of older ones in its world.
    pub(crate) fn id_for(&self, realm: u64, world: World) -> u64 {
        let mut table = lock(&self.table);
        if let Some(id) = table.by_context.get(&(realm, world)) {
            return *id;
        }
        table.last += 1;
        let id = table.last;
        let retired: Vec<(u64, World)> = table
            .by_context
            .keys()
            .filter(|(old, old_world)| *old_world == world && *old < realm)
            .copied()
            .collect();
        for key in retired {
            if let Some(old_id) = table.by_context.remove(&key) {
                table.by_id.remove(&old_id);
            }
        }
        table.by_context.insert((realm, world), id);
        table.by_id.insert(id, (realm, world));
        id
    }

    /// What `id` stands for, if it was issued and not yet retired.
    pub(crate) fn resolve(&self, id: u64) -> Option<(u64, World)> {
        lock(&self.table).by_id.get(&id).copied()
    }

    pub(crate) fn unique_id(&self, realm: u64, world: World) -> String {
        format!("{UNIQUE_PREFIX}{realm}-{}", world.name())
    }

    /// The realm and world a `uniqueId` names.
    pub(crate) fn parse_unique_id(&self, unique: &str) -> Option<(u64, World)> {
        let (realm, world) = unique.strip_prefix(UNIQUE_PREFIX)?.split_once('-')?;
        Some((realm.parse().ok()?, World::parse(world)?))
    }
}

/// The console calls and uncaught exceptions of the page's current document,
/// oldest first, shared by every session.
pub(crate) struct ConsoleRing {
    inner: Mutex<Ring>,
    sequence: watch::Sender<u64>,
}

#[derive(Default)]
struct Ring {
    items: VecDeque<(u64, PageEvent)>,
    next: u64,
    newest_realm: u64,
}

impl ConsoleRing {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(Ring::default()),
            sequence: watch::channel(0).0,
        }
    }

    fn realm_of(event: &PageEvent) -> Option<u64> {
        match event {
            PageEvent::Console(call) => Some(call.context),
            PageEvent::Exception(event) => Some(event.context),
            _ => None,
        }
    }

    /// Keeps a console call or exception; anything else is ignored. A message
    /// of a newer document drops the older documents' messages, which
    /// Chromium clears with their context.
    pub(crate) fn record(&self, event: PageEvent) {
        let Some(realm) = Self::realm_of(&event) else {
            return;
        };
        let mut ring = lock(&self.inner);
        if realm < ring.newest_realm {
            return;
        }
        if realm > ring.newest_realm {
            ring.newest_realm = realm;
            ring.items
                .retain(|(_, kept)| Self::realm_of(kept) == Some(realm));
        }
        if ring.items.len() == CONSOLE_CAPACITY {
            ring.items.pop_front();
        }
        let sequence = ring.next;
        ring.next += 1;
        ring.items.push_back((sequence, event));
        drop(ring);
        self.sequence.send_replace(sequence + 1);
    }

    /// The sequence number of the oldest message still held.
    pub(crate) fn oldest(&self) -> u64 {
        let ring = lock(&self.inner);
        ring.items
            .front()
            .map_or(ring.next, |(sequence, _)| *sequence)
    }

    /// The messages from `cursor` on, and the cursor after them.
    pub(crate) fn since(&self, cursor: u64) -> (Vec<PageEvent>, u64) {
        let ring = lock(&self.inner);
        let events = ring
            .items
            .iter()
            .filter(|(sequence, _)| *sequence >= cursor)
            .map(|(_, event)| event.clone())
            .collect();
        (events, ring.next)
    }

    /// Changes whenever a message is recorded.
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.sequence.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use tenta_browser::ConsoleCall;

    use super::*;

    fn log(realm: u64, text: &str) -> PageEvent {
        PageEvent::Console(ConsoleCall {
            kind: "log".to_owned(),
            args: vec![tenta_browser::RemoteValue::String(text.to_owned())],
            stack: Vec::new(),
            timestamp_ms: 0.0,
            context: realm,
        })
    }

    fn texts(ring: &ConsoleRing) -> Vec<String> {
        ring.since(ring.oldest())
            .0
            .into_iter()
            .map(|event| match event {
                PageEvent::Console(call) => format!("{:?}", call.args[0]),
                other => panic!("unexpected {other:?}"),
            })
            .collect()
    }

    #[test]
    fn the_ring_keeps_the_newest_thousand() {
        let ring = ConsoleRing::new();
        for n in 0..1200 {
            ring.record(log(7, &n.to_string()));
        }
        let kept = texts(&ring);
        assert_eq!(kept.len(), CONSOLE_CAPACITY);
        assert_eq!(kept[0], "String(\"200\")");
        let (rest, _) = ring.since(ring.oldest() + 999);
        assert_eq!(rest.len(), 1);
    }

    #[test]
    fn a_newer_document_clears_the_older_ones_messages() {
        let ring = ConsoleRing::new();
        ring.record(log(3, "old"));
        ring.record(log(4, "new"));
        ring.record(log(3, "straggler"));
        assert_eq!(texts(&ring), ["String(\"new\")"]);
    }

    #[test]
    fn context_ids_are_a_counter_and_retire_with_their_realm() {
        let ids = ContextIds::default();
        let first = ids.id_for(40, World::Main);
        assert_eq!(first, 1);
        assert_eq!(ids.id_for(40, World::Main), first);
        assert_eq!(ids.resolve(first), Some((40, World::Main)));
        let second = ids.id_for(57, World::Main);
        assert_eq!(second, 2);
        assert_eq!(ids.resolve(first), None);
        assert_eq!(
            ids.parse_unique_id(&ids.unique_id(57, World::Main)),
            Some((57, World::Main))
        );
        assert_eq!(ids.parse_unique_id("tenta-57-other"), None);
    }
}
