//! Collection of the document's stylesheets for the render session.
//!
//! The render session takes author CSS as a flat, ordered `Vec<StyleSource>`,
//! one per `<style>` or `<link rel=stylesheet>` element. This module builds
//! that list, fetching linked sheets and the `@import` targets of every sheet
//! (the engine's Stylo loader serves those from [`StyleSource::imports`]), and
//! tells the caller when the list may have changed ([`StyleSignature`]).
//!
//! Fetching is separated from assembling. [`collect_style_sources`] fetches
//! whatever the cache lacks and then assembles; [`StyleState::cached_sources`]
//! only assembles. A `<link>` whose body has not arrived is therefore absent
//! from the list rather than blocking it, and its arrival bumps
//! [`StyleState::completed_fetches`], which is part of the signature.
//!
//! Fetches are sequential and share the navigation's deadline. They are not
//! registered with the settle tracker: whoever calls into this module (the
//! screenshot and geometry paths) holds the settle itself. Scripts are only
//! bounded per request; the page keeps no cumulative network ledger, so the
//! byte and request counters here are the only cumulative charge against
//! `Budgets::network_bytes` and `Budgets::network_requests` per navigation.
//! A preferred-set choice by `title` is not modelled: every non-alternate
//! sheet applies. `crossorigin` is not sent (the script path does not either),
//! so it is not part of the signature.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the page's render entry points are the only callers"
    )
)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::time::{Duration, Instant};

use tenta_core::atom::atoms;
use tenta_core::{Budgets, FsPolicy, NodeId};
use tenta_css::cascade::{ImportedSheet, StyleSource, import_urls};
use tenta_dom::Document;
use tenta_html::{Encoding, QuirksMode};
use tenta_net::policy::cookies::Site;
use tenta_net::{Client, NetError, Request, ResourceType};
use url::Url;

use crate::clock::Clock;
use crate::fetch_policy::{FetchPolicy, HintDisposition, PreloadKind};

/// How deep `@import` chains are followed. A sheet linked from the document is
/// depth 0, so its imports are depth 1.
const MAX_IMPORT_DEPTH: u8 = 4;
/// How many distinct sheets one `<style>`/`<link>` may import, transitively.
const MAX_IMPORTS_PER_SOURCE: usize = 64;

/// What fetching a stylesheet needs from the page.
pub(crate) struct StyleContext<'a> {
    /// The page's network client, with its interceptors.
    pub client: &'a Client,
    /// Gate for every `file:` read; there is no other filesystem access.
    pub fs_policy: &'a FsPolicy,
    /// Bounds the bytes and the number of requests stylesheets may cost.
    pub budgets: &'a Budgets,
    pub clock: &'a dyn Clock,
    /// The page's header overrides, sent exactly as scripts send them.
    pub extra_headers: &'a [(String, String)],
    /// The document: initiator of every request, which decides `Referer`,
    /// `Origin` and `Sec-Fetch-Site`.
    pub document_url: &'a Url,
    /// A fetch that cannot finish before this is not issued.
    pub deadline: Instant,
    pub fetch_policy: FetchPolicy,
}

/// A stylesheet that could not be fetched, or a limit that cut a tree short.
/// The page keeps rendering without it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StyleFailure {
    pub url: Url,
    pub reason: String,
}

/// A hash of everything that can change the list of sheets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StyleSignature(u64);

