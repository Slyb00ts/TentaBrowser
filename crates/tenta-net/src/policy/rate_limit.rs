//! Per-host politeness (`docs/spec/20-platform-behaviour.md` §11).
//!
//! Runtime-agnostic on purpose: [`HostLimiter::acquire`] answers `Ready` or
//! `WaitFor(duration)` and the caller decides how to wait, so the same
//! limiter works under tokio, under a blocking CLI loop and under tests with
//! a [`ManualClock`](super::clock::ManualClock).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use hashbrown::HashMap;
use parking_lot::Mutex;

use super::clock::{Clock, SystemClock};

/// Politeness limits applied to every host.
#[derive(Debug, Clone, Copy)]
pub struct HostLimits {
    /// Requests allowed in flight to one host at a time.
    pub per_host_concurrency: usize,
    /// Minimum spacing between two request starts to one host.
    pub min_interval: Duration,
}

impl Default for HostLimits {
    fn default() -> Self {
        Self {
            per_host_concurrency: 4,
            min_interval: Duration::ZERO,
        }
    }
}

/// The answer to an [`HostLimiter::acquire`] call.
#[derive(Debug)]
pub enum Acquire {
    /// The request may start now. The slot is held until the permit drops.
    Ready(Permit),
    /// The caller must wait at least this long and ask again.
    WaitFor(Duration),
}

impl Acquire {
    pub fn is_ready(&self) -> bool {
        matches!(self, Acquire::Ready(_))
    }

    pub fn wait(&self) -> Option<Duration> {
        match self {
            Acquire::WaitFor(duration) => Some(*duration),
            Acquire::Ready(_) => None,
        }
    }

    pub fn permit(self) -> Option<Permit> {
        match self {
            Acquire::Ready(permit) => Some(permit),
            Acquire::WaitFor(_) => None,
        }
    }
}

#[derive(Debug, Default)]
struct HostState {
    in_flight: usize,
    /// The earliest time the next request to this host may start.
    next_start: Option<SystemTime>,
    /// Extra spacing asked for by the host itself (`Crawl-delay`).
    crawl_delay: Option<Duration>,
}

#[derive(Debug)]
struct Inner {
    limits: HostLimits,
    clock: Arc<dyn Clock>,
    hosts: Mutex<HashMap<String, HostState>>,
}

/// Per-host concurrency and spacing.
#[derive(Debug, Clone)]
pub struct HostLimiter {
    inner: Arc<Inner>,
}

impl Default for HostLimiter {
    fn default() -> Self {
        Self::new(HostLimits::default())
    }
}

impl HostLimiter {
    pub fn new(limits: HostLimits) -> Self {
        Self::with_clock(limits, Arc::new(SystemClock))
    }

