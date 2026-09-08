//! The search model (`docs/spec/08-search.md` §1) and the JSON the CLI and
//! the MCP tool print (§5).
//!
//! Durations serialise as fractional milliseconds (`*_ms`), URLs as strings
//! and dates as `YYYY-MM-DD`, because the consumers are agents reading JSON,
//! not Rust callers.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use serde::{Serialize, Serializer};
use url::Url;

use crate::error::SearchError;

/// A plan id: the stem of `providers/plans/<id>.toml` and the name of its
/// fixture directory.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderId(Box<str>);

impl ProviderId {
    /// Validates and interns a plan id. Ids are lowercase ASCII words joined
    /// by `_` or `-`, because they name files and fixture directories.
    pub fn new(id: &str) -> Result<Self, SearchError> {
        if id.is_empty() || id.len() > 64 {
            return Err(SearchError::InvalidRequest(format!(
                "provider id {id:?} must be 1..=64 characters"
            )));
        }
        if !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        {
            return Err(SearchError::InvalidRequest(format!(
                "provider id {id:?} may only contain a-z, 0-9, '_' and '-'"
            )));
        }
        Ok(Self(id.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ProviderId {
    type Err = SearchError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ProviderId::new(s)
    }
}

impl Serialize for ProviderId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// A search provider. The four named ones are the providers the survey
/// covers; `Custom` names any other plan in `providers/plans/`, which is how
/// `duckduckgo_lite` and a private provider are reached.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Provider {
    DuckDuckGo,
    Bing,
    Brave,
    Google,
    Custom(ProviderId),
}

impl Provider {
    /// The plan this provider is served by. `DuckDuckGo` is the HTML
    /// endpoint; the Lite endpoint is `Custom("duckduckgo_lite")`.
    pub fn plan_id(&self) -> &str {
        match self {
            Provider::DuckDuckGo => "duckduckgo_html",
            Provider::Bing => "bing",
            Provider::Brave => "brave",
            Provider::Google => "google_chromium",
            Provider::Custom(id) => id.as_str(),
        }
    }

    /// The short name the CLI takes and the JSON prints.
    pub fn name(&self) -> &str {
        match self {
            Provider::DuckDuckGo => "duckduckgo",
            Provider::Bing => "bing",
            Provider::Brave => "brave",
            Provider::Google => "google",
            Provider::Custom(id) => id.as_str(),
        }
    }

    /// The provider a plan id belongs to; the inverse of [`Provider::plan_id`].
    pub fn from_plan_id(id: &str) -> Result<Self, SearchError> {
        Ok(match id {
            "duckduckgo_html" => Provider::DuckDuckGo,
            "bing" => Provider::Bing,
            "brave" => Provider::Brave,
            "google_chromium" => Provider::Google,
            other => Provider::Custom(ProviderId::new(other)?),
        })
    }
}

impl FromStr for Provider {
    type Err = SearchError;

    /// Accepts the short names, their common aliases and any plan id.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let name = s.trim().to_ascii_lowercase();
        Ok(match name.as_str() {
            "ddg" | "duckduckgo" | "duckduckgo_html" | "duckduckgo-html" => Provider::DuckDuckGo,
            "bing" => Provider::Bing,
            "brave" => Provider::Brave,
            "google" | "google_chromium" | "google-chromium" => Provider::Google,
            "ddg-lite" | "ddg_lite" => Provider::Custom(ProviderId::new("duckduckgo_lite")?),
            other => Provider::Custom(ProviderId::new(other)?),
        })
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl Serialize for Provider {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

/// Which path a query takes (`docs/spec/08-search.md` §2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// One request to the provider's public page, no JavaScript.
    Fast,
    /// A Tier 2 page load with JavaScript. P2.
    Full,
    /// Fast where the provider supports it, Full otherwise.
    #[default]
    Auto,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Fast => "fast",
            Mode::Full => "full",
            Mode::Auto => "auto",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Mode {
    type Err = SearchError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fast" => Ok(Mode::Fast),
            "full" => Ok(Mode::Full),
            "auto" => Ok(Mode::Auto),
            other => Err(SearchError::InvalidRequest(format!(
                "unknown mode {other:?}; expected fast, full or auto"
            ))),
        }
    }
}

/// Adult-content filter level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SafeSearch {
    Off,
    #[default]
    Moderate,
    Strict,
}

impl SafeSearch {
    pub fn as_str(self) -> &'static str {
        match self {
            SafeSearch::Off => "off",
            SafeSearch::Moderate => "moderate",
            SafeSearch::Strict => "strict",
        }
    }
}