impl StyleSignature {
    /// The signature of `doc` after `completed_fetches` stylesheet bodies have
    /// arrived.
    ///
    /// It covers the `<style>` and `<link rel~=stylesheet>` elements in tree
    /// order (node and generation, so a replaced element differs from the one
    /// it replaced), each `<style>`'s text, the attributes that decide whether
    /// and how a sheet applies, the base URL and the document encoding (a late
    /// `<meta charset>` restart changes how cached bodies are decoded). It
    /// deliberately ignores the document's mutation generation: most
    /// mutations cannot change a sheet.
    pub(crate) fn of(doc: &Document, completed_fetches: u64) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        completed_fetches.hash(&mut hasher);
        doc.url().map(Url::as_str).hash(&mut hasher);
        doc.base_url().map(Url::as_str).hash(&mut hasher);
        doc.encoding().hash(&mut hasher);
        (doc.quirks_mode() as u8).hash(&mut hasher);
        for id in doc.descendants(doc.document_node()) {
            let Some(kind) = element_kind(doc, id) else {
                continue;
            };
            doc.node_ref(id).hash(&mut hasher);
            for name in [
                atoms::HREF,
                atoms::REL,
                atoms::MEDIA,
                atoms::DISABLED,
                atoms::TYPE,
            ] {
                doc.attribute(id, name).hash(&mut hasher);
            }
            if kind == ElementKind::Style {
                style_text(doc, id).hash(&mut hasher);
            }
        }
        Self(hasher.finish())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ElementKind {
    Style,
    Link,
}

/// Which stylesheet-bearing element `id` is, if any. A `<link>` qualifies by
/// `rel`; whether it is a usable sheet is decided later.
fn element_kind(doc: &Document, id: NodeId) -> Option<ElementKind> {
    if doc.namespace(id)? != atoms::NS_HTML {
        return None;
    }
    match doc.tag(id)? {
        tag if tag == atoms::STYLE => Some(ElementKind::Style),
        tag if tag == atoms::LINK && rel_tokens(doc, id).any(|token| token == "stylesheet") => {
            Some(ElementKind::Link)
        }
        _ => None,
    }
}

fn rel_tokens(doc: &Document, id: NodeId) -> impl Iterator<Item = String> + '_ {
    doc.attribute(id, atoms::REL)
        .unwrap_or_default()
        .split_ascii_whitespace()
        .map(str::to_ascii_lowercase)
}

fn style_text(doc: &Document, id: NodeId) -> String {
    doc.children(id)
        .filter_map(|child| doc.text_data(child))
        .collect()
}

/// An empty or `text/css` `type` is a stylesheet; any other value is a
/// language this engine does not have.
fn type_is_css(doc: &Document, id: NodeId) -> bool {
    doc.attribute(id, atoms::TYPE)
        .map(str::trim)
        .is_none_or(|value| value.is_empty() || value.eq_ignore_ascii_case("text/css"))
}

fn media_of(doc: &Document, id: NodeId) -> Option<String> {
    doc.attribute(id, atoms::MEDIA)
        .map(str::trim)
        .filter(|media| !media.is_empty())
        .map(str::to_owned)
}

/// The cache key of a URL: a fragment is never sent, so it must not split the
/// cache.
fn key_of(url: &Url) -> Url {
    let mut key = url.clone();
    key.set_fragment(None);
    key
}

/// One element that contributes a sheet, in tree order.
enum Candidate {
    Inline {
        owner: NodeId,
        text: String,
        media: Option<String>,
    },
    Link {
        owner: NodeId,
        url: Url,
        media: Option<String>,
        disabled: bool,
    },
}

fn candidates(doc: &Document, base_url: &Url) -> Vec<Candidate> {
    let mut found = Vec::new();
    for owner in doc.descendants(doc.document_node()) {
        let Some(kind) = element_kind(doc, owner) else {
            continue;
        };
        if !type_is_css(doc, owner) {
            continue;
        }
        let media = media_of(doc, owner);
        match kind {
            ElementKind::Style => found.push(Candidate::Inline {
                owner,
                text: style_text(doc, owner),
                media,
            }),
            ElementKind::Link => {
                // An alternate sheet is not applied until the user picks it,
                // and nothing here can pick one.
                if rel_tokens(doc, owner).any(|token| token == "alternate") {
                    continue;
                }
                let Some(href) = doc
                    .attribute(owner, atoms::HREF)
                    .map(str::trim)
                    .filter(|href| !href.is_empty())
                else {
                    continue;
                };
                let Ok(url) = base_url.join(href) else {
                    continue;
                };
                found.push(Candidate::Link {
                    owner,
                    url: key_of(&url),
                    media,
                    disabled: doc.has_attribute(owner, atoms::DISABLED),
                });
            }
        }
    }
    found
}

/// The direct `@import` targets of `text`, as cache keys.
fn direct_imports(text: &str, sheet_url: &Url) -> Vec<Url> {
    import_urls(text, sheet_url).iter().map(key_of).collect()
}

/// A fetched, decoded sheet.
struct Sheet {
    /// The URL the body came from after redirects: the base for its `url()`s.
    url: Url,
    text: String,
    /// The encoding the body was decoded with, which its own imports inherit.
    encoding: &'static Encoding,
    /// Its direct `@import` targets.
    imports: Vec<Url>,
}

