//! Page-budgeted growth for the journal's retained hash-table allocations.
//!
//! The caller owns each table and its reservation for the same lifetime. The
//! reservation field must follow the table field, so table destruction occurs
//! first. Growth allocates and charges a replacement while the old table is
//! still live, then moves entries without a further allocation.

use std::hash::Hash;
use std::mem::{align_of, size_of};

use hashbrown::{HashMap, HashSet};
use tenta_core::{NativeMemoryBudget, NativeMemoryComponent, NativeMemoryReservation};

use super::charged_deque::CapacityError;

/// Upper bound for hashbrown 0.16.1's `RawTable` allocation requested by an
/// empty table's `try_reserve`. Its SSE2, NEON, and LSX groups are 16 bytes;
/// the generic group is no wider. Precharging before allocation preserves the
/// old+new peak. Keep the small-table and 7/8-load rules aligned with that
/// pinned version, and check the actual allocation after every reserve.
fn table_allocation_bound<T>(capacity: usize) -> Option<usize> {
    const GROUP_WIDTH: usize = 16;
    let buckets = if capacity < 15 {
        // Every small table uses at most 16 buckets, including zero-sized or
        // one-byte entries whose control alignment requires the larger table.
        16
    } else {
        capacity
            .checked_mul(8)?
            .checked_div(7)?
            .checked_next_power_of_two()?
    };
    let control_alignment = align_of::<T>().max(GROUP_WIDTH);
    let bucket_bytes = size_of::<T>().checked_mul(buckets)?;
    let control_offset =
        bucket_bytes.checked_add(control_alignment - 1)? & !(control_alignment - 1);
    let bytes = control_offset
        .checked_add(buckets)?
        .checked_add(GROUP_WIDTH)?;
    let maximum = (isize::MAX as usize).checked_sub(control_alignment - 1)?;
    (bytes <= maximum).then_some(bytes)
}

pub fn reserve_map<K: Eq + Hash, V>(
    map: &mut HashMap<K, V>,
    charge: &mut NativeMemoryReservation,
    budget: &NativeMemoryBudget,
    additional: usize,
) -> Result<(), CapacityError> {
    if budget.first_failure().is_some() {
        return Err(CapacityError::Budget);
    }
    let needed = map.len().checked_add(additional).ok_or_else(|| {
        charge.fail_allocation();
        CapacityError::Allocation
    })?;
    if needed <= map.capacity() {
        return Ok(());
    }
    let preferred = map.capacity().saturating_mul(2).max(4).max(needed);
    let mut requested = preferred;
    loop {
        let speculative = requested > needed;
        let mut next_charge = budget.begin(NativeMemoryComponent::MutationObserver);
        let Some(upper_bound) = table_allocation_bound::<(K, V)>(requested) else {
            if speculative {
                requested = needed;
                continue;
            }
            next_charge.fail_allocation();
            return Err(CapacityError::Allocation);
        };
        let admitted = if speculative {
            next_charge.try_charge_optional(upper_bound)
        } else {
            next_charge.charge(upper_bound)
        };
        if !admitted {
            if speculative {
                requested = needed;
                continue;
            }
            return Err(CapacityError::Budget);
        }
        let mut replacement = HashMap::new();
        if replacement.try_reserve(requested).is_err() {
            if speculative {
                requested = needed;
                continue;
            }
            next_charge.fail_allocation();
            return Err(CapacityError::Allocation);
        }
        let actual_bytes = replacement.allocation_size();
        if actual_bytes > upper_bound {
            // A changed hashbrown layout must not publish an undercharged
            // table. The replacement is dropped before returning.
            next_charge.fail_allocation();
            return Err(CapacityError::Allocation);
        }
        replacement.extend(map.drain());
        let old_map = std::mem::replace(map, replacement);
        let old_charge = std::mem::replace(charge, next_charge);
        drop(old_map);
        drop(old_charge);
        return Ok(());
    }
}

