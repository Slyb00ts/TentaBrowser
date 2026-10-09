//! The persistent per-element style store.

use std::cell::{Cell, OnceCell};

use servo_arc::Arc;
use style::data::ElementDataWrapper;
use style::properties::PropertyDeclarationBlock;
use style::shared_lock::Locked;
use tenta_core::NodeRef;

/// Restyle bits kept per element until the incremental path consumes them.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub struct RestyleBits(pub u8);

/// The parsed `style=""` block of one element and the document generation it
/// was parsed at. `TElement::style_attribute` returns a borrow of the block, so
/// it has to live as long as the entry.
#[derive(Debug, Default)]
pub struct StyleAttrSlot {
    /// `Document::mutation_generation` the block was parsed at; the cache is
    /// stale when the document has moved on and the attribute changed.
    pub source_generation: Cell<u64>,
    /// `None` inside means the element has no usable `style` attribute.
    pub block: OnceCell<Option<Arc<Locked<PropertyDeclarationBlock>>>>,
}

/// Stylo's per-element state plus the flags `TElement` reports.
#[derive(Debug, Default)]
pub struct StoreEntry {
    /// Arena generation of the node this entry belongs to.
    pub generation: u32,
    /// Stylo's element data, valid only while `has_data` is set (Stylo's
    /// `has_data`/`borrow_data` report absence until `ensure_data` ran).
    pub data: ElementDataWrapper,
    /// Whether `data` is initialised.
    pub has_data: Cell<bool>,
    /// A descendant needs a style recomputation.
    pub dirty_descendants: Cell<bool>,
    /// Restyle hint and dirtiness bits for the incremental path.
    pub restyle: Cell<RestyleBits>,
    /// The parsed `style` attribute.
    pub style_attr: StyleAttrSlot,
}

/// Style state for every arena slot, owned by the engine and kept across
/// flushes. Entries are keyed by slot *and* generation: a recycled slot gets a
/// fresh entry, so a new node never sees the previous occupant's styles.
#[derive(Debug, Default)]
pub struct StyleStore {
    entries: Vec<StoreEntry>,
}

impl StyleStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes `node` addressable: grows the store to the slot, and replaces the
    /// entry when it belongs to another generation. Idempotent for a node that
    /// already owns its entry.
    pub fn claim(&mut self, node: NodeRef) {
        let slot = node.id.raw() as usize;
        if self.entries.len() <= slot {
            self.entries.resize_with(slot + 1, StoreEntry::default);
        }
        let entry = &mut self.entries[slot];
        if entry.generation != node.generation {
            *entry = StoreEntry {
                generation: node.generation,
                ..StoreEntry::default()
            };
        }
    }

    /// The entry of `node`, when it exists and belongs to the same generation.
    pub fn get(&self, node: NodeRef) -> Option<&StoreEntry> {
        self.entries
            .get(node.id.raw() as usize)
            .filter(|entry| entry.generation == node.generation)
    }

    /// Number of slots the store covers.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the store covers no slot.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All entries by slot, for building the per-flush handle table.
    pub fn entries(&self) -> &[StoreEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use tenta_core::NodeId;

    use super::*;

    #[test]
    fn a_recycled_slot_does_not_inherit_the_previous_occupant() {
        let mut store = StyleStore::new();
        let old = NodeRef::new(NodeId(3), 0);
        store.claim(old);
        store.get(old).unwrap().has_data.set(true);
        store.get(old).unwrap().dirty_descendants.set(true);

        let recycled = NodeRef::new(NodeId(3), 1);
        assert!(store.get(recycled).is_none(), "claimed lazily");
        store.claim(recycled);
        let entry = store.get(recycled).unwrap();
        assert!(!entry.has_data.get());
        assert!(!entry.dirty_descendants.get());
        assert!(
            store.get(old).is_none(),
            "old generation no longer resolves"
        );
    }

    #[test]
    fn claiming_twice_keeps_the_entry() {
        let mut store = StyleStore::new();
        let node = NodeRef::new(NodeId(1), 0);
        store.claim(node);
        store.get(node).unwrap().has_data.set(true);
        store.claim(node);
        assert!(store.get(node).unwrap().has_data.get());
        assert_eq!(store.len(), 2);
        assert!(!store.is_empty());
        assert_eq!(store.entries().len(), 2);
    }
}
