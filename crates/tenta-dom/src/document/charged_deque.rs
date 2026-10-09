//! A bounded queue whose native capacity charge follows its backing buffer.
//!
//! MutationObserver records and the pending-observer FIFO can leave their
//! journal through `takeRecords()` or a notify snapshot. Returning a bare
//! VecDeque would strand its reservation in the journal while the allocation
//! remains live elsewhere.

use std::collections::VecDeque;
use std::mem::size_of;
use std::ops::Deref;

use tenta_core::{NativeMemoryBudget, NativeMemoryComponent, NativeMemoryReservation};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityError {
    Budget,
    Allocation,
}

#[derive(Debug)]
pub struct ChargedDeque<T> {
    queue: VecDeque<T>,
    budget: NativeMemoryBudget,
    // Last field: the charge remains through the queue's element and backing
    // buffer destruction, including when this owner is moved out of a journal.
    reservation: NativeMemoryReservation,
}

impl<T> ChargedDeque<T> {
    pub fn new(budget: NativeMemoryBudget) -> Self {
        let reservation = budget.begin(NativeMemoryComponent::MutationObserver);
        Self {
            queue: VecDeque::new(),
            budget,
            reservation,
        }
    }

    pub fn try_reserve(&mut self, additional: usize) -> Result<(), CapacityError> {
        if self.budget.first_failure().is_some() {
            return Err(CapacityError::Budget);
        }
        let needed = self.queue.len().checked_add(additional).ok_or_else(|| {
            self.reservation.fail_allocation();
            CapacityError::Allocation
        })?;
        if needed <= self.queue.capacity() {
            return Ok(());
        }

        // Keep the old allocation and charge alive while the replacement is
        // prepared. Prefer geometric growth, but its refusal is not a page
        // failure: the required minimum may still fit. Both optional charges
        // and allocator rounding use the same fallback to exact-needed size.
        let preferred = self.queue.capacity().saturating_mul(2).max(4).max(needed);
        let mut requested = preferred;
        loop {
            let speculative = requested > needed;
            let mut new_charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
            let Some(minimum_bytes) = requested.checked_mul(size_of::<T>()) else {
                if speculative {
                    requested = needed;
                    continue;
                }
                new_charge.fail_allocation();
                return Err(CapacityError::Allocation);
            };
            let admitted = if speculative {
                new_charge.try_charge_optional(minimum_bytes)
            } else {
                new_charge.charge(minimum_bytes)
            };
            if !admitted {
                if speculative {
                    requested = needed;
                    continue;
                }
                return Err(CapacityError::Budget);
            }
            let mut replacement = VecDeque::new();
            if replacement.try_reserve_exact(requested).is_err() {
                if speculative {
                    requested = needed;
                    continue;
                }
                new_charge.fail_allocation();
                return Err(CapacityError::Allocation);
            }
            let Some(actual_bytes) = replacement.capacity().checked_mul(size_of::<T>()) else {
                if speculative {
                    requested = needed;
                    continue;
                }
                new_charge.fail_allocation();
                return Err(CapacityError::Allocation);
            };
            if actual_bytes > minimum_bytes {
                let extra = actual_bytes - minimum_bytes;
                let admitted = if speculative {
                    new_charge.try_charge_optional(extra)
                } else {
                    new_charge.charge(extra)
                };
                if !admitted {
                    if speculative {
                        requested = needed;
                        continue;
                    }
                    return Err(CapacityError::Budget);
                }
            }

            replacement.extend(self.queue.drain(..));
            let old_queue = std::mem::replace(&mut self.queue, replacement);
            let old_charge = std::mem::replace(&mut self.reservation, new_charge);
            drop(old_queue);
            drop(old_charge);
            return Ok(());
        }
    }

    /// The caller must have reserved enough room before its DOM mutation.
    pub fn push_back_prepared(&mut self, value: T) {
        assert!(self.queue.len() < self.queue.capacity());
        self.queue.push_back(value);
    }

    pub fn front_mut(&mut self) -> Option<&mut T> {
        self.queue.front_mut()
    }

    pub fn pop_front(&mut self) -> Option<T> {
        self.queue.pop_front()
    }

