//! Partial native page-memory accounting for known host-owned workspaces.
//!
//! JS heaps, the DOM arena, network buffers, and other existing allocations
//! are not yet charged here. One page shares this ledger with its document,
//! child document contexts, JavaScript engine, and any in-flight Send wire.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use parking_lot::Mutex;

use crate::BudgetKind;

const MIB: u64 = 1024 * 1024;

/// Native producers currently accounted against a page's memory budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeMemoryComponent {
    /// Engine-neutral structured serialization, transfer and deserialization.
    StructuredClone,
    /// Undelivered and JS-owned mutation record/list native queues.
    MutationObserver,
    /// Synchronous Web Crypto scratch bytes.
    WebCrypto,
}

impl NativeMemoryComponent {
    const fn index(self) -> usize {
        match self {
            Self::StructuredClone => 0,
            Self::MutationObserver => 1,
            Self::WebCrypto => 2,
        }
    }

    /// Stable producer name used in the public budget failure.
    pub const fn name(self) -> &'static str {
        match self {
            Self::StructuredClone => "structured clone",
            Self::MutationObserver => "mutation observer",
            Self::WebCrypto => "web crypto",
        }
    }
}

/// Distinguishes an actual configured limit from a failed system allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeMemoryFailureCause {
    /// The configured page total or component sublimit was reached.
    Limit,
    /// A fallible native allocation failed after its charge was admitted.
    Allocation,
}

/// The first hard failure is sticky for the page's remaining lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeMemoryFailure {
    /// The public budget family; currently always page memory.
    pub kind: BudgetKind,
    /// The producer whose operation first crossed the bound.
    pub component: NativeMemoryComponent,
    /// Distinguishes a configured limit from an allocator failure.
    pub cause: NativeMemoryFailureCause,
}

impl NativeMemoryFailure {
    fn code(self) -> u8 {
        1 + (self.component.index() as u8) * 2
            + u8::from(self.cause == NativeMemoryFailureCause::Allocation)
    }

    fn from_code(code: u8) -> Option<Self> {
        let (component, cause) = match code {
            1 => (
                NativeMemoryComponent::StructuredClone,
                NativeMemoryFailureCause::Limit,
            ),
            2 => (
                NativeMemoryComponent::StructuredClone,
                NativeMemoryFailureCause::Allocation,
            ),
            3 => (
                NativeMemoryComponent::MutationObserver,
                NativeMemoryFailureCause::Limit,
            ),
            4 => (
                NativeMemoryComponent::MutationObserver,
                NativeMemoryFailureCause::Allocation,
            ),
            5 => (
                NativeMemoryComponent::WebCrypto,
                NativeMemoryFailureCause::Limit,
            ),
            6 => (
                NativeMemoryComponent::WebCrypto,
                NativeMemoryFailureCause::Allocation,
            ),
            _ => return None,
        };
        Some(Self {
            kind: BudgetKind::PageMemory,
            component,
            cause,
        })
    }
}

#[derive(Default)]
struct Usage {
    total: usize,
    component: [usize; 3],
}

struct Inner {
    total_limit: usize,
    component_limit: [usize; 3],
    usage: Mutex<Usage>,
    /// Native watchdog/interrupt paths only read this scalar. All writes are
    /// serialized by `usage`, preserving the actual first failure.
    first_failure: AtomicU8,
}

/// One Arc per page, shared across host producers and Send wire ownership.
#[derive(Clone)]
pub struct NativeMemoryBudget(Arc<Inner>);

impl std::fmt::Debug for NativeMemoryBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeMemoryBudget")
            .field("total_limit", &self.0.total_limit)
            .field("component_limit", &self.0.component_limit)
            .field("first_failure", &self.first_failure())
            .finish_non_exhaustive()
    }
}

impl NativeMemoryBudget {
    /// Each currently-known native producer cap is one eighth of the page
    /// budget. Zero is a hard zero, not an unlimited sentinel.
    pub fn for_page(page_memory_mb: u32) -> Self {
        let total: usize = (u64::from(page_memory_mb) * MIB)
            .try_into()
            .unwrap_or(usize::MAX);
        Self::with_limits(total, total / 8, total / 8, total / 8)
    }

    /// Allows a page configuration to lower component caps independently.
    pub fn with_limits(total: usize, clone: usize, observer: usize, crypto: usize) -> Self {
        Self(Arc::new(Inner {
            total_limit: total,
            component_limit: [clone.min(total), observer.min(total), crypto.min(total)],
            usage: Mutex::new(Usage::default()),
            first_failure: AtomicU8::new(0),
        }))
    }

