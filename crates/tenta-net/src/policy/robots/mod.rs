//! `robots.txt` per RFC 9309 (`docs/spec/20-platform-behaviour.md` §11).
//!
//! `tentabrowser-cli scrape` honours robots.txt by default. The parser and
//! the matcher take bytes; fetching `/robots.txt` is the caller's job, which
//! keeps this module free of IO and lets the CLI, the library API and tests
//! share one implementation.
//!
//! The path-matching rules (`*`, `$`, longest match wins, Allow breaks ties)
//! and the URL-to-path reduction follow Google's reference implementation
//! (<https://github.com/google/robotstxt>, Apache-2.0); its behaviour is what
//! site owners test their files against, and RFC 9309 was written from it.

mod matcher;

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use hashbrown::HashMap;
use parking_lot::RwLock;
use url::Url;

pub use matcher::{path_params_query, pattern_matches};

/// The default lifetime of a fetched `robots.txt`, matching Google's
/// documented 24-hour cache.
pub const DEFAULT_TTL: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleKind {
    Allow,
    Disallow,
}

#[derive(Debug, Clone)]
struct Rule {
    kind: RuleKind,
    pattern: String,
}

#[derive(Debug, Clone, Default)]
struct Group {
    /// Product tokens of the group, lowercased; `*` is the global group.
    agents: Vec<String>,
    rules: Vec<Rule>,
    crawl_delay: Option<Duration>,
}

/// A parsed `robots.txt`.
#[derive(Debug, Clone, Default)]
pub struct RobotsTxt {
    groups: Vec<Group>,
    sitemaps: Vec<String>,
}

impl RobotsTxt {
    /// Parses a `robots.txt` body. Parsing never fails: RFC 9309 §2.2 asks
    /// crawlers to ignore anything they cannot understand.
    pub fn parse(bytes: &[u8]) -> RobotsTxt {
        let text = String::from_utf8_lossy(bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let mut robots = RobotsTxt::default();
        let mut current: Option<Group> = None;
        // A group is a run of user-agent lines followed by its rules; the
        // next user-agent line after a rule starts a new group.
        let mut seen_rule = false;

        for raw_line in text.split(['\n', '\r']) {
            let line = strip_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }
            let Some((field, value)) = split_field(line) else {
                continue;
            };
            match classify_field(field) {
                Some(Field::UserAgent) => {
                    if seen_rule && let Some(group) = current.take() {
                        robots.groups.push(group);
                    }
                    seen_rule = false;
                    let group = current.get_or_insert_with(Group::default);
                    let token = matcher::extract_product_token(value);
                    if !token.is_empty() {
                        group.agents.push(token);
                    }
                }
                Some(Field::Rule(kind)) => {
                    let Some(group) = current.as_mut() else {
                        continue;
                    };
                    seen_rule = true;
                    group.rules.push(Rule {
                        kind,
                        pattern: matcher::escape_pattern(value),
                    });
                }
                Some(Field::CrawlDelay) => {
                    if let Some(group) = current.as_mut()
                        && let Ok(seconds) = value.trim().parse::<f64>()
                        && seconds.is_finite()
                        && seconds >= 0.0
                    {
                        group.crawl_delay = Some(Duration::from_secs_f64(seconds.min(86_400.0)));
                    }
                }
                Some(Field::Sitemap) => {
                    let value = value.trim();
                    if !value.is_empty() {
                        robots.sitemaps.push(value.to_string());
                    }
                }
                None => {}
            }
        }
        if let Some(group) = current.take() {
            robots.groups.push(group);
        }
        robots
    }

    /// Sitemap URLs, in file order. They are global, not per group.
    pub fn sitemaps(&self) -> &[String] {
        &self.sitemaps
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.sitemaps.is_empty()
    }

