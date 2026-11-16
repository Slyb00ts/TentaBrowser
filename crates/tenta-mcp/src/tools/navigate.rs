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
    let load = Load {
        wait_until: args.wait_until,
        timeout_ms: args.timeout_ms,
    };
    let loaded = open(session, url.as_str(), &load).await?;
    session.history.visit(&loaded.final_url);
    Ok(loaded.output("Navigated to", session))
}

/// How a load waits; the arguments `browser_navigate`, `browser_back`,
/// `browser_forward` and `browser_reload` share.
#[derive(Default)]
pub(super) struct Load {
    pub wait_until: Option<String>,
    pub timeout_ms: Option<u64>,
}

/// The schema properties of [`Load`].
pub(super) fn load_properties() -> (serde_json::Value, serde_json::Value) {
    (
        json!({
            "type": "string",
            "enum": ["load", "domcontentloaded", "networkidle"],
            "description": "What to wait for. `networkidle` (default) also waits for the page's load-driven requests and timers, up to the busy cap."
        }),
        json!({
            "type": "integer",
            "minimum": 1000,
            "maximum": 120000,
            "description": "Deadline for the whole navigation. Default 30000."
        }),
    )
}

/// A finished load.
pub(super) struct Loaded {
    pub final_url: String,
    summary_tail: String,
    structured: serde_json::Value,
}

impl Loaded {
    /// The tool result: `verb` starts the summary (`Navigated to`, `Went back to`).
    pub fn output(mut self, verb: &str, session: &BrowserSession) -> ToolOutput {
        let (back, forward) = session.history.around();
        if let Some(fields) = self.structured.as_object_mut() {
            fields.insert(
                "history".to_owned(),
                json!({ "back": back, "forward": forward }),
            );
        }
        ToolOutput::text(format!("{verb} {}", self.summary_tail), self.structured)
    }
}

/// Loads `url` in the session's page and waits as `load` says. Does not touch
/// the session history: the caller knows whether this is a new entry.
pub(super) async fn open(
    session: &mut BrowserSession,
    url: &str,
    load: &Load,
) -> Result<Loaded, ToolError> {
    let wait_until = match load.wait_until.as_deref() {
        Some("load") => Settle::Load,
        Some("domcontentloaded") => Settle::DomContent,
        _ => Settle::NetworkIdle,
    };
    let timeout = load
        .timeout_ms
        .map_or(session.options().navigation_timeout, Duration::from_millis);
    let page = session.page().await?;
    let navigation = match page
        .goto(url, Goto::new().wait_until(wait_until).timeout(timeout))
        .await
    {
        Ok(navigation) => navigation,
        Err(error) => {
            // A page whose script navigates while it loads (a redirect written
            // in JavaScript) replaces the navigation that loaded it; what the
            // agent asked for is the page that results.
            let aborted =
                matches!(&error, tenta_core::Error::Net { code, .. } if code.contains("ABORTED"));
            let replaced = if aborted && page.navigation_in_flight() {
                let _ = tokio::time::timeout(timeout, page.navigation_settled()).await;
                page.navigation()
            } else {
                None
            };
            match replaced {
                Some(navigation) => navigation,
                None => return Err(error.into()),
            }
        }
    };
    if let Some(code) = &navigation.error {
        let kind = crate::error::net_kind(code);
        let error = ToolError::new(kind, format!("{url} could not be loaded: {code}"));
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
                "{url} did not finish loading within {} ms",
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
    let summary_tail = format!(
        "{} (HTTP {}). Title: {:?}.{} Call browser_snapshot to read the page.",
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
    Ok(Loaded {
        final_url: navigation.final_url.to_string(),
        summary_tail,
        structured: json!({
            "url": navigation.url.as_str(),
            "final_url": navigation.final_url.as_str(),
            "status": navigation.status,
            "ok": navigation.ok(),
            "content_type": navigation.content_type,
            "title": title,
            "redirects": navigation.redirects.len(),
            "settled": settled.to_json(),
        }),
    })
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
