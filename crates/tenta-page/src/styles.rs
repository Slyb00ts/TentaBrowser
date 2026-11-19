//! Collection of the document's stylesheets for the render session.
//!
//! The render session takes author CSS as a flat, ordered `Vec<StyleSource>`,
//! one per `<style>` or `<link rel=stylesheet>` element. This module builds
//! that list, fetching linked sheets and the `@import` targets of every sheet
//! (the engine's Stylo loader serves those from [`StyleSource::imports`]), and
//! tells the caller when the list may have changed ([`StyleSignature`]).
//!
//! Fetching is separated from assembling, and listing from fetching.
//! [`plan_fetches`] copies what a fetch needs out of the document,
//! [`fetch_planned`] fetches whatever the cache lacks without borrowing the
//! document, and [`StyleState::cached_sources`] only assembles. A `<link>`
//! whose body has not arrived is therefore absent
//! from the list rather than blocking it, and its arrival bumps
//! [`StyleState::completed_fetches`], which is part of the signature.
//!
//! Fetches are sequential and share the navigation's deadline. They are not
//! registered with the settle tracker: the screenshot path waits for them
//! itself, and a concurrent settle call does not count them. Scripts are only
//! bounded per request; the page keeps no cumulative network ledger, so the
//! byte and request counters here are the only cumulative charge against
//! `Budgets::network_bytes` and `Budgets::network_requests` per navigation.
//! A preferred-set choice by `title` is not modelled: every non-alternate
//! sheet applies. `crossorigin` is not sent (the script path does not either),
//! so it is not part of the signature.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::time::{Duration, Instant};

use tenta_core::atom::atoms;
use tenta_core::{Budgets, FsPolicy, NodeId};
use tenta_css::cascade::{ImportedSheet, StyleSource, import_prologue, import_urls};
use tenta_dom::Document;
use tenta_html::{Encoding, QuirksMode};
use tenta_net::policy::cookies::Site;
use tenta_net::request::{RequestMode, origin_of, same_origin};
use tenta_net::{Client, NetError, Request, ResourceType};
use url::Url;

use crate::clock::Clock;
use crate::fetch_policy::{FetchPolicy, HintDisposition, PreloadKind};

/// How deep `@import` chains are followed. A sheet linked from the document is
/// depth 0, so its imports are depth 1.
const MAX_IMPORT_DEPTH: u8 = 4;
/// How many distinct sheets one `<style>`/`<link>` may import, transitively.
const MAX_IMPORTS_PER_SOURCE: usize = 64;
/// The largest stylesheet body accepted.
const MAX_SHEET_BYTES: u64 = 8 * 1024 * 1024;
/// The most stylesheet bytes one navigation may fetch.
const MAX_NAVIGATION_SHEET_BYTES: u64 = 64 * 1024 * 1024;

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
    /// The page's network observation point, when it has one.
    pub net: Option<&'a crate::network::NetworkHub>,
    /// The document: initiator of every request, which decides `Referer`,
    /// `Origin` and `Sec-Fetch-Site`.
    pub document_url: &'a Url,
    /// A fetch that cannot finish before this is not issued.
    pub deadline: Instant,
    pub fetch_policy: FetchPolicy,
    /// The request is CORS-flagged (`<link crossorigin>`): it sends `Origin`,
    /// and a cross-origin answer without a matching `Access-Control-Allow-Origin`
    /// fails the sheet. The imports of such a sheet are CORS requests too.
    pub cors: bool,
}

/// A stylesheet that could not be fetched, or a limit that cut a tree short.
/// The page keeps rendering without it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StyleFailure {
    pub url: Url,
    pub reason: String,
}

