//! The timer heap for `setTimeout`/`setInterval` (`docs/spec/05-javascript.md`
//! §5).
//!
//! Timers are ordered by deadline, ties broken by insertion order so equal
//! deadlines fire in the order they were scheduled (the HTML spec's ordering).
//! Nested timers are clamped like Chromium: once the timer nesting level is
//! above five, a requested timeout below 4 ms is raised to 4 ms, which is what
//! keeps a `setTimeout(f, 0)` recursion from busy-looping.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use super::time::MonoTime;

/// The HTML "maximum nesting level" past which short timeouts are clamped.
const CLAMP_NESTING_LEVEL: u32 = 5;
/// The floor a clamped timeout is raised to.
const CLAMP_MIN_MS: u64 = 4;

/// The delay a timer asked for at `nesting_level` actually waits: the Chromium
/// nesting clamp raises a short delay to 4 ms once timers are nested more than
/// five deep.
pub fn effective_delay_ms(delay_ms: u64, nesting_level: u32) -> u64 {
    if nesting_level > CLAMP_NESTING_LEVEL {
        delay_ms.max(CLAMP_MIN_MS)
    } else {
        delay_ms
    }
}

/// Identifies a scheduled timer. Also the value `setTimeout` returns to script.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct TimerId(u64);

impl TimerId {
    /// The integer handle exposed to script.
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// Reconstructs a `TimerId` from the integer handle script holds, so
    /// `clearTimeout(id)` can round-trip a value that crossed the JS boundary.
    /// An id that names no live timer is harmless: cancelling it is a no-op.
    pub fn from_u64(value: u64) -> Self {
        TimerId(value)
    }
}

/// One scheduled timer on the heap.
#[derive(Clone, Copy, Debug)]
struct Entry {
    deadline: MonoTime,
    /// Insertion order, so equal deadlines keep FIFO order.
    seq: u64,
    id: TimerId,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.seq == other.seq
    }
}
impl Eq for Entry {}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        // `BinaryHeap` is a max-heap; reverse so the earliest deadline (then the
        // lowest sequence) is greatest and pops first.
        other
            .deadline
            .cmp(&self.deadline)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A monotonic clamp-and-order timer queue. It does not hold the callbacks; the
/// caller keeps a `TimerId -> callback` map and fires them in the order
/// [`TimerQueue::take_expired`] returns.
#[derive(Debug, Default)]
pub struct TimerQueue {
    heap: BinaryHeap<Entry>,
    live: std::collections::HashSet<TimerId>,
    next_id: u64,
    next_seq: u64,
}

impl TimerQueue {
    /// A new, empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether any timer is still scheduled.
    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Drops every scheduled timer without resetting the id or insertion-order
    /// sequences. A handle issued before this call must never name a timer
    /// scheduled afterwards.
    pub(crate) fn clear(&mut self) {
        self.heap.clear();
        self.live.clear();
    }

    /// Schedules a timer `delay_ms` after `now`, applying the Chromium nesting
    /// clamp, and returns its id. `nesting_level` is the current timer nesting
    /// level (1 for a timer scheduled from a top-level task).
    pub fn schedule(&mut self, now: MonoTime, delay_ms: u64, nesting_level: u32) -> TimerId {
        let effective_ms = effective_delay_ms(delay_ms, nesting_level);
        let deadline = now.saturating_add(std::time::Duration::from_millis(effective_ms));

        let id = TimerId(self.next_id);
        self.next_id += 1;
        let seq = self.next_seq;
        self.next_seq += 1;

        self.heap.push(Entry { deadline, seq, id });
        self.live.insert(id);
        id
    }

    /// Re-arms an existing `id` for another `delay_ms` after `now`, applying the
    /// same nesting clamp as [`TimerQueue::schedule`]. Used to repeat a
    /// `setInterval` timer under its original id so the handle script holds stays
    /// valid across firings; the caller passes the id [`TimerQueue::take_expired`]
    /// just returned, whose previous heap slot is already gone.
    pub fn reschedule(&mut self, id: TimerId, now: MonoTime, delay_ms: u64, nesting_level: u32) {
        let effective_ms = effective_delay_ms(delay_ms, nesting_level);
        let deadline = now.saturating_add(std::time::Duration::from_millis(effective_ms));
        let seq = self.next_seq;
        self.next_seq += 1;
        self.heap.push(Entry { deadline, seq, id });
        self.live.insert(id);
    }