    /// Starts an independently owned charge for one known native producer.
    pub fn begin(&self, component: NativeMemoryComponent) -> NativeMemoryReservation {
        NativeMemoryReservation {
            inner: Arc::clone(&self.0),
            component,
            bytes: 0,
        }
    }

    /// This query is lock-free for engine interrupt polling.
    pub fn first_failure(&self) -> Option<NativeMemoryFailure> {
        NativeMemoryFailure::from_code(self.0.first_failure.load(Ordering::Acquire))
    }

    /// The currently retained native bytes in one component.
    pub fn used(&self, component: NativeMemoryComponent) -> usize {
        self.0.usage.lock().component[component.index()]
    }

    /// The currently retained bytes charged by all known native consumers.
    pub fn total_used(&self) -> usize {
        self.0.usage.lock().total
    }

    /// Whether two handles refer to the same page accounting state.
    pub fn same_page(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// A Send, non-Clone RAII charge. Moving it into a wire preserves the source
/// page's ownership until destination decode or final drop.
pub struct NativeMemoryReservation {
    inner: Arc<Inner>,
    component: NativeMemoryComponent,
    bytes: usize,
}

impl std::fmt::Debug for NativeMemoryReservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeMemoryReservation")
            .field("component", &self.component)
            .field("bytes", &self.bytes)
            .finish()
    }
}

impl NativeMemoryReservation {
    /// The mutex covers only two counter updates and first-failure recording.
    /// Never hold it over native allocation, JS, DOM work, or payload drops.
    pub fn charge(&mut self, bytes: usize) -> bool {
        self.reserve_bytes(bytes, true)
    }

    /// Reserve optional geometric capacity without poisoning the page if it
    /// does not fit. The caller must retry with its required minimum through
    /// `charge`; only that mandatory refusal is a hard budget failure.
    pub fn try_charge_optional(&mut self, bytes: usize) -> bool {
        self.reserve_bytes(bytes, false)
    }

    fn reserve_bytes(&mut self, bytes: usize, hard: bool) -> bool {
        let mut usage = self.inner.usage.lock();
        if self.inner.first_failure.load(Ordering::Relaxed) != 0 {
            return false;
        }
        let index = self.component.index();
        let total = usage.total.checked_add(bytes);
        let component = usage.component[index].checked_add(bytes);
        if !total.is_some_and(|total| total <= self.inner.total_limit)
            || !component.is_some_and(|component| component <= self.inner.component_limit[index])
        {
            if hard {
                self.record_failure(NativeMemoryFailureCause::Limit);
            }
            return false;
        }
        usage.total = total.expect("the checked limit was accepted");
        usage.component[index] = component.expect("the checked limit was accepted");
        self.bytes += bytes;
        true
    }

    /// Records an allocation failure inside the allowed cap. The caller must
    /// use fallible allocation after each successful charge.
    pub fn fail_allocation(&self) {
        let _usage = self.inner.usage.lock();
        if self.inner.first_failure.load(Ordering::Relaxed) == 0 {
            self.record_failure(NativeMemoryFailureCause::Allocation);
        }
    }

    fn record_failure(&self, cause: NativeMemoryFailureCause) {
        self.inner.first_failure.store(
            NativeMemoryFailure {
                kind: BudgetKind::PageMemory,
                component: self.component,
                cause,
            }
            .code(),
            Ordering::Release,
        );
    }
}