/// What a memoised signature was computed for: the document, its mutation
/// generation, the completed fetches, and a hash of the URLs, encoding and mode.
type SignatureKey = (u64, u64, u64, u64, u64);

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
    ///
    /// `script` is the stamp of the document's script sheets
    /// (`ScriptSheets::stamp`): it moves when a sheet is edited through CSSOM
    /// or adopted, neither of which mutates the document.
    pub(crate) fn of(doc: &Document, completed_fetches: u64, script: u64) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        completed_fetches.hash(&mut hasher);
        script.hash(&mut hasher);
        doc.url().map(Url::as_str).hash(&mut hasher);
        doc.base_url().map(Url::as_str).hash(&mut hasher);
        doc.encoding().hash(&mut hasher);
        (doc.quirks_mode() as u8).hash(&mut hasher);
        for id in style_scan(doc) {
            let Some(kind) = element_kind(doc, id) else {
                continue;
            };
            doc.node_ref(id).hash(&mut hasher);
            // A sheet styles the tree it is in, so moving its element across
            // a shadow boundary changes what it applies to.
            if doc.has_shadow_trees() {
                doc.containing_shadow_root(id).hash(&mut hasher);
            }
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
/// The nodes that can carry a sheet that applies: the document's, and those
/// of every shadow tree once the document has one, in shadow-including tree
/// order.
fn style_scan(doc: &Document) -> Box<dyn Iterator<Item = NodeId> + '_> {
    if doc.has_shadow_trees() {
        Box::new(
            doc.shadow_including_subtree(doc.document_node())
                .into_iter(),
        )
    } else {
        Box::new(doc.descendants(doc.document_node()))
    }
}

/// The shadow root whose tree `owner` is in, which is what its sheet styles.
fn scope_of(doc: &Document, owner: NodeId) -> Option<NodeId> {
    doc.has_shadow_trees()
        .then(|| doc.containing_shadow_root(owner))
        .flatten()
}

enum Candidate {
    Inline {
        owner: NodeId,
        scope: Option<NodeId>,
        text: String,
        media: Option<String>,
    },
    Link {
        owner: NodeId,
        scope: Option<NodeId>,
        url: Url,
        media: Option<String>,
        disabled: bool,
    },
}

fn candidates(doc: &Document, base_url: &Url) -> Vec<Candidate> {
    let mut found = Vec::new();
    for owner in style_scan(doc) {
        let Some(kind) = element_kind(doc, owner) else {
            continue;
        };
        if !type_is_css(doc, owner) {
            continue;
        }
        let media = media_of(doc, owner);
        let scope = scope_of(doc, owner);
        match kind {
            ElementKind::Style => found.push(Candidate::Inline {
                owner,
                scope,
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
                    scope,
                    url: key_of(&url),
                    media,
                    disabled: doc.has_attribute(owner, atoms::DISABLED),
                });
            }
        }
    }
    found
}

/// A stylesheet-bearing element as the load-time loader sees it
/// (`page/sheet_loader.rs`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SheetSource {
    /// A `<link rel=stylesheet>` with a usable `href`, as a cache key, and
    /// whether the element asks for CORS (`crossorigin`).
    Link(Url, bool),
    /// A `<style>` whose text imports other sheets; the hash is of its text.
    Style(u64),
}

/// One element the loader has to act on.
#[derive(Clone, Debug)]
pub(crate) struct SheetElement {
    pub node: NodeId,
    pub source: SheetSource,
    /// `rel` has `alternate`: fetched for `link.sheet` and its events, but never
    /// applied, and it neither blocks scripts nor delays `load`.
    pub alternate: bool,
    /// The `media` attribute names an environment this one is in, so the sheet
    /// can hold script back while it loads.
    pub media_matches: bool,
    /// The `@import` targets of a `<style>`; empty for a link.
    pub imports: Vec<Url>,
}

/// The connected `<link rel=stylesheet>` and `<style>` elements the loader
/// fetches for, in tree order. A `disabled` link, a `type` other than CSS, a
/// link without a resolvable `href` and a `<style>` without `@import` have
/// nothing to fetch and are left out.
pub(crate) fn sheet_elements(
    doc: &Document,
    base_url: &Url,
    viewport: (f32, f32),
) -> Vec<SheetElement> {
    let mut found = Vec::new();
    // The tag index covers the document tree only; with shadow trees the
    // sheet-bearing elements inside them are found by walking.
    let nodes: Vec<NodeId> = if doc.has_shadow_trees() {
        doc.shadow_including_subtree(doc.document_node())
    } else {
        ["link", "style"]
            .into_iter()
            .flat_map(|tag| doc.get_elements_by_tag_name(tag))
            .collect()
    };
    for node in nodes {
        let Some(kind) = element_kind(doc, node) else {
            continue;
        };
        if !type_is_css(doc, node) {
            continue;
        }
        let media_matches = media_matches_viewport(media_of(doc, node).as_deref(), viewport);
        match kind {
            ElementKind::Link => {
                if doc.has_attribute(node, atoms::DISABLED) {
                    continue;
                }
                let Some(href) = doc
                    .attribute(node, atoms::HREF)
                    .map(str::trim)
                    .filter(|href| !href.is_empty())
                else {
                    continue;
                };
                let Ok(url) = base_url.join(href) else {
                    continue;
                };
                found.push(SheetElement {
                    node,
                    source: SheetSource::Link(
                        key_of(&url),
                        doc.has_attribute(node, atoms::CROSSORIGIN),
                    ),
                    alternate: rel_tokens(doc, node).any(|token| token == "alternate"),
                    media_matches,
                    imports: Vec::new(),
                });
            }
            ElementKind::Style => {
                let text = style_text(doc, node);
                let imports = direct_imports(&text, base_url);
                if imports.is_empty() {
                    continue;
                }
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                text.hash(&mut hasher);
                found.push(SheetElement {
                    node,
                    source: SheetSource::Style(hasher.finish()),
                    alternate: false,
                    media_matches,
                    imports,
                });
            }
        }
    }
    found.sort_by_key(|element| element.node);
    found
}

