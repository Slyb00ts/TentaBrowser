//! Log setup and the redaction switch (`docs/spec/19-security.md` §7).
//!
//! Logs go to stderr so that stdout stays exactly the extraction. Query
//! strings carry session tokens on real sites, so the URL of every request
//! this process makes is rendered without one unless the developer asked for
//! the raw form with `--log-unredacted`.

use std::sync::atomic::{AtomicBool, Ordering};

use url::Url;

static UNREDACTED: AtomicBool = AtomicBool::new(false);

/// Installs the stderr subscriber and records the redaction choice.
pub fn init(level: &str, unredacted: bool) {
    UNREDACTED.store(unredacted, Ordering::Relaxed);
    let level = level
        .parse::<tracing::Level>()
        .unwrap_or(tracing::Level::WARN);
    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(std::io::stderr)
        .with_target(false)
        .without_time()
        .try_init();
}

/// A URL as it may appear in a log line.
pub fn url(url: &Url) -> String {
    if UNREDACTED.load(Ordering::Relaxed) {
        return url.to_string();
    }
    tenta_net::policy::redact::url_for_log(url)
}

/// A URL that has not been parsed yet; an unparseable string is logged as it
/// was given, because there is nothing in it to redact reliably.
pub fn url_str(url: &str) -> String {
    match Url::parse(url) {
        Ok(parsed) => self::url(&parsed),
        Err(_) => url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_string_is_dropped_unless_the_developer_asked_for_it() {
        let target = Url::parse("https://example.test/search?token=secret#frag").expect("url");

        UNREDACTED.store(false, Ordering::Relaxed);
        let redacted = url(&target);
        assert!(!redacted.contains("secret"), "{redacted}");
        assert!(redacted.contains("<redacted>"), "{redacted}");

        UNREDACTED.store(true, Ordering::Relaxed);
        assert!(url(&target).contains("token=secret"));
        UNREDACTED.store(false, Ordering::Relaxed);

        assert_eq!(url_str("not a url"), "not a url");
    }
}