impl FromStr for SafeSearch {
    type Err = SearchError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(SafeSearch::Off),
            "moderate" => Ok(SafeSearch::Moderate),
            "strict" => Ok(SafeSearch::Strict),
            other => Err(SearchError::InvalidRequest(format!(
                "unknown safe search level {other:?}"
            ))),
        }
    }
}

/// How recent a result has to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Freshness {
    Day,
    Week,
    Month,
    Year,
}

impl Freshness {
    pub fn as_str(self) -> &'static str {
        match self {
            Freshness::Day => "day",
            Freshness::Week => "week",
            Freshness::Month => "month",
            Freshness::Year => "year",
        }
    }
}

impl FromStr for Freshness {
    type Err = SearchError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "day" | "d" => Ok(Freshness::Day),
            "week" | "w" => Ok(Freshness::Week),
            "month" | "m" => Ok(Freshness::Month),
            "year" | "y" => Ok(Freshness::Year),
            other => Err(SearchError::InvalidRequest(format!(
                "unknown freshness {other:?}"
            ))),
        }
    }
}

/// A BCP 47 primary language subtag, lowercase (`pl`, `en`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LanguageTag(Box<str>);

impl LanguageTag {
    pub fn new(tag: &str) -> Result<Self, SearchError> {
        let tag = tag.trim().to_ascii_lowercase();
        if !(2..=8).contains(&tag.len()) || !tag.bytes().all(|b| b.is_ascii_lowercase()) {
            return Err(SearchError::InvalidRequest(format!(
                "language subtag {tag:?} must be 2..=8 ASCII letters"
            )));
        }
        Ok(Self(tag.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LanguageTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An ISO 3166-1 alpha-2 region, uppercase (`PL`, `US`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Region([u8; 2]);

impl Region {
    pub fn new(region: &str) -> Result<Self, SearchError> {
        let region = region.trim();
        let bytes = region.as_bytes();
        if bytes.len() != 2 || !bytes.iter().all(|b| b.is_ascii_alphabetic()) {
            return Err(SearchError::InvalidRequest(format!(
                "region {region:?} must be two ASCII letters"
            )));
        }
        Ok(Self([
            bytes[0].to_ascii_uppercase(),
            bytes[1].to_ascii_uppercase(),
        ]))
    }

    /// `PL`.
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or("")
    }

    /// `pl`, the form DuckDuckGo's `kl` and Brave's `country` take.
    pub fn to_lowercase(self) -> String {
        self.as_str().to_ascii_lowercase()
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Language and optional region: the `hl`/`gl`, `kl` and `setlang`/`cc` pair
/// every provider spells differently.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Locale {
    pub language: LanguageTag,
    pub region: Option<Region>,
}

impl Locale {
    pub fn new(language: LanguageTag, region: Option<Region>) -> Self {
        Self { language, region }
    }

    /// Parses `pl-PL`, `pl_PL` or `pl`.
    pub fn parse(tag: &str) -> Result<Self, SearchError> {
        let tag = tag.trim();
        let (language, region) = match tag.split_once(['-', '_']) {
            Some((language, region)) => (language, Some(Region::new(region)?)),
            None => (tag, None),
        };
        Ok(Self {
            language: LanguageTag::new(language)?,
            region,
        })
    }

    /// `pl-PL`, or `pl` without a region.
    pub fn tag(&self) -> String {
        match self.region {
            Some(region) => format!("{}-{}", self.language, region),
            None => self.language.to_string(),
        }
    }

    /// The `Accept-Language` header a browser in this locale sends: the full
    /// tag, its bare language, and English as the last fallback.
    pub fn accept_language(&self) -> String {
        let language = self.language.as_str();
        let mut value = self.tag();
        if self.region.is_some() {
            value.push_str(&format!(",{language};q=0.9"));
        }
        if language != "en" {
            value.push_str(if self.region.is_some() {
                ",en-US;q=0.8,en;q=0.7"
            } else {
                ",en;q=0.9"
            });
        }
        value
    }
}

impl Default for Locale {
    /// English with no region: the neutral setting, which is DuckDuckGo's
    /// `wt-wt` and Bing's and Brave's unset market.
    fn default() -> Self {
        Self {
            language: LanguageTag(Box::from("en")),
            region: None,
        }
    }
}

impl FromStr for Locale {
    type Err = SearchError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Locale::parse(s)
    }
}

impl fmt::Display for Locale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.tag())
    }
}

/// Largest `limit` the model accepts (`docs/spec/08-search.md` §1).
pub const MAX_LIMIT: u32 = 50;

/// One query (`docs/spec/08-search.md` §1). Build it with
/// [`SearchRequest::new`] and the chaining setters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchRequest {
    pub query: String,
    /// One provider, or several for fan-out.
    pub providers: Vec<Provider>,
    pub locale: Locale,
    /// 1..=[`MAX_LIMIT`] results after merging.
    pub limit: u32,
    /// 1-based result page.
    pub page: u32,
    pub safe: SafeSearch,
    pub freshness: Option<Freshness>,
    /// Restricts the query to one site; the plan's `site` parameter when it
    /// has one, otherwise a `site:` prefix on the query.
    pub site: Option<String>,
    pub mode: Mode,
}

