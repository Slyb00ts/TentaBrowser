//! `tentabrowser scrape`: many URLs, politely
//! (`docs/spec/20-platform-behaviour.md` §11, `docs/spec/21` §2 O-3/O-5).
//!
//! One engine, one context, one bounded set of in-flight fetches. `robots.txt`
//! is fetched once per origin through the same context and honoured unless
//! `--ignore-robots`; per-host concurrency and spacing come from
//! `HostLimiter`, and a `Retry-After` on 429 or 503 delays the host and buys
//! the URL one retry.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::io::{self, BufRead, Write};
use std::iter::Peekable;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tentabrowser::net::rate_limit::{Acquire, HostLimiter, HostLimits, Permit};
use tentabrowser::net::robots::{RobotsCache, RobotsStatus, robots_url};
use tentabrowser::{BrowserContext, Document, Goto, PageOptions, Settle};
use tokio::task::{JoinSet, LocalSet};
use url::Url;

use crate::cli::{Global, ScrapeArgs, ScrapeDump};
use crate::error::{CliError, CliResult, EXIT_TRANSPORT};
use crate::rate_limit;
use crate::setup;

/// The URL as text with any credentials stripped: the requested URL is echoed
/// in several output lines, and credentials must never appear there, the same
/// as `final_url` and the `fetch` command (N-R5-1).
fn redacted_url(url: &Url) -> String {
    if url.has_authority() && (!url.username().is_empty() || url.password().is_some()) {
        let mut clean = url.clone();
        let _ = clean.set_username("");
        let _ = clean.set_password(None);
        clean.as_str().to_owned()
    } else {
        url.as_str().to_owned()
    }
}

/// How many times one URL is retried after a `Retry-After`.
const MAX_ATTEMPTS: u32 = 2;

pub async fn run(global: &Global, args: &ScrapeArgs) -> CliResult<()> {
    let mut seeds = seeds(args)?.peekable();
    if seeds.peek().is_none() {
        return Err(CliError::usage(
            "scrape needs at least one URL, as arguments or through --from",
        ));
    }
    if args.concurrency == 0 || args.per_host == 0 {
        return Err(CliError::usage(
            "--concurrency and --per-host must be at least 1",
        ));
    }

    let page_threads = args.concurrency.min(tentabrowser::default_page_threads());
    let session = setup::open(global, page_threads)?;
    let user_agent = setup::user_agent(global, &session.engine.options().profile);
    let timeout = Duration::from_millis(global.timeout);
    let crawler = Crawler {
        context: session.context.clone(),
        limiter: HostLimiter::new(HostLimits {
            per_host_concurrency: args.per_host,
            min_interval: Duration::from_millis(args.delay),
        }),
        robots: RobotsCache::default(),
        user_agent,
        timeout,
        retry_after_ceiling: rate_limit::retry_after_ceiling(timeout),
        dump: args.dump,
        ignore_robots: args.ignore_robots,
    };

    // `LocalSet` keeps the in-flight fetches on this thread: the CLI runs a
    // current-thread runtime and nothing here needs to be `Send`.
    let local = LocalSet::new();
    let outcome = local.run_until(crawler.crawl(args, seeds)).await;
    session.close();
    outcome
}

/// One URL waiting to be fetched.
#[derive(Clone, Debug)]
struct Task {
    url: Url,
    depth: usize,
    /// Emission order, so `--ordered` can restore it.
    seq: usize,
    attempt: u32,
}

/// The result of checking a `Retry-After` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetryAfterResult {
    /// No retry-after header, or the status is not 429/503.
    None,
    /// Retry after this duration.
    Retry(Duration),
    /// The host requested a delay exceeding the ceiling; give up on it.
    Overwhelmed,
}

/// What one fetch produced.
struct Outcome {
    task: Task,
    line: Value,
    /// Links worth following, already filtered.
    discovered: Vec<Url>,
    /// The host's retry status.
    retry_after: RetryAfterResult,
    failed: bool,
}