    pub fn with_clock(limits: HostLimits, clock: Arc<dyn Clock>) -> Self {
        Self {
            inner: Arc::new(Inner {
                limits,
                clock,
                hosts: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn limits(&self) -> HostLimits {
        self.inner.limits
    }

    pub fn now(&self) -> SystemTime {
        self.inner.clock.now()
    }

    /// Asks for permission to start a request to `host`.
    pub fn acquire(&self, host: &str) -> Acquire {
        let key = host.to_ascii_lowercase();
        let now = self.inner.clock.now();
        let mut hosts = self.inner.hosts.lock();
        let state = hosts.entry_ref(key.as_str()).or_default();

        if let Some(next) = state.next_start
            && next > now
        {
            return Acquire::WaitFor(
                next.duration_since(now)
                    .unwrap_or(Duration::ZERO)
                    .max(Duration::from_millis(1)),
            );
        }
        if state.in_flight >= self.inner.limits.per_host_concurrency {
            // No completion event exists in a pull API, so the caller is asked
            // to come back after the spacing interval (or promptly when there
            // is none).
            let retry = self.inner.limits.min_interval.max(Duration::from_millis(1));
            return Acquire::WaitFor(retry);
        }

        state.in_flight += 1;
        let spacing = self
            .inner
            .limits
            .min_interval
            .max(state.crawl_delay.unwrap_or(Duration::ZERO));
        if spacing > Duration::ZERO {
            state.next_start = Some(now + spacing);
        }
        Acquire::Ready(Permit {
            inner: Arc::clone(&self.inner),
            host: key,
        })
    }

    /// Records a `Retry-After` value received from `host` and returns the
    /// delay it asks for. Both the delta-seconds and the HTTP-date forms are
    /// accepted (RFC 9110 §10.2.3).
    pub fn note_retry_after(&self, host: &str, value: &[u8]) -> Option<Duration> {
        let now = self.inner.clock.now();
        let delay = parse_retry_after(value, now)?;
        self.delay_host(host, delay);
        Some(delay)
    }

    /// Blocks new requests to `host` for `delay`.
    pub fn delay_host(&self, host: &str, delay: Duration) {
        let key = host.to_ascii_lowercase();
        let now = self.inner.clock.now();
        let mut hosts = self.inner.hosts.lock();
        let state = hosts.entry_ref(key.as_str()).or_default();
        let until = now + delay;
        state.next_start = Some(match state.next_start {
            Some(existing) if existing > until => existing,
            _ => until,
        });
    }

    /// Applies a `robots.txt` `Crawl-delay` to a host.
    pub fn set_crawl_delay(&self, host: &str, delay: Duration) {
        let key = host.to_ascii_lowercase();
        let mut hosts = self.inner.hosts.lock();
        hosts.entry_ref(key.as_str()).or_default().crawl_delay = Some(delay);
    }

    pub fn in_flight(&self, host: &str) -> usize {
        self.inner
            .hosts
            .lock()
            .get(&host.to_ascii_lowercase())
            .map(|state| state.in_flight)
            .unwrap_or(0)
    }

    /// Drops state for hosts with nothing in flight and no pending delay.
    pub fn purge_idle(&self) -> usize {
        let now = self.inner.clock.now();
        let mut hosts = self.inner.hosts.lock();
        let before = hosts.len();
        hosts.retain(|_, state| {
            state.in_flight > 0
                || state.crawl_delay.is_some()
                || state.next_start.is_some_and(|next| next > now)
        });
        before - hosts.len()
    }
}

/// Holds one per-host slot. Dropping it releases the slot.
#[derive(Debug)]
pub struct Permit {
    inner: Arc<Inner>,
    host: String,
}

impl Permit {
    pub fn host(&self) -> &str {
        &self.host
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut hosts = self.inner.hosts.lock();
        if let Some(state) = hosts.get_mut(self.host.as_str()) {
            state.in_flight = state.in_flight.saturating_sub(1);
        }
    }
}

/// Parses `Retry-After` into a delay from `now`. A date in the past becomes
/// [`Duration::ZERO`] rather than an error, matching what browsers do.
pub fn parse_retry_after(value: &[u8], now: SystemTime) -> Option<Duration> {
    let text = std::str::from_utf8(value).ok()?.trim();
    if text.is_empty() {
        return None;
    }
    if text.bytes().all(|b| b.is_ascii_digit()) {
        return text.parse::<u64>().ok().map(Duration::from_secs);
    }
    let when = httpdate::parse_http_date(text).ok()?;
    Some(when.duration_since(now).unwrap_or(Duration::ZERO))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::clock::ManualClock;

    fn limiter(concurrency: usize, interval: Duration) -> (HostLimiter, Arc<ManualClock>) {
        let clock = Arc::new(ManualClock::epoch());
        let limiter = HostLimiter::with_clock(
            HostLimits {
                per_host_concurrency: concurrency,
                min_interval: interval,
            },
            clock.clone(),
        );
        (limiter, clock)
    }

    #[test]
    fn concurrency_is_capped_per_host() {
        let (limiter, _clock) = limiter(2, Duration::ZERO);
        let a = limiter.acquire("example.com").permit().expect("first slot");
        let b = limiter
            .acquire("example.com")
            .permit()
            .expect("second slot");
        assert_eq!(limiter.in_flight("example.com"), 2);
        assert!(!limiter.acquire("example.com").is_ready());
        // A different host has its own budget.
        assert!(limiter.acquire("other.com").is_ready());
        drop(a);
        assert_eq!(limiter.in_flight("example.com"), 1);
        assert!(limiter.acquire("example.com").is_ready());
        drop(b);
    }

    #[test]
    fn hosts_are_case_insensitive() {
        let (limiter, _clock) = limiter(1, Duration::ZERO);
        let _permit = limiter.acquire("Example.COM").permit().expect("slot");
        assert_eq!(limiter.in_flight("example.com"), 1);
        assert!(!limiter.acquire("EXAMPLE.com").is_ready());
    }

    #[test]
    fn min_interval_spaces_request_starts() {
        let (limiter, clock) = limiter(4, Duration::from_millis(500));
        let first = limiter.acquire("example.com");
        assert!(first.is_ready());
        drop(first);

        let wait = limiter.acquire("example.com").wait().expect("must wait");
        assert_eq!(wait, Duration::from_millis(500));

        clock.advance(Duration::from_millis(200));
        assert_eq!(
            limiter.acquire("example.com").wait(),
            Some(Duration::from_millis(300))
        );

        clock.advance(Duration::from_millis(300));
        assert!(limiter.acquire("example.com").is_ready());
    }

    #[test]
    fn crawl_delay_overrides_a_shorter_min_interval() {
        let (limiter, clock) = limiter(4, Duration::from_millis(100));
        limiter.set_crawl_delay("example.com", Duration::from_secs(2));
        drop(limiter.acquire("example.com").permit().expect("slot"));
        assert_eq!(
            limiter.acquire("example.com").wait(),
            Some(Duration::from_secs(2))
        );
        clock.advance(Duration::from_secs(2));
        assert!(limiter.acquire("example.com").is_ready());
    }

    #[test]
    fn retry_after_seconds_and_dates() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_777);
        assert_eq!(
            parse_retry_after(b"120", now),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            parse_retry_after(b"  30  ", now),
            Some(Duration::from_secs(30))
        );
        assert_eq!(parse_retry_after(b"0", now), Some(Duration::ZERO));
        assert_eq!(parse_retry_after(b"", now), None);
        assert_eq!(parse_retry_after(b"soon", now), None);
        assert_eq!(parse_retry_after(b"-5", now), None);

        let date = httpdate::fmt_http_date(now + Duration::from_secs(90));
        assert_eq!(
            parse_retry_after(date.as_bytes(), now),
            Some(Duration::from_secs(90))
        );
        let past = httpdate::fmt_http_date(now - Duration::from_secs(90));
        assert_eq!(
            parse_retry_after(past.as_bytes(), now),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn retry_after_blocks_the_host() {
        let (limiter, clock) = limiter(4, Duration::ZERO);
        assert!(limiter.acquire("example.com").is_ready());
        assert_eq!(
            limiter.note_retry_after("example.com", b"60"),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            limiter.acquire("example.com").wait(),
            Some(Duration::from_secs(60))
        );
        // Other hosts are unaffected.
        assert!(limiter.acquire("other.com").is_ready());
        clock.advance(Duration::from_secs(60));
        assert!(limiter.acquire("example.com").is_ready());
    }

    #[test]
    fn the_longest_pending_delay_wins() {
        let (limiter, _clock) = limiter(4, Duration::ZERO);
        limiter.delay_host("example.com", Duration::from_secs(60));
        limiter.delay_host("example.com", Duration::from_secs(10));
        assert_eq!(
            limiter.acquire("example.com").wait(),
            Some(Duration::from_secs(60))
        );
    }

    #[test]
    fn idle_hosts_are_purged() {
        let (limiter, clock) = limiter(4, Duration::from_millis(10));
        let held = limiter.acquire("busy.example").permit().expect("slot");
        drop(limiter.acquire("idle.example").permit().expect("slot"));
        clock.advance(Duration::from_secs(1));
        assert_eq!(limiter.purge_idle(), 1);
        assert_eq!(limiter.in_flight("busy.example"), 1);
        drop(held);
    }
}