/// Per-navigation stylesheet state: fetched bodies by URL (without fragment),
/// how many fetches have completed, and what failed.
#[derive(Default)]
pub(crate) struct StyleState {
    /// `None` is a fetch that failed for good; it is remembered so a
    /// re-collect does not retry a 404 on every render. A fetch that was
    /// merely not attempted (deadline, budget) is absent, so a later collect
    /// may try again.
    sheets: HashMap<Url, Option<Sheet>>,
    completed: u64,
    bytes: u64,
    requests: u32,
    failures: Vec<StyleFailure>,
    /// Failures already reported, so a repeated collect does not repeat them.
    reported: HashSet<String>,
}

/// What a collect decides once per document rather than once per fetch.
struct Env {
    fallback: &'static Encoding,
    quirks: bool,
}

enum Fetch {
    Done(Result<Sheet, String>),
    /// Not attempted; a later collect may.
    Retry(String),
}

impl StyleState {
    /// How many stylesheet fetches have finished, successfully or not. With the
    /// document's mutation generation this is the whole cache key for "has
    /// anything changed since the last collect".
    pub(crate) fn completed_fetches(&self) -> u64 {
        self.completed
    }

    pub(crate) fn signature(&self, doc: &Document) -> StyleSignature {
        StyleSignature::of(doc, self.completed)
    }

    /// Failures since the last call, for the page to report as events.
    pub(crate) fn take_failures(&mut self) -> Vec<StyleFailure> {
        std::mem::take(&mut self.failures)
    }

    /// The sheets the cache can serve for `doc` right now. A linked sheet that
    /// is not cached, or failed, is absent; so is an import that is not.
    pub(crate) fn cached_sources(&self, doc: &Document, base_url: &Url) -> Vec<StyleSource> {
        let mut sources = Vec::new();
        for candidate in candidates(doc, base_url) {
            match candidate {
                Candidate::Inline { owner, text, media } => {
                    let imports = self.gather(&direct_imports(&text, base_url));
                    sources.push(StyleSource {
                        owner: Some(owner),
                        url: base_url.clone(),
                        text,
                        media,
                        disabled: false,
                        imports,
                    });
                }
                Candidate::Link {
                    owner,
                    url,
                    media,
                    disabled: true,
                } => sources.push(StyleSource {
                    owner: Some(owner),
                    url,
                    text: String::new(),
                    media,
                    disabled: true,
                    imports: Vec::new(),
                }),
                Candidate::Link {
                    owner, url, media, ..
                } => {
                    if let Some(Some(sheet)) = self.sheets.get(&url) {
                        sources.push(StyleSource {
                            owner: Some(owner),
                            url: sheet.url.clone(),
                            text: sheet.text.clone(),
                            media,
                            disabled: false,
                            imports: self.gather(&sheet.imports),
                        });
                    }
                }
            }
        }
        sources
    }

    /// Every cached sheet reachable from `direct` within the depth and count
    /// limits, once each. Order is irrelevant: the engine looks them up by URL.
    fn gather(&self, direct: &[Url]) -> Vec<ImportedSheet> {
        let mut gathered = Vec::new();
        let mut seen = HashSet::new();
        let mut queue: VecDeque<(Url, u8)> = direct.iter().cloned().map(|url| (url, 1)).collect();
        while let Some((url, depth)) = queue.pop_front() {
            if !seen.insert(url.clone()) {
                continue;
            }
            if seen.len() > MAX_IMPORTS_PER_SOURCE {
                break;
            }
            let Some(Some(sheet)) = self.sheets.get(&url) else {
                continue;
            };
            gathered.push(ImportedSheet {
                url,
                final_url: sheet.url.clone(),
                text: sheet.text.clone(),
            });
            if depth < MAX_IMPORT_DEPTH {
                queue.extend(sheet.imports.iter().cloned().map(|url| (url, depth + 1)));
            }
        }
        gathered
    }