/// The frontier: one FIFO per host, the hosts that may dispatch now in a
/// round-robin ring, and the hosts that must wait in a heap keyed by when
/// they may try again. Every operation is O(1) or O(log hosts) whatever the
/// number of queued URLs, so a slow host never costs a scan of the others.
struct Frontier {
    queues: HashMap<String, VecDeque<Task>>,
    ready_hosts: VecDeque<String>,
    blocked: BinaryHeap<Reverse<(Instant, String)>>,
    blocked_hosts: HashSet<String>,
    len: usize,
}

impl Frontier {
    fn new() -> Self {
        Self {
            queues: HashMap::new(),
            ready_hosts: VecDeque::new(),
            blocked: BinaryHeap::new(),
            blocked_hosts: HashSet::new(),
            len: 0,
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn host_of(task: &Task) -> String {
        task.url.host_str().unwrap_or_default().to_ascii_lowercase()
    }

    /// Appends `task` to its host's queue. A host that had nothing queued
    /// joins the ring unless it is parked, in which case the heap brings it
    /// back when its wait expires.
    fn push(&mut self, task: Task) {
        self.insert(task, false);
    }

    /// Puts `task` at the front of its host's queue: a retry the host asked
    /// to delay keeps its place.
    fn push_front(&mut self, task: Task) {
        self.insert(task, true);
    }

    fn insert(&mut self, task: Task, front: bool) {
        let host = Self::host_of(&task);
        let queue = self.queues.entry(host.clone()).or_default();
        let was_empty = queue.is_empty();
        if front {
            queue.push_front(task);
        } else {
            queue.push_back(task);
        }
        self.len += 1;
        if was_empty && !self.blocked_hosts.contains(&host) {
            self.ready_hosts.push_back(host);
        }
    }

    /// How many hosts may try to dispatch right now.
    fn ready_host_count(&self) -> usize {
        self.ready_hosts.len()
    }

    /// The next host allowed to try, taken off the ring. The caller either
    /// dispatches its first task ([`Frontier::take_front`]) or parks it
    /// ([`Frontier::block`]).
    fn next_host(&mut self) -> Option<String> {
        self.ready_hosts.pop_front()
    }

    /// Removes the host's first task; a host with more work goes to the back
    /// of the ring.
    fn take_front(&mut self, host: &str) -> Option<Task> {
        let queue = self.queues.get_mut(host)?;
        let task = queue.pop_front()?;
        self.len -= 1;
        if queue.is_empty() {
            self.queues.remove(host);
        } else {
            self.ready_hosts.push_back(host.to_owned());
        }
        Some(task)
    }

    /// Parks a host taken off the ring until `until`.
    fn block(&mut self, host: String, until: Instant) {
        if self.blocked_hosts.insert(host.clone()) {
            self.blocked.push(Reverse((until, host)));
        }
    }

    /// Returns the hosts whose wait expired to the ring.
    fn promote_expired(&mut self, now: Instant) {
        while let Some(Reverse((until, _))) = self.blocked.peek() {
            if *until > now {
                break;
            }
            let Some(Reverse((_, host))) = self.blocked.pop() else {
                break;
            };
            self.blocked_hosts.remove(&host);
            if self
                .queues
                .get(&host)
                .is_some_and(|queue| !queue.is_empty())
            {
                self.ready_hosts.push_back(host);
            }
        }
    }

    /// When the earliest parked host may try again.
    fn next_wakeup(&self) -> Option<Instant> {
        self.blocked.peek().map(|Reverse((until, _))| *until)
    }

    /// Drops all queued URLs for a host and returns them.
    fn drop_host(&mut self, host: &str) -> Vec<Task> {
        if let Some(queue) = self.queues.remove(host) {
            self.len -= queue.len();
            // Remove from ready ring if present.
            self.ready_hosts.retain(|h| h != host);
            queue.into_iter().collect()
        } else {
            Vec::new()
        }
    }

    /// Returns the hosts currently in the ready ring.
    fn ready_hosts(&self) -> &VecDeque<String> {
        &self.ready_hosts
    }
}

struct Crawler {
    context: BrowserContext,
    limiter: HostLimiter,
    robots: RobotsCache,
    user_agent: String,
    timeout: Duration,
    retry_after_ceiling: Duration,
    dump: ScrapeDump,
    ignore_robots: bool,
}

impl Crawler {
    async fn crawl(self, args: &ScrapeArgs, mut seeds: Peekable<Seeds>) -> CliResult<()> {
        use tokio::select;

        let crawler = Arc::new(self);
        let mut frontier = Frontier::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut next_seq = 0usize;
        // Seeds are pulled as the crawl needs them, a small window ahead of
        // the in-flight set, so a long `--from` list never has to fit in
        // memory and `--max-pages` stops the read as well as the fetches.
        let mut lookahead = args.concurrency.saturating_mul(4).max(16);
        let max_reached = |dispatched: usize| args.max_pages.is_some_and(|max| dispatched >= max);

        let mut writer = LineWriter::new(args.ordered);
        let mut in_flight: JoinSet<Outcome> = JoinSet::new();
        let mut dispatched = 0usize;
        let mut succeeded = 0usize;
        let mut failed = 0usize;

        loop {
            // Grow lookahead dynamically while every ready host is at its per-host limit,
            // to prevent host diversity loss on host-grouped seed lists (N-R2-5).
            if !frontier.ready_hosts().is_empty() {
                let all_at_limit = frontier
                    .ready_hosts()
                    .iter()
                    .all(|host| crawler.limiter.in_flight(host) >= args.per_host);
                if all_at_limit && lookahead < 4096 {
                    lookahead = lookahead.saturating_mul(2).min(4096);
                }
            }

            // Dispatch tasks until we hit concurrency limit or run out of work.
            loop {
                while frontier.len() < lookahead
                    && let Some(seed) = seeds.next()
                {
                    let url = seed?;
                    if seen.insert(url.as_str().to_owned()) {
                        frontier.push(Task {
                            url,
                            depth: 0,
                            seq: next_seq,
                            attempt: 1,
                        });
                        next_seq += 1;
                    }
                }

                if in_flight.len() >= args.concurrency || max_reached(dispatched) {
                    break;
                }

                if let Some((task, permit)) = crawler.next_ready(&mut frontier).await {
                    dispatched += 1;
                    let crawler = Arc::clone(&crawler);
                    in_flight.spawn_local(async move { crawler.fetch(task, permit).await });
                } else {
                    // No task ready right now; break to select! on wakeup or outcome
                    break;
                }
            }

            // If nothing in flight and frontier is empty, we're done.
            if in_flight.is_empty() && frontier.is_empty() && seeds.peek().is_none() {
                break;
            }
            if max_reached(dispatched) && in_flight.is_empty() {
                break;
            }

            // Select between: sleeping until a parked host wakes up, or processing an outcome.
            // This ensures outcomes are processed immediately instead of waiting for the next wakeup.
            select! {
                Some(joined) = in_flight.join_next() => {
                    let outcome = joined.map_err(|error| {
                        CliError::new(EXIT_TRANSPORT, format!("a fetch task failed: {error}"))
                    })?;

                    // Handle Retry-After logic first, which may cause a retry or give up.
                    match outcome.retry_after {
                        RetryAfterResult::Retry(delay) => {
                            if let Some(host) = outcome.task.url.host_str() {
                                crawler.limiter.delay_host(host, delay);
                            }
                            if outcome.task.attempt < MAX_ATTEMPTS {
                                let mut retry = outcome.task.clone();
                                retry.attempt += 1;
                                // Re-queued at the front of its host: the limiter holds
                                // the host back for the delay it asked for.
                                frontier.push_front(retry);
                                dispatched -= 1;
                                continue;
                            }
                        }
                        RetryAfterResult::Overwhelmed => {
                            // The host requested a delay exceeding the ceiling.
                            // Emit the triggering URL's response line first, then drop remaining queued URLs and log.
                            if let Some(host) = outcome.task.url.host_str() {
                                let host_lower = host.to_ascii_lowercase();
                                tracing::warn!(
                                    "host {} returned Retry-After exceeding ceiling; dropping remaining URLs",
                                    host_lower
                                );
                                // Emit the response line for the URL that triggered the overwhelmed status.
                                writer.emit(outcome.task.seq, outcome.line)?;
                                failed += 1;
                                // Drop all queued URLs for this host and emit error lines for each.
                                let dropped = frontier.drop_host(&host_lower);
                                for task in dropped.iter() {
                                    let error_line = json!({
                                        "url": redacted_url(&task.url),
                                        "error": "overwhelmed_host",
                                        "message": "host requested retry-after exceeding timeout ceiling"
                                    });
                                    writer.emit(task.seq, error_line)?;
                                }
                                failed += dropped.len();
                            } else {
                                failed += 1;
                            }
                            continue; // Skip normal success/failure counting and link following.
                        }
                        RetryAfterResult::None => {}
                    }

                    // Normal success/failure counting and link following.
                    if outcome.failed {
                        failed += 1;
                    } else {
                        succeeded += 1;
                    }
                    if args.follow_links && outcome.task.depth < args.depth {
                        for url in outcome.discovered {
                            if args.same_host && url.host_str() != outcome.task.url.host_str() {
                                continue;
                            }
                            if !seen.insert(url.as_str().to_owned()) {
                                continue;
                            }
                            frontier.push(Task {
                                url,
                                depth: outcome.task.depth + 1,
                                seq: next_seq,
                                attempt: 1,
                            });
                            next_seq += 1;
                        }
                    }
                    writer.emit(outcome.task.seq, outcome.line)?;
                }
                _ = async {
                    if let Some(wakeup) = frontier.next_wakeup() {
                        tokio::time::sleep_until(tokio::time::Instant::from_std(wakeup)).await
                    } else {
                        std::future::pending().await
                    }
                }, if !frontier.is_empty() => {
                    // A parked host's wait expired; loop back to try dispatching
                }
            }
        }
        writer.finish()?;

        if succeeded == 0 && failed > 0 {
            return Err(CliError::new(
                EXIT_TRANSPORT,
                format!("all {failed} pages failed; see the `error` field of each line"),
            ));
        }
        Ok(())
    }

    /// The next task whose host has a free slot, with that slot held.
    ///
    /// Each host in the ring gets one try per pass; a host the limiter
    /// refuses is parked for the wait the limiter names. Returns `None` if
    /// no host may dispatch right now; the caller checks `frontier.next_wakeup()`
    /// and `select!`s between sleeping until then and processing in-flight outcomes.
    async fn next_ready(&self, frontier: &mut Frontier) -> Option<(Task, Permit)> {
        let now = Instant::now();
        frontier.promote_expired(now);
        for _ in 0..frontier.ready_host_count() {
            let Some(host) = frontier.next_host() else {
                break;
            };
            match self.limiter.acquire(&host) {
                Acquire::Ready(permit) => {
                    if let Some(task) = frontier.take_front(&host) {
                        return Some((task, permit));
                    }
                }
                Acquire::WaitFor(wait) => frontier.block(host, now + wait),
            }
        }
        None
    }

    /// One page: robots check (with timeout), fetch, extraction.
    async fn fetch(&self, task: Task, permit: Permit) -> Outcome {
        let started = Instant::now();
        // Ensure robots.txt is fetched with the configured timeout, not inline in the coordinator.
        // This runs as part of the per-URL task, ensuring the robots request doesn't block
        // the dispatch loop and other in-flight outcomes.
        self.ensure_robots(&task.url).await;
        if !self.ignore_robots && !self.robots_allows(&task.url) {
            drop(permit);
            return Outcome {
                line: json!({ "url": redacted_url(&task.url), "error": "robots" }),
                task,
                discovered: Vec::new(),
                retry_after: RetryAfterResult::None,
                failed: true,
            };
        }

        let document = self.load(&task.url).await;
        drop(permit);
        let elapsed = started.elapsed();

        match document {
            Ok(document) => {
                let retry_after = retry_after(&document, self.retry_after_ceiling);
                let discovered = same_host_links(&document, &task.url);
                Outcome {
                    line: self.line(&task.url, &document, elapsed),
                    failed: document.status() >= 400,
                    task,
                    discovered,
                    retry_after,
                }
            }
            Err(error) => Outcome {
                line: json!({
                    "url": redacted_url(&task.url),
                    "error": error.error_name().unwrap_or("error"),
                    "message": error.to_string(),
                    "timing_ms": millis(elapsed),
                }),
                task,
                discovered: Vec::new(),
                retry_after: RetryAfterResult::None,
                failed: true,
            },
        }
    }

    async fn load(&self, url: &Url) -> CliResult<Document> {
        tracing::debug!(url = %crate::logging::url(url), "scrape");
        let page = self.context.new_page(PageOptions::tier1()).await?;
        let goto = Goto::default()
            .wait_until(Settle::Load)
            .timeout(self.timeout);
        let navigation = page.goto(url.as_str(), goto).await;
        let document = match navigation {
            Ok(_) => page.take_document().await,
            Err(error) => Err(error),
        };
        let _ = page.close().await;
        document.map_err(CliError::from)
    }

    /// Whether the cached `robots.txt` allows `url`. An origin the cache has
    /// never heard of is allowed: [`Crawler::ensure_robots`] runs first and
    /// only leaves the cache empty for a URL with no host.
    fn robots_allows(&self, url: &Url) -> bool {
        self.robots
            .is_allowed(url, &self.user_agent, std::time::SystemTime::now())
            .unwrap_or(true)
    }

    /// Fetches and caches the origin's `robots.txt` if it is not cached yet.
    ///
    /// RFC 9309 §2.3.1 through [`RobotsStatus`]: a 4xx means no restrictions,
    /// a 5xx or a transport failure means nothing is allowed.
    /// Fetches with the same User-Agent that will be used for page fetches.
    async fn ensure_robots(&self, url: &Url) {
        if self.ignore_robots {
            return;
        }
        let now = std::time::SystemTime::now();
        if self.robots.is_allowed(url, &self.user_agent, now).is_some() {
            return;
        }
        let Some(robots) = robots_url(url) else {
            return;
        };
        let origin = tentabrowser::net::robots::origin_of(url);

        // Build a request with the override user-agent so robots.txt rules
        // are evaluated against the same identity that will fetch pages.
        // The robots.txt fetch is a document navigation from the page origin.
        let status = match tentabrowser::net::Request::get(robots.as_str()) {
            Ok(request) => {
                let mut request = match request.header("User-Agent", &self.user_agent) {
                    Ok(r) => r,
                    Err(_) => {
                        // User-Agent header invalid; use default
                        tentabrowser::net::Request::get(robots.as_str()).expect("always valid")
                    }
                };
                // Apply the configured timeout to the robots fetch.
                request.limits.total = Some(self.timeout);
                // Set the first party to the page's origin so cookies and same-site
                // classification reflect the page being scraped, not the robot.
                if let Some(site) = tentabrowser::net::Site::from_url(url) {
                    request = request.first_party(site);
                }
                match self.context.client().send(request).await {
                    Ok(response) => {
                        let status = response.status.as_u16();
                        match response.bytes().await {
                            Ok(body) => RobotsStatus::from_response(status, &body),
                            Err(_) => RobotsStatus::from_transport_failure(),
                        }
                    }
                    Err(_) => RobotsStatus::from_transport_failure(),
                }
            }
            Err(_) => RobotsStatus::from_transport_failure(),
        };
        if let Some(delay) = status.crawl_delay(&self.user_agent)
            && let Some(host) = url.host_str()
        {
            self.limiter.set_crawl_delay(host, delay);
        }
        self.robots.insert(origin, status, now);
    }

    fn line(&self, url: &Url, document: &Document, elapsed: Duration) -> Value {
        let mut line = json!({
            "url": redacted_url(url),
            "final_url": document.final_url().as_str(),
            "status": document.status(),
            "title": document.title(),
            "timing_ms": millis(elapsed),
        });
        let payload = &mut line;
        match self.dump {
            ScrapeDump::Text => payload["text"] = Value::String(document.text()),
            ScrapeDump::Markdown => payload["markdown"] = Value::String(document.markdown()),
            ScrapeDump::Links => {
                payload["links"] = serde_json::to_value(document.links()).unwrap_or(Value::Null);
            }
            ScrapeDump::Json => {
                let full = document.to_json();
                payload["text"] = full["text"].clone();
                payload["markdown"] = full["markdown"].clone();
                payload["links"] = full["links"].clone();
                payload["metadata"] = full["metadata"].clone();
                payload["headers"] = full["headers"].clone();
            }
        }
        line
    }
}

/// `Retry-After` on the statuses that mean "come back later", capped at
/// the configured ceiling (max of timeout and 60 s). Returns `Overwhelmed`
/// if the delay exceeds the ceiling, `Retry` if within the ceiling, or
/// `None` if the header is not present or the status is not 429/503.
fn retry_after(document: &Document, ceiling: Duration) -> RetryAfterResult {
    if !matches!(document.status(), 429 | 503) {
        return RetryAfterResult::None;
    }
    let Some(value) = document.header("retry-after") else {
        return RetryAfterResult::None;
    };
    let Some(delay) = tentabrowser::net::rate_limit::parse_retry_after(
        value.as_bytes(),
        std::time::SystemTime::now(),
    ) else {
        return RetryAfterResult::None;
    };
    // If the delay exceeds the ceiling, the host is considered overwhelmed.
    if delay > ceiling {
        RetryAfterResult::Overwhelmed
    } else {
        RetryAfterResult::Retry(delay)
    }
}

/// Links worth following: http(s) only, absolute, and never the page itself.
fn same_host_links(document: &Document, from: &Url) -> Vec<Url> {
    document
        .links()
        .into_iter()
        .filter_map(|link| Url::parse(&link.url).ok())
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .filter(|url| url.as_str() != from.as_str())
        .collect()
}

fn millis(elapsed: Duration) -> f64 {
    (elapsed.as_secs_f64() * 1000.0 * 1000.0).round() / 1000.0
}

/// Reads seed URLs from arguments and `--from`, respecting `--max-pages`.
/// Streams URLs from the input instead of loading all at once into memory.
/// For `--max-pages`, reads only enough URLs to potentially satisfy the limit
/// (plus a small buffer for retries and discovered URLs).
/// The URLs to crawl: the arguments first, then `--from` read line by line.
struct Seeds {
    args: std::vec::IntoIter<Url>,
    lines: Option<Box<dyn Iterator<Item = io::Result<String>> + Send>>,
    source: String,
}

impl Iterator for Seeds {
    type Item = CliResult<Url>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(url) = self.args.next() {
            return Some(Ok(url));
        }
        let lines = self.lines.as_mut()?;
        loop {
            let line = match lines.next()? {
                Ok(line) => line,
                Err(err) => {
                    return Some(Err(CliError::usage(format!(
                        "--from {}: {err}",
                        self.source
                    ))));
                }
            };
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            return Some(
                Url::parse(line).map_err(|err| CliError::usage(format!("{line:?}: {err}"))),
            );
        }
    }
}

/// Standard input one line at a time, locking per line so the iterator can
/// live inside the crawl future.
struct StdinLines;

impl Iterator for StdinLines {
    type Item = io::Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line = String::new();
        match io::stdin().read_line(&mut line) {
            Ok(0) => None,
            Ok(_) => Some(Ok(line)),
            Err(err) => Some(Err(err)),
        }
    }
}

