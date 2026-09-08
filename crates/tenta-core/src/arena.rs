//! Generational arena: a `Vec`-backed slab whose keys carry a generation so
//! that a stale key never aliases a reused slot. Used for pages, contexts,
//! frames and every other table of objects addressed by id.
//!
//! The DOM (`docs/spec/04-dom-and-css.md` §1) uses its own struct-of-arrays
//! layout with the same generation rule; this type is the general-purpose
//! building block for everything else.

use std::fmt;

/// Key into an [`Arena`]: slot index plus the generation it was issued in.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    index: u32,
    generation: u32,
}

impl Key {
    #[inline]
    pub const fn index(self) -> u32 {
        self.index
    }

    #[inline]
    pub const fn generation(self) -> u32 {
        self.generation
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({}v{})", self.index, self.generation)
    }
}

enum Slot<T> {
    Occupied {
        generation: u32,
        value: T,
    },
    Free {
        generation: u32,
        next_free: Option<u32>,
    },
}

/// A generational arena.
pub struct Arena<T> {
    slots: Vec<Slot<T>>,
    free_head: Option<u32>,
    len: usize,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Arena<T> {
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_head: None,
            len: 0,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: Vec::with_capacity(capacity),
            free_head: None,
            len: 0,
        }
    }

    /// Number of live values.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Number of slots ever allocated (live plus free). Traversal caps use
    /// this as the visited-count bound.
    #[inline]
    pub fn capacity_used(&self) -> usize {
        self.slots.len()
    }

    /// Inserts a value and returns its key.
    pub fn insert(&mut self, value: T) -> Key {
        self.len += 1;
        if let Some(index) = self.free_head {
            let slot = &mut self.slots[index as usize];
            let Slot::Free {
                generation,
                next_free,
            } = *slot
            else {
                unreachable!("free list points at an occupied slot");
            };
            self.free_head = next_free;
            *slot = Slot::Occupied { generation, value };
            return Key { index, generation };
        }
        let index = u32::try_from(self.slots.len()).expect("arena exceeds u32 slots");
        self.slots.push(Slot::Occupied {
            generation: 0,
            value,
        });
        Key {
            index,
            generation: 0,
        }
    }

    /// Removes the value at `key`, returning it if the key was live. The
    /// slot's generation is bumped so the old key can never resolve again.
    pub fn remove(&mut self, key: Key) -> Option<T> {
        let slot = self.slots.get_mut(key.index as usize)?;
        match slot {
            Slot::Occupied { generation, .. } if *generation == key.generation => {
                let next_generation = generation.wrapping_add(1);
                let old = std::mem::replace(
                    slot,
                    Slot::Free {
                        generation: next_generation,
                        next_free: self.free_head,
                    },
                );
                self.free_head = Some(key.index);
                self.len -= 1;
                match old {
                    Slot::Occupied { value, .. } => Some(value),
                    Slot::Free { .. } => unreachable!(),
                }
            }
            _ => None,
        }
    }

    #[inline]
    pub fn contains(&self, key: Key) -> bool {
        self.get(key).is_some()
    }

    #[inline]
    pub fn get(&self, key: Key) -> Option<&T> {
        match self.slots.get(key.index as usize)? {
            Slot::Occupied { generation, value } if *generation == key.generation => Some(value),
            _ => None,
        }
    }

    #[inline]
    pub fn get_mut(&mut self, key: Key) -> Option<&mut T> {
        match self.slots.get_mut(key.index as usize)? {
            Slot::Occupied { generation, value } if *generation == key.generation => Some(value),
            _ => None,
        }
    }

    /// Iterates over live entries in slot order.
    pub fn iter(&self) -> impl Iterator<Item = (Key, &T)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| match slot {
                Slot::Occupied { generation, value } => Some((
                    Key {
                        index: index as u32,
                        generation: *generation,
                    },
                    value,
                )),
                Slot::Free { .. } => None,
            })
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Key, &mut T)> {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(index, slot)| match slot {
                Slot::Occupied { generation, value } => Some((
                    Key {
                        index: index as u32,
                        generation: *generation,
                    },
                    value,
                )),
                Slot::Free { .. } => None,
            })
    }

    /// Removes every value. Generations are preserved so that keys issued
    /// before the clear stay invalid.
    pub fn clear(&mut self) {
        let slot_count = self.slots.len();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let generation = match slot {
                Slot::Occupied { generation, .. } => generation.wrapping_add(1),
                Slot::Free { generation, .. } => *generation,
            };
            *slot = Slot::Free {
                generation,
                next_free: if index + 1 < slot_count {
                    Some(index as u32 + 1)
                } else {
                    None
                },
            };
        }
        self.free_head = if self.slots.is_empty() { None } else { Some(0) };
        self.len = 0;
    }
}

impl<T> std::ops::Index<Key> for Arena<T> {
    type Output = T;

    fn index(&self, key: Key) -> &T {
        self.get(key).expect("stale or invalid arena key")
    }
}

impl<T> std::ops::IndexMut<Key> for Arena<T> {
    fn index_mut(&mut self, key: Key) -> &mut T {
        self.get_mut(key).expect("stale or invalid arena key")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_remove_roundtrip() {
        let mut arena = Arena::new();
        let a = arena.insert("a");
        let b = arena.insert("b");
        assert_eq!(arena.len(), 2);
        assert_eq!(arena[a], "a");
        assert_eq!(arena.remove(a), Some("a"));
        assert_eq!(arena.get(a), None);
        assert_eq!(arena.remove(a), None);
        assert_eq!(arena[b], "b");
        assert_eq!(arena.len(), 1);
    }

    #[test]
    fn stale_keys_do_not_alias_reused_slots() {
        let mut arena = Arena::new();
        let first = arena.insert(1);
        arena.remove(first);
        let second = arena.insert(2);
        assert_eq!(first.index(), second.index());
        assert_ne!(first.generation(), second.generation());
        assert_eq!(arena.get(first), None);
        assert_eq!(arena.get(second), Some(&2));
    }

    #[test]
    fn free_list_is_lifo_and_iteration_skips_holes() {
        let mut arena = Arena::new();
        let keys: Vec<_> = (0..5).map(|i| arena.insert(i)).collect();
        arena.remove(keys[1]);
        arena.remove(keys[3]);
        let live: Vec<_> = arena.iter().map(|(_, v)| *v).collect();
        assert_eq!(live, vec![0, 2, 4]);
        let reused = arena.insert(30);
        assert_eq!(reused.index(), 3);
        let reused = arena.insert(10);
        assert_eq!(reused.index(), 1);
        assert_eq!(arena.capacity_used(), 5);
    }

    #[test]
    fn clear_invalidates_every_key() {
        let mut arena = Arena::new();
        let k = arena.insert(1);
        arena.clear();
        assert!(arena.is_empty());
        assert!(!arena.contains(k));
        let k2 = arena.insert(2);
        assert_eq!(k2.index(), 0);
        assert_eq!(arena.get(k2), Some(&2));
    }
}