/// Whether a `media` attribute matches a screen of `viewport` CSS pixels.
///
/// This decides only whether a loading sheet holds scripts back; the render
/// session applies the real media rules to the sheet. Media types `all` and
/// `screen`, `not`/`only`, `and`, `(min|max-)width`, `(min|max-)height`,
/// `orientation`, the range form (`width >= 600px`) and the user-preference
/// features of a default light, no-motion screen are evaluated; any other
/// feature is false, as an unknown feature is in CSS.
pub(crate) fn media_matches_viewport(media: Option<&str>, viewport: (f32, f32)) -> bool {
    let Some(media) = media else {
        return true;
    };
    media.split(',').any(|query| {
        let query = query.trim().to_ascii_lowercase();
        let (negated, rest) = match query.strip_prefix("not ") {
            Some(rest) => (true, rest.trim()),
            None => (false, query.strip_prefix("only ").unwrap_or(&query).trim()),
        };
        let mut parts = rest.split(" and ").map(str::trim);
        let first = parts.next().unwrap_or("");
        let mut ok = true;
        let mut features: Vec<&str> = Vec::new();
        if first.starts_with('(') || first.is_empty() {
            if !first.is_empty() {
                features.push(first);
            }
        } else {
            ok = matches!(first, "all" | "screen");
        }
        features.extend(parts);
        ok = ok
            && features
                .iter()
                .all(|feature| feature_matches(feature, viewport));
        ok != negated
    })
}

fn css_length(text: &str) -> Option<f32> {
    let text = text.trim();
    if let Some(number) = text.strip_suffix("px") {
        number.trim().parse().ok()
    } else if let Some(number) = text.strip_suffix("rem").or_else(|| text.strip_suffix("em")) {
        number.trim().parse::<f32>().ok().map(|n| n * 16.0)
    } else if text == "0" {
        Some(0.0)
    } else {
        None
    }
}

fn feature_matches(feature: &str, (width, height): (f32, f32)) -> bool {
    let Some(inner) = feature.strip_prefix('(').and_then(|f| f.strip_suffix(')')) else {
        return false;
    };
    let inner = inner.trim();
    // The range form: `width >= 600px`, `400px <= width`, `height < 500px`.
    for op in [">=", "<=", ">", "<", "="] {
        if let Some((left, right)) = inner.split_once(op) {
            let (left, right) = (left.trim(), right.trim());
            let (name, value, flipped) = if left == "width" || left == "height" {
                (left, right, false)
            } else {
                (right, left, true)
            };
            let actual = match name {
                "width" => width,
                "height" => height,
                _ => return false,
            };
            let Some(value) = css_length(value) else {
                return false;
            };
            // `actual OP value`, mirrored when the name was on the right.
            return match (op, flipped) {
                (">=", false) | ("<=", true) => actual >= value,
                ("<=", false) | (">=", true) => actual <= value,
                (">", false) | ("<", true) => actual > value,
                ("<", false) | (">", true) => actual < value,
                _ => (actual - value).abs() < f32::EPSILON,
            };
        }
    }
    let (name, value) = match inner.split_once(':') {
        Some((name, value)) => (name.trim(), Some(value.trim())),
        None => (inner, None),
    };
    match (name, value) {
        ("width", Some(v)) => css_length(v).is_some_and(|v| (width - v).abs() < f32::EPSILON),
        ("min-width", Some(v)) => css_length(v).is_some_and(|v| width >= v),
        ("max-width", Some(v)) => css_length(v).is_some_and(|v| width <= v),
        ("height", Some(v)) => css_length(v).is_some_and(|v| (height - v).abs() < f32::EPSILON),
        ("min-height", Some(v)) => css_length(v).is_some_and(|v| height >= v),
        ("max-height", Some(v)) => css_length(v).is_some_and(|v| height <= v),
        ("orientation", Some("landscape")) => width >= height,
        ("orientation", Some("portrait")) => width < height,
        ("prefers-color-scheme", Some("light"))
        | ("prefers-reduced-motion", Some("no-preference"))
        | ("hover", Some("hover"))
        | ("pointer", Some("fine"))
        | ("display-mode", Some("browser"))
        | ("scripting", Some("enabled")) => true,
        ("color" | "width" | "height" | "hover" | "pointer", None) => true,
        _ => false,
    }
}

