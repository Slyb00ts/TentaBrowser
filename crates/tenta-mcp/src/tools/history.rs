//! `browser_back`, `browser_forward` and `browser_reload`.
//!
//! The session keeps the URLs its page has shown. Going back or forward loads
//! the entry's URL again: there is no back/forward cache, so the page is
//! fetched (or answered from the HTTP cache) and its scripts run from the
//! start.

use serde::Deserialize;
use serde_json::json;

use super::navigate::{Load, load_properties, open};
use super::{Tool, ToolOutput, handler};
use crate::error::ToolError;
use crate::session::BrowserSession;

pub fn tools() -> Vec<Tool> {
    let (wait_until, timeout_ms) = load_properties();
    let schema = json!({
        "type": "object",
        "properties": {
            "wait_until": wait_until,
            "timeout_ms": timeout_ms
        },
        "additionalProperties": false,
        "examples": [{}, {"wait_until": "load"}]
    });
    vec![
        Tool::new(
            "browser_back",
            "Go back",
            "Goes to the previous page of this session's history: the page shown before the last \
             browser_navigate or the last click, key press or form submission that navigated. The \
             page is loaded again from its URL (there is no back/forward cache), so form input and \
             script state of the earlier visit are not restored, and a page that was the answer to \
             a form POST is requested with GET. Element refs become stale. Fails with \
             `invalid_arguments` when there is no earlier page. Returns what browser_navigate \
             returns, plus how many entries lie behind and ahead.",
            schema.clone(),
            back_handler,
        ),
        Tool::new(
            "browser_forward",
            "Go forward",
            "Goes to the next page of this session's history, undoing a browser_back. The page is \
             loaded again from its URL. Fails with `invalid_arguments` when there is no later page: \
             a new navigation after going back discards the entries ahead. Returns what \
             browser_navigate returns.",
            schema.clone(),
            forward_handler,
        ),
        Tool::new(
            "browser_reload",
            "Reload the page",
            "Loads the current page again from its URL and waits for it to settle. Use it to see \
             fresh content or to start the page's scripts over. Element refs become stale. Fails \
             with `invalid_arguments` when the session has not opened a page yet. Returns what \
             browser_navigate returns.",
            schema,
            reload_handler,
        ),
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadArgs {
    wait_until: Option<String>,
    timeout_ms: Option<u64>,
}

impl LoadArgs {
    fn into_load(self) -> Load {
        Load {
            wait_until: self.wait_until,
            timeout_ms: self.timeout_ms,
        }
    }
}

handler!(back_handler, LoadArgs, back);
handler!(forward_handler, LoadArgs, forward);
handler!(reload_handler, LoadArgs, reload);

async fn travel(
    session: &mut BrowserSession,
    args: LoadArgs,
    delta: isize,
) -> Result<ToolOutput, ToolError> {
    let Some(url) = session.history.peek(delta).map(str::to_owned) else {
        return Err(ToolError::invalid(if delta < 0 {
            "there is no earlier page in this session's history"
        } else {
            "there is no later page in this session's history"
        }));
    };
    let loaded = open(session, &url, &args.into_load()).await?;
    // Moved only once the load succeeded: a failed attempt leaves the
    // history where it was, so the agent can try again or go elsewhere.
    session.history.step(delta);
    Ok(loaded.output(
        if delta < 0 {
            "Went back to"
        } else {
            "Went forward to"
        },
        session,
    ))
}

async fn back(session: &mut BrowserSession, args: LoadArgs) -> Result<ToolOutput, ToolError> {
    travel(session, args, -1).await
}

async fn forward(session: &mut BrowserSession, args: LoadArgs) -> Result<ToolOutput, ToolError> {
    travel(session, args, 1).await
}

async fn reload(session: &mut BrowserSession, args: LoadArgs) -> Result<ToolOutput, ToolError> {
    let Some(url) = session.history.current().map(str::to_owned) else {
        return Err(ToolError::invalid(
            "there is no page to reload; call browser_navigate first",
        ));
    };
    let loaded = open(session, &url, &args.into_load()).await?;
    Ok(loaded.output("Reloaded", session))
}