impl SearchRequest {
    /// A query against DuckDuckGo with the model's defaults.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            providers: vec![Provider::DuckDuckGo],
            locale: Locale::default(),
            limit: 10,
            page: 1,
            safe: SafeSearch::default(),
            freshness: None,
            site: None,
            mode: Mode::default(),
        }
    }

    pub fn providers(mut self, providers: impl IntoIterator<Item = Provider>) -> Self {
        self.providers = providers.into_iter().collect();
        self
    }

    pub fn provider(mut self, provider: Provider) -> Self {
        self.providers = vec![provider];
        self
    }

    pub fn locale(mut self, locale: Locale) -> Self {
        self.locale = locale;
        self
    }

    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }

    pub fn page(mut self, page: u32) -> Self {
        self.page = page;
        self
    }

    pub fn safe(mut self, safe: SafeSearch) -> Self {
        self.safe = safe;
        self
    }

    pub fn freshness(mut self, freshness: Freshness) -> Self {
        self.freshness = Some(freshness);
        self
    }

    pub fn site(mut self, site: impl Into<String>) -> Self {
        self.site = Some(site.into());
        self
    }

    pub fn mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    /// Checks the caller's request before anything is sent.
    pub fn validate(&self) -> Result<(), SearchError> {
        if self.query.trim().is_empty() {
            return Err(SearchError::InvalidRequest("query is empty".into()));
        }
        if self.query.len() > 2048 {
            return Err(SearchError::InvalidRequest(format!(
                "query is {} bytes; the limit is 2048",
                self.query.len()
            )));
        }
        if self.limit == 0 || self.limit > MAX_LIMIT {
            return Err(SearchError::InvalidRequest(format!(
                "limit {} is outside 1..={MAX_LIMIT}",
                self.limit
            )));
        }
        if self.page == 0 {
            return Err(SearchError::InvalidRequest("page is 1-based".into()));
        }
        if self.providers.is_empty() {
            return Err(SearchError::InvalidRequest("no provider requested".into()));
        }
        if let Some(site) = &self.site
            && (site.trim().is_empty() || site.contains(char::is_whitespace))
        {
            return Err(SearchError::InvalidRequest(format!(
                "site {site:?} is not a host"
            )));
        }
        Ok(())
    }

    /// The query as it goes to the provider: `site:` folded in when the plan
    /// has no site parameter.
    pub fn query_with_site(&self, plan_has_site_param: bool) -> String {
        match &self.site {
            Some(site) if !plan_has_site_param => format!("site:{site} {}", self.query),
            _ => self.query.clone(),
        }
    }
}