/// The direct `@import` targets of `text`, as cache keys.
pub(crate) fn direct_imports(text: &str, sheet_url: &Url) -> Vec<Url> {
    import_urls(text, sheet_url).iter().map(key_of).collect()
}

/// A fetched, decoded sheet.
struct Sheet {
    /// The URL the body came from after redirects: the base for its `url()`s.
    url: Url,
    text: std::rc::Rc<str>,
    /// Fetched with CORS and passed the check: readable by script whatever its
    /// origin.
    cors: bool,
    /// The encoding the body was decoded with, which its own imports inherit.
    encoding: &'static Encoding,
    /// Its direct `@import` targets.
    imports: Vec<Url>,
    /// The size of the body on the wire, charged against the byte budget.
    bytes: u64,
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
    /// URLs whose fetch is running right now, so two callers (the load-time
    /// loader and a screenshot) never fetch one sheet twice: the second waits
    /// for the first.
    in_flight: HashSet<Url>,
    /// Woken whenever a fetch in `in_flight` ends.
    arrived: std::rc::Rc<tokio::sync::Notify>,
    /// The signature the render session's sources were last collected for.
    pub(crate) applied: Option<StyleSignature>,
    /// The last signature computed and what it was computed for (document,
    /// mutation generation, completed fetches), so a script reading geometry in
    /// a loop does not walk the tree each time.
    memo: std::cell::Cell<Option<(SignatureKey, StyleSignature)>>,
}