    pub fn clear(&mut self) {
        self.queue.clear();
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        self.queue.retain(|value| keep(value));
    }

    pub fn iter_mut(&mut self) -> std::collections::vec_deque::IterMut<'_, T> {
        self.queue.iter_mut()
    }

    pub fn take(&mut self) -> Self {
        std::mem::replace(self, Self::new(self.budget.clone()))
    }

    #[cfg(test)]
    pub fn retained_buffer_bytes(&self) -> usize {
        self.queue.capacity().saturating_mul(size_of::<T>())
    }
}

impl<T> Deref for ChargedDeque<T> {
    type Target = VecDeque<T>;

    fn deref(&self) -> &Self::Target {
        &self.queue
    }
}

pub struct ChargedDequeIntoIter<T> {
    iter: std::collections::vec_deque::IntoIter<T>,
    // The iterator owns the original allocation until its final drop.
    _reservation: NativeMemoryReservation,
}

impl<T> Iterator for ChargedDequeIntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<T> ExactSizeIterator for ChargedDequeIntoIter<T> {}

impl<T> IntoIterator for ChargedDeque<T> {
    type Item = T;
    type IntoIter = ChargedDequeIntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        let Self {
            queue,
            budget: _,
            reservation,
        } = self;
        ChargedDequeIntoIter {
            iter: queue.into_iter(),
            _reservation: reservation,
        }
    }
}

impl<'a, T> IntoIterator for &'a ChargedDeque<T> {
    type Item = &'a T;
    type IntoIter = std::collections::vec_deque::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.queue.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_charge_follows_taken_buffer_and_partially_consumed_iterator() {
        let budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
        let mut source = ChargedDeque::new(budget.clone());
        source.try_reserve(12).unwrap();
        for value in 0..12usize {
            source.push_back_prepared(value);
        }
        let retained = budget.used(NativeMemoryComponent::MutationObserver);
        assert!(retained >= source.retained_buffer_bytes());
        let moved = source.take();
        drop(source);
        assert_eq!(
            budget.used(NativeMemoryComponent::MutationObserver),
            retained
        );
        let mut items = moved.into_iter();
        assert_eq!(items.next(), Some(0));
        assert_eq!(
            budget.used(NativeMemoryComponent::MutationObserver),
            retained
        );
        drop(items);
        assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
    }

    #[test]
    fn refused_growth_preserves_existing_records_and_charge() {
        let budget = NativeMemoryBudget::with_limits(256, 0, 256, 0);
        let mut queue = ChargedDeque::new(budget.clone());
        queue.try_reserve(1).unwrap();
        queue.push_back_prepared(7usize);
        let retained = budget.used(NativeMemoryComponent::MutationObserver);
        assert_eq!(queue.try_reserve(100), Err(CapacityError::Budget));
        assert_eq!(queue.front(), Some(&7));
        assert_eq!(
            budget.used(NativeMemoryComponent::MutationObserver),
            retained
        );
        drop(queue);
        assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
        assert!(budget.first_failure().is_some());
    }

    #[test]
    fn geometric_growth_bounds_replacements_under_repeated_single_pushes() {
        let budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
        let mut queue = ChargedDeque::new(budget.clone());
        let mut replacements = 0usize;
        let mut capacity = 0usize;
        for value in 0..1000usize {
            queue.try_reserve(1).unwrap();
            if queue.capacity() != capacity {
                replacements += 1;
                capacity = queue.capacity();
            }
            queue.push_back_prepared(value);
        }
        assert_eq!(queue.len(), 1000);
        assert!(
            replacements <= 12,
            "geometric growth had {replacements} replacements"
        );
        assert!(budget.first_failure().is_none());
        drop(queue);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn speculative_refusal_falls_back_without_poisoning_the_page() {
        let budget = NativeMemoryBudget::with_limits(260, 0, 260, 0);
        let mut queue = ChargedDeque::new(budget.clone());
        queue.try_reserve(100).unwrap();
        let initial_capacity = queue.capacity();
        for value in 0..initial_capacity {
            queue.push_back_prepared(value as u8);
        }
        queue.try_reserve(1).unwrap();
        assert!(queue.capacity() > initial_capacity);
        assert!(budget.first_failure().is_none());
    }
}