    /// Fetches every import reachable from `direct` that the cache lacks,
    /// within the limits [`StyleState::gather`] applies. `root` names the
    /// source in failure reports.
    async fn load_imports(
        &mut self,
        ctx: &StyleContext<'_>,
        env: &Env,
        root: &Url,
        direct: Vec<Url>,
        inherited: &'static Encoding,
    ) {
        let mut seen = HashSet::new();
        let mut queue: VecDeque<(Url, u8, &'static Encoding)> =
            direct.into_iter().map(|url| (url, 1, inherited)).collect();
        while let Some((url, depth, encoding)) = queue.pop_front() {
            if !seen.insert(url.clone()) {
                continue;
            }
            if seen.len() > MAX_IMPORTS_PER_SOURCE {
                self.report_once(
                    root,
                    format!(
                        "more than {MAX_IMPORTS_PER_SOURCE} imported sheets; the rest are ignored"
                    ),
                );
                break;
            }
            self.ensure(ctx, env, &url, encoding).await;
            let Some(Some(sheet)) = self.sheets.get(&url) else {
                continue;
            };
            if sheet.imports.is_empty() {
                continue;
            }
            if depth < MAX_IMPORT_DEPTH {
                let encoding = sheet.encoding;
                queue.extend(
                    sheet
                        .imports
                        .iter()
                        .cloned()
                        .map(|url| (url, depth + 1, encoding)),
                );
            } else {
                self.report_once(
                    root,
                    format!("imports nest deeper than {MAX_IMPORT_DEPTH}; the deepest are ignored"),
                );
            }
        }
    }

    /// Fetches `url` into the cache unless it is already there.
    async fn ensure(
        &mut self,
        ctx: &StyleContext<'_>,
        env: &Env,
        url: &Url,
        fallback: &'static Encoding,
    ) {
        if self.sheets.contains_key(url) {
            return;
        }
        match self.fetch(ctx, env, url, fallback).await {
            Fetch::Done(outcome) => {
                self.completed += 1;
                match outcome {
                    Ok(sheet) => {
                        self.sheets.insert(url.clone(), Some(sheet));
                    }
                    Err(reason) => {
                        tracing::warn!(url = %url, %reason, "a stylesheet could not be loaded");
                        self.failures.push(StyleFailure {
                            url: url.clone(),
                            reason,
                        });
                        self.sheets.insert(url.clone(), None);
                    }
                }
            }
            Fetch::Retry(reason) => self.report_once(url, reason),
        }
    }

    fn report_once(&mut self, url: &Url, reason: String) {
        if self.reported.insert(format!("{url} {reason}")) {
            tracing::warn!(url = %url, %reason, "a stylesheet was not loaded");
            self.failures.push(StyleFailure {
                url: url.clone(),
                reason,
            });
        }
    }

    async fn fetch(
        &mut self,
        ctx: &StyleContext<'_>,
        env: &Env,
        url: &Url,
        fallback: &'static Encoding,
    ) -> Fetch {
        let remaining = ctx.deadline.saturating_duration_since(ctx.clock.now());
        if remaining.is_zero() {
            return Fetch::Retry("the navigation deadline has passed".to_owned());
        }
        if self.requests >= ctx.budgets.network_requests {
            return Fetch::Retry("the page's request budget is spent".to_owned());
        }
        let byte_limit = ctx.budgets.network_bytes.saturating_sub(self.bytes);
        if byte_limit == 0 {
            return Fetch::Retry("the page's network byte budget is spent".to_owned());
        }
        self.requests += 1;
        let loaded = match url.scheme() {
            "http" | "https" => read_remote(ctx, env, url, remaining, byte_limit).await,
            "file" => read_local(ctx, url, byte_limit).map(|body| (body, None, url.clone())),
            "data" => {
                read_data(url, byte_limit).map(|(body, charset)| (body, charset, url.clone()))
            }
            scheme => Err(format!(
                "the {scheme}: scheme is not loaded as a stylesheet"
            )),
        };
        Fetch::Done(loaded.map(|(body, charset, final_url)| {
            self.bytes += body.len() as u64;
            let (text, encoding) = decode_stylesheet(&body, charset.as_deref(), fallback);
            let imports = direct_imports(&text, &final_url);
            Sheet {
                url: final_url,
                text,
                encoding,
                imports,
            }
        }))
    }
}

/// Collects the sheets of `doc`, fetching the linked ones and every import the
/// cache lacks.
///
/// A fetch failure is recorded in `state` (see [`StyleState::take_failures`])
/// and leaves that sheet out; it never fails the collection. Sheets are fetched
/// one at a time in document order.
pub(crate) async fn collect_style_sources(
    doc: &Document,
    base_url: &Url,
    ctx: &StyleContext<'_>,
    state: &mut StyleState,
) -> Vec<StyleSource> {
    // A tier that only records style hints leaves the sheets for the upgrade.
    if ctx.fetch_policy.disposition(PreloadKind::Style) == HintDisposition::Issue {
        let env = Env {
            fallback: Encoding::for_label(doc.encoding().as_bytes()).unwrap_or_else(utf8),
            quirks: doc.quirks_mode() == QuirksMode::Quirks,
        };
        for candidate in candidates(doc, base_url) {
            match candidate {
                Candidate::Inline { text, .. } => {
                    let direct = direct_imports(&text, base_url);
                    state
                        .load_imports(ctx, &env, base_url, direct, env.fallback)
                        .await;
                }
                Candidate::Link {
                    url,
                    disabled: false,
                    ..
                } => {
                    state.ensure(ctx, &env, &url, env.fallback).await;
                    if let Some(Some(sheet)) = state.sheets.get(&url) {
                        let (direct, encoding) = (sheet.imports.clone(), sheet.encoding);
                        state.load_imports(ctx, &env, &url, direct, encoding).await;
                    }
                }
                Candidate::Link { .. } => {}
            }
        }
    }
    state.cached_sources(doc, base_url)
}

fn utf8() -> &'static Encoding {
    Encoding::for_label(b"utf-8").expect("utf-8 is a known label")
}

