//! Parse plans: the data that maps a provider's public SERP to results
//! (`docs/spec/08-search.md` §2, format in `providers/README.md`).
//!
//! Plans are the one place site-specific knowledge lives, and they are data,
//! not code (AGENTS.md). The plans shipped with the engine are embedded at
//! build time with `include_str!` so a binary needs no files next to it;
//! [`PlanSet::with_dir`] adds or replaces plans from a directory, which is
//! how `Provider::Custom` and the tests reach their own.
//!
//! Everything a plan says is validated once, at load: selectors are compiled
//! through `tenta-dom`'s selector engine, templates may only use the
//! placeholders the model can fill, block patterns must be literals (there is
//! no regex on the hot path, `08` §3.4) and a plan whose `path` is `api` is
//! refused outright, because `search` only ever uses the providers' public
//! pages (O-7).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use tenta_core::Atom;
use url::Url;

use crate::error::{PlanError, SearchError};
use crate::model::{Freshness, Kind, Locale, Provider, SafeSearch, SearchRequest};

/// The plans that ship with the engine, embedded at build time.
const EMBEDDED: &[(&str, &str)] = &[
    (
        "bing",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../providers/plans/bing.toml"
        )),
    ),
    (
        "brave",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../providers/plans/brave.toml"
        )),
    ),
    (
        "duckduckgo_html",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../providers/plans/duckduckgo_html.toml"
        )),
    ),
    (
        "duckduckgo_lite",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../providers/plans/duckduckgo_lite.toml"
        )),
    ),
    (
        "google_chromium",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../providers/plans/google_chromium.toml"
        )),
    ),
    (
        "google_nojs",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../providers/plans/google_nojs.toml"
        )),
    ),
];

/// Which path the plan describes (`docs/spec/08-search.md` §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanPath {
    /// One request to the public page, no JavaScript.
    Fast,
    /// A page load with JavaScript (P2).
    Full,
}

/// A validated selector: the source string plus the cheap test that rejects
/// most elements before the selector engine runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    source: String,
    prefilter: Prefilter,
}

impl Selector {
    /// Compiles `source` to validate it, and derives the prefilter.
    ///
    /// Matching itself goes through `Document::query_selector*`, which keeps
    /// its own compiled-selector cache per document, so a selector is parsed
    /// once per response rather than once per result.
    pub fn compile(plan: &str, source: &str) -> Result<Self, PlanError> {
        let source = source.trim();
        if source.is_empty() {
            return Err(PlanError::new(plan, "empty selector"));
        }
        tenta_dom::selectors::compile(source)
            .map_err(|err| PlanError::new(plan, format!("selector {source:?}: {err}")))?;
        Ok(Self {
            source: source.to_owned(),
            prefilter: Prefilter::of(source),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.source
    }

    pub fn prefilter(&self) -> &Prefilter {
        &self.prefilter
    }
}

/// The rightmost-compound test used before the selector engine runs. It is
/// what makes the streaming parser's stop predicate (`08` §3.3) cost an
/// integer comparison per closed element instead of a selector match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prefilter {
    Id(Atom),
    Class(Atom),
    Tag(Atom),
    /// No usable hint: the selector engine decides on its own.
    None,
}

impl Prefilter {
    fn of(selector: &str) -> Prefilter {
        if selector.contains(',') {
            return Prefilter::None;
        }
        let stripped = strip_pseudo_arguments(selector);
        // Attribute and pseudo-class arguments left blanks behind; the
        // rightmost compound is what follows the last real combinator.
        let stripped = stripped.trim();
        let compound = match stripped.rfind([' ', '>', '+', '~']) {
            Some(position) => stripped[position + 1..].trim(),
            None => stripped.trim(),
        };
        if let Some(id) = pick(compound, '#') {
            return Prefilter::Id(Atom::new(&id));
        }
        if let Some(class) = pick(compound, '.') {
            return Prefilter::Class(Atom::new(&class));
        }
        let tag: String = compound
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if tag.is_empty() {
            Prefilter::None
        } else {
            Prefilter::Tag(Atom::new(&tag.to_ascii_lowercase()))
        }
    }
}

fn pick(compound: &str, marker: char) -> Option<String> {
    let start = compound.find(marker)? + marker.len_utf8();
    let name: String = compound[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Replaces the contents of `(...)` and `[...]` with spaces so the compound
/// scan never mistakes a pseudo-class argument for the selector's own tail.
fn strip_pseudo_arguments(selector: &str) -> String {
    let mut out = String::with_capacity(selector.len());
    let mut depth = 0usize;
    for character in selector.chars() {
        match character {
            '(' | '[' => {
                depth += 1;
                out.push(' ');
            }
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                out.push(' ');
            }
            _ if depth > 0 => out.push(' '),
            _ => out.push(character),
        }
    }
    out
}

/// How the plan gets from a result element to the destination URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnwrapDecode {
    /// The parameter holds the URL directly (percent-decoded by the parser).
    None,
    /// Bing's `u=a1<base64url>` click wrapper.
    BingA1Base64Url,
}

/// Where the SafeSearch setting goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SafeCarrier {
    Param(String),
    Cookie(String),
}