/// What kind of result this is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Organic,
    News,
    Video,
    Answer,
    Knowledge,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Organic => "organic",
            Kind::News => "news",
            Kind::Video => "video",
            Kind::Answer => "answer",
            Kind::Knowledge => "knowledge",
        }
    }
}

impl FromStr for Kind {
    type Err = SearchError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "organic" => Ok(Kind::Organic),
            "news" => Ok(Kind::News),
            "video" => Ok(Kind::Video),
            "answer" => Ok(Kind::Answer),
            "knowledge" => Ok(Kind::Knowledge),
            other => Err(SearchError::InvalidRequest(format!(
                "unknown result kind {other:?}"
            ))),
        }
    }
}

/// A titled link: a sitelink under a result, or an answer's source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Link {
    pub title: String,
    #[serde(serialize_with = "serialize_url")]
    pub url: Url,
}

/// A direct answer the provider put above its results.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Answer {
    pub text: String,
    #[serde(
        serialize_with = "serialize_url_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub source: Option<Url>,
}

/// One result (`docs/spec/08-search.md` §1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SearchResult {
    /// 1-based position after merging and ranking.
    pub rank: u32,
    pub title: String,
    #[serde(serialize_with = "serialize_url")]
    pub url: Url,
    /// The URL as the provider displays it, breadcrumbs and all.
    pub display_url: String,
    pub snippet: String,
    #[serde(
        serialize_with = "serialize_date_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub published: Option<time::Date>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sitelinks: Vec<Link>,
    pub source: Provider,
    pub kind: Kind,
}

/// Where the time went. `engine_overhead = total - network`
/// (`docs/spec/15-testing-and-benchmarks.md` §2, `S-search-fast`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Timing {
    #[serde(rename = "dns_ms", serialize_with = "serialize_millis")]
    pub dns: Duration,
    #[serde(rename = "connect_ms", serialize_with = "serialize_millis")]
    pub connect: Duration,
    #[serde(rename = "ttfb_ms", serialize_with = "serialize_millis")]
    pub ttfb: Duration,
    /// Wall time of the whole query, cache lookup and merge included.
    #[serde(rename = "total_ms", serialize_with = "serialize_millis")]
    pub total: Duration,
    /// `total` minus the time parked on the network: the number R-06 budgets
    /// at 5 ms p50.
    #[serde(rename = "engine_overhead_ms", serialize_with = "serialize_millis")]
    pub engine_overhead: Duration,
}

/// What one provider contributed to a response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProviderMeta {
    /// Which provider this is. Not in the spec's sketch, but a fan-out
    /// response is unreadable without it.
    pub provider: Provider,
    /// The URL actually requested, parameters included.
    #[serde(serialize_with = "serialize_url")]
    pub endpoint: Url,
    pub plan_version: u32,
    pub mode_used: Mode,
    pub cache_hit: bool,
    /// Results this provider contributed before merging.
    pub results: u32,
    /// Why this provider contributed nothing, in a fan-out that partly
    /// succeeded. `None` when the provider answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The answer to a [`SearchRequest`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub related: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<Answer>,
    pub timing: Timing,
    pub providers: Vec<ProviderMeta>,
}

