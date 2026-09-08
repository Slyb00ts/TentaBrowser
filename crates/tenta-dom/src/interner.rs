//! Document-local interning of attribute *values* (`docs/spec/04-dom-and-css.md` §2,
//! `docs/spec/02-architecture.md` §7).
//!
//! `tenta_core::Atom` is a process-wide table of **names**: element and
//! attribute local names, namespaces and known keywords. The set of those is
//! bounded by the markup vocabulary. Ids and classes are not: they come
//! straight off the network and a page can mint as many as it likes, so
//! interning them process-wide is a memory-exhaustion vector
//! (`docs/spec/19-security.md` §5) and outlives the document that produced
//! them.
//!
//! This interner is owned by one [`crate::Document`]. Handles are 32-bit, so
//! the element columns stay the same width they were with atoms, entries are
//! reference counted and reclaimed as soon as the last element stops using
//! them, and everything left dies with the arena.

use std::sync::Arc;

use hashbrown::HashMap;

/// Handle into a [`DocInterner`]. Only meaningful for the document that
/// produced it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct LocalName(u32);

impl LocalName {
    /// The "no value" handle: an element with no id, or an empty class list.
    pub const NONE: LocalName = LocalName(u32::MAX);

    /// Whether the handle names a string.
    #[inline]
    pub const fn is_some(self) -> bool {
        self.0 != u32::MAX
    }

    /// Raw slot, stable while the handle is held.
    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

impl Default for LocalName {
    fn default() -> Self {
        LocalName::NONE
    }
}

/// A per-document string table for id and class values.
#[derive(Default)]
pub struct DocInterner {
    /// Slot contents; a freed slot holds `None`.
    strings: Vec<Option<Arc<str>>>,
    /// Live references per slot, from element columns only.
    refs: Vec<u32>,
    map: HashMap<Arc<str>, u32>,
    free: Vec<u32>,
    /// Bytes of string data currently held, counted once per distinct string.
    bytes: usize,
}

impl DocInterner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Interns `s`, taking a reference to it.
    pub fn intern(&mut self, s: &str) -> LocalName {
        if let Some(&slot) = self.map.get(s) {
            self.refs[slot as usize] += 1;
            return LocalName(slot);
        }
        let shared: Arc<str> = Arc::from(s);
        let slot = match self.free.pop() {
            Some(slot) => {
                self.strings[slot as usize] = Some(shared.clone());
                self.refs[slot as usize] = 1;
                slot
            }
            None => {
                let slot = u32::try_from(self.strings.len()).expect("document name table overflow");
                assert!(slot != LocalName::NONE.0, "document name table overflow");
                self.strings.push(Some(shared.clone()));
                self.refs.push(1);
                slot
            }
        };
        self.bytes += s.len();
        self.map.insert(shared, slot);
        LocalName(slot)
    }

    /// Looks a string up without interning it. Query paths use this so a
    /// selector for a name no element carries costs nothing.
    #[inline]
    pub fn get(&self, s: &str) -> Option<LocalName> {
        self.map.get(s).map(|&slot| LocalName(slot))
    }

    /// Takes another reference to an already-interned handle.
    pub fn retain(&mut self, name: LocalName) {
        if name.is_some() {
            self.refs[name.0 as usize] += 1;
        }
    }

    /// Drops a reference, reclaiming the slot when the last one goes.
    pub fn release(&mut self, name: LocalName) {
        if !name.is_some() {
            return;
        }
        let slot = name.0 as usize;
        debug_assert!(self.refs[slot] > 0, "document name refcount underflow");
        self.refs[slot] -= 1;
        if self.refs[slot] == 0 {
            if let Some(text) = self.strings[slot].take() {
                self.bytes -= text.len();
                self.map.remove(&text);
            }
            self.free.push(name.0);
        }
    }

    /// The string behind a handle, or `""` for [`LocalName::NONE`].
    #[inline]
    pub fn resolve(&self, name: LocalName) -> &str {
        if !name.is_some() {
            return "";
        }
        self.strings[name.0 as usize].as_deref().unwrap_or_default()
    }

    /// Distinct live strings.
    #[inline]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Bytes charged to this table: the string data plus the slot and map
    /// bookkeeping (`docs/spec/04-dom-and-css.md` §8).
    pub fn memory_bytes(&self) -> usize {
        self.bytes
            + self.strings.len() * (size_of::<Option<Arc<str>>>() + size_of::<u32>())
            + self.map.capacity() * (size_of::<Arc<str>>() + size_of::<u32>() + 1)
            + self.free.capacity() * size_of::<u32>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_by_value_and_reference_counted() {
        let mut interner = DocInterner::new();
        let a = interner.intern("main");
        let b = interner.intern("main");
        assert_eq!(a, b);
        assert_eq!(interner.resolve(a), "main");
        assert_eq!(interner.len(), 1);

        interner.release(a);
        assert_eq!(interner.len(), 1, "one reference is still held");
        interner.release(b);
        assert_eq!(interner.len(), 0, "the last release reclaims the slot");
        assert_eq!(interner.get("main"), None);
    }

    #[test]
    fn reclaimed_slots_are_reused_so_the_table_does_not_grow() {
        let mut interner = DocInterner::new();
        for index in 0..1000 {
            let name = interner.intern(&format!("value-{index}"));
            interner.release(name);
        }
        assert_eq!(interner.len(), 0);
        assert_eq!(
            interner.strings.len(),
            1,
            "every value reused the one free slot"
        );
        assert!(interner.memory_bytes() > 0);
    }

    #[test]
    fn the_none_handle_resolves_to_the_empty_string() {
        let mut interner = DocInterner::new();
        assert!(!LocalName::NONE.is_some());
        assert_eq!(interner.resolve(LocalName::NONE), "");
        interner.release(LocalName::NONE);
        interner.retain(LocalName::NONE);
        assert!(interner.is_empty());
    }
}
