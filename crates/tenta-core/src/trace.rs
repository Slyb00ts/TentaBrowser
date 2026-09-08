//! Tracing conventions (`docs/spec/21-project-operations.md` §5).
//!
//! Levels: `error`/`warn`/`info` for host lifecycle, navigations and budget
//! hits; `debug` for redacted protocol traffic; `trace` for engine
//! internals. Spans carry the ids of `crate::ids` as fields so a page's
//! events can be filtered with `page=<id>`.

pub use tracing::{Level, debug, debug_span, error, info, info_span, trace, trace_span, warn};

use crate::ids::{ContextId, PageId};

/// Span for work on behalf of a page. Page threads enter it per task.
#[inline]
pub fn page_span(page: PageId) -> tracing::Span {
    tracing::info_span!("page", page = page.raw())
}

/// Span for work on behalf of a browser context.
#[inline]
pub fn context_span(context: ContextId) -> tracing::Span {
    tracing::info_span!("context", context = context.raw())
}

/// Records a budget hit at `warn` with stable field names, so metrics and
/// log queries can key on them.
pub fn budget_hit(page: PageId, kind: crate::budgets::BudgetKind, component: Option<&str>) {
    tracing::warn!(
        page = page.raw(),
        budget = %kind,
        component = component.unwrap_or(""),
        "budget exceeded"
    );
}