/// Reads a `file:` sheet. Only a `file:` document may load one, and only from
/// inside a read root of the page's `FsPolicy`.
fn read_local(ctx: &StyleContext<'_>, url: &Url, limit: u64) -> Result<Vec<u8>, String> {
    if ctx.document_url.scheme() != "file" {
        return Err("a file: stylesheet is only loaded for a file: document".to_owned());
    }
    let path = url
        .to_file_path()
        .map_err(|()| "the file: URL is not a local path".to_owned())?;
    let (file, length) = ctx.fs_policy.open_read(&path, limit).map_err(|error| {
        format!("the file could not be read under the filesystem policy: {error}")
    })?;
    let mut body = Vec::with_capacity(usize::try_from(length).unwrap_or(0));
    // The file may have grown since its length was read.
    file.take(limit.saturating_add(1))
        .read_to_end(&mut body)
        .map_err(|error| error.to_string())?;
    if body.len() as u64 > limit {
        return Err("the file exceeds the size limit".to_owned());
    }
    Ok(body)
}

/// Fetches a sheet through the page's client. Returns the body, the response's
/// declared charset and the URL it was finally served from.
async fn read_remote(
    ctx: &StyleContext<'_>,
    env: &Env,
    url: &Url,
    remaining: Duration,
    byte_limit: u64,
) -> Result<(Vec<u8>, Option<String>, Url), String> {
    let request = style_request(ctx, url, remaining, byte_limit)
        .map_err(|error| format!("a page header cannot go on the wire: {error:?}"))?;
    let response = ctx
        .client
        .send(request)
        .await
        .map_err(|error| format!("{error:?}"))?;
    if !matches!(response.url.scheme(), "http" | "https") {
        return Err(format!(
            "the sheet was served from a {}: URL",
            response.url.scheme()
        ));
    }
    if !response.status.is_success() {
        return Err(format!("the origin answered {}", response.status.as_u16()));
    }
    let content_type = response.header("content-type");
    let nosniff = response
        .header("x-content-type-options")
        .and_then(|value| value.split(',').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("nosniff"));
    let is_css = content_type.is_some_and(|value| {
        value
            .split(';')
            .next()
            .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("text/css"))
    });
    // Standards mode wants `text/css`; only quirks mode tolerates another type,
    // and nosniff forbids the tolerance.
    if !is_css && (nosniff || (content_type.is_some() && !env.quirks)) {
        return Err("the response is not served as text/css".to_owned());
    }
    let charset = content_type.and_then(charset_param);
    let final_url = key_of(&response.url);
    let body = response
        .body
        .collect()
        .await
        .map_err(|error| format!("{error:?}"))?;
    // The transport enforces the limit on the wire; a local answer (an
    // interceptor, the cache) is checked here.
    if body.len() as u64 > byte_limit {
        return Err("the response exceeds the page's remaining byte budget".to_owned());
    }
    Ok((body.to_vec(), charset, final_url))
}