/// What a collect decides once per document rather than once per fetch.
pub(crate) struct Env {
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
        // What `StyleSignature::of` reads besides the elements: the URLs, the
        // encoding and the mode can change without a mutation.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        doc.url().map(Url::as_str).hash(&mut hasher);
        doc.base_url().map(Url::as_str).hash(&mut hasher);
        doc.encoding().hash(&mut hasher);
        (doc.quirks_mode() as u8).hash(&mut hasher);
        let script = crate::script_sheets::ScriptSheets::of(doc.document_id())
            .map_or(0, |sheets| sheets.stamp());
        let key = (
            doc.document_id(),
            doc.mutation_generation(),
            self.completed_fetches(),
            hasher.finish(),
            script,
        );
        if let Some((memo_key, signature)) = self.memo.get()
            && memo_key == key
        {
            return signature;
        }
        let signature = StyleSignature::of(doc, self.completed_fetches(), script);
        self.memo.set(Some((key, signature)));
        signature
    }

    /// The decoded text of the sheet at `url` and the URL it came from, when it
    /// loaded.
    pub(crate) fn loaded_sheet(&self, url: &Url) -> Option<(Url, std::rc::Rc<str>, bool)> {
        match self.sheets.get(url) {
            Some(Some(sheet)) => Some((sheet.url.clone(), sheet.text.clone(), sheet.cors)),
            _ => None,
        }
    }

    /// Forgets that `url` failed, so the next link to it fetches it again.
    pub(crate) fn forget_failed(&mut self, url: &Url) {
        if matches!(self.sheets.get(url), Some(None)) {
            self.sheets.remove(url);
            self.reported
                .retain(|entry| !entry.starts_with(url.as_str()));
        }
    }

    /// Whether the sheet at `url` loaded (`Some(true)`), failed for good
    /// (`Some(false)`), or has not been fetched.
    pub(crate) fn sheet_state(&self, url: &Url) -> Option<bool> {
        self.sheets.get(url).map(Option::is_some)
    }

    /// Failures since the last call, for the page to report as events.
    pub(crate) fn take_failures(&mut self) -> Vec<StyleFailure> {
        std::mem::take(&mut self.failures)
    }

    /// The sheets the cache can serve for `doc` right now. A linked sheet that
    /// is not cached, or failed, is absent; so is an import that is not.
    pub(crate) fn cached_sources(&self, doc: &Document, base_url: &Url) -> Vec<StyleSource> {
        let script = crate::script_sheets::ScriptSheets::of(doc.document_id());
        // What script did to the sheet of `owner`, whose source is `text`
        // (from `href`, for a link): the text to parse in its place, and
        // whether script turned the sheet off.
        let edited = |owner: NodeId, href: Option<&Url>, text: &str| {
            let edit = script.as_ref()?.element(
                doc.node_ref(owner),
                tenta_js::stylesheets::source_fingerprint(href, text),
            )?;
            // The script view of a sheet has no `@import` rules, so the ones
            // the source began with are kept as they were written.
            let text = edit
                .text
                .map(|rules| format!("{}\n{rules}", import_prologue(text)));
            Some((text, edit.disabled))
        };
        let mut sources = Vec::new();
        for candidate in candidates(doc, base_url) {
            match candidate {
                Candidate::Inline {
                    owner,
                    scope,
                    text,
                    media,
                } => {
                    let (text, disabled) = match edited(owner, None, &text) {
                        Some((edited, disabled)) => (edited.unwrap_or(text), disabled),
                        None => (text, false),
                    };
                    let imports = self.gather(&direct_imports(&text, base_url));
                    sources.push(StyleSource {
                        owner: Some(owner),
                        scope,
                        url: base_url.clone(),
                        text,
                        media,
                        disabled,
                        imports,
                    });
                }
                Candidate::Link {
                    owner,
                    scope,
                    url,
                    media,
                    disabled: true,
                } => sources.push(StyleSource {
                    owner: Some(owner),
                    scope,
                    url,
                    text: String::new(),
                    media,
                    disabled: true,
                    imports: Vec::new(),
                }),
                Candidate::Link {
                    owner,
                    scope,
                    url,
                    media,
                    ..
                } => {
                    if let Some(Some(sheet)) = self.sheets.get(&url) {
                        let (text, disabled) = match edited(owner, Some(&sheet.url), &sheet.text) {
                            Some((edited, disabled)) => {
                                (edited.unwrap_or_else(|| sheet.text.to_string()), disabled)
                            }
                            None => (sheet.text.to_string(), false),
                        };
                        sources.push(StyleSource {
                            owner: Some(owner),
                            scope,
                            url: sheet.url.clone(),
                            text,
                            media,
                            disabled,
                            imports: self.gather(&sheet.imports),
                        });
                    }
                }
            }
        }
        // Adopted sheets apply after the sheets of the tree they are adopted
        // into, and trees do not order against each other, so the end of the
        // list is after for all of them. A constructed sheet has no owner
        // element, no `@import` and the document's base URL.
        if let Some(script) = &script {
            for sheet in script.adopted() {
                let scope = match sheet.scope {
                    Some(root) => match doc.resolve(root) {
                        Ok(root) => Some(root),
                        // The shadow root is gone, and its sheets with it.
                        Err(_) => continue,
                    },
                    None => None,
                };
                sources.push(StyleSource {
                    owner: None,
                    scope,
                    url: base_url.clone(),
                    text: sheet.text,
                    media: None,
                    disabled: sheet.disabled,
                    imports: Vec::new(),
                });
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
                text: sheet.text.to_string(),
            });
            if depth < MAX_IMPORT_DEPTH {
                queue.extend(sheet.imports.iter().cloned().map(|url| (url, depth + 1)));
            }
        }
        gathered
    }
}

/// Releases the claim on a URL's fetch when the fetch ends or is dropped, and
/// wakes whoever waited for it.
struct FetchClaim<'a> {
    state: &'a RefCell<StyleState>,
    url: &'a Url,
}

impl Drop for FetchClaim<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.in_flight.remove(self.url);
            state.arrived.notify_waiters();
        }
    }
}