    /// Whether `user_agent` may fetch `target`, which may be a path, a
    /// path with a query, or an absolute URL.
    pub fn is_allowed(&self, user_agent: &str, target: &str) -> bool {
        let path = path_params_query(target);
        let token = matcher::extract_product_token(user_agent);

        let mut specific = Match::default();
        let mut global = Match::default();
        let mut saw_specific_group = false;

        for group in &self.groups {
            let is_global = group.agents.iter().any(|a| a == "*");
            let is_specific = group.agents.iter().any(|a| a == &token);
            if is_specific {
                saw_specific_group = true;
            }
            if !is_global && !is_specific {
                continue;
            }
            let target_match = if is_specific {
                &mut specific
            } else {
                &mut global
            };
            for rule in &group.rules {
                let Some(priority) = pattern_matches(&path, &rule.pattern) else {
                    continue;
                };
                match rule.kind {
                    RuleKind::Allow => target_match.allow = target_match.allow.max(priority),
                    RuleKind::Disallow => {
                        target_match.disallow = target_match.disallow.max(priority)
                    }
                }
            }
        }

        // A group naming this crawler wins outright, even when it says
        // nothing about this path (RFC 9309 §2.2.1).
        if specific.allow > 0 || specific.disallow > 0 {
            return specific.allow >= specific.disallow;
        }
        if saw_specific_group {
            return true;
        }
        if global.allow > 0 || global.disallow > 0 {
            return global.allow >= global.disallow;
        }
        true
    }

