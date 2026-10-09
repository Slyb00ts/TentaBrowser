//! `browser_navigate` and `browser_close`.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tenta_browser::{Goto, Settle};

use super::{Tool, ToolOutput, handler};
use crate::error::{ErrorKind, ToolError};
use crate::session::{BrowserSession, Settled};

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "browser_navigate",
            "Navigate to a URL",
            "Loads an http or https URL in the session's page and waits for it to settle \
             (the load event, then the page's own load-driven scripts and requests). Use it to \
             open a page, or to go somewhere directly instead of clicking. Returns the final URL \
             after redirects, the HTTP status, the title and whether the page settled; an HTTP \
             4xx or 5xx status is reported, not an error, because the error page is content. \
             Fails with `navigation_failed` when the connection or TLS fails, `blocked` when \
             policy refuses the destination (private networks, other schemes), `timeout` when \
             the navigation does not finish. Settling means load-driven work has finished, not \
             that content arriving later over sockets or workers has rendered; use \
             browser_wait_for for that. Call browser_snapshot next to read the page.",
            json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Absolute http or https URL."
                    },
                    "wait_until": {
                        "type": "string",
                        "enum": ["load", "domcontentloaded", "networkidle"],
                        "description": "What to wait for. `networkidle` (default) also waits for the page's load-driven requests and timers, up to the busy cap."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "minimum": 1000,
                        "maximum": 120000,
                        "description": "Deadline for the whole navigation. Default 30000."
                    }
                },
                "required": ["url"],
                "additionalProperties": false,
                "examples": [
                    {"url": "https://example.com/"},
                    {"url": "https://example.com/app", "wait_until": "load", "timeout_ms": 10000}
                ]
            }),
            navigate_handler,
        ),
        Tool::new(
            "browser_close",
            "Close the page",
            "Closes the session's page and forgets every element ref. The next tool call opens a \
             fresh blank page in the same browser context, so cookies and storage survive. Use it \
             to release memory when a task is done, or to start over from a clean page.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
                "examples": [{}]
            }),
            close_handler,
        ),
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NavigateArgs {
    url: String,
    wait_until: Option<String>,
    timeout_ms: Option<u64>,
}

handler!(navigate_handler, NavigateArgs, navigate);
handler!(close_handler, CloseArgs, close);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CloseArgs {}

async fn navigate(
    session: &mut BrowserSession,
    args: NavigateArgs,
) -> Result<ToolOutput, ToolError> {
    let url = url::Url::parse(&args.url)
        .map_err(|error| ToolError::invalid(format!("`{}` is not a URL: {error}", args.url)))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ToolError::new(
            ErrorKind::Blocked,
            format!(
                "the `{}` scheme is not allowed; browser_navigate opens http and https URLs",
                url.scheme()
            ),
        ));
    }
    let wait_until = match args.wait_until.as_deref() {
        Some("load") => Settle::Load,
        Some("domcontentloaded") => Settle::DomContent,
        _ => Settle::NetworkIdle,
    };
    let timeout = args
        .timeout_ms
        .map_or(session.options().navigation_timeout, Duration::from_millis);
    let page = session.page().await?;
    let navigation = page
        .goto(
            url.as_str(),
            Goto::new().wait_until(wait_until).timeout(timeout),
        )
        .await?;
    if let Some(code) = &navigation.error {
        let kind = crate::error::net_kind(code);
        let error = ToolError::new(kind, format!("{} could not be loaded: {code}", args.url));
        return Err(if kind == ErrorKind::NavigationFailed {
            error.retryable()
        } else {
            error
        });
    }
    let settled = Settled::from_state(navigation.settle);
    if settled.timed_out && navigation.status == 0 {
        return Err(ToolError::new(
            ErrorKind::Timeout,
            format!(
                "{} did not finish loading within {} ms",
                args.url,
                timeout.as_millis()
            ),
        )
        .retryable());
    }
    // With the render tier the first layout is the slow part of a snapshot;
    // doing it here lets the snapshot (and clicks) reuse it.
    if session.engine().max_tier() >= tenta_core::Tier::Render {
        let _ = super::snapshot::layout_of(&page).await;
    }
    let title = page.title().unwrap_or_default();
    let summary = format!(
        "Navigated to {} (HTTP {}). Title: {:?}.{} Call browser_snapshot to read the page.",
        navigation.final_url,
        navigation.status,
        title,
        if settled.timed_out {
            " The page had not settled when the deadline passed."
        } else if settled.busy {
            " The page was still busy when the busy cap passed."
        } else {
            ""
        },
    );
    Ok(ToolOutput::text(
        summary,
        json!({
            "url": navigation.url.as_str(),
            "final_url": navigation.final_url.as_str(),
            "status": navigation.status,
            "ok": navigation.ok(),
            "content_type": navigation.content_type,
            "title": title,
            "redirects": navigation.redirects.len(),
            "settled": settled.to_json(),
        }),
    ))
}

async fn close(session: &mut BrowserSession, _args: CloseArgs) -> Result<ToolOutput, ToolError> {
    let had_page = session.has_page();
    session.discard_page().await;
    Ok(ToolOutput::text(
        if had_page {
            "Closed the page."
        } else {
            "There was no open page."
        },
        json!({ "closed": had_page }),
    ))
}