/// Fetches every import reachable from `direct` that the cache lacks, within
/// the limits [`StyleState::gather`] applies. `root` names the source in
/// failure reports. The state is only borrowed between awaits.
pub(crate) async fn load_imports(
    state: &RefCell<StyleState>,
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
            state.borrow_mut().report_once(
                root,
                format!("more than {MAX_IMPORTS_PER_SOURCE} imported sheets; the rest are ignored"),
            );
            break;
        }
        ensure(state, ctx, env, &url, encoding).await;
        let next = {
            let state = state.borrow();
            let Some(Some(sheet)) = state.sheets.get(&url) else {
                continue;
            };
            (!sheet.imports.is_empty()).then(|| (sheet.imports.clone(), sheet.encoding))
        };
        let Some((imports, encoding)) = next else {
            continue;
        };
        if depth < MAX_IMPORT_DEPTH {
            queue.extend(imports.into_iter().map(|url| (url, depth + 1, encoding)));
        } else {
            state.borrow_mut().report_once(
                root,
                format!("imports nest deeper than {MAX_IMPORT_DEPTH}; the deepest are ignored"),
            );
        }
    }
}

/// Fetches `url` into the cache unless it is already there. A fetch another
/// caller has running is waited for, not repeated.
async fn ensure(
    state: &RefCell<StyleState>,
    ctx: &StyleContext<'_>,
    env: &Env,
    url: &Url,
    fallback: &'static Encoding,
) {
    loop {
        let arrived = {
            let mut guard = state.borrow_mut();
            // A sheet fetched without CORS does not satisfy a CORS request: it
            // was sent without `Origin` and was not checked.
            let cached = match guard.sheets.get(url) {
                Some(Some(sheet)) => !ctx.cors || sheet.cors,
                Some(None) => true,
                None => false,
            };
            if cached {
                return;
            }
            if guard.in_flight.insert(url.clone()) {
                break;
            }
            std::rc::Rc::clone(&guard.arrived)
        };
        let notified = arrived.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        // The fetch may have ended between the borrow above and the
        // registration.
        if !state.borrow().in_flight.contains(url) {
            continue;
        }
        notified.await;
    }
    let _claim = FetchClaim { state, url };
    match fetch(state, ctx, env, url, fallback).await {
        Fetch::Done(outcome) => {
            let mut state = state.borrow_mut();
            state.completed += 1;
            match outcome {
                Ok(sheet) => {
                    state.bytes += sheet.bytes;
                    state.sheets.insert(url.clone(), Some(sheet));
                }
                Err(reason) => {
                    tracing::warn!(url = %url, %reason, "a stylesheet could not be loaded");
                    state.failures.push(StyleFailure {
                        url: url.clone(),
                        reason,
                    });
                    state.sheets.insert(url.clone(), None);
                }
            }
        }
        Fetch::Retry(reason) => state.borrow_mut().report_once(url, reason),
    }
}

impl StyleState {
    fn report_once(&mut self, url: &Url, reason: String) {
        if self.reported.insert(format!("{url} {reason}")) {
            tracing::warn!(url = %url, %reason, "a stylesheet was not loaded");
            self.failures.push(StyleFailure {
                url: url.clone(),
                reason,
            });
        }
    }

    /// Charges one request against the budgets, or says why none may be made.
    fn reserve(&mut self, ctx: &StyleContext<'_>) -> Result<(Duration, u64), String> {
        let remaining = ctx.deadline.saturating_duration_since(ctx.clock.now());
        if remaining.is_zero() {
            return Err("the navigation deadline has passed".to_owned());
        }
        if self.requests >= ctx.budgets.network_requests {
            return Err("the page's request budget is spent".to_owned());
        }
        let byte_limit = ctx
            .budgets
            .network_bytes
            .saturating_sub(self.bytes)
            .min(MAX_NAVIGATION_SHEET_BYTES.saturating_sub(self.bytes))
            .min(MAX_SHEET_BYTES);
        if byte_limit == 0 {
            return Err("the page's network byte budget is spent".to_owned());
        }
        self.requests += 1;
        Ok((remaining, byte_limit))
    }
}

async fn fetch(
    state: &RefCell<StyleState>,
    ctx: &StyleContext<'_>,
    env: &Env,
    url: &Url,
    fallback: &'static Encoding,
) -> Fetch {
    let (remaining, byte_limit) = match state.borrow_mut().reserve(ctx) {
        Ok(reserved) => reserved,
        Err(reason) => return Fetch::Retry(reason),
    };
    let loaded = match url.scheme() {
        "http" | "https" => read_remote(ctx, env, url, remaining, byte_limit).await,
        "file" => read_local(ctx, url, byte_limit).map(|body| (body, None, url.clone())),
        "data" => read_data(url, byte_limit).map(|(body, charset)| (body, charset, url.clone())),
        scheme => Err(format!(
            "the {scheme}: scheme is not loaded as a stylesheet"
        )),
    };
    Fetch::Done(loaded.map(|(body, charset, final_url)| {
        let (text, encoding) = decode_stylesheet(&body, charset.as_deref(), fallback);
        let imports = direct_imports(&text, &final_url);
        Sheet {
            url: final_url,
            text: text.into(),
            cors: ctx.cors,
            encoding,
            imports,
            bytes: body.len() as u64,
        }
    }))
}

