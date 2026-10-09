//! The adapter that lets Stylo traverse the arena document.
//!
//! Three pieces with different lifetimes:
//!
//! - [`StyleStore`] persists in the engine across flushes.
//! - [`FlushView`] and its [`Slot`] table are built per flush and borrow the
//!   document, the store and the atom maps.
//! - [`Handle`] is the copyable element/node reference Stylo passes around.

mod hints;
mod store;
mod style_attr;

use std::cell::OnceCell;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::mem::size_of;

use selectors::matching::VisitedHandlingMode;
use selectors::sink::Push;
use servo_arc::ArcBorrow;
use style::applicable_declarations::ApplicableDeclarationBlock;
use style::context::QuirksMode;
use style::properties::PropertyDeclarationBlock;
use style::shared_lock::{Locked, SharedRwLock};
use tenta_core::NodeId;
use tenta_dom::Document;

use super::atoms::AtomMaps;
pub use store::{RestyleBits, StoreEntry, StyleAttrSlot, StyleStore};

/// Everything a traversal reads, borrowed for the duration of one flush.
pub struct FlushView<'a> {
    /// The document being styled; read live, never copied.
    pub doc: &'a Document,
    /// The persistent atom tables.
    pub atoms: &'a AtomMaps,
    /// The engine's only lock; `TDocument::shared_lock` returns it.
    pub lock: &'a SharedRwLock,
    /// The document's quirks mode in Stylo's type, for `TDocument::quirks_mode`.
    pub quirks: QuirksMode,
    slots: OnceCell<&'a [Slot<'a>]>,
}

/// One arena slot as seen during a flush: its node and its store entry.
pub struct Slot<'a> {
    /// The view the slot navigates through.
    pub view: &'a FlushView<'a>,
    /// The node occupying the slot.
    pub node: NodeId,
    /// The node's persistent style state.
    pub entry: &'a StoreEntry,
}

/// A reference to one slot. Stylo stores elements as `usize` in its
/// style-sharing cache and asserts equal size at run time, so this must stay
/// exactly one machine word.
#[derive(Copy, Clone)]
pub struct Handle<'a>(pub &'a Slot<'a>);

impl PartialEq for Handle<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl Eq for Handle<'_> {}

impl Hash for Handle<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::from_ref(self.0).hash(state);
    }
}

impl fmt::Debug for Handle<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle({:?})", self.0.node)
    }
}

const _: () = assert!(size_of::<Handle<'static>>() == size_of::<usize>());

impl<'a> FlushView<'a> {
    /// A view of `doc` with no slot table yet. `lock` is the engine's lock.
    pub fn new(doc: &'a Document, atoms: &'a AtomMaps, lock: &'a SharedRwLock) -> Self {
        let quirks = match doc.quirks_mode() {
            tenta_html::QuirksMode::NoQuirks => QuirksMode::NoQuirks,
            tenta_html::QuirksMode::LimitedQuirks => QuirksMode::LimitedQuirks,
            tenta_html::QuirksMode::Quirks => QuirksMode::Quirks,
        };
        Self {
            doc,
            atoms,
            lock,
            quirks,
            slots: OnceCell::new(),
        }
    }

    /// One [`Slot`] per arena slot of the document, backed by `store`.
    ///
    /// `store` must have been claimed for every live node of the document.
    pub fn make_slots(&'a self, store: &'a StyleStore) -> Vec<Slot<'a>> {
        store
            .entries()
            .iter()
            .enumerate()
            .map(|(index, entry)| Slot {
                view: self,
                node: NodeId(index as u32),
                entry,
            })
            .collect()
    }

    /// Installs the slot table. Returns `false` when one was installed
    /// already.
    pub fn install_slots(&'a self, table: &'a [Slot<'a>]) -> bool {
        self.slots.set(table).is_ok()
    }

    /// The handle of `node`, once the slot table is installed and covers it.
    pub fn handle(&'a self, node: NodeId) -> Option<Handle<'a>> {
        let table = self.slots.get()?;
        table.get(node.raw() as usize).map(Handle)
    }
}

impl<'a> Handle<'a> {
    /// The node this handle refers to.
    pub fn node(&self) -> NodeId {
        self.0.node
    }

    /// The element's parsed `style` attribute (`TElement::style_attribute`).
    pub fn style_attribute(&self) -> Option<ArcBorrow<'a, Locked<PropertyDeclarationBlock>>> {
        style_attr::style_attribute(self.0)
    }

    /// Pushes the element's presentational hints
    /// (`TElement::synthesize_presentational_hints_for_legacy_attributes`).
    pub fn synthesize_hints<V>(&self, visited: VisitedHandlingMode, hints: &mut V)
    where
        V: Push<ApplicableDeclarationBlock>,
    {
        hints::synthesize(self.0, visited, hints);
    }
}

#[cfg(test)]
mod tests {
    use tenta_html::{Parser, ParserOptions};

    use super::*;

    fn parse(html: &str) -> Document {
        let mut parser = Parser::new(Document::new(), ParserOptions::default());
        parser.feed_last(html.as_bytes());
        parser.finish()
    }

    #[test]
    fn handle_is_one_machine_word() {
        assert_eq!(size_of::<Handle<'static>>(), size_of::<usize>());
        assert_eq!(size_of::<Option<Handle<'static>>>(), size_of::<usize>());
    }

    #[test]
    fn every_arena_slot_gets_a_handle_through_the_installed_table() {
        let doc = parse("<p>x</p>");
        let mut store = StyleStore::new();
        for slot in 0..doc.slots() {
            store.claim(doc.node_ref(NodeId(slot as u32)));
        }
        let mut atoms = AtomMaps::new();
        atoms.begin_flush(&doc);

        let lock = SharedRwLock::new();
        let view = FlushView::new(&doc, &atoms, &lock);
        assert!(view.handle(NodeId(0)).is_none(), "no table yet");
        let table = view.make_slots(&store);
        assert!(view.install_slots(&table));
        assert!(!view.install_slots(&table), "installed once");
        for slot in 0..doc.slots() {
            let handle = view.handle(NodeId(slot as u32)).unwrap();
            assert_eq!(handle.node(), NodeId(slot as u32));
        }
        assert!(view.handle(NodeId(doc.slots() as u32)).is_none());
    }

    #[test]
    fn the_view_exposes_the_lock_and_quirks_mode_and_handles_compare_by_slot() {
        let doc = parse("<p>x</p>");
        let mut store = StyleStore::new();
        for slot in 0..doc.slots() {
            store.claim(doc.node_ref(NodeId(slot as u32)));
        }
        let atoms = AtomMaps::new();
        let lock = SharedRwLock::new();
        let view = FlushView::new(&doc, &atoms, &lock);
        assert!(std::ptr::eq(view.lock, &lock));
        assert_eq!(view.quirks, QuirksMode::Quirks, "no doctype");
        let table = view.make_slots(&store);
        view.install_slots(&table);
        let a = view.handle(NodeId(0)).unwrap();
        let b = view.handle(NodeId(0)).unwrap();
        let c = view.handle(NodeId(1)).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(format!("{a:?}"), "Handle(NodeId(0))");
        let hash = |h: &Handle<'_>| {
            let mut state = std::collections::hash_map::DefaultHasher::new();
            h.hash(&mut state);
            state.finish()
        };
        assert_eq!(hash(&a), hash(&b));
    }
}