impl Drop for NativeMemoryReservation {
    fn drop(&mut self) {
        let mut usage = self.inner.usage.lock();
        usage.total -= self.bytes;
        usage.component[self.component.index()] -= self.bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn nested_components_charge_one_page_and_release_without_resetting_failure() {
        let budget = NativeMemoryBudget::with_limits(10, 8, 8, 8);
        let mut clone = budget.begin(NativeMemoryComponent::StructuredClone);
        let mut observer = budget.begin(NativeMemoryComponent::MutationObserver);
        assert!(clone.charge(6));
        assert!(observer.charge(4));
        assert!(!observer.charge(1));
        assert_eq!(budget.total_used(), 10);
        assert_eq!(
            budget.first_failure(),
            Some(NativeMemoryFailure {
                kind: BudgetKind::PageMemory,
                component: NativeMemoryComponent::MutationObserver,
                cause: NativeMemoryFailureCause::Limit,
            })
        );
        drop(clone);
        drop(observer);
        assert_eq!(budget.total_used(), 0);
        assert!(
            !budget
                .begin(NativeMemoryComponent::StructuredClone)
                .charge(1)
        );
    }

    #[test]
    fn component_cap_and_allocation_cause_are_distinct() {
        let budget = NativeMemoryBudget::with_limits(100, 4, 20, 20);
        let mut clone = budget.begin(NativeMemoryComponent::StructuredClone);
        assert!(clone.charge(4));
        assert!(!clone.charge(1));
        assert_eq!(budget.total_used(), 4);
        assert_eq!(
            budget.first_failure().unwrap().cause,
            NativeMemoryFailureCause::Limit
        );
        clone.fail_allocation();
        assert_eq!(
            budget.first_failure().unwrap().cause,
            NativeMemoryFailureCause::Limit,
            "a later allocator failure cannot replace the actual first limit"
        );

        let budget = NativeMemoryBudget::with_limits(100, 20, 20, 20);
        let clone = budget.begin(NativeMemoryComponent::StructuredClone);
        clone.fail_allocation();
        assert_eq!(
            budget.first_failure().unwrap().cause,
            NativeMemoryFailureCause::Allocation
        );
    }

    #[test]
    fn web_crypto_uses_its_own_cap_and_retains_first_failure() {
        let budget = NativeMemoryBudget::with_limits(100, 80, 80, 8);
        let mut crypto = budget.begin(NativeMemoryComponent::WebCrypto);
        assert!(crypto.charge(8));
        assert_eq!(budget.used(NativeMemoryComponent::WebCrypto), 8);
        assert!(!crypto.charge(1));
        assert_eq!(
            budget.first_failure().unwrap().component,
            NativeMemoryComponent::WebCrypto
        );
        assert_eq!(
            budget.first_failure().unwrap().cause,
            NativeMemoryFailureCause::Limit
        );
        crypto.fail_allocation();
        assert_eq!(
            budget.first_failure().unwrap().cause,
            NativeMemoryFailureCause::Limit
        );
        drop(crypto);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn optional_growth_refusal_preserves_headroom_for_required_charge() {
        let budget = NativeMemoryBudget::with_limits(128, 0, 128, 0);
        let mut observer = budget.begin(NativeMemoryComponent::MutationObserver);
        assert!(observer.charge(32));
        assert!(!observer.try_charge_optional(128));
        assert_eq!(budget.first_failure(), None);
        assert!(observer.charge(64));
        assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 96);
        drop(observer);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn zero_and_send_reservation_are_explicit() {
        fn is_send<T: Send>() {}
        is_send::<NativeMemoryReservation>();
        let budget = NativeMemoryBudget::for_page(0);
        assert!(
            !budget
                .begin(NativeMemoryComponent::StructuredClone)
                .charge(1)
        );
        assert_eq!(
            budget.first_failure().unwrap().component.name(),
            "structured clone"
        );
    }

    #[test]
    fn cross_thread_admission_is_atomic_and_drop_releases_the_winner() {
        let budget = NativeMemoryBudget::with_limits(100, 100, 100, 100);
        let start = Arc::new(Barrier::new(3));
        let observed = Arc::new(Barrier::new(3));
        let release = Arc::new(Barrier::new(3));
        let results = Arc::new(Mutex::new(Vec::new()));
        let mut threads = Vec::new();
        for component in [
            NativeMemoryComponent::StructuredClone,
            NativeMemoryComponent::MutationObserver,
        ] {
            let budget = budget.clone();
            let start = Arc::clone(&start);
            let observed = Arc::clone(&observed);
            let release = Arc::clone(&release);
            let results = Arc::clone(&results);
            let reservation = budget.begin(component);
            threads.push(std::thread::spawn(move || {
                let mut reservation = reservation;
                start.wait();
                let admitted = reservation.charge(70);
                results.lock().push(admitted);
                observed.wait();
                release.wait();
            }));
        }
        start.wait();
        observed.wait();
        assert_eq!(results.lock().iter().filter(|&&ok| ok).count(), 1);
        assert_eq!(budget.total_used(), 70);
        assert_eq!(
            budget.first_failure().unwrap().cause,
            NativeMemoryFailureCause::Limit
        );
        release.wait();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(budget.total_used(), 0);
        assert!(budget.first_failure().is_some());
    }
}
