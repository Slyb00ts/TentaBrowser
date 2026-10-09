//! `browser_wait_for`: content that arrives after the page settled.

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;
use tenta_dom::extract::text::collapsed_text;

use super::{Tool, ToolOutput, handler};
use crate::error::{ErrorKind, ToolError};
use crate::session::{BrowserSession, read_doc};

/// The longest `time_ms` sleeps.
const MAX_SLEEP: Duration = Duration::from_secs(30);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_TIMEOUT: Duration = Duration::from_secs(60);

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "browser_wait_for",
            "Wait for content or time",
            "Waits until the page shows text, stops showing text, or contains an element matching \
             a CSS selector, or until a fixed time has passed. Use it for content that arrives \
             after the page settled: results fetched by script, content over sockets or from \
             workers. Acting tools only wait for load-driven work to finish, so call this after \
             them when the next step depends on something appearing. Give one or more of `text`, \
             `text_gone`, `selector` and `time_ms`; all given conditions must hold. `time_ms` \
             sleeps first (at most 30000). The wait is bounded by `timeout_ms` (default 10000, \
             at most 60000) and fails with `timeout`, naming the unmet conditions. Text matches \
             the page's visible text, whitespace collapsed; a selector matches any element in the \
             document.",
            json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "minLength": 1, "description": "Text that must appear on the page."},
                    "text_gone": {"type": "string", "minLength": 1, "description": "Text that must no longer appear."},
                    "selector": {"type": "string", "minLength": 1, "description": "CSS selector that must match an element."},
                    "time_ms": {"type": "integer", "minimum": 0, "maximum": 30000, "description": "Sleep this long first."},
                    "timeout_ms": {"type": "integer", "minimum": 100, "maximum": 60000, "description": "Give up after this long. Default 10000."}
                },
                "additionalProperties": false,
                "examples": [{"text": "Results"}, {"selector": "#done", "timeout_ms": 5000}, {"time_ms": 500}]
            }),
            wait_handler,
        )
        .read_only(),
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitArgs {
    text: Option<String>,
    text_gone: Option<String>,
    selector: Option<String>,
    time_ms: Option<u64>,
    timeout_ms: Option<u64>,
}

handler!(wait_handler, WaitArgs, wait);

/// What the page shows right now, against the conditions.
#[derive(Clone, Copy, Default)]
struct Status {
    text: bool,
    gone: bool,
    selector: bool,
    selector_invalid: bool,
}

async fn wait(session: &mut BrowserSession, args: WaitArgs) -> Result<ToolOutput, ToolError> {
    if args.text.is_none()
        && args.text_gone.is_none()
        && args.selector.is_none()
        && args.time_ms.is_none()
    {
        return Err(ToolError::invalid(
            "give at least one of `text`, `text_gone`, `selector` and `time_ms`",
        ));
    }
    let started = Instant::now();
    let timeout = args
        .timeout_ms
        .map_or(DEFAULT_TIMEOUT, Duration::from_millis)
        .min(MAX_TIMEOUT);
    let deadline = started + timeout;
    if let Some(sleep) = args.time_ms {
        tokio::time::sleep(Duration::from_millis(sleep).min(MAX_SLEEP)).await;
    }
    let conditions = args.text.is_some() || args.text_gone.is_some() || args.selector.is_some();
    let page = session.page().await?;
    let mut status = Status::default();
    if conditions {
        // The whole polling, reads of the document and waits for a
        // navigation included, ends at the deadline.
        let polled = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            poll(session, &page, &args, &mut status),
        )
        .await;
        match polled {
            Ok(done) => done?,
            Err(_) => {
                return Err(ToolError::new(
                    ErrorKind::Timeout,
                    format!("after {} ms: {}", timeout.as_millis(), unmet(&args, status)),
                )
                .retryable());
            }
        }
    }
    let waited = started.elapsed();
    Ok(ToolOutput::text(
        format!("Wait finished after {} ms.", waited.as_millis()),
        json!({
            "waited_ms": u64::try_from(waited.as_millis()).unwrap_or(u64::MAX),
            "text": args.text.is_some(),
            "text_gone": args.text_gone.is_some(),
            "selector": args.selector.is_some(),
            "time_ms": args.time_ms,
        }),
    ))
}

fn unmet(args: &WaitArgs, status: Status) -> String {
    let mut unmet = Vec::new();
    if args.text.is_some() && !status.text {
        unmet.push(format!(
            "text {:?} did not appear",
            args.text.as_deref().unwrap_or_default()
        ));
    }
    if args.text_gone.is_some() && !status.gone {
        unmet.push(format!(
            "text {:?} is still shown",
            args.text_gone.as_deref().unwrap_or_default()
        ));
    }
    if args.selector.is_some() && !status.selector {
        unmet.push(format!(
            "no element matches {:?}",
            args.selector.as_deref().unwrap_or_default()
        ));
    }
    unmet.join("; ")
}

/// Polls until every condition holds. `status` keeps the last verdict, so a
/// caller that cancels this future can still say what was missing.
async fn poll(
    session: &BrowserSession,
    page: &tenta_browser::Page,
    args: &WaitArgs,
    status: &mut Status,
) -> Result<(), ToolError> {
    let mut interval = Duration::from_millis(25);
    loop {
        super::snapshot::after_navigation(session, page).await;
        let (text, gone, selector) = (
            args.text.clone(),
            args.text_gone.clone(),
            args.selector.clone(),
        );
        let probe = read_doc(page, move |doc| {
            let mut status = Status {
                text: true,
                gone: true,
                selector: true,
                selector_invalid: false,
            };
            if text.is_some() || gone.is_some() {
                let shown = collapsed_text(doc, doc.document_node());
                status.text = text
                    .as_deref()
                    .is_none_or(|text| shown.contains(&collapse(text)));
                status.gone = gone
                    .as_deref()
                    .is_none_or(|gone| !shown.contains(&collapse(gone)));
            }
            if let Some(selector) = &selector {
                match doc.query_selector(doc.document_node(), selector) {
                    Ok(found) => status.selector = found.is_some(),
                    Err(_) => {
                        status.selector = false;
                        status.selector_invalid = true;
                    }
                }
            }
            status
        })
        .await;
        match probe {
            Ok(probed) => *status = probed,
            // A navigation replacing the document or a busy render is not a
            // verdict; ask again.
            Err(error) if error.kind == ErrorKind::InvalidArguments || error.retryable => {}
            Err(error) => return Err(error),
        }
        if status.selector_invalid {
            return Err(ToolError::invalid(format!(
                "`{}` is not a valid CSS selector",
                args.selector.as_deref().unwrap_or_default()
            )));
        }
        if status.text && status.gone && status.selector {
            return Ok(());
        }
        tokio::time::sleep(interval).await;
        interval = (interval * 2).min(Duration::from_millis(250));
    }
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