pub fn reserve_set<K: Eq + Hash>(
    set: &mut HashSet<K>,
    charge: &mut NativeMemoryReservation,
    budget: &NativeMemoryBudget,
    additional: usize,
) -> Result<(), CapacityError> {
    if budget.first_failure().is_some() {
        return Err(CapacityError::Budget);
    }
    let needed = set.len().checked_add(additional).ok_or_else(|| {
        charge.fail_allocation();
        CapacityError::Allocation
    })?;
    if needed <= set.capacity() {
        return Ok(());
    }
    let preferred = set.capacity().saturating_mul(2).max(4).max(needed);
    let mut requested = preferred;
    loop {
        let speculative = requested > needed;
        let mut next_charge = budget.begin(NativeMemoryComponent::MutationObserver);
        // hashbrown 0.16.1 represents HashSet<K> as HashMap<K, ()>.
        let Some(upper_bound) = table_allocation_bound::<(K, ())>(requested) else {
            if speculative {
                requested = needed;
                continue;
            }
            next_charge.fail_allocation();
            return Err(CapacityError::Allocation);
        };
        let admitted = if speculative {
            next_charge.try_charge_optional(upper_bound)
        } else {
            next_charge.charge(upper_bound)
        };
        if !admitted {
            if speculative {
                requested = needed;
                continue;
            }
            return Err(CapacityError::Budget);
        }
        let mut replacement = HashSet::new();
        if replacement.try_reserve(requested).is_err() {
            if speculative {
                requested = needed;
                continue;
            }
            next_charge.fail_allocation();
            return Err(CapacityError::Allocation);
        }
        let actual_bytes = replacement.allocation_size();
        if actual_bytes > upper_bound {
            next_charge.fail_allocation();
            return Err(CapacityError::Allocation);
        }
        replacement.extend(set.drain());
        let old_set = std::mem::replace(set, replacement);
        let old_charge = std::mem::replace(charge, next_charge);
        drop(old_set);
        drop(old_charge);
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Eq, Hash, PartialEq)]
    #[repr(align(64))]
    struct Aligned(u8);

    fn check_bound<K: Eq + Hash, V>() {
        for capacity in [1, 3, 7, 14, 15, 16, 28, 1000] {
            let mut map = HashMap::<K, V>::new();
            map.try_reserve(capacity).unwrap();
            assert!(
                map.allocation_size() <= table_allocation_bound::<(K, V)>(capacity).unwrap(),
                "map bound for capacity {capacity}"
            );
            let mut set = HashSet::<K>::new();
            set.try_reserve(capacity).unwrap();
            assert!(
                set.allocation_size() <= table_allocation_bound::<(K, ())>(capacity).unwrap(),
                "set bound for capacity {capacity}"
            );
        }
    }

    #[test]
    fn preallocation_bound_covers_small_zst_high_alignment_and_load_transition() {
        check_bound::<(), ()>();
        check_bound::<u8, u8>();
        check_bound::<Aligned, u8>();
        check_bound::<u64, Aligned>();
    }

    #[test]
    fn repeated_map_and_set_growth_retains_exact_allocations() {
        let budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
        let mut map = HashMap::new();
        let mut set = HashSet::new();
        let mut map_charge = budget.begin(NativeMemoryComponent::MutationObserver);
        let mut set_charge = budget.begin(NativeMemoryComponent::MutationObserver);
        let mut map_growths = 0;
        let mut set_growths = 0;
        for id in 0..1000usize {
            let old_map = map.capacity();
            let old_set = set.capacity();
            reserve_map(&mut map, &mut map_charge, &budget, 1).unwrap();
            reserve_set(&mut set, &mut set_charge, &budget, 1).unwrap();
            map_growths += usize::from(map.capacity() != old_map);
            set_growths += usize::from(set.capacity() != old_set);
            map.insert(id, id);
            set.insert(id);
        }
        assert!(map_growths <= 12);
        assert!(set_growths <= 12);
        assert!(
            budget.used(NativeMemoryComponent::MutationObserver)
                >= map.allocation_size() + set.allocation_size()
        );
        drop(map);
        drop(set);
        drop(map_charge);
        drop(set_charge);
        assert_eq!(budget.total_used(), 0);
    }
}
