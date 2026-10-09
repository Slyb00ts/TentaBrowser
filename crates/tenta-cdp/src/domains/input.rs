//! The `Input` domain: pointer and keyboard events as a user would cause
//! them.
//!
//! A mouse event is resolved against the layout of the document as it is now
//! (`geometry::element_at`) and dispatched to that element by the page-side
//! script (`interaction.js`): `mousemove`, `mousedown`, `mouseup`, `click`
//! with focus changes and the activation behaviour of checkboxes, radios,
//! labels and submit buttons. Keys become `keydown`, `keypress`, `input` and
//! `keyup` with editing of text inputs and textareas. Events are synthetic
//! (`isTrusted` is false) and there is no scrolling, so `mouseWheel` is
//! accepted and ignored. Touch and drag input are refused by name.

use serde::Deserialize;
use serde_json::{Value, json};
use tenta_browser::{Goto, RemoteValue};

use super::geometry;
use super::nodes::{self, path_of, read};
use crate::backend::Target;
use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry, parse_params};
use crate::error::CdpError;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Input);
}

struct Input;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MouseEvent {
    r#type: String,
    x: f64,
    y: f64,
    #[serde(default)]
    modifiers: u32,
    button: Option<String>,
    buttons: Option<u32>,
    click_count: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyEvent {
    r#type: String,
    #[serde(default)]
    modifiers: u32,
    text: Option<String>,
    code: Option<String>,
    key: Option<String>,
    windows_virtual_key_code: Option<u32>,
    location: Option<u32>,
    #[serde(default)]
    auto_repeat: bool,
    #[serde(default)]
    commands: Vec<String>,
}

#[derive(Deserialize)]
struct InsertText {
    text: String,
}

/// The `Input.DispatchMouseEvent` modifier bits.
fn modifier_flags(modifiers: u32) -> Value {
    json!({
        "altKey": modifiers & 1 != 0,
        "ctrlKey": modifiers & 2 != 0,
        "metaKey": modifiers & 4 != 0,
        "shiftKey": modifiers & 8 != 0,
    })
}

fn button_number(button: Option<&str>) -> Result<u32, CdpError> {
    match button.unwrap_or("none") {
        "none" | "left" => Ok(0),
        "middle" => Ok(1),
        "right" => Ok(2),
        "back" => Ok(3),
        "forward" => Ok(4),
        other => Err(CdpError::invalid_params(format!(
            "unknown button {other:?}"
        ))),
    }
}

fn merged(base: Value, extra: Value) -> Value {
    let (Value::Object(mut base), Value::Object(extra)) = (base, extra) else {
        unreachable!("both are object literals");
    };
    base.extend(extra);
    Value::Object(base)
}

/// Runs a page-side action and carries out what it reports: a navigation to
/// start, or the refusal of something the page side cannot do.
async fn act(cx: &CommandContext, target: &Target, expression: &str) -> Result<(), CdpError> {
    let RemoteValue::Object(report) = nodes::run(cx, target, expression).await? else {
        return Ok(());
    };
    for (name, value) in report {
        match (name.as_str(), value) {
            ("unsupported", RemoteValue::String(what)) => {
                return Err(CdpError::server(format!(
                    "Editing a {what} element is not supported"
                )));
            }
            // The page side already refuses other schemes; the host checks
            // again because a refused navigation replaces the document with
            // an error page, which a default action must never do.
            ("navigate", RemoteValue::String(url))
                if url::Url::parse(&url)
                    .is_ok_and(|url| matches!(url.scheme(), "http" | "https")) =>
            {
                let options = Goto::new().timeout(cx.backend().options().navigation_timeout);
                // The navigation outlives the command, as a click's does.
                if let Ok(pending) = target.page().begin_goto(&url, options) {
                    tokio::spawn(pending.finish());
                }
            }
            _ => {}
        }
    }
    Ok(())
}

async fn mouse(cx: &CommandContext, target: &Target, event: MouseEvent) -> Result<(), CdpError> {
    let kind = match event.r#type.as_str() {
        "mousePressed" | "mouseReleased" | "mouseMoved" => event.r#type.as_str(),
        // There is no scroll position to change.
        "mouseWheel" => return Ok(()),
        other => {
            return Err(CdpError::invalid_params(format!(
                "Unknown mouse event type: {other}"
            )));
        }
    };
    if !(event.x.is_finite() && event.y.is_finite()) {
        return Err(CdpError::invalid_params("x and y must be finite numbers"));
    }
    let button = button_number(event.button.as_deref())?;
    nodes::after_navigation(cx, target.page()).await;
    let Some(node) = geometry::element_at(target, event.x, event.y).await? else {
        return Ok(());
    };
    let Some(path) = read(target.page(), move |doc| path_of(doc, node)).await? else {
        return Ok(());
    };
    let init = merged(
        json!({
            "x": event.x,
            "y": event.y,
            "button": button,
            "buttons": event.buttons.unwrap_or(0),
            "detail": event.click_count.unwrap_or(0),
        }),
        modifier_flags(event.modifiers),
    );
    let expression = format!(
        "globalThis.__tentaCdp.mouse(globalThis.__tentaCdp.nodeAt({}), {}, {})",
        serde_json::to_string(&path).expect("a list of numbers"),
        serde_json::to_string(kind).expect("a string"),
        init,
    );
    act(cx, target, &expression).await
}

