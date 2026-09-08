//! Applying a parse plan to a parsed SERP (`docs/spec/08-search.md` §2).
//!
//! The reference implementation of these semantics is
//! `providers/check_plans.py`; this module is the production one and is held
//! to the same output by the fixture tests. Selector errors cannot happen
//! here: every selector was compiled when the plan was loaded, so a failed
//! match is treated as "no match" rather than propagated.

use std::collections::HashSet;

use base64::Engine;
use tenta_core::NodeId;
use tenta_dom::Document;
use tenta_dom::extract::text::collapsed_text;
use url::Url;

use crate::error::BlockClass;
use crate::model::{Answer, Kind, Link, Provider, SearchResult};
use crate::plan::{Field, ParsePlan, Selector, UnwrapDecode};

/// Bytes of the body scanned for `[block]` patterns, as in
/// `providers/check_plans.py`.
pub const PATTERN_SAMPLE_BYTES: usize = 200_000;

/// What a plan pulled out of one SERP.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extraction {
    pub results: Vec<SearchResult>,
    pub related: Vec<String>,
    pub answer: Option<Answer>,
}

/// The block signal that fired, and the class it maps to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockSignal {
    pub class: BlockClass,
    pub detail: String,
}

/// Applies `plan` to `document`. `base` is the URL the document came from;
/// relative hrefs resolve against it.
pub fn extract(
    document: &Document,
    plan: &ParsePlan,
    base: &Url,
    provider: &Provider,
) -> Extraction {
    let rules = &plan.results;
    let root = document.document_node();
    let scope = rules
        .container
        .as_ref()
        .and_then(|selector| query_one(document, root, selector))
        .unwrap_or(root);

    let excluded: Vec<NodeId> = rules
        .exclude
        .iter()
        .flat_map(|selector| query_all(document, scope, selector))
        .collect();
    let excluded: HashSet<NodeId> = excluded.into_iter().collect();

    let items: Vec<NodeId> = query_all(document, scope, &rules.item)
        .into_iter()
        .filter(|item| !is_excluded(document, *item, &excluded))
        .collect();

    // Fields that live after the item rather than inside it (DuckDuckGo
    // Lite's table layout) need document order, which is otherwise never
    // materialised.
    let order = (!rules.following.is_empty()).then(|| DocumentOrder::of(document, scope));
    let following_nodes: Vec<(Field, Vec<NodeId>)> = rules
        .following
        .iter()
        .filter_map(|field| {
            let selector = match field {
                Field::Snippet => rules.snippet.as_ref(),
                Field::DisplayUrl => rules.display_url.as_ref(),
                Field::Date => rules.date.as_ref(),
            }?;
            Some((*field, query_all(document, scope, selector)))
        })
        .collect();

    let mut results = Vec::with_capacity(items.len());
    for (index, item) in items.iter().copied().enumerate() {
        let href = query_one(document, item, &rules.url)
            .and_then(|node| document.attribute_str(node, &rules.url_attr))
            .unwrap_or_default();
        let title = query_one(document, item, &rules.title)
            .map(|node| collapsed_text(document, node))
            .unwrap_or_default();
        if href.is_empty() || title.is_empty() {
            continue;
        }
        let Some(url) = destination(href, plan, base) else {
            continue;
        };

        let upper = items.get(index + 1).copied();
        let field_text = |field: Field, inside: Option<&Selector>| -> String {
            if rules.following.contains(&field) {
                let Some(order) = order.as_ref() else {
                    return String::new();
                };
                following_nodes
                    .iter()
                    .find(|(candidate, _)| *candidate == field)
                    .and_then(|(_, nodes)| order.between(nodes, item, upper))
                    .map(|node| collapsed_text(document, node))
                    .unwrap_or_default()
            } else {
                inside
                    .and_then(|selector| query_one(document, item, selector))
                    .map(|node| collapsed_text(document, node))
                    .unwrap_or_default()
            }
        };

        let snippet = field_text(Field::Snippet, rules.snippet.as_ref());
        let display_url = field_text(Field::DisplayUrl, rules.display_url.as_ref());
        let published = rules.date.as_ref().and_then(|selector| {
            let node = if rules.following.contains(&Field::Date) {
                order.as_ref().and_then(|order| {
                    following_nodes
                        .iter()
                        .find(|(field, _)| *field == Field::Date)
                        .and_then(|(_, nodes)| order.between(nodes, item, upper))
                })?
            } else {
                query_one(document, item, selector)?
            };
            let text = match &rules.date_attr {
                Some(attr) => document.attribute_str(node, attr)?.to_owned(),
                None => collapsed_text(document, node),
            };
            parse_date(&text)
        });

        let sitelinks = rules
            .sitelinks
            .as_ref()
            .map(|selector| {
                query_all(document, item, selector)
                    .into_iter()
                    .filter_map(|node| {
                        let title = collapsed_text(document, node);
                        let href = document.attribute_str(node, &rules.url_attr)?;
                        let url = destination(href, plan, base)?;
                        (!title.is_empty()).then_some(Link { title, url })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let kind = rules
            .kind
            .iter()
            .find(|(_, selector)| query_one(document, item, selector).is_some())
            .map(|(kind, _)| *kind)
            .unwrap_or(Kind::Organic);

        results.push(SearchResult {
            rank: results.len() as u32 + 1,
            title,
            url,
            display_url,
            snippet,
            published,
            sitelinks,
            source: provider.clone(),
            kind,
        });
    }

    let related = plan
        .related
        .as_ref()
        .map(|related| {
            let mut seen = HashSet::new();
            query_all(document, root, &related.item)
                .into_iter()
                .filter_map(|node| match &related.attr {
                    Some(attr) => document.attribute_str(node, attr).map(str::to_owned),
                    None => Some(collapsed_text(document, node)),
                })
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty() && seen.insert(text.clone()))
                .collect()
        })
        .unwrap_or_default();

    let answer = plan.answer.as_ref().and_then(|rules| {
        let scope = rules
            .container
            .as_ref()
            .and_then(|selector| query_one(document, root, selector))
            .unwrap_or(root);
        let text =
            query_one(document, scope, &rules.text).map(|node| collapsed_text(document, node))?;
        if text.is_empty() {
            return None;
        }
        let source = rules
            .source
            .as_ref()
            .and_then(|selector| query_one(document, scope, selector))
            .and_then(|node| document.attribute_str(node, &rules.source_attr))
            .and_then(|href| destination(href, plan, base));
        Some(Answer { text, source })
    });

    Extraction {
        results,
        related,
        answer,
    }
}

/// Whether the response is a refusal, and of which class
/// (`docs/spec/08-search.md` §4).
///
/// Body signals win over the status code: a 429 whose page says "flagged as
/// suspicious" is rate limiting, while a 503 carrying a CAPTCHA is a CAPTCHA.
pub fn detect_block(
    plan: &ParsePlan,
    document: &Document,
    body: &[u8],
    status: u16,
) -> Option<BlockSignal> {
    let root = document.document_node();
    for selector in &plan.block.selectors {
        if query_one(document, root, selector).is_some() {
            return Some(signal(plan, format!("selector {}", selector.as_str())));
        }
    }
    let sample = &body[..body.len().min(PATTERN_SAMPLE_BYTES)];
    for pattern in &plan.block.patterns {
        if contains_ignore_ascii_case(sample, pattern.as_bytes()) {
            return Some(signal(plan, format!("pattern {pattern:?}")));
        }
    }
    if plan.block.status.contains(&status) {
        // A status with no page signal is the provider throttling this exit
        // IP: 429 says so outright, 403 and 503 are how the front ends in
        // front of Brave and DuckDuckGo say the same thing.
        return Some(BlockSignal {
            class: plan.block.class.unwrap_or(BlockClass::RateLimited),
            detail: format!("HTTP {status}"),
        });
    }
    None
}

/// Detects a soft block: a SERP whose results do not echo any query term in
/// titles/snippets/URLs, and whose page does not echo the query in title/search-box.
/// This indicates a localised interstitial, consent page, or bot-score fallback
/// where the results are not relevant to the query (M-R4-1).
pub fn detect_soft_block(
    query: &str,
    results: &[SearchResult],
    document: &tenta_dom::Document,
) -> Option<BlockSignal> {
    if results.is_empty() {
        return None;
    }

    // Extract non-stopword terms from the query (use Unicode lowercase; match by prefix/stem).
    let terms = extract_query_terms(query);
    if terms.is_empty() {
        return None;
    }

    // Check if the page echoes the query in <title> or search-box input (M-R4-1).
    // DDG, Bing, Brave all echo the query; a correct SERP should show it somewhere.
    if page_echoes_query(document, query, &terms) {
        return None;
    }

    // Check if any result echoes any query term in title, snippet, or URL.
    for result in results {
        for term in &terms {
            let title = result.title.to_lowercase();
            let snippet = result.snippet.to_lowercase();
            let url = result.url.as_str().to_lowercase();

            if term_matches(&title, term)
                || term_matches(&snippet, term)
                || term_matches(&url, term)
            {
                // At least one result echoes a query term.
                return None;
            }
        }
    }

    // No result echoed any query term and page doesn't echo it either.
    Some(BlockSignal {
        class: BlockClass::Soft,
        detail: "results do not echo query terms".to_string(),
    })
}

/// Whether lower-cased `text` echoes a lower-cased query term.
///
/// A term of four or more characters matches on a stem: its first
/// `len - 2` characters, at least three and at most six, so the inflected
/// forms of an inflected language ("kotów", "kota") count as echoes of
/// "koty" and "kotów" alike. Shorter terms must appear whole. A lenient
/// stem only makes the gate fire less, which is the safe direction: a
/// missed soft block costs a bad result set, a false one reports a ban.
/// Lengths are in characters, not bytes: a byte prefix would split "kotów"
/// inside the "ó".
fn term_matches(text: &str, term: &str) -> bool {
    let chars = term.chars().count();
    if chars >= 4 {
        let stem: String = term.chars().take((chars - 2).clamp(3, 6)).collect();
        text.contains(stem.as_str())
    } else {
        text.contains(term)
    }
}

/// Checks if the page echoes the query in <title> or search-box input value (M-R4-1).
fn page_echoes_query(document: &tenta_dom::Document, query: &str, terms: &[String]) -> bool {
    let query_lower = query.to_lowercase();

    // Check page title
    if let Ok(Some(title_node)) = document.query_selector(document.document_node(), "title") {
        let title_text = document.text_content(title_node);
        let title_lower = title_text.to_lowercase();
        if title_lower.contains(&query_lower) {
            return true;
        }
        // Also check if title contains any query term
        for term in terms {
            if term_matches(&title_lower, term) {
                return true;
            }
        }
    }

    // Check search-box input value (common inputs: q, s, search, query, etc.)
    let search_selectors = &[
        "input[name='q']",
        "input[name='s']",
        "input[name='search']",
        "input[name='query']",
        "input.search-input",
        "input[placeholder*='search' i]",
        "input[placeholder*='Search' i]",
    ];

    for selector in search_selectors {
        if let Ok(Some(input_node)) = document.query_selector(document.document_node(), selector)
            && let Some(value) = document.attribute_str(input_node, "value")
        {
            let value_lower = value.to_lowercase();
            if value_lower.contains(&query_lower)
                || terms.iter().any(|term| term_matches(&value_lower, term))
            {
                return true;
            }
        }
    }

    false
}

/// Extracts non-stopword query terms for relevance checking.
/// Uses Unicode lowercase to handle inflected languages (e.g., Polish).
fn extract_query_terms(query: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "and", "or", "is", "in", "on", "at", "for", "to", "of", "by", "with",
    ];

    query
        .split_whitespace()
        .filter_map(|term| {
            let lower = term.to_lowercase();
            if lower.chars().count() > 2 && !STOPWORDS.contains(&lower.as_str()) {
                Some(lower)
            } else {
                None
            }
        })
        .collect()
}

fn signal(plan: &ParsePlan, detail: String) -> BlockSignal {
    BlockSignal {
        class: plan.block.class.unwrap_or_else(|| class_for_text(&detail)),
        detail,
    }
}

/// Derives the block class from the signal that fired. The plans name their
/// signals in the provider's own words, and those words are what distinguish
/// "turn on JavaScript" from a CAPTCHA from rate limiting.
fn class_for_text(detail: &str) -> BlockClass {
    const JS: &[&str] = &["enablejs", "javascript"];
    const RATE: &[&str] = &[
        "flagged",
        "suspicious",
        "rate limit",
        "too many requests",
        "429",
    ];
    let detail = detail.to_ascii_lowercase();
    let hit = |needles: &[&str]| needles.iter().any(|needle| detail.contains(needle));
    if hit(JS) {
        BlockClass::JsRequired
    } else if hit(RATE) {
        BlockClass::RateLimited
    } else {
        // Everything else the plans name is an interstitial in the way of the
        // results: `captcha`, `recaptcha`, `/sorry/`, DuckDuckGo's
        // `anomaly-modal`, Cloudflare's `challenge` and "Just a moment",
        // "unusual traffic" and its Polish wording. `[block] class`
        // overrides this when a provider invents something else.
        BlockClass::Captcha
    }
}

/// ASCII-case-insensitive substring search; `08` §3.4 keeps regex off this
/// path, and the plans' patterns are literals (checked at load).
pub fn contains_ignore_ascii_case(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    let lower = needle[0].to_ascii_lowercase();
    let upper = needle[0].to_ascii_uppercase();
    let mut start = 0;
    while start + needle.len() <= haystack.len() {
        let Some(offset) = memchr::memchr2(lower, upper, &haystack[start..]) else {
            return false;
        };
        let at = start + offset;
        if at + needle.len() > haystack.len() {
            return false;
        }
        if haystack[at..at + needle.len()].eq_ignore_ascii_case(needle) {
            return true;
        }
        start = at + 1;
    }
    false
}

/// Resolves a result href to the destination URL: protocol-relative and
/// relative hrefs against `base`, then the plan's redirect unwrapping, then
/// the tracking parameters the plan names.
pub fn destination(href: &str, plan: &ParsePlan, base: &Url) -> Option<Url> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    let resolved = base.join(href).ok()?;
    let rules = &plan.results;
    let mut url = match &rules.url_unwrap_param {
        Some(param) => {
            let wrapped = resolved
                .query_pairs()
                .find(|(name, _)| name == param.as_str())
                .map(|(_, value)| value.into_owned());
            match wrapped {
                Some(value) => {
                    let decoded = decode_wrapped(&value, &rules.url_unwrap_decode)?;
                    Url::parse(&decoded).or_else(|_| base.join(&decoded)).ok()?
                }
                None => resolved,
            }
        }
        None => resolved,
    };
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if !rules.drop_params.is_empty() && url.query().is_some() {
        let kept: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(name, _)| !rules.drop_params.iter().any(|drop| drop == name.as_ref()))
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        if kept.is_empty() {
            url.set_query(None);
        } else {
            let mut query = url.query_pairs_mut();
            query.clear();
            for (name, value) in kept {
                query.append_pair(&name, &value);
            }
        }
    }
    Some(url)
}