/// The URL arguments are validated up front (a typo is a usage error before
/// the engine starts); `--from` is opened here and read as the crawl goes.
fn seeds(args: &ScrapeArgs) -> CliResult<Seeds> {
    let mut urls = Vec::with_capacity(args.urls.len());
    for raw in &args.urls {
        let raw = raw.trim();
        urls.push(Url::parse(raw).map_err(|err| CliError::usage(format!("{raw:?}: {err}")))?);
    }
    let lines: Option<Box<dyn Iterator<Item = io::Result<String>> + Send>> = match &args.from {
        None => None,
        Some(source) if source == "-" => Some(Box::new(StdinLines)),
        Some(source) => {
            let file = std::fs::File::open(source)
                .map_err(|err| CliError::usage(format!("--from {source}: {err}")))?;
            Some(Box::new(io::BufReader::new(file).lines()))
        }
    };
    Ok(Seeds {
        args: urls.into_iter(),
        lines,
        source: args.from.clone().unwrap_or_default(),
    })
}

/// Prints the JSON lines, either as they finish or in the order the URLs were
/// given.
struct LineWriter {
    ordered: bool,
    next: usize,
    held: HashMap<usize, Value>,
}

impl LineWriter {
    fn new(ordered: bool) -> Self {
        Self {
            ordered,
            next: 0,
            held: HashMap::new(),
        }
    }