impl SearchResponse {
    /// The whole response as one JSON object.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|err| {
            // Serialising this model cannot fail; a `Display` fallback keeps
            // the CLI printing something rather than panicking.
            format!("{{\"error\":\"{err}\"}}")
        })
    }

    /// The `--json` form: one object per result, then one trailing object
    /// carrying the response's metadata (`docs/spec/08-search.md` §5).
    pub fn to_json_lines(&self) -> String {
        let mut out = String::with_capacity(256 * (self.results.len() + 1));
        for result in &self.results {
            out.push_str(&serde_json::to_string(result).unwrap_or_default());
            out.push('\n');
        }
        out.push_str(&serde_json::to_string(&MetaLine { meta: self.meta() }).unwrap_or_default());
        out.push('\n');
        out
    }

    fn meta(&self) -> ResponseMeta<'_> {
        ResponseMeta {
            results: self.results.len() as u32,
            related: &self.related,
            answer: self.answer.as_ref(),
            timing_total: self.timing.total,
            timing: self.timing,
            providers: &self.providers,
        }
    }

    /// Distinct hosts in the result list, in rank order; the shape agents use
    /// to decide what to open next.
    pub fn hosts(&self) -> Vec<&str> {
        let mut seen = BTreeSet::new();
        self.results
            .iter()
            .filter_map(|result| result.url.host_str())
            .filter(|host| seen.insert(*host))
            .collect()
    }
}

#[derive(Serialize)]
struct MetaLine<'a> {
    meta: ResponseMeta<'a>,
}

#[derive(Serialize)]
struct ResponseMeta<'a> {
    results: u32,
    related: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    answer: Option<&'a Answer>,
    #[serde(rename = "timing_ms", serialize_with = "serialize_millis")]
    timing_total: std::time::Duration,
    timing: Timing,
    providers: &'a [ProviderMeta],
}

fn serialize_url<S: Serializer>(url: &Url, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(url.as_str())
}

fn serialize_url_option<S: Serializer>(
    url: &Option<Url>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match url {
        Some(url) => serializer.serialize_str(url.as_str()),
        None => serializer.serialize_none(),
    }
}