/// What fetching the sheets of a document needs from the document, copied out
/// so the document is not borrowed while the fetches are awaited.
pub(crate) struct FetchPlan {
    env: Env,
    candidates: Vec<Candidate>,
}

impl Env {
    /// The encoding a sheet's own imports inherit.
    pub(crate) fn fallback(&self) -> &'static Encoding {
        self.fallback
    }

    /// What fetching for `doc` decides once: the encoding its sheets inherit and
    /// whether MIME tolerance applies.
    /// `encoding` is the parser's choice while the parse is still running (the
    /// document only records it at the end).
    pub(crate) fn of(doc: &Document, encoding: Option<&'static Encoding>) -> Env {
        Env {
            fallback: encoding
                .or_else(|| Encoding::for_label(doc.encoding().as_bytes()))
                .unwrap_or_else(utf8),
            quirks: doc.quirks_mode() == QuirksMode::Quirks,
        }
    }
}

/// Lists what a fetch for `doc` would load. `None` when the tier only records
/// style hints and leaves the sheets for the upgrade.
pub(crate) fn plan_fetches(
    doc: &Document,
    base_url: &Url,
    ctx: &StyleContext<'_>,
) -> Option<FetchPlan> {
    if ctx.fetch_policy.disposition(PreloadKind::Style) != HintDisposition::Issue {
        return None;
    }
    Some(FetchPlan {
        env: Env::of(doc, None),
        candidates: candidates(doc, base_url),
    })
}

impl FetchPlan {
    /// The linked sheets of the plan that `state` neither holds nor has given
    /// up on: what a fetch would still load.
    pub(crate) fn unfetched(&self, state: &StyleState) -> Vec<Url> {
        self.candidates
            .iter()
            .filter_map(|candidate| match candidate {
                Candidate::Link {
                    url,
                    disabled: false,
                    ..
                } if !state.sheets.contains_key(url) => Some(url.clone()),
                _ => None,
            })
            .collect()
    }
}

/// Fetches the linked sheets of `plan` and every import the cache lacks.
///
/// A fetch failure is recorded in `state` (see [`StyleState::take_failures`])
/// and leaves that sheet out; it never fails the fetch. Sheets are fetched one
/// at a time in document order. `state` is only borrowed between awaits, so the
/// load-time loader and the geometry host share it.
pub(crate) async fn fetch_planned(
    plan: FetchPlan,
    base_url: &Url,
    ctx: &StyleContext<'_>,
    state: &RefCell<StyleState>,
) {
    let FetchPlan { env, candidates } = plan;
    for candidate in candidates {
        match candidate {
            Candidate::Inline { text, .. } => {
                let direct = direct_imports(&text, base_url);
                load_imports(state, ctx, &env, base_url, direct, env.fallback).await;
            }
            Candidate::Link {
                url,
                disabled: false,
                ..
            } => {
                fetch_root(state, ctx, &env, &url).await;
            }
            Candidate::Link { .. } => {}
        }
    }
}

/// Fetches the linked sheet at `url` and every import below it.
pub(crate) async fn fetch_root(
    state: &RefCell<StyleState>,
    ctx: &StyleContext<'_>,
    env: &Env,
    url: &Url,
) {
    ensure(state, ctx, env, url, env.fallback).await;
    let next = match state.borrow().sheets.get(url) {
        Some(Some(sheet)) => Some((sheet.imports.clone(), sheet.encoding)),
        _ => None,
    };
    if let Some((direct, encoding)) = next {
        load_imports(state, ctx, env, url, direct, encoding).await;
    }
}

/// Collects the sheets of `doc` in one call, for tests that hold the document
/// across the fetches. The page fetches through [`plan_fetches`] and
/// [`fetch_planned`] and assembles with [`StyleState::cached_sources`], so a
/// script engine's document is never borrowed across an await.
#[cfg(test)]
pub(crate) async fn collect_style_sources(
    doc: &Document,
    base_url: &Url,
    ctx: &StyleContext<'_>,
    state: &RefCell<StyleState>,
) -> Vec<StyleSource> {
    if let Some(plan) = plan_fetches(doc, base_url, ctx) {
        fetch_planned(plan, base_url, ctx, state).await;
    }
    state.borrow().cached_sources(doc, base_url)
}

