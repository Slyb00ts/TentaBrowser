//! Atom maps from `tenta_core::Atom` to Stylo atoms.
//!
//! The tables are owned by the engine, grow only, and are filled lazily: a
//! selector hook costs one indexed load instead of a string conversion.

use std::cell::OnceCell;

use style::{Atom, LocalName};
use tenta_core::Atom as TentaAtom;
use tenta_dom::Document;

/// One element's id attribute as a Stylo atom, valid for one slot occupant
/// and one document generation.
#[derive(Debug, Default)]
struct IdEntry {
    node_generation: u32,
    stamp: u64,
    atom: OnceCell<Option<Atom>>,
}

/// Persistent, grow-only atom tables.
#[derive(Debug, Default)]
pub struct AtomMaps {
    local_names: Vec<OnceCell<LocalName>>,
    ids: Vec<IdEntry>,
}

impl AtomMaps {
    /// Empty tables.
    pub fn new() -> Self {
        Self::default()
    }

    /// Prepares for a flush of `doc`: grows the tables to the current atom
    /// and slot counts and discards id entries that belong to a previous
    /// occupant of a slot or to an earlier document generation. Tables never
    /// shrink.
    pub fn begin_flush(&mut self, doc: &Document) {
        let atoms = TentaAtom::count();
        if self.local_names.len() < atoms {
            self.local_names.resize_with(atoms, OnceCell::new);
        }
        let slots = doc.slots();
        if self.ids.len() < slots {
            self.ids.resize_with(slots, IdEntry::default);
        }
        let stamp = doc.mutation_generation();
        for (slot, entry) in self.ids.iter_mut().enumerate().take(slots) {
            let generation = doc.node_ref(tenta_core::NodeId(slot as u32)).generation;
            if entry.node_generation != generation || entry.stamp != stamp {
                *entry = IdEntry {
                    node_generation: generation,
                    stamp,
                    atom: OnceCell::new(),
                };
            }
        }
    }

    /// The Stylo local name for a tag atom, converted once per atom.
    ///
    /// The atom must have been interned before the last [`AtomMaps::begin_flush`].
    pub fn local_name(&self, atom: TentaAtom) -> &LocalName {
        self.local_names[atom.raw() as usize].get_or_init(|| LocalName::from(atom.as_str()))
    }

    /// The Stylo atom of the id of arena slot `slot`, computed by `make` once
    /// per slot occupant and generation.
    pub fn id(&self, slot: usize, make: impl FnOnce() -> Option<Atom>) -> Option<&Atom> {
        self.ids[slot].atom.get_or_init(make).as_ref()
    }

    /// Number of tag atoms the table can address.
    pub fn local_name_capacity(&self) -> usize {
        self.local_names.len()
    }
}