/// `[request.safe]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafePlan {
    pub carrier: SafeCarrier,
    pub strict: String,
    pub moderate: String,
    pub off: String,
}

impl SafePlan {
    fn value(&self, safe: SafeSearch) -> &str {
        match safe {
            SafeSearch::Strict => &self.strict,
            SafeSearch::Moderate => &self.moderate,
            SafeSearch::Off => &self.off,
        }
    }
}

/// `[request.freshness]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreshnessPlan {
    pub param: String,
    pub day: String,
    pub week: String,
    pub month: String,
    pub year: String,
}

impl FreshnessPlan {
    fn value(&self, freshness: Freshness) -> &str {
        match freshness {
            Freshness::Day => &self.day,
            Freshness::Week => &self.week,
            Freshness::Month => &self.month,
            Freshness::Year => &self.year,
        }
    }
}

/// `[request.page]`: the offset parameter for page N.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PagePlan {
    pub first: i64,
    pub step: i64,
}

/// `[request]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestPlan {
    pub endpoint: Url,
    pub post: bool,
    pub query_param: String,
    pub params: BTreeMap<String, String>,
    pub locale: BTreeMap<String, String>,
    pub safe: Option<SafePlan>,
    pub freshness: Option<FreshnessPlan>,
    pub page: Option<(String, PagePlan)>,
    pub site_param: Option<String>,
    pub headers: BTreeMap<String, String>,
}

/// `[cookies] policy`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CookiePolicy {
    /// Never send the provider's cookies back.
    #[default]
    None,
    /// Keep them for the warm pool's lifetime.
    Session,
    /// Keep them through the session cache.
    Persist,
}

/// `[cookies]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CookiePlan {
    pub policy: CookiePolicy,
    pub preset: BTreeMap<String, String>,
}

/// `[results]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultsPlan {
    /// Extraction scope and the streaming parser's early-stop marker.
    pub container: Option<Selector>,
    pub item: Selector,
    pub exclude: Vec<Selector>,
    pub title: Selector,
    pub url: Selector,
    pub url_attr: String,
    pub url_unwrap_param: Option<String>,
    pub url_unwrap_decode: UnwrapDecode,
    pub display_url: Option<Selector>,
    pub snippet: Option<Selector>,
    pub date: Option<Selector>,
    pub date_attr: Option<String>,
    pub sitelinks: Option<Selector>,
    /// Selector per result kind; the first match decides.
    pub kind: Vec<(Kind, Selector)>,
    /// Fields that live after the item in document order (table layouts).
    pub following: Vec<Field>,
    /// Query parameters stripped from the destination URL.
    pub drop_params: Vec<String>,
}

/// A field that can be taken from outside the item element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Snippet,
    DisplayUrl,
    Date,
}

impl Field {
    fn parse(name: &str) -> Option<Field> {
        match name {
            "snippet" => Some(Field::Snippet),
            "display_url" => Some(Field::DisplayUrl),
            "date" => Some(Field::Date),
            _ => None,
        }
    }
}

/// `[related]`: the provider's "related searches".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelatedPlan {
    pub item: Selector,
    /// Attribute to read instead of the element's text.
    pub attr: Option<String>,
}

/// `[answer]`: the direct answer above the results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnswerPlan {
    pub container: Option<Selector>,
    pub text: Selector,
    pub source: Option<Selector>,
    pub source_attr: String,
}

/// How the provider paginates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaginationKind {
    /// Follow the `next` link's attribute.
    Link,
    /// Submit the hidden fields of the form holding `next`.
    Form,
    /// Use `[request.page]`.
    Param,
}

/// `[pagination]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaginationPlan {
    pub kind: PaginationKind,
    pub next: Option<Selector>,
    pub attr: String,
}

