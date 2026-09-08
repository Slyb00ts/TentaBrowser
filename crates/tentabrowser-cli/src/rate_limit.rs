//! Rate limit handling for the scrape command, including Retry-After ceiling.
//!
//! When `Retry-After` exceeds a configured ceiling (max of --timeout and 60 s),
//! the host is considered overwhelmed and its remaining queued URLs are dropped.

use std::time::Duration;

/// The maximum `Retry-After` delay to honour; larger values mean the host
/// should be given up on. Calculated as the max of the configured timeout
/// and 60 seconds, giving the host time to recover without permanently
/// blocking the crawl.
pub fn retry_after_ceiling(timeout: Duration) -> Duration {
    timeout.max(Duration::from_secs(60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceiling_is_max_of_timeout_and_60_seconds() {
        // Timeout below 60s: ceiling is 60s
        assert_eq!(
            retry_after_ceiling(Duration::from_secs(30)),
            Duration::from_secs(60)
        );
        // Timeout at 60s: ceiling is 60s
        assert_eq!(
            retry_after_ceiling(Duration::from_secs(60)),
            Duration::from_secs(60)
        );
        // Timeout above 60s: ceiling is the timeout
        assert_eq!(
            retry_after_ceiling(Duration::from_secs(120)),
            Duration::from_secs(120)
        );
    }
}