async fn key(cx: &CommandContext, target: &Target, event: KeyEvent) -> Result<(), CdpError> {
    match event.r#type.as_str() {
        "keyDown" | "keyUp" | "rawKeyDown" | "char" => {}
        other => {
            return Err(CdpError::invalid_params(format!(
                "Unknown key event type: {other}"
            )));
        }
    }
    let init = merged(
        json!({
            "key": event.key.clone().or_else(|| event.text.clone()).unwrap_or_default(),
            "code": event.code.unwrap_or_default(),
            "text": event.text.unwrap_or_default(),
            "keyCode": event.windows_virtual_key_code.unwrap_or(0),
            "location": event.location.unwrap_or(0),
            "repeat": event.auto_repeat,
            "commands": event.commands,
        }),
        modifier_flags(event.modifiers),
    );
    let expression = format!(
        "globalThis.__tentaCdp.key({}, {})",
        serde_json::to_string(&event.r#type).expect("a string"),
        init,
    );
    act(cx, target, &expression).await
}

impl DomainHandler for Input {
    fn domain(&self) -> &'static str {
        "Input"
    }

    fn methods(&self) -> &'static [&'static str] {
        &[
            "dispatchMouseEvent",
            "dispatchKeyEvent",
            "insertText",
            "dispatchTouchEvent",
            "dispatchDragEvent",
            "setInterceptDrags",
        ]
    }

    fn handle<'a>(
        &'a self,
        cx: CommandContext,
        method: &'a str,
        params: Value,
    ) -> CommandFuture<'a> {
        Box::pin(async move {
            match method {
                "dispatchMouseEvent" => {
                    mouse(&cx, cx.target()?, parse_params(params)?).await?;
                    Ok(json!({}))
                }
                "dispatchKeyEvent" => {
                    key(&cx, cx.target()?, parse_params(params)?).await?;
                    Ok(json!({}))
                }
                "insertText" => {
                    let InsertText { text } = parse_params(params)?;
                    let expression = format!(
                        "globalThis.__tentaCdp.insertText({})",
                        serde_json::to_string(&text).expect("a string")
                    );
                    act(&cx, cx.target()?, &expression).await?;
                    Ok(json!({}))
                }
                "dispatchTouchEvent" | "dispatchDragEvent" | "setInterceptDrags" => {
                    Err(CdpError::server(format!(
                        "Input.{method} is not supported: touch and drag input are not built"
                    )))
                }
                other => Err(CdpError::method_not_found(&format!("Input.{other}"))),
            }
        })
    }
}