/// `[block]`: how a refusal is recognised (`docs/spec/08-search.md` §4).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockPlan {
    pub status: Vec<u16>,
    pub selectors: Vec<Selector>,
    /// Literal, ASCII-case-insensitive substrings of the body.
    pub patterns: Vec<String>,
    /// Forces the block class instead of deriving it from the signal.
    pub class: Option<crate::error::BlockClass>,
}

/// A validated parse plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsePlan {
    pub provider: String,
    pub version: u32,
    pub path: PlanPath,
    /// False when the selectors were never confirmed against a live SERP.
    pub verified: bool,
    pub notes: String,
    pub request: RequestPlan,
    pub cookies: CookiePlan,
    pub consent: Vec<Selector>,
    pub consent_reject: Option<Selector>,
    pub results: ResultsPlan,
    pub related: Option<RelatedPlan>,
    pub answer: Option<AnswerPlan>,
    pub pagination: Option<PaginationPlan>,
    pub block: BlockPlan,
    /// `[expect] blocked`: the recorded fixtures are block pages, so this
    /// plan exists to detect a refusal, not to extract results.
    pub expect_blocked: bool,
}

impl ParsePlan {
    /// Parses and validates one plan file.
    pub fn parse(id: &str, text: &str) -> Result<Self, PlanError> {
        let raw: RawPlan = toml::from_str(text)
            .map_err(|err| PlanError::new(id, format!("invalid TOML: {err}")))?;
        raw.validate(id)
    }

    /// The provider this plan serves.
    pub fn provider(&self) -> Result<Provider, SearchError> {
        Provider::from_plan_id(&self.provider)
    }

    /// Whether the fast path can serve this provider at all.
    pub fn serves_fast_path(&self) -> bool {
        self.path == PlanPath::Fast
    }

    /// Builds the request for `query`. `endpoint` replaces the plan's
    /// endpoint when the caller overrode it (tests, mirrors).
    pub fn build_request(
        &self,
        request: &SearchRequest,
        endpoint: Option<&Url>,
    ) -> Result<PreparedRequest, SearchError> {
        let provider = self.provider()?;
        let mut pairs: Vec<(String, String)> = Vec::with_capacity(8);
        let query = request.query_with_site(self.request.site_param.is_some());
        pairs.push((self.request.query_param.clone(), query));

        for (name, value) in &self.request.params {
            pairs.push((name.clone(), value.clone()));
        }
        for (name, template) in &self.request.locale {
            // A template that needs a value this locale does not have (no
            // region) is dropped, which is exactly the provider's default.
            if let Some(value) = fill(template, &request.locale) {
                pairs.push((name.clone(), value));
            }
        }
        let mut cookies = self.cookies.preset.clone();
        if let Some(safe) = &self.request.safe {
            let value = safe.value(request.safe).to_owned();
            match &safe.carrier {
                SafeCarrier::Param(param) => pairs.push((param.clone(), value)),
                SafeCarrier::Cookie(_) => {
                    let (name, value) = value.split_once('=').ok_or_else(|| {
                        SearchError::Plan(PlanError::new(
                            &self.provider,
                            format!("safe cookie {value:?} is not name=value"),
                        ))
                    })?;
                    cookies.insert(name.to_owned(), value.to_owned());
                }
            }
        }
        if let Some(freshness) = request.freshness {
            let plan = self.request.freshness.as_ref().ok_or_else(|| {
                SearchError::InvalidRequest(format!(
                    "{provider} has no freshness parameter in its plan"
                ))
            })?;
            pairs.push((plan.param.clone(), plan.value(freshness).to_owned()));
        }
        if let Some(site) = &request.site
            && let Some(param) = &self.request.site_param
        {
            pairs.push((param.clone(), site.clone()));
        }
        if request.page > 1 {
            match &self.request.page {
                Some((param, page)) => {
                    let offset = page.first + i64::from(request.page - 1) * page.step;
                    pairs.push((param.clone(), offset.to_string()));
                }
                None => {
                    return Err(SearchError::Pagination {
                        provider,
                        page: request.page,
                        detail: match &self.pagination {
                            Some(plan) => format!(
                                "the plan paginates by {:?}, which needs page 1's document",
                                plan.kind
                            ),
                            None => "the plan has no [request.page] parameter".to_owned(),
                        },
                    });
                }
            }
        }

        let mut url = endpoint
            .cloned()
            .unwrap_or_else(|| self.request.endpoint.clone());
        let body = if self.request.post {
            let mut encoder = url::form_urlencoded::Serializer::new(String::new());
            for (name, value) in &pairs {
                encoder.append_pair(name, value);
            }
            Some(encoder.finish())
        } else {
            {
                let mut query = url.query_pairs_mut();
                for (name, value) in &pairs {
                    query.append_pair(name, value);
                }
            }
            None
        };

        let mut headers: Vec<(String, String)> = Vec::with_capacity(4);
        for (name, template) in &self.request.headers {
            if let Some(value) = fill(template, &request.locale) {
                headers.push((name.clone(), value));
            }
        }
        if !cookies.is_empty() && self.cookies.policy != CookiePolicy::None {
            let value = cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            headers.push(("Cookie".to_owned(), value));
        }

        Ok(PreparedRequest {
            url,
            post: self.request.post,
            body,
            headers,
        })
    }
}