    /// Whether `id` names a live (scheduled, not cancelled) timer.
    pub fn is_live(&self, id: TimerId) -> bool {
        self.live.contains(&id)
    }

    /// Cancels a timer (`clearTimeout`/`clearInterval`). Firing it later becomes
    /// a no-op. Its heap slot is reclaimed lazily when it reaches the top.
    pub fn cancel(&mut self, id: TimerId) {
        self.live.remove(&id);
    }

    /// The earliest live deadline, or `None` if no timer is scheduled. Prunes
    /// cancelled entries it finds at the top.
    pub fn next_deadline(&mut self) -> Option<MonoTime> {
        self.prune_cancelled();
        self.heap.peek().map(|entry| entry.deadline)
    }

    /// Removes and returns the ids of every timer due at or before `now`, in
    /// fire order (deadline, then insertion). Cancelled timers are dropped, not
    /// returned.
    pub fn take_expired(&mut self, now: MonoTime) -> Vec<TimerId> {
        let mut fired = Vec::new();
        while let Some(entry) = self.heap.peek() {
            if entry.deadline > now {
                break;
            }
            let entry = self.heap.pop().expect("peek returned an entry");
            if self.live.remove(&entry.id) {
                fired.push(entry.id);
            }
        }
        fired
    }

    /// Drops cancelled entries sitting at the top of the heap so
    /// [`TimerQueue::next_deadline`] reports a deadline that will actually fire.
    fn prune_cancelled(&mut self) {
        while let Some(entry) = self.heap.peek() {
            if self.live.contains(&entry.id) {
                break;
            }
            self.heap.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deeply_nested_zero_timeout_is_clamped_to_four_milliseconds() {
        let mut timers = TimerQueue::new();
        let now = MonoTime::ZERO;
        // Nesting within the limit: no clamp, fires immediately.
        let shallow = timers.schedule(now, 0, 3);
        // Nesting past the limit: clamped up to 4 ms.
        let deep = timers.schedule(now, 0, 6);

        // At t=0 only the unclamped timer is due.
        assert_eq!(timers.take_expired(now), vec![shallow]);
        // The clamped one fires at 4 ms.
        assert_eq!(
            timers.take_expired(now.saturating_add(std::time::Duration::from_millis(4))),
            vec![deep]
        );
    }

    #[test]
    fn equal_deadlines_fire_in_insertion_order() {
        let mut timers = TimerQueue::new();
        let now = MonoTime::ZERO;
        let first = timers.schedule(now, 10, 1);
        let second = timers.schedule(now, 10, 1);
        let third = timers.schedule(now, 10, 1);
        let due = timers.take_expired(now.saturating_add(std::time::Duration::from_millis(10)));
        assert_eq!(due, vec![first, second, third]);
    }

    #[test]
    fn earlier_deadlines_fire_first_regardless_of_insertion_order() {
        let mut timers = TimerQueue::new();
        let now = MonoTime::ZERO;
        let late = timers.schedule(now, 100, 1);
        let early = timers.schedule(now, 5, 1);
        let due = timers.take_expired(now.saturating_add(std::time::Duration::from_millis(200)));
        assert_eq!(due, vec![early, late]);
    }

    #[test]
    fn a_cancelled_timer_never_fires() {
        let mut timers = TimerQueue::new();
        let now = MonoTime::ZERO;
        let kept = timers.schedule(now, 10, 1);
        let dropped = timers.schedule(now, 10, 1);
        timers.cancel(dropped);
        let due = timers.take_expired(now.saturating_add(std::time::Duration::from_millis(10)));
        assert_eq!(due, vec![kept]);
        assert!(timers.is_empty());
    }

    #[test]
    fn next_deadline_skips_cancelled_timers_at_the_top() {
        let mut timers = TimerQueue::new();
        let now = MonoTime::ZERO;
        let soon = timers.schedule(now, 5, 1);
        let _later = timers.schedule(now, 50, 1);
        timers.cancel(soon);
        // The 5 ms timer is cancelled, so the next real deadline is the 50 ms one.
        assert_eq!(
            timers.next_deadline(),
            Some(now.saturating_add(std::time::Duration::from_millis(50)))
        );
    }

    #[test]
    fn an_empty_queue_has_no_next_deadline() {
        let mut timers = TimerQueue::new();
        assert!(timers.is_empty());
        assert_eq!(timers.next_deadline(), None);
    }
}