const DATE_FORMAT: &[time::format_description::FormatItem<'static>] =
    time::macros::format_description!("[year]-[month]-[day]");

fn serialize_date_option<S: Serializer>(
    date: &Option<time::Date>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match date {
        Some(date) => {
            let text = date
                .format(&DATE_FORMAT)
                .map_err(serde::ser::Error::custom)?;
            serializer.serialize_str(&text)
        }
        None => serializer.serialize_none(),
    }
}

fn serialize_millis<S: Serializer>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
    let millis = value.as_nanos() as f64 / 1_000_000.0;
    serializer.serialize_f64((millis * 1000.0).round() / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_language_names_the_locale_then_falls_back_to_english() {
        let cases = [
            ("pl-PL", "pl-PL,pl;q=0.9,en-US;q=0.8,en;q=0.7"),
            ("en-US", "en-US,en;q=0.9"),
            ("de", "de,en;q=0.9"),
            ("en", "en"),
        ];
        for (tag, expected) in cases {
            let locale = Locale::parse(tag).expect("locale");
            assert_eq!(locale.accept_language(), expected, "{tag}");
            assert_eq!(locale.tag(), tag);
        }
    }

    #[test]
    fn a_locale_is_parsed_from_either_separator_and_normalised() {
        let locale = Locale::parse("pl_pl").expect("locale");
        assert_eq!(locale.language.as_str(), "pl");
        assert_eq!(locale.region.expect("region").as_str(), "PL");
        assert_eq!(locale.region.expect("region").to_lowercase(), "pl");
        assert_eq!(Locale::default().tag(), "en");

        // BCP 47 allows longer primary subtags than the two-letter ones the
        // providers use, so they parse; a one-letter tag is not a language.
        assert_eq!(
            Locale::parse("haw-US").expect("locale").language.as_str(),
            "haw"
        );
        assert!(Locale::parse("p").is_err());
        assert!(Locale::parse("pl-POL").is_err());
        assert!(Locale::parse("pl-1").is_err());
    }

    #[test]
    fn provider_names_map_to_plans_and_back() {
        assert_eq!(Provider::DuckDuckGo.plan_id(), "duckduckgo_html");
        assert_eq!(Provider::Google.plan_id(), "google_chromium");
        assert_eq!(
            "ddg".parse::<Provider>().expect("ddg"),
            Provider::DuckDuckGo
        );
        assert_eq!(
            "DuckDuckGo".parse::<Provider>().expect("name"),
            Provider::DuckDuckGo
        );
        assert_eq!(
            "ddg-lite".parse::<Provider>().expect("alias").plan_id(),
            "duckduckgo_lite"
        );
        assert_eq!(
            Provider::from_plan_id("duckduckgo_lite").expect("plan id"),
            "duckduckgo_lite".parse::<Provider>().expect("name")
        );
        assert_eq!(
            Provider::from_plan_id("bing").expect("bing"),
            Provider::Bing
        );
        assert!("Not A Provider".parse::<Provider>().is_err());
        assert_eq!(Provider::Bing.to_string(), "bing");
    }

    #[test]
    fn the_request_model_refuses_what_the_providers_cannot_serve() {
        assert!(SearchRequest::new("rust").validate().is_ok());
        assert!(SearchRequest::new("   ").validate().is_err());
        assert!(SearchRequest::new("rust").limit(0).validate().is_err());
        assert!(
            SearchRequest::new("rust")
                .limit(MAX_LIMIT + 1)
                .validate()
                .is_err()
        );
        assert!(
            SearchRequest::new("rust")
                .limit(MAX_LIMIT)
                .validate()
                .is_ok()
        );
        assert!(SearchRequest::new("rust").page(0).validate().is_err());
        assert!(SearchRequest::new("rust").providers([]).validate().is_err());
        assert!(
            SearchRequest::new("rust")
                .site("example.com")
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn a_site_restriction_without_a_plan_parameter_is_folded_into_the_query() {
        let request = SearchRequest::new("rust").site("servo.org");
        assert_eq!(request.query_with_site(false), "site:servo.org rust");
        assert_eq!(request.query_with_site(true), "rust");
    }

    #[test]
    fn a_result_serialises_to_the_shape_the_cli_prints() {
        let result = SearchResult {
            rank: 1,
            title: "Servo".into(),
            url: Url::parse("https://servo.org/").expect("url"),
            display_url: "servo.org".into(),
            snippet: "engine".into(),
            published: Some(
                time::Date::from_calendar_date(2026, time::Month::January, 2).expect("date"),
            ),
            sitelinks: vec![Link {
                title: "Download".into(),
                url: Url::parse("https://servo.org/download").expect("url"),
            }],
            source: Provider::Custom(ProviderId::new("duckduckgo_lite").expect("id")),
            kind: Kind::News,
        };
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&result).expect("json")).expect("value");

        assert_eq!(json["rank"], 1);
        assert_eq!(json["url"], "https://servo.org/");
        assert_eq!(json["published"], "2026-01-02");
        assert_eq!(json["source"], "duckduckgo_lite");
        assert_eq!(json["kind"], "news");
        assert_eq!(json["sitelinks"][0]["url"], "https://servo.org/download");
    }

    #[test]
    fn durations_serialise_as_milliseconds() {
        let timing = Timing {
            dns: Duration::from_micros(1500),
            connect: Duration::ZERO,
            ttfb: Duration::from_millis(12),
            total: Duration::from_micros(13_400),
            engine_overhead: Duration::from_nanos(1_400_500),
        };
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&timing).expect("json")).expect("value");
        assert_eq!(json["dns_ms"], 1.5);
        assert_eq!(json["connect_ms"], 0.0);
        assert_eq!(json["ttfb_ms"], 12.0);
        assert_eq!(json["total_ms"], 13.4);
        assert_eq!(json["engine_overhead_ms"], 1.401);
    }
}