    fn emit(&mut self, seq: usize, line: Value) -> CliResult<()> {
        if !self.ordered {
            return print_line(&line);
        }
        self.held.insert(seq, line);
        while let Some(line) = self.held.remove(&self.next) {
            print_line(&line)?;
            self.next += 1;
        }
        Ok(())
    }

    /// Prints whatever is left: a URL that was never dispatched (`--max-pages`)
    /// leaves a hole in the sequence, and holding lines back forever would be
    /// worse than printing them slightly out of order.
    fn finish(&mut self) -> CliResult<()> {
        let mut rest: Vec<_> = self.held.drain().collect();
        rest.sort_by_key(|(seq, _)| *seq);
        for (_, line) in rest {
            print_line(&line)?;
        }
        Ok(())
    }
}

fn print_line(line: &Value) -> CliResult<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let rendered = serde_json::to_string(line).map_err(|err| {
        CliError::new(EXIT_TRANSPORT, format!("serialising a scrape line: {err}"))
    })?;
    writeln!(out, "{rendered}")
        .map_err(|err| CliError::new(EXIT_TRANSPORT, format!("writing output: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn scrape_args(extra: &[&str]) -> ScrapeArgs {
        let cli = crate::cli::Cli::parse_from(
            ["tentabrowser", "scrape"]
                .into_iter()
                .chain(extra.iter().copied()),
        );
        let crate::cli::Command::Scrape(args) = cli.command else {
            panic!("expected scrape");
        };
        args
    }

    #[test]
    fn seeds_come_from_arguments_and_from_a_file() {
        let dir = tenta_testutil::temp_dir("scrape-seeds");
        let list = dir.write(
            "urls.txt",
            "# comment\nhttp://example.test/a\n\nhttp://example.test/b\n",
        );
        let args = scrape_args(&[
            "http://example.test/seed",
            "--from",
            list.to_str().expect("utf-8"),
        ]);
        let seeds: Vec<Url> = seeds(&args)
            .expect("seeds")
            .collect::<CliResult<Vec<Url>>>()
            .expect("every line parses");
        assert_eq!(
            seeds.iter().map(Url::as_str).collect::<Vec<_>>(),
            [
                "http://example.test/seed",
                "http://example.test/a",
                "http://example.test/b"
            ]
        );
    }

    #[test]
    fn a_seed_that_is_not_a_url_is_a_usage_error() {
        let args = scrape_args(&["not a url"]);
        assert_eq!(
            match seeds(&args) {
                Ok(_) => panic!("a URL that does not parse is a usage error"),
                Err(error) => error.exit_code(),
            },
            crate::error::EXIT_USAGE
        );
    }

    #[test]
    fn ordered_output_holds_lines_until_their_turn() {
        let mut writer = LineWriter::new(true);
        writer.emit(2, json!({"url": "c"})).expect("held");
        assert_eq!(writer.held.len(), 1, "line 2 waits for 0 and 1");
        writer.emit(0, json!({"url": "a"})).expect("printed");
        assert_eq!(writer.next, 1);
        writer.emit(1, json!({"url": "b"})).expect("printed");
        assert_eq!(writer.next, 3, "1 unblocked 2");
        assert!(writer.held.is_empty());
    }

    #[test]
    fn unordered_output_prints_immediately() {
        let mut writer = LineWriter::new(false);
        writer.emit(9, json!({"url": "a"})).expect("printed");
        assert!(writer.held.is_empty());
    }

    fn task(url: &str, seq: usize) -> Task {
        Task {
            url: Url::parse(url).expect("url"),
            depth: 0,
            seq,
            attempt: 1,
        }
    }

    fn drain(frontier: &mut Frontier) -> Vec<usize> {
        std::iter::from_fn(|| {
            let host = frontier.next_host()?;
            frontier.take_front(&host).map(|task| task.seq)
        })
        .collect()
    }

    #[test]
    fn the_frontier_round_robins_hosts_and_keeps_each_host_fifo() {
        let mut frontier = Frontier::new();
        frontier.push(task("http://a.test/1", 0));
        frontier.push(task("http://a.test/2", 1));
        frontier.push(task("http://b.test/1", 2));
        assert_eq!(frontier.len(), 3);
        assert_eq!(drain(&mut frontier), [0, 2, 1]);
        assert!(frontier.is_empty());
        assert!(frontier.next_wakeup().is_none());
    }

    #[test]
    fn a_parked_host_leaves_the_ring_until_its_wait_expires() {
        let mut frontier = Frontier::new();
        frontier.push(task("http://a.test/1", 0));
        frontier.push(task("http://b.test/1", 1));
        let now = Instant::now();
        let until = now + Duration::from_millis(50);

        let host = frontier.next_host().expect("a.test is first");
        assert_eq!(host, "a.test");
        frontier.block(host, until);
        assert_eq!(frontier.next_host().as_deref(), Some("b.test"));
        assert_eq!(frontier.take_front("b.test").map(|t| t.seq), Some(1));
        assert!(frontier.next_host().is_none(), "a.test is parked");
        assert_eq!(frontier.next_wakeup(), Some(until));

        // A task queued for a parked host does not put it back on the ring.
        frontier.push(task("http://a.test/2", 2));
        assert!(frontier.next_host().is_none());

        frontier.promote_expired(until - Duration::from_millis(1));
        assert!(frontier.next_host().is_none());
        frontier.promote_expired(until);
        assert_eq!(frontier.next_host().as_deref(), Some("a.test"));
        assert_eq!(frontier.take_front("a.test").map(|t| t.seq), Some(0));
        assert_eq!(frontier.len(), 1);
    }

    #[test]
    fn a_retry_goes_to_the_front_of_its_host_queue() {
        let mut frontier = Frontier::new();
        frontier.push(task("http://a.test/1", 0));
        frontier.push(task("http://a.test/2", 1));
        let host = frontier.next_host().expect("a.test");
        let first = frontier.take_front(&host).expect("task 0");
        assert_eq!(first.seq, 0);
        frontier.push_front(first);
        assert_eq!(drain(&mut frontier), [0, 1]);
    }

    #[test]
    fn seeds_are_read_no_further_than_the_crawl_pulls() {
        let mut seeds = Seeds {
            args: Vec::new().into_iter(),
            lines: Some(Box::new((0..).map(|i| {
                assert!(i < 3, "the list was read past the window");
                Ok(format!("http://h.test/{i}"))
            }))),
            source: "-".to_owned(),
        };
        let first: Vec<String> = seeds
            .by_ref()
            .take(3)
            .map(|seed| seed.expect("url").to_string())
            .collect();
        assert_eq!(
            first,
            ["http://h.test/0", "http://h.test/1", "http://h.test/2"]
        );
    }

    #[test]
    fn frontier_supports_multiple_hosts_in_ready_ring() {
        // Verify that the frontier can hold multiple hosts with queued tasks.
        // This is the prerequisite for dynamic lookahead growth to show host diversity.
        let mut frontier = Frontier::new();

        // Queue tasks for host A (8 tasks) and then host B (4 tasks).
        for i in 0..8 {
            frontier.push(Task {
                url: Url::parse(&format!("http://a.test/{i}")).expect("url"),
                depth: 0,
                seq: i,
                attempt: 1,
            });
        }
        for i in 0..4 {
            frontier.push(Task {
                url: Url::parse(&format!("http://b.test/{i}")).expect("url"),
                depth: 0,
                seq: 8 + i,
                attempt: 1,
            });
        }

        // Both hosts should be visible in the frontier's queues.
        // With dynamic lookahead in the main loop, both hosts would be loaded
        // together and could be dispatched in parallel, maintaining diversity.
        assert!(
            frontier.queues.contains_key("a.test"),
            "host a should be queued"
        );
        assert!(
            frontier.queues.contains_key("b.test"),
            "host b should be queued (demonstrating lookahead can load multiple hosts)"
        );
        assert_eq!(frontier.len(), 12, "all 12 tasks should be queued");
    }
}