fn utf8() -> &'static Encoding {
    Encoding::for_label(b"utf-8").expect("utf-8 is a known label")
}

/// Reads a `file:` sheet. Only a `file:` document may load one, and only from
/// inside a read root of the page's `FsPolicy`.
fn read_local(ctx: &StyleContext<'_>, url: &Url, limit: u64) -> Result<Vec<u8>, String> {
    read_local_file(ctx.document_url, ctx.fs_policy, url, limit, "stylesheet")
}

/// Reads the `file:` subresource `url` of the document at `document_url`
/// (`what` names it in errors): only a `file:` document may load one, only from
/// inside a read root of `fs_policy`, never following links, never truncated.
pub(crate) fn read_local_file(
    document_url: &Url,
    fs_policy: &FsPolicy,
    url: &Url,
    limit: u64,
    what: &str,
) -> Result<Vec<u8>, String> {
    if document_url.scheme() != "file" {
        return Err(format!(
            "a file: {what} is only loaded for a file: document"
        ));
    }
    let path = url
        .to_file_path()
        .map_err(|()| "the file: URL is not a local path".to_owned())?;
    let (file, length) = fs_policy.open_read(&path, limit).map_err(|error| {
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
    let mut request = style_request(ctx, url, remaining, byte_limit)
        .map_err(|error| format!("a page header cannot go on the wire: {error:?}"))?;
    let mut tracked = ctx
        .net
        .and_then(|net| net.begin(&mut request, crate::page::next_request_id(), None));
    let response = match ctx.client.send(request).await {
        Ok(response) => response,
        Err(error) => {
            if let Some(tracked) = &mut tracked {
                tracked.failed(&error);
            }
            return Err(format!("{error:?}"));
        }
    };
    if let Some(tracked) = &mut tracked {
        tracked.response(&response);
    }
    if !matches!(response.url.scheme(), "http" | "https") {
        return Err(format!(
            "the sheet was served from a {}: URL",
            response.url.scheme()
        ));
    }
    if !response.status.is_success() {
        let status = response.status.as_u16();
        finish_unread(&mut tracked, response).await;
        return Err(format!("the origin answered {status}"));
    }
    if ctx.cors && !same_origin(&response.url, ctx.document_url) {
        let allowed = response
            .header("access-control-allow-origin")
            .map(str::trim)
            .is_some_and(|value| {
                value == "*" || origin_of(ctx.document_url).is_some_and(|origin| origin == value)
            });
        if !allowed {
            finish_unread(&mut tracked, response).await;
            return Err(
                "the CORS check failed: no matching Access-Control-Allow-Origin".to_owned(),
            );
        }
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
    // Standards mode wants `text/css` when a type is given (Chrome 154 accepts
    // a missing one); only quirks mode tolerates another type, and only for a
    // sheet of the document's own origin; nosniff forbids the tolerance.
    let tolerated = env.quirks && !nosniff && same_origin(&response.url, ctx.document_url);
    if !is_css && (nosniff || (content_type.is_some() && !tolerated)) {
        finish_unread(&mut tracked, response).await;
        return Err("the response is not served as text/css".to_owned());
    }
    let charset = content_type.and_then(charset_param);
    let final_url = key_of(&response.url);
    let body = response.body.collect().await.map_err(|error| {
        if let Some(tracked) = &mut tracked {
            tracked.failed(&error);
        }
        format!("{error:?}")
    })?;
    if let Some(tracked) = &mut tracked {
        tracked.complete(&body);
    }
    // The transport enforces the limit on the wire; a local answer (an
    // interceptor, the cache) is checked here.
    if body.len() as u64 > byte_limit {
        return Err("the response exceeds the page's remaining byte budget".to_owned());
    }
    Ok((body.to_vec(), charset, final_url))
}

/// Ends a tracked request whose body the page does not use: it is read only
/// when a network host watches the traffic.
async fn finish_unread(
    tracked: &mut Option<crate::network::Tracked>,
    response: tenta_net::Response,
) {
    if let Some(tracked) = tracked
        && let Ok(body) = response.body.collect().await
    {
        tracked.complete(&body);
    }
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
    if ctx.cors {
        request.mode = RequestMode::Cors;
    }
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
