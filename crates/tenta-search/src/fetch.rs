//! One provider request: send, stream the body into the parser, stop the tree
//! builder when the results container closes (`docs/spec/08-search.md` §3.3).
//!
//! Nothing here interprets the page; that is `crate::extract`. What this
//! module owns is the split between time spent waiting for the network and
//! time spent in the engine, because `engine_overhead = total - network` is
//! the number R-06 budgets.

use std::time::{Duration, Instant};

use tenta_core::NodeId;
use tenta_dom::Document;
use tenta_html::{ParseStatus, Parser, ParserOptions};
use tenta_net::proxy::Proxy;
use tenta_net::{Client, NetError, Request, ResourceType};
use url::Url;

use crate::plan::{ParsePlan, Prefilter, PreparedRequest};

/// Response bodies larger than this are refused: a SERP is 30–300 KB, and a
/// multi-megabyte "SERP" is a redirect loop or an attack, not a page to parse.
pub const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// Where a provider request spent its time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetTiming {
    pub dns: Duration,
    pub connect: Duration,
    pub ttfb: Duration,
    /// From `send()` to the response head.
    pub head: Duration,
    /// Everything the query spent parked on the network: the head, plus every
    /// wait for a body chunk. This is the `network` of
    /// `engine_overhead = total - network`.
    pub network: Duration,
}

/// A fetched and parsed SERP.
pub struct Fetched {
    pub status: u16,
    /// The URL that answered, after redirects; the base for relative hrefs.
    pub url: Url,
    pub document: Document,
    /// The bytes that were read. Short of the whole body when the parser
    /// stopped at the results container.
    pub body: Vec<u8>,
    /// Whether the tree builder stopped at the container instead of at EOF.
    pub stopped_early: bool,
    pub timing: NetTiming,
}

/// The context's identity overrides, applied after the plan's own headers
/// (`docs/spec/21-project-operations.md` §2: `--user-agent` applies in every
/// mode, `search` included).
#[derive(Clone, Copy, Debug, Default)]
pub struct HeaderOverrides<'a> {
    pub user_agent: Option<&'a str>,
    pub accept_language: Option<&'a str>,
}

/// How one attempt is sent: the proxy in use, whether the pool is bypassed
/// (the `reset` retry), the deadline, and the context's identity overrides.
#[derive(Clone, Copy, Debug)]
pub struct FetchOptions<'a> {
    pub proxy: Option<&'a Proxy>,
    pub fresh_connection: bool,
    pub timeout: Option<Duration>,
    pub default_locale: &'a str,
    pub overrides: HeaderOverrides<'a>,
}

impl Default for FetchOptions<'_> {
    fn default() -> Self {
        Self {
            proxy: None,
            fresh_connection: false,
            timeout: None,
            default_locale: "en",
            overrides: HeaderOverrides::default(),
        }
    }
}