    /// The `Crawl-delay` of the group that applies to `user_agent`.
    pub fn crawl_delay(&self, user_agent: &str) -> Option<Duration> {
        let token = matcher::extract_product_token(user_agent);
        let mut global = None;
        for group in &self.groups {
            if group.agents.iter().any(|a| a == &token) {
                if let Some(delay) = group.crawl_delay {
                    return Some(delay);
                }
            } else if group.agents.iter().any(|a| a == "*") && global.is_none() {
                global = group.crawl_delay;
            }
        }
        global
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct Match {
    allow: usize,
    disallow: usize,
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(at) => &line[..at],
        None => line,
    }
}

/// Splits `key: value`. A missing colon is tolerated and the first run of
/// whitespace separates key from value instead, which is what Google's
/// parser does and what a surprising number of real files rely on.
fn split_field(line: &str) -> Option<(&str, &str)> {
    if let Some(at) = line.find(':') {
        return Some((line[..at].trim(), line[at + 1..].trim()));
    }
    let at = line.find(char::is_whitespace)?;
    Some((line[..at].trim(), line[at..].trim()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    UserAgent,
    Rule(RuleKind),
    CrawlDelay,
    Sitemap,
}

/// Keys are matched by case-insensitive prefix, including the frequent typos
/// Google's parser accepts, because files in the wild contain them and a
/// crawler that ignores a misspelled `Disallow` fetches what it was asked
/// not to.
fn classify_field(key: &str) -> Option<Field> {
    const USER_AGENT: &[&str] = &["user-agent", "useragent", "user agent"];
    const DISALLOW: &[&str] = &[
        "disallow",
        "dissallow",
        "dissalow",
        "disalow",
        "diasllow",
        "disallaw",
    ];
    const SITEMAP: &[&str] = &["sitemap", "site-map"];
    const CRAWL_DELAY: &[&str] = &["crawl-delay", "crawldelay", "crawl delay"];

    // Byte comparison, not string slicing: the key may hold arbitrary UTF-8
    // (or, after lossy decoding, replacement characters) and slicing it at a
    // fixed length would panic on a character boundary.
    let key = key.as_bytes();
    let starts_with = |candidate: &str| {
        let candidate = candidate.as_bytes();
        key.len() >= candidate.len() && key[..candidate.len()].eq_ignore_ascii_case(candidate)
    };
    let starts_with_any = |candidates: &[&str]| candidates.iter().copied().any(starts_with);

    if starts_with_any(USER_AGENT) {
        Some(Field::UserAgent)
    } else if starts_with_any(DISALLOW) {
        Some(Field::Rule(RuleKind::Disallow))
    } else if starts_with("allow") {
        Some(Field::Rule(RuleKind::Allow))
    } else if starts_with_any(CRAWL_DELAY) {
        Some(Field::CrawlDelay)
    } else if starts_with_any(SITEMAP) {
        Some(Field::Sitemap)
    } else {
        None
    }
}

/// What the crawler learned about an origin's `robots.txt`.
///
/// RFC 9309 §2.3.1: a 4xx means there are no restrictions, a 5xx or a
/// network failure means the crawler must assume it is disallowed.
#[derive(Debug, Clone)]
pub enum RobotsStatus {
    /// The file was fetched and parsed.
    Parsed(RobotsTxt),
    /// 4xx or an empty body: everything is allowed.
    NotFound,
    /// 5xx or a transport failure: nothing is allowed.
    Unavailable,
}

impl RobotsStatus {
    /// Classifies a fetch result. `body` is ignored unless the status is 2xx.
    pub fn from_response(status: u16, body: &[u8]) -> RobotsStatus {
        match status {
            200..=299 => RobotsStatus::Parsed(RobotsTxt::parse(body)),
            // A redirect the caller did not follow is as good as absent.
            300..=499 => RobotsStatus::NotFound,
            _ => RobotsStatus::Unavailable,
        }
    }

    /// A transport failure before any status line.
    pub fn from_transport_failure() -> RobotsStatus {
        RobotsStatus::Unavailable
    }

    pub fn is_allowed(&self, user_agent: &str, target: &str) -> bool {
        match self {
            RobotsStatus::Parsed(robots) => robots.is_allowed(user_agent, target),
            RobotsStatus::NotFound => true,
            RobotsStatus::Unavailable => false,
        }
    }

    pub fn crawl_delay(&self, user_agent: &str) -> Option<Duration> {
        match self {
            RobotsStatus::Parsed(robots) => robots.crawl_delay(user_agent),
            _ => None,
        }
    }

    pub fn sitemaps(&self) -> &[String] {
        match self {
            RobotsStatus::Parsed(robots) => robots.sitemaps(),
            _ => &[],
        }
    }
}

/// The origin key a `robots.txt` applies to: scheme, host and port.
pub fn origin_of(url: &Url) -> String {
    let mut origin = String::with_capacity(32);
    origin.push_str(url.scheme());
    origin.push_str("://");
    if let Some(host) = url.host_str() {
        origin.push_str(host);
    }
    if let Some(port) = url.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    origin
}

/// The `robots.txt` URL for an origin, or `None` for a URL without a host.
pub fn robots_url(url: &Url) -> Option<Url> {
    url.host()?;
    let mut robots = url.clone();
    robots.set_query(None);
    robots.set_fragment(None);
    robots.set_path("/robots.txt");
    let _ = robots.set_username("");
    let _ = robots.set_password(None);
    Some(robots)
}

/// Per-origin cache of parsed `robots.txt` files. The caller does the
/// fetching; this only remembers the answer for [`DEFAULT_TTL`].
#[derive(Debug)]
pub struct RobotsCache {
    ttl: Duration,
    entries: RwLock<HashMap<String, (Arc<RobotsStatus>, SystemTime)>>,
}

impl Default for RobotsCache {
    fn default() -> Self {
        Self::new(DEFAULT_TTL)
    }
}

impl RobotsCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: RwLock::new(HashMap::new()),
        }
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    pub fn insert(&self, origin: impl Into<String>, status: RobotsStatus, now: SystemTime) {
        self.entries
            .write()
            .insert(origin.into(), (Arc::new(status), now + self.ttl));
    }

    /// The cached answer for an origin, or `None` when the caller must fetch.
    pub fn get(&self, origin: &str, now: SystemTime) -> Option<Arc<RobotsStatus>> {
        let entries = self.entries.read();
        let (status, expires) = entries.get(origin)?;
        if *expires <= now {
            return None;
        }
        Some(Arc::clone(status))
    }

    /// `Some(allowed)` when the origin is cached, `None` when the caller must
    /// fetch `robots_url(url)` and call [`RobotsCache::insert`].
    pub fn is_allowed(&self, url: &Url, user_agent: &str, now: SystemTime) -> Option<bool> {
        let status = self.get(&origin_of(url), now)?;
        Some(status.is_allowed(user_agent, url.as_str()))
    }

    pub fn purge_expired(&self, now: SystemTime) -> usize {
        let mut entries = self.entries.write();
        let before = entries.len();
        entries.retain(|_, (_, expires)| *expires > now);
        before - entries.len()
    }

    pub fn clear(&self) {
        self.entries.write().clear();
    }

    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }
}

#[cfg(test)]
mod tests;