/// What the transport has to send for one provider query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedRequest {
    pub url: Url,
    pub post: bool,
    /// Form-encoded parameters when the plan uses POST.
    pub body: Option<String>,
    /// Plan headers after templating; the profile decides their final order.
    pub headers: Vec<(String, String)>,
}

/// Fills `{language}`, `{region}`, `{region_lower}` and `{accept_language}`.
/// Returns `None` when the locale cannot supply one of them, which drops the
/// parameter rather than sending an empty value.
fn fill(template: &str, locale: &Locale) -> Option<String> {
    let mut out = String::with_capacity(template.len() + 8);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}')? + start;
        let name = &rest[start + 1..end];
        let value = match name {
            "language" => locale.language.to_string(),
            "region" => locale.region?.as_str().to_owned(),
            "region_lower" => locale.region?.to_lowercase(),
            "accept_language" => locale.accept_language(),
            _ => return None,
        };
        out.push_str(&value);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// Every plan the engine can serve, by plan id.
#[derive(Clone, Debug, Default)]
pub struct PlanSet {
    plans: BTreeMap<String, Arc<ParsePlan>>,
}

impl PlanSet {
    /// The plans embedded at build time from `providers/plans/`.
    pub fn embedded() -> Result<Self, PlanError> {
        let mut plans = BTreeMap::new();
        for (id, text) in EMBEDDED {
            plans.insert((*id).to_owned(), Arc::new(ParsePlan::parse(id, text)?));
        }
        Ok(Self { plans })
    }

    /// Loads every `*.toml` in `directory`, replacing plans of the same id.
    pub fn with_dir(mut self, directory: impl AsRef<Path>) -> Result<Self, PlanError> {
        let directory = directory.as_ref();
        let entries = std::fs::read_dir(directory).map_err(|err| {
            PlanError::new(
                directory.display().to_string(),
                format!("reading plan directory: {err}"),
            )
        })?;
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        for entry in entries {
            let path = entry
                .map_err(|err| {
                    PlanError::new(directory.display().to_string(), format!("reading: {err}"))
                })?
                .path();
            if path.extension().is_some_and(|ext| ext == "toml") {
                files.push(path);
            }
        }
        files.sort();
        for path in files {
            let id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| PlanError::new(path.display().to_string(), "plan file has no name"))?
                .to_owned();
            let text = std::fs::read_to_string(&path)
                .map_err(|err| PlanError::new(&id, format!("reading {}: {err}", path.display())))?;
            let plan = ParsePlan::parse(&id, &text)?;
            self.plans.insert(id, Arc::new(plan));
        }
        Ok(self)
    }

    /// Only the plans in `directory`; nothing embedded.
    pub fn from_dir(directory: impl AsRef<Path>) -> Result<Self, PlanError> {
        Self::default().with_dir(directory)
    }

    pub fn get(&self, plan_id: &str) -> Option<&Arc<ParsePlan>> {
        self.plans.get(plan_id)
    }

    /// The plan serving `provider`.
    pub fn for_provider(&self, provider: &Provider) -> Result<Arc<ParsePlan>, SearchError> {
        self.get(provider.plan_id()).cloned().ok_or_else(|| {
            SearchError::Plan(PlanError::new(
                provider.plan_id(),
                "no such plan; add providers/plans/<id>.toml or point SearchOptions::plans_dir at it",
            ))
        })
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.plans.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.plans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plans.is_empty()
    }
}

