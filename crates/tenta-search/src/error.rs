//! Failures the search path reports (`docs/spec/08-search.md` §4).
//!
//! Every failure an agent can act on is typed: a blocked provider names its
//! block class so the caller can switch providers or wait, a plan that no
//! longer matches its provider is a [`BlockClass::Layout`] block with a
//! fixture-diff hint, and everything else is either a transport error or a
//! caller mistake.

use tenta_net::NetError;

use crate::model::{Mode, Provider};

/// Why a provider refused to answer (`docs/spec/08-search.md` §4).
///
/// The class decides where the escalation ladder starts, so it is part of the
/// public contract and not a diagnostic string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockClass {
    /// The connection was reset or closed before any response head arrived.
    /// Retried once on a fresh connection before anything else.
    Reset,
    /// HTTP 429, or a "suspicious traffic" page. IP-keyed: only a different
    /// proxy helps.
    RateLimited,
    /// A CAPTCHA or interstitial challenge. IP-keyed like `RateLimited`.
    Captcha,
    /// The provider served a "turn on JavaScript" page; the fast path cannot
    /// serve it. `Mode::Full` is P2.
    JsRequired,
    /// A 200 whose markup the plan's selectors did not match: a provider
    /// layout change. The fix is a fresh fixture and a plan bump, never a
    /// retry.
    Layout,
    /// A 200 whose results do not echo any query term in titles/snippets/URLs
    /// or carries a localized "no results" marker. Indicates a localised
    /// interstitial, consent page, or bot-score fallback.
    Soft,
}

impl BlockClass {
    /// The name used in `SearchError::Blocked` and in the spec's table.
    pub fn as_str(self) -> &'static str {
        match self {
            BlockClass::Reset => "reset",
            BlockClass::RateLimited => "rate-limited",
            BlockClass::Captcha => "captcha",
            BlockClass::JsRequired => "js-required",
            BlockClass::Layout => "layout",
            BlockClass::Soft => "soft",
        }
    }

    /// Whether rotating to another exit IP is the ladder step that can help
    /// (`docs/spec/08-search.md` §4, step 3).
    pub fn is_ip_keyed(self) -> bool {
        matches!(self, BlockClass::RateLimited | BlockClass::Captcha)
    }
}

impl std::fmt::Display for BlockClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A parse plan that could not be loaded, validated or compiled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanError {
    /// The plan file or provider id the problem belongs to.
    pub plan: String,
    pub detail: String,
}

impl PlanError {
    pub fn new(plan: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            plan: plan.into(),
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "plan {}: {}", self.plan, self.detail)
    }
}

impl std::error::Error for PlanError {}

/// Everything `Search::query` can fail with.
///
/// `Clone` because identical concurrent queries coalesce on one in-flight
/// future (`08` §3.6) and every waiter is handed the same outcome.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SearchError {
    /// The provider refused to answer; `reason` says where the ladder stopped.
    #[error("{provider} blocked the query ({reason}): {detail}")]
    Blocked {
        provider: Provider,
        reason: BlockClass,
        /// What was observed: the status, the selector or the pattern that
        /// matched, or the fixture-diff hint for a layout change.
        detail: String,
    },

    /// The transport failed with a Chromium `net::ERR_*` name.
    #[error("network: {0}")]
    Net(#[from] NetError),

    /// A parse plan is missing, malformed or not applicable.
    #[error("{0}")]
    Plan(#[from] PlanError),

    /// The provider does not serve this mode in this phase. Google needs
    /// `Mode::Full`, which lands in P2 (`docs/spec/16-roadmap.md`).
    #[error("{provider} does not support {mode} mode in this build")]
    Unsupported { provider: Provider, mode: Mode },

    /// The provider paginates through a form or a link the fast path cannot
    /// reconstruct from parameters alone.
    #[error("{provider}: page {page} is not reachable on the fast path: {detail}")]
    Pagination {
        provider: Provider,
        page: u32,
        detail: String,
    },

    /// The caller's request is outside the model's bounds.
    #[error("invalid search request: {0}")]
    InvalidRequest(String),

    /// No provider was left to query after applying `providers_enabled`.
    #[error("no provider available: {0}")]
    NoProvider(String),
}

impl SearchError {
    /// The block class, when this failure is a block.
    pub fn block_class(&self) -> Option<BlockClass> {
        match self {
            SearchError::Blocked { reason, .. } => Some(*reason),
            _ => None,
        }
    }

    /// The provider the failure belongs to, when it belongs to one.
    pub fn provider(&self) -> Option<&Provider> {
        match self {
            SearchError::Blocked { provider, .. }
            | SearchError::Unsupported { provider, .. }
            | SearchError::Pagination { provider, .. } => Some(provider),
            _ => None,
        }
    }
}