fn decode_wrapped(value: &str, decode: &UnwrapDecode) -> Option<String> {
    match decode {
        UnwrapDecode::None => Some(value.to_owned()),
        UnwrapDecode::BingA1Base64Url => {
            // `u=a1<base64url>`; anything else is already a plain URL.
            let Some(encoded) = value.strip_prefix("a1") else {
                return Some(value.to_owned());
            };
            let trimmed = encoded.trim_end_matches('=');
            let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(trimmed)
                .ok()?;
            Some(String::from_utf8_lossy(&bytes).into_owned())
        }
    }
}

const ISO_DATE: &[time::format_description::FormatItem<'static>] =
    time::macros::format_description!("[year]-[month]-[day]");

/// Parses the date a plan's `date` rule produced. ISO 8601 only: that is what
/// `<time datetime>` carries, and guessing at "3 weeks ago" would put a wrong
/// date in the output.
pub fn parse_date(text: &str) -> Option<time::Date> {
    let text = text.trim();
    if text.len() < 10 {
        return None;
    }
    time::Date::parse(text.get(..10)?, &ISO_DATE).ok()
}

fn is_excluded(document: &Document, item: NodeId, excluded: &HashSet<NodeId>) -> bool {
    if excluded.contains(&item) {
        return true;
    }
    document
        .ancestors(item)
        .any(|ancestor| excluded.contains(&ancestor))
}

fn query_one(document: &Document, root: NodeId, selector: &Selector) -> Option<NodeId> {
    document
        .query_selector(root, selector.as_str())
        .ok()
        .flatten()
}

fn query_all(document: &Document, root: NodeId, selector: &Selector) -> Vec<NodeId> {
    document
        .query_selector_all(root, selector.as_str())
        .unwrap_or_default()
}

/// Document-order indices for the nodes under one scope, built once when a
/// plan needs `following` fields.
struct DocumentOrder {
    index: std::collections::HashMap<NodeId, u32>,
}

impl DocumentOrder {
    fn of(document: &Document, scope: NodeId) -> Self {
        let mut index = std::collections::HashMap::new();
        for (position, node) in document.descendants_inclusive(scope).enumerate() {
            index.insert(node, position as u32);
        }
        Self { index }
    }

    fn position(&self, node: NodeId) -> Option<u32> {
        self.index.get(&node).copied()
    }

    /// The first candidate after `item` and before `next_item`.
    fn between(
        &self,
        candidates: &[NodeId],
        item: NodeId,
        next_item: Option<NodeId>,
    ) -> Option<NodeId> {
        let lower = self.position(item)?;
        let upper = next_item
            .and_then(|node| self.position(node))
            .unwrap_or(u32::MAX);
        candidates.iter().copied().find(|node| {
            self.position(*node)
                .is_some_and(|at| at > lower && at < upper)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{ParsePlan, PlanSet};

    fn plans() -> PlanSet {
        PlanSet::embedded().expect("plans")
    }

    fn destination_of(plan: &ParsePlan, href: &str) -> Option<String> {
        let base = plan.request.endpoint.clone();
        destination(href, plan, &base).map(|url| url.to_string())
    }

    #[test]
    fn duckduckgo_redirect_wrappers_are_unwrapped() {
        let plans = plans();
        let plan = plans.get("duckduckgo_html").expect("plan");
        assert_eq!(
            destination_of(
                plan,
                "//duckduckgo.com/l/?uddg=https%3A%2F%2Fservo.org%2F&rut=abc"
            )
            .as_deref(),
            Some("https://servo.org/")
        );
        // A direct href stays as it is.
        assert_eq!(
            destination_of(plan, "https://servo.org/download").as_deref(),
            Some("https://servo.org/download")
        );
        // Anything that is not http(s) after unwrapping is dropped.
        assert_eq!(
            destination_of(plan, "//duckduckgo.com/l/?uddg=javascript%3Aalert(1)"),
            None
        );
        assert_eq!(destination_of(plan, "   "), None);
    }

    #[test]
    fn bing_click_wrappers_are_base64url_decoded() {
        let plans = plans();
        let plan = plans.get("bing").expect("plan");
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode("https://rust-lang.org/tools/install");
        assert_eq!(
            destination_of(
                plan,
                &format!("https://www.bing.com/ck/a?!&&p=1&u=a1{encoded}")
            )
            .as_deref(),
            Some("https://rust-lang.org/tools/install")
        );
        // A `u` that is not an "a1" payload is taken as the URL itself.
        assert_eq!(
            destination_of(
                plan,
                "https://www.bing.com/ck/a?u=https%3A%2F%2Fservo.org%2F"
            )
            .as_deref(),
            Some("https://servo.org/")
        );
    }

    #[test]
    fn relative_hrefs_resolve_against_the_page_they_came_from() {
        let plans = plans();
        let plan = plans.get("brave").expect("plan");
        let base = url::Url::parse("https://search.brave.com/search?q=rust").expect("url");
        assert_eq!(
            destination("/next?offset=1", plan, &base).map(|url| url.to_string()),
            Some("https://search.brave.com/next?offset=1".to_owned())
        );
        assert_eq!(
            destination("//example.test/page", plan, &base).map(|url| url.to_string()),
            Some("https://example.test/page".to_owned())
        );
    }

    #[test]
    fn only_the_parameters_the_plan_names_are_dropped() {
        let mut plan = (**plans().get("brave").expect("plan")).clone();
        plan.results.drop_params = vec!["utm_source".into(), "ref".into()];
        assert_eq!(
            destination_of(&plan, "https://example.test/a?utm_source=serp&id=7&ref=x").as_deref(),
            Some("https://example.test/a?id=7")
        );
        assert_eq!(
            destination_of(&plan, "https://example.test/a?utm_source=serp").as_deref(),
            Some("https://example.test/a")
        );
    }

    #[test]
    fn dates_are_parsed_from_iso_8601_and_nothing_else() {
        use time::Month;
        assert_eq!(
            parse_date("2026-08-31T09:00:00Z"),
            Some(time::Date::from_calendar_date(2026, Month::August, 31).expect("date"))
        );
        assert_eq!(
            parse_date("  2026-01-02 "),
            Some(time::Date::from_calendar_date(2026, Month::January, 2).expect("date"))
        );
        assert_eq!(parse_date("3 weeks ago"), None);
        assert_eq!(parse_date("31 August 2026"), None);
        assert_eq!(parse_date("2026-13-01"), None);
        assert_eq!(parse_date("ą"), None, "a byte slice would panic here");
    }
}