// ----------------------------------------------------------- TOML surface

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlan {
    provider: String,
    version: u32,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    verified: Option<bool>,
    #[serde(default)]
    notes: Option<String>,
    request: RawRequest,
    #[serde(default)]
    cookies: Option<RawCookies>,
    #[serde(default)]
    consent: Option<RawConsent>,
    results: RawResults,
    #[serde(default)]
    related: Option<RawRelated>,
    #[serde(default)]
    answer: Option<RawAnswer>,
    #[serde(default)]
    pagination: Option<RawPagination>,
    #[serde(default)]
    block: Option<RawBlock>,
    #[serde(default)]
    expect: Option<RawExpect>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRequest {
    endpoint: String,
    #[serde(default)]
    method: Option<String>,
    query_param: String,
    #[serde(default)]
    params: BTreeMap<String, String>,
    #[serde(default)]
    locale: BTreeMap<String, String>,
    #[serde(default)]
    safe: Option<RawSafe>,
    #[serde(default)]
    freshness: Option<RawFreshness>,
    #[serde(default)]
    page: Option<RawPage>,
    #[serde(default)]
    site: Option<RawSite>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSafe {
    #[serde(default)]
    param: Option<String>,
    #[serde(default)]
    header: Option<String>,
    strict: String,
    moderate: String,
    off: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFreshness {
    param: String,
    day: String,
    week: String,
    month: String,
    year: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPage {
    param: String,
    first: i64,
    step: i64,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSite {
    param: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCookies {
    #[serde(default)]
    policy: Option<String>,
    #[serde(default)]
    preset: BTreeMap<String, String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConsent {
    #[serde(default)]
    selectors: Vec<String>,
    #[serde(default)]
    reject: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResults {
    #[serde(default)]
    container: Option<String>,
    item: String,
    #[serde(default)]
    exclude: Vec<String>,
    title: String,
    url: String,
    #[serde(default)]
    url_attr: Option<String>,
    #[serde(default)]
    url_unwrap_param: Option<String>,
    #[serde(default)]
    url_unwrap_decode: Option<String>,
    #[serde(default)]
    url_unwrap_regex: Option<String>,
    #[serde(default)]
    display_url: Option<String>,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    date_attr: Option<String>,
    #[serde(default)]
    sitelinks: Option<String>,
    #[serde(default)]
    kind: BTreeMap<String, String>,
    #[serde(default)]
    following: Vec<String>,
    #[serde(default)]
    drop_params: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRelated {
    item: String,
    #[serde(default)]
    attr: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAnswer {
    #[serde(default)]
    container: Option<String>,
    text: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    source_attr: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPagination {
    kind: String,
    #[serde(default)]
    next: Option<String>,
    #[serde(default)]
    attr: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBlock {
    #[serde(default)]
    status: Vec<u16>,
    #[serde(default)]
    selectors: Vec<String>,
    #[serde(default)]
    patterns: Vec<String>,
    #[serde(default)]
    class: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExpect {
    #[serde(default)]
    blocked: bool,
}

/// Placeholders the model can fill. `{api_key}` and `{cse_id}` are gone with
/// `google_api.toml`: no vendor search API is ever called (O-7).
const PLACEHOLDERS: &[&str] = &["language", "region", "region_lower", "accept_language"];

fn check_template(plan: &str, what: &str, template: &str) -> Result<(), PlanError> {
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let end = rest[start..].find('}').ok_or_else(|| {
            PlanError::new(
                plan,
                format!("{what}: unterminated placeholder in {template:?}"),
            )
        })? + start;
        let name = &rest[start + 1..end];
        if name == "api_key" || name == "cse_id" {
            return Err(PlanError::new(
                plan,
                format!(
                    "{what}: {{{name}}} names a vendor search API; search only uses the \
                     providers' public pages (docs/spec/08-search.md §2, O-7)"
                ),
            ));
        }
        if !PLACEHOLDERS.contains(&name) {
            return Err(PlanError::new(
                plan,
                format!("{what}: unknown placeholder {{{name}}}; known: {PLACEHOLDERS:?}"),
            ));
        }
        rest = &rest[end + 1..];
    }
    Ok(())
}

/// Characters that would make a pattern a regular expression. The Python
/// reference (`providers/check_plans.py`) uses `re.search`; the engine keeps
/// regex off the hot path (`08` §3.4) and takes literal substrings only, so a
/// pattern that looks like a regex is refused instead of silently matching
/// something else.
const REGEX_METACHARACTERS: &[char] = &[
    '\\', '^', '$', '.', '|', '?', '*', '+', '(', ')', '[', ']', '{', '}',
];

impl RawPlan {
    fn validate(self, id: &str) -> Result<ParsePlan, PlanError> {
        if self.provider != id {
            return Err(PlanError::new(
                id,
                format!(
                    "provider = {:?} does not match the file name",
                    self.provider
                ),
            ));
        }
        if self.version == 0 {
            return Err(PlanError::new(id, "version must be >= 1"));
        }
        let path = match self.path.as_deref().unwrap_or("fast") {
            "fast" => PlanPath::Fast,
            "full" => PlanPath::Full,
            "api" => {
                return Err(PlanError::new(
                    id,
                    "path = \"api\" is refused: search only uses the providers' public pages \
                     (docs/spec/08-search.md §2, O-7)",
                ));
            }
            other => {
                return Err(PlanError::new(
                    id,
                    format!("unknown path {other:?}; expected fast or full"),
                ));
            }
        };

        let endpoint = Url::parse(&self.request.endpoint)
            .map_err(|err| PlanError::new(id, format!("endpoint: {err}")))?;
        if !matches!(endpoint.scheme(), "http" | "https") {
            return Err(PlanError::new(
                id,
                format!("endpoint scheme {:?} is not http(s)", endpoint.scheme()),
            ));
        }
        let post = match self.request.method.as_deref().unwrap_or("GET") {
            "GET" | "get" => false,
            "POST" | "post" => true,
            other => {
                return Err(PlanError::new(
                    id,
                    format!("method {other:?} is not GET or POST"),
                ));
            }
        };
        if self.request.query_param.trim().is_empty() {
            return Err(PlanError::new(id, "query_param is empty"));
        }
        for (name, template) in self
            .request
            .locale
            .iter()
            .chain(self.request.params.iter())
            .chain(self.request.headers.iter())
        {
            check_template(id, name, template)?;
        }

        let safe = match self.request.safe {
            Some(safe) => {
                let carrier = match (safe.param, safe.header) {
                    (Some(param), None) => SafeCarrier::Param(param),
                    (None, Some(header)) => {
                        if !header.eq_ignore_ascii_case("cookie") {
                            return Err(PlanError::new(
                                id,
                                format!("safe header {header:?}: only Cookie is supported"),
                            ));
                        }
                        SafeCarrier::Cookie(header)
                    }
                    (Some(_), Some(_)) => {
                        return Err(PlanError::new(id, "safe: set param or header, not both"));
                    }
                    (None, None) => {
                        return Err(PlanError::new(id, "safe: needs param or header"));
                    }
                };
                Some(SafePlan {
                    carrier,
                    strict: safe.strict,
                    moderate: safe.moderate,
                    off: safe.off,
                })
            }
            None => None,
        };

        let freshness = self.request.freshness.map(|raw| FreshnessPlan {
            param: raw.param,
            day: raw.day,
            week: raw.week,
            month: raw.month,
            year: raw.year,
        });
        let page = match self.request.page {
            Some(raw) => {
                if raw.step == 0 {
                    return Err(PlanError::new(id, "page.step must not be 0"));
                }
                Some((
                    raw.param,
                    PagePlan {
                        first: raw.first,
                        step: raw.step,
                    },
                ))
            }
            None => None,
        };

        let cookies = match self.cookies {
            Some(raw) => CookiePlan {
                policy: match raw.policy.as_deref().unwrap_or("none") {
                    "none" => CookiePolicy::None,
                    "session" => CookiePolicy::Session,
                    "persist" => CookiePolicy::Persist,
                    other => {
                        return Err(PlanError::new(
                            id,
                            format!("cookie policy {other:?}; expected none, session or persist"),
                        ));
                    }
                },
                preset: raw.preset,
            },
            None => CookiePlan::default(),
        };

        let results = self.results.validate(id)?;

        let related = match self.related {
            Some(raw) => Some(RelatedPlan {
                item: Selector::compile(id, &raw.item)?,
                attr: raw.attr,
            }),
            None => None,
        };
        let answer = match self.answer {
            Some(raw) => Some(AnswerPlan {
                container: raw
                    .container
                    .as_deref()
                    .map(|source| Selector::compile(id, source))
                    .transpose()?,
                text: Selector::compile(id, &raw.text)?,
                source: raw
                    .source
                    .as_deref()
                    .map(|source| Selector::compile(id, source))
                    .transpose()?,
                source_attr: raw.source_attr.unwrap_or_else(|| "href".to_owned()),
            }),
            None => None,
        };
        let pagination = match self.pagination {
            Some(raw) => Some(PaginationPlan {
                kind: match raw.kind.as_str() {
                    "link" => PaginationKind::Link,
                    "form" => PaginationKind::Form,
                    "param" => PaginationKind::Param,
                    other => {
                        return Err(PlanError::new(
                            id,
                            format!("pagination kind {other:?}; expected link, form or param"),
                        ));
                    }
                },
                next: raw
                    .next
                    .as_deref()
                    .map(|source| Selector::compile(id, source))
                    .transpose()?,
                attr: raw.attr.unwrap_or_else(|| "href".to_owned()),
            }),
            None => None,
        };

        let block = match self.block {
            Some(raw) => {
                let mut selectors = Vec::with_capacity(raw.selectors.len());
                for source in &raw.selectors {
                    selectors.push(Selector::compile(id, source)?);
                }
                for pattern in &raw.patterns {
                    if pattern.is_empty() {
                        return Err(PlanError::new(id, "block pattern is empty"));
                    }
                    if let Some(bad) = pattern.chars().find(|c| REGEX_METACHARACTERS.contains(c)) {
                        return Err(PlanError::new(
                            id,
                            format!(
                                "block pattern {pattern:?} contains {bad:?}: patterns are literal, \
                                 ASCII-case-insensitive substrings, not regular expressions"
                            ),
                        ));
                    }
                }
                BlockPlan {
                    status: raw.status,
                    selectors,
                    patterns: raw.patterns,
                    class: match raw.class.as_deref() {
                        None => None,
                        Some("reset") => Some(crate::error::BlockClass::Reset),
                        Some("rate-limited") => Some(crate::error::BlockClass::RateLimited),
                        Some("captcha") => Some(crate::error::BlockClass::Captcha),
                        Some("js-required") => Some(crate::error::BlockClass::JsRequired),
                        Some("layout") => Some(crate::error::BlockClass::Layout),
                        Some("soft") => Some(crate::error::BlockClass::Soft),
                        Some(other) => {
                            return Err(PlanError::new(
                                id,
                                format!("block class {other:?} is not a block class"),
                            ));
                        }
                    },
                }
            }
            None => BlockPlan::default(),
        };

        let (consent, consent_reject) = match self.consent {
            Some(raw) => {
                let mut selectors = Vec::with_capacity(raw.selectors.len());
                for source in &raw.selectors {
                    selectors.push(Selector::compile(id, source)?);
                }
                let reject = raw
                    .reject
                    .as_deref()
                    .map(|source| Selector::compile(id, source))
                    .transpose()?;
                (selectors, reject)
            }
            None => (Vec::new(), None),
        };

        Ok(ParsePlan {
            provider: self.provider,
            version: self.version,
            path,
            verified: self.verified.unwrap_or(true),
            notes: self.notes.unwrap_or_default(),
            request: RequestPlan {
                endpoint,
                post,
                query_param: self.request.query_param,
                params: self.request.params,
                locale: self.request.locale,
                safe,
                freshness,
                page,
                site_param: self.request.site.map(|site| site.param),
                headers: self.request.headers,
            },
            cookies,
            consent,
            consent_reject,
            results,
            related,
            answer,
            pagination,
            block,
            expect_blocked: self.expect.map(|expect| expect.blocked).unwrap_or(false),
        })
    }
}

impl RawResults {
    fn validate(self, id: &str) -> Result<ResultsPlan, PlanError> {
        if self.url_unwrap_regex.is_some() {
            return Err(PlanError::new(
                id,
                "url_unwrap_regex is not supported: the fast path runs no regex \
                 (docs/spec/08-search.md §3.4). Use url_unwrap_param.",
            ));
        }
        let url_unwrap_decode = match self.url_unwrap_decode.as_deref() {
            None => UnwrapDecode::None,
            Some("bing-a1-base64url") => UnwrapDecode::BingA1Base64Url,
            Some(other) => {
                return Err(PlanError::new(
                    id,
                    format!("unknown url_unwrap_decode {other:?}"),
                ));
            }
        };
        if url_unwrap_decode != UnwrapDecode::None && self.url_unwrap_param.is_none() {
            return Err(PlanError::new(
                id,
                "url_unwrap_decode needs url_unwrap_param",
            ));
        }
        let mut exclude = Vec::with_capacity(self.exclude.len());
        for source in &self.exclude {
            exclude.push(Selector::compile(id, source)?);
        }
        let mut kind = Vec::with_capacity(self.kind.len());
        for (name, source) in &self.kind {
            let parsed = name
                .parse::<Kind>()
                .map_err(|_| PlanError::new(id, format!("kind {name:?} is not a result kind")))?;
            kind.push((parsed, Selector::compile(id, source)?));
        }
        let display_url = self
            .display_url
            .as_deref()
            .map(|source| Selector::compile(id, source))
            .transpose()?;
        let snippet = self
            .snippet
            .as_deref()
            .map(|source| Selector::compile(id, source))
            .transpose()?;
        let date = self
            .date
            .as_deref()
            .map(|source| Selector::compile(id, source))
            .transpose()?;
        let mut following = Vec::with_capacity(self.following.len());
        for name in &self.following {
            let field = Field::parse(name).ok_or_else(|| {
                PlanError::new(
                    id,
                    format!("following: {name:?} is not one of snippet, display_url, date"),
                )
            })?;
            let present = match field {
                Field::Snippet => snippet.is_some(),
                Field::DisplayUrl => display_url.is_some(),
                Field::Date => date.is_some(),
            };
            if !present {
                return Err(PlanError::new(
                    id,
                    format!("following names {name:?}, which the plan does not define"),
                ));
            }
            following.push(field);
        }
        Ok(ResultsPlan {
            container: self
                .container
                .as_deref()
                .map(|source| Selector::compile(id, source))
                .transpose()?,
            item: Selector::compile(id, &self.item)?,
            exclude,
            title: Selector::compile(id, &self.title)?,
            url: Selector::compile(id, &self.url)?,
            url_attr: self.url_attr.unwrap_or_else(|| "href".to_owned()),
            url_unwrap_param: self.url_unwrap_param,
            url_unwrap_decode,
            display_url,
            snippet,
            date,
            date_attr: self.date_attr,
            sitelinks: self
                .sitelinks
                .as_deref()
                .map(|source| Selector::compile(id, source))
                .transpose()?,
            kind,
            following,
            drop_params: self.drop_params,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LanguageTag, Region};

    fn locale(tag: &str) -> Locale {
        Locale::parse(tag).expect("locale")
    }

    #[test]
    fn templates_are_filled_from_the_locale_or_dropped() {
        let polish = locale("pl-PL");
        assert_eq!(fill("{language}", &polish).as_deref(), Some("pl"));
        assert_eq!(fill("{region}", &polish).as_deref(), Some("PL"));
        assert_eq!(fill("{region_lower}", &polish).as_deref(), Some("pl"));
        assert_eq!(
            fill("{region_lower}-{language}", &polish).as_deref(),
            Some("pl-pl")
        );
        assert_eq!(fill("web", &polish).as_deref(), Some("web"));

        // Without a region the parameter cannot be built, and a parameter
        // that cannot be built is left out rather than sent empty.
        let neutral = Locale::new(LanguageTag::new("en").expect("language"), None);
        assert_eq!(fill("{region}", &neutral), None);
        assert_eq!(fill("{language}", &neutral).as_deref(), Some("en"));

        let american = Locale::new(
            LanguageTag::new("en").expect("language"),
            Some(Region::new("us").expect("region")),
        );
        assert_eq!(
            fill("{region_lower}-{language}", &american).as_deref(),
            Some("us-en")
        );
    }

    #[test]
    fn the_prefilter_is_the_rightmost_compounds_cheapest_test() {
        assert_eq!(
            Prefilter::of("ol#b_results"),
            Prefilter::Id(Atom::new("b_results"))
        );
        assert_eq!(
            Prefilter::of("div#links"),
            Prefilter::Id(Atom::new("links"))
        );
        assert_eq!(Prefilter::of("body"), Prefilter::Tag(Atom::new("body")));
        assert_eq!(
            Prefilter::of("div.result-content > a"),
            Prefilter::Tag(Atom::new("a"))
        );
        assert_eq!(
            Prefilter::of("div.snippet[data-type=web]"),
            Prefilter::Class(Atom::new("snippet"))
        );
        // A pseudo-class argument is not the selector's own tail.
        assert_eq!(
            Prefilter::of("tr:has(a.result-link)"),
            Prefilter::Tag(Atom::new("tr"))
        );
        assert_eq!(Prefilter::of("div.g, div[data-snhf]"), Prefilter::None);
        assert_eq!(Prefilter::of("[data-pos]"), Prefilter::None);
    }

    #[test]
    fn every_shipped_plan_has_a_usable_early_stop_marker() {
        let plans = PlanSet::embedded().expect("plans");
        for id in ["bing", "brave", "duckduckgo_html", "duckduckgo_lite"] {
            let plan = plans.get(id).expect("plan");
            let container = plan
                .results
                .container
                .as_ref()
                .unwrap_or_else(|| panic!("{id} has no results container to stop at"));
            assert_ne!(
                *container.prefilter(),
                Prefilter::None,
                "{id}: the stop predicate would run the selector engine on every element"
            );
        }
    }
}