/// Sends `prepared` and parses the answer with `plan`'s early-stop marker.
pub async fn fetch_and_parse(
    client: &Client,
    plan: &ParsePlan,
    prepared: &PreparedRequest,
    options: FetchOptions<'_>,
) -> Result<Fetched, NetError> {
    let FetchOptions {
        proxy,
        fresh_connection,
        timeout,
        default_locale,
        overrides,
    } = options;
    let mut request = if prepared.post {
        let body = prepared.body.clone().unwrap_or_default();
        Request::post(prepared.url.as_str(), body)?
            .header("Content-Type", "application/x-www-form-urlencoded")?
    } else {
        Request::get(prepared.url.as_str())?
    };
    // Track which headers will be overridden so we don't add them from the plan
    let override_user_agent = overrides.user_agent.is_some();
    let override_accept_language = overrides.accept_language.is_some();

    for (name, value) in &prepared.headers {
        // Skip headers that will be overridden (M-R2-3)
        if (override_user_agent && name.eq_ignore_ascii_case("User-Agent"))
            || (override_accept_language && name.eq_ignore_ascii_case("Accept-Language"))
        {
            continue;
        }
        request = request.header(name, value)?;
    }
    if let Some(user_agent) = overrides.user_agent {
        request = request.header("User-Agent", user_agent)?;
    }
    if let Some(accept_language) = overrides.accept_language {
        request = request.header("Accept-Language", accept_language)?;
    }
    request = request
        .resource_type(ResourceType::Document)
        .max_body_bytes(MAX_BODY_BYTES);
    if let Some(timeout) = timeout {
        request = request.timeout(timeout);
    }
    if let Some(proxy) = proxy {
        request = request.proxy(proxy.clone());
    }
    // A search is a top-level navigation to the provider, which is what the
    // cookie jar has to classify it as; the default context is already that,
    // except that a POST is not a safe method.
    request.first_party = tenta_net::policy::cookies::Site::from_url(&request.url);
    if prepared.post {
        let mut context = request.same_site_context();
        context.safe_method = false;
        request.same_site = Some(context);
    }
    if fresh_connection {
        // The retry after a reset must not join the request that was reset.
        request.allow_coalescing = false;
    }

    let response = client.send(request).await?;
    // The transport's own measurement of `send()` to the response head. It
    // excludes the hop to the io thread and back, so that hop counts as
    // engine overhead, which is the conservative way round.
    let head = response.timing.total;
    let status = response.status.as_u16();
    let url = response.url.clone();
    let charset = response
        .header("content-type")
        .and_then(charset_of)
        .map(str::to_owned);

    let mut timing = NetTiming {
        dns: response.timing.dns,
        connect: response.timing.connect,
        ttfb: response.timing.ttfb,
        head,
        network: head,
    };

    let mut parser = Parser::new(
        new_document(&url, plan),
        parser_options(charset.as_deref(), default_locale),
    );
    let mut body = Vec::with_capacity(64 * 1024);
    let mut stream = response.body;
    let mut stopped_early = false;
    let mut restarted = false;

    loop {
        let waiting = Instant::now();
        let chunk = stream.next_chunk().await;
        timing.network += waiting.elapsed();
        let chunk = match chunk {
            Some(Ok(chunk)) => chunk,
            Some(Err(error)) => return Err(error),
            None => break,
        };
        body.extend_from_slice(&chunk);
        match parser.feed(&chunk) {
            ParseStatus::Continue => {}
            ParseStatus::Stopped => {
                stopped_early = true;
                break;
            }
            ParseStatus::Script => {
                // Cannot happen: search extraction never runs scripts, so this
                // parser is built without `pause_on_script` and tokenizes past
                // script elements. Stop rather than buffer a tail nothing will
                // ever parse.
                stopped_early = true;
                break;
            }
            ParseStatus::EncodingChanged(encoding) => {
                // A `<meta charset>` that contradicts a tentative encoding:
                // throw the tree away and re-run the bytes seen so far. The
                // HTML spec makes the new encoding certain, so this happens
                // at most once.
                if restarted {
                    // Cannot happen: the restart names the encoding, which
                    // makes it certain. Stop reading rather than spin.
                    break;
                }
                restarted = true;
                parser = parser.restart_with_encoding(new_document(&url, plan), encoding);
                if parser.feed(&body) == ParseStatus::Stopped {
                    stopped_early = true;
                    break;
                }
            }
        }
    }
    drop(stream);

    let document = parser.finish();
    // A body that fits in the encoding pre-scan window is parsed by
    // `finish()`, so the tree builder's own flag is the authoritative one:
    // `feed` only reports the stop when it happened while it was running.
    let stopped_early = stopped_early || document.parse_stopped();

    Ok(Fetched {
        status,
        url,
        document,
        body,
        stopped_early,
        timing,
    })
}

fn parser_options(charset: Option<&str>, default_locale: &str) -> ParserOptions<'static> {
    // A charset the transport declared is certain (Encoding Standard §4.2),
    // so naming it here lets the tree builder start on the first chunk
    // instead of buffering the encoding pre-scan window first
    // (`docs/spec/08-search.md` §3.3).
    let encoding = charset.and_then(|label| tenta_html::Encoding::for_label(label.as_bytes()));
    ParserOptions {
        // Tier 1: no scripts run, and `<noscript>` content is the content a
        // scriptless browser sees, which is what the plans were written for.
        scripting_enabled: false,
        encoding,
        transport_charset: charset.map(str::to_owned),
        default_locale: default_locale.to_owned(),
        ..ParserOptions::default()
    }
}

/// A document that stops the tree builder once the plan's results container
/// closes (`docs/spec/08-search.md` §3.3).
///
/// The stop is skipped for plans that also extract related searches or a
/// direct answer: providers put those after the results container, and
/// stopping there would throw them away. No shipped plan does, so the fast
/// path always stops.
fn new_document(url: &Url, plan: &ParsePlan) -> Document {
    let mut document = Document::with_url(url.clone());
    let whole_document = plan.related.is_some() || plan.answer.is_some();
    if let Some(container) = plan.results.container.as_ref().filter(|_| !whole_document) {
        let selector = container.as_str().to_owned();
        let prefilter = container.prefilter().clone();
        document.set_stop_after(Some(Box::new(move |document: &Document, node: NodeId| {
            // The prefilter is an integer comparison; the selector engine
            // only runs for the handful of elements that pass it.
            let candidate = match &prefilter {
                Prefilter::Id(id) => document.element_id(node) == Some(id.as_str()),
                Prefilter::Class(class) => document.classes(node).any(|c| c == class.as_str()),
                Prefilter::Tag(tag) => document
                    .tag(node)
                    .is_some_and(|t| t.as_str() == tag.as_str()),
                Prefilter::None => true,
            };
            candidate && document.matches(node, &selector).unwrap_or(false)
        })));
    }
    document
}

/// The `charset` parameter of a `Content-Type`.
fn charset_of(content_type: &str) -> Option<&str> {
    content_type.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"'))
    })
}