/// The request for one stylesheet. The page's headers go on it as they go on a
/// script's, and the document is the initiator and the first party.
fn style_request(
    ctx: &StyleContext<'_>,
    url: &Url,
    remaining: Duration,
    byte_limit: u64,
) -> Result<Request, NetError> {
    let mut request = Request::new(http::Method::GET, url.clone())
        .resource_type(ResourceType::Stylesheet)
        .initiator(ctx.document_url.clone())
        .timeout(remaining)
        .max_body_bytes(byte_limit);
    request.first_party = Site::from_url(ctx.document_url);
    for (name, value) in ctx.extra_headers {
        request = request.header(name, value)?;
    }
    Ok(request)
}

/// Decodes a `data:` URL's payload and returns its declared charset.
fn read_data(url: &Url, limit: u64) -> Result<(Vec<u8>, Option<String>), String> {
    let rest = url
        .as_str()
        .strip_prefix("data:")
        .ok_or_else(|| "not a data: URL".to_owned())?;
    let rest = rest.split('#').next().unwrap_or(rest);
    let (meta, payload) = rest
        .split_once(',')
        .ok_or_else(|| "a data: URL has no payload".to_owned())?;
    let meta = meta.trim();
    let base64 = meta
        .rsplit(';')
        .next()
        .is_some_and(|last| last.trim().eq_ignore_ascii_case("base64"));
    let bytes = percent_decode(payload);
    let body = if base64 {
        decode_base64(&bytes).ok_or_else(|| "the data: URL's base64 is invalid".to_owned())?
    } else {
        bytes
    };
    if body.len() as u64 > limit {
        return Err("the data: URL exceeds the size limit".to_owned());
    }
    Ok((body, charset_param(meta)))
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = |byte: u8| char::from(byte).to_digit(16);
        match (bytes[index], bytes.get(index + 1), bytes.get(index + 2)) {
            (b'%', Some(&high), Some(&low)) if hex(high).is_some() && hex(low).is_some() => {
                decoded.push((hex(high).unwrap_or(0) * 16 + hex(low).unwrap_or(0)) as u8);
                index += 3;
            }
            (byte, _, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    decoded
}

/// The Fetch standard's forgiving base64 decode.
fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    let mut data: Vec<u8> = input
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if data.len().is_multiple_of(4) {
        for _ in 0..2 {
            if data.last() == Some(&b'=') {
                data.pop();
            }
        }
    }
    if data.len() % 4 == 1 {
        return None;
    }
    let mut decoded = Vec::with_capacity(data.len() * 3 / 4);
    let (mut accumulator, mut bits) = (0u32, 0u32);
    for byte in data {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            decoded.push((accumulator >> bits) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    Some(decoded)
}

/// The `charset` parameter of a `Content-Type` value or `data:` media type.
fn charset_param(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"').to_owned())
    })
}

/// Decodes a stylesheet by CSS Syntax's order: a byte-order mark, then the
/// transport's charset, then `@charset`, then the referencing document's (or
/// importing sheet's) encoding.
fn decode_stylesheet(
    body: &[u8],
    transport_charset: Option<&str>,
    fallback: &'static Encoding,
) -> (String, &'static Encoding) {
    if let Some((encoding, _)) = Encoding::for_bom(body) {
        return (
            encoding.decode_with_bom_removal(body).0.into_owned(),
            encoding,
        );
    }
    let encoding = transport_charset
        .and_then(|label| Encoding::for_label(label.as_bytes()))
        .or_else(|| at_charset(body))
        .unwrap_or(fallback);
    (
        encoding.decode_without_bom_handling(body).0.into_owned(),
        encoding,
    )
}

/// The encoding an `@charset "...";` rule at the very start of `body` names.
/// A UTF-16 label is read as UTF-8: the rule itself was legible as ASCII, so
/// the sheet cannot be UTF-16.
fn at_charset(body: &[u8]) -> Option<&'static Encoding> {
    let rest = body.strip_prefix(b"@charset \"")?;
    let end = rest.iter().position(|&byte| byte == b'"')?;
    if rest.get(end + 1) != Some(&b';') {
        return None;
    }
    let encoding = Encoding::for_label(&rest[..end])?;
    Some(if encoding.name().starts_with("UTF-16") {
        utf8()
    } else {
        encoding
    })
}

#[cfg(test)]
mod tests;
