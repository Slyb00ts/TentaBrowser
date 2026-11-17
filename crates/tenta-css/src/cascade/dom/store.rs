//! The persistent per-element style store.

use std::cell::{Cell, OnceCell};

use servo_arc::Arc;
use style::data::ElementDataWrapper;
use style::properties::PropertyDeclarationBlock;
use style::shared_lock::Locked;
use tenta_core::atom::atoms;
use tenta_core::{NodeId, NodeRef};
use tenta_dom::Document;
use url::Url;

use super::style_attr;

/// Restyle bits kept per element until the incremental path consumes them.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub struct RestyleBits(pub u8);

/// The parsed `style=""` block of one element and what it was parsed from.
/// `TElement::style_attribute` returns a borrow of the block, so it has to live
/// as long as the entry; replacing it needs `&mut`, which is why staleness is
/// settled by [`StyleStore::refresh`] before a flush, not during it.
#[derive(Debug, Default)]
pub struct StyleAttrSlot {
    /// `Document::mutation_generation` the block was last known current at.
    /// While it equals the document's, the attribute text is not compared.
    pub source_generation: Cell<u64>,
    /// The attribute text the block was parsed from; unset when the element
    /// had no `style` attribute at parse time.
    pub source: OnceCell<Box<str>>,
    /// `None` inside means the element has no usable `style` attribute.
    pub block: OnceCell<Option<Arc<Locked<PropertyDeclarationBlock>>>>,
}

/// The presentational-hint declarations of one element.
#[derive(Debug, Default)]
pub struct HintsSlot {
    /// `Document::mutation_generation` the block was built at. Hints read the
    /// element's ancestors (`cellpadding`, the body's `link` colour), so any
    /// document mutation makes them stale.
    pub source_generation: Cell<u64>,
    /// `None` inside means the element has no hints.
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
    /// Raw `selectors::matching::ElementSelectorFlags` recorded by matching.
    /// Kept across flushes because incremental restyle reads the parent's
    /// `HAS_SLOW_SELECTOR*` and `HAS_EMPTY_SELECTOR` bits long after the
    /// matching that set them. The flags type has no `Default`, hence the bits.
    pub selector_flags: Cell<usize>,
    /// Restyle hint and dirtiness bits for the incremental path.
    pub restyle: Cell<RestyleBits>,
    /// The parsed `style` attribute.
    pub style_attr: StyleAttrSlot,
    /// The synthesized presentational hints.
    pub hints: HintsSlot,
    /// The element's state flags and the `Document::mutation_generation` they
    /// were computed at. Stylo asks for the state of an element once per
    /// style-sharing candidate it is compared with, and computing it reads
    /// attributes and walks ancestors.
    pub state: Cell<Option<(u64, u64)>>,
    /// `Document::mutation_generation` at the last [`StyleStore::refresh`].
    /// The cache readers refuse to run when it is not the document's current
    /// generation, because a skipped refresh would serve stale styles.
    pub refreshed_generation: Cell<u64>,
}

/// Style state for every arena slot, owned by the engine and kept across
/// flushes. Entries are keyed by slot *and* generation: a recycled slot gets a
/// fresh entry, so a new node never sees the previous occupant's styles.
#[derive(Debug, Default)]
pub struct StyleStore {
    entries: Vec<StoreEntry>,
    /// The base URL every cached `url()` was resolved against.
    base: Option<Url>,
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

    /// Drops every cached `style=""` block and hint block that no longer
    /// matches `doc`, and stamps every entry as current. The engine calls it
    /// before each flush, after [`StyleStore::claim`]: the caches are filled
    /// through shared references during the traversal and cannot be replaced
    /// there, and the readers panic on an entry that was not stamped.
    ///
    /// A `style` block survives a document mutation when its attribute text is
    /// unchanged, so unrelated edits do not reparse every inline style. A
    /// changed base URL drops every cache, since `url()` values resolve
    /// against it.
    ///
    /// The caches are plain cells: the store is for one thread at a time.
    pub fn refresh(&mut self, doc: &Document) {
        let generation = doc.mutation_generation();
        let base = style_attr::document_base(doc);
        let base_changed = self.base.as_ref() != Some(&base);
        self.base = Some(base);
        for (slot, entry) in self.entries.iter_mut().enumerate() {
            entry.refreshed_generation.set(generation);
            let attr = &mut entry.style_attr;
            if base_changed {
                attr.source.take();
                attr.block.take();
                entry.hints.block.take();
                continue;
            }
            if attr.block.get().is_some() && attr.source_generation.get() != generation {
                let node = NodeId(slot as u32);
                let current = doc.attribute(node, atoms::STYLE);
                if attr.source.get().map(|text| &**text) == current {
                    attr.source_generation.set(generation);
                } else {
                    attr.source.take();
                    attr.block.take();
                }
            }
            if entry.hints.source_generation.get() != generation {
                entry.hints.block.take();
            }
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
