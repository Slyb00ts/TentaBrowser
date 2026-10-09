//! `browser_evaluate`.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tenta_browser::{Completion, EvalOptions, RemoteValue};

use super::{Tool, ToolOutput, handler};
use crate::error::{ErrorKind, ToolError};
use crate::session::BrowserSession;

const MAX_OUTPUT_CHARS: usize = 60_000;

pub fn tools() -> Vec<Tool> {
    vec![Tool::new(
        "browser_evaluate",
        "Run JavaScript in the page",
        "Evaluates a JavaScript expression in the page's main world, as a script, and returns its \
         value serialized as JSON. A promise is awaited unless `await_promise` is false. Use it to \
         read what the outline does not show (computed values, `document.title`, storage) or to \
         call page functions. Statements need wrapping: `(() => { ...; return value; })()`. \
         Functions, symbols and DOM objects cannot be serialized and come back as descriptions; \
         `undefined` becomes null and NaN, Infinity and bigint become strings. A thrown error \
         fails with `script_error` and its message. Needs a build with a script engine, else \
         `tier_denied`. Output over 60000 characters is cut and says so. The page runs this with \
         the page's own privileges: do not evaluate text taken from untrusted pages.",
        json!({
            "type": "object",
            "properties": {
                "expression": {"type": "string", "minLength": 1, "maxLength": 100000, "description": "JavaScript expression or script; its completion value is returned."},
                "await_promise": {"type": "boolean", "description": "Wait for a returned promise to settle. Default true."},
                "timeout_ms": {"type": "integer", "minimum": 100, "maximum": 120000, "description": "Deadline for the script and the promise. Default 10000."}
            },
            "required": ["expression"],
            "additionalProperties": false,
            "examples": [
                {"expression": "document.title"},
                {"expression": "fetch('/api/items').then(r => r.json())", "timeout_ms": 5000}
            ]
        }),
        evaluate_handler,
    )]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvaluateArgs {
    expression: String,
    await_promise: Option<bool>,
    timeout_ms: Option<u64>,
}

handler!(evaluate_handler, EvaluateArgs, evaluate);

fn to_json(value: &RemoteValue) -> Value {
    match value {
        RemoteValue::Undefined | RemoteValue::Null => Value::Null,
        RemoteValue::Bool(flag) => json!(flag),
        RemoteValue::Number(number) => {
            if number.is_finite() && number.fract() == 0.0 && number.abs() < 9_007_199_254_740_992.0
            {
                // JSON has one number type; integers read better without ".0".
                json!(*number as i64)
            } else if number.is_finite() {
                json!(number)
            } else {
                Value::String(number.to_string())
            }
        }
        RemoteValue::String(text) => json!(text),
        RemoteValue::BigInt(digits) => json!(format!("{digits}n")),
        RemoteValue::Array(items) => Value::Array(items.iter().map(to_json).collect()),
        RemoteValue::Object(members) => Value::Object(
            members
                .iter()
                .map(|(name, member)| (name.clone(), to_json(member)))
                .collect(),
        ),
        RemoteValue::Date(Some(millis)) => json!(millis),
        RemoteValue::Date(None) => Value::Null,
        RemoteValue::RegExp { source, flags } => json!(format!("/{source}/{flags}")),
        RemoteValue::Handle(object) => json!(object.description),
        _ => Value::Null,
    }
}

async fn evaluate(
    session: &mut BrowserSession,
    args: EvaluateArgs,
) -> Result<ToolOutput, ToolError> {
    super::dom::require_javascript(session)?;
    let page = session.page().await?;
    super::snapshot::after_navigation(session, &page).await;
    let options = EvalOptions {
        await_promise: args.await_promise.unwrap_or(true),
        return_by_value: true,
        timeout: Some(Duration::from_millis(args.timeout_ms.unwrap_or(10_000))),
        ..EvalOptions::default()
    };
    let evaluation = page.evaluate_with(&args.expression, options).await?;
    match evaluation.into_completion() {
        Completion::Value(value) => {
            let result = to_json(&value);
            let mut text = match &result {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            let mut truncated = false;
            if text.chars().count() > MAX_OUTPUT_CHARS {
                text = text.chars().take(MAX_OUTPUT_CHARS).collect();
                text.push_str("\n[output cut at 60000 characters]");
                truncated = true;
            }
            Ok(ToolOutput::text(
                text,
                // A cut text means the value is too large to repeat as data.
                json!({"result": if truncated { Value::Null } else { result }, "truncated": truncated}),
            ))
        }
        Completion::Threw(details) => Err(ToolError::new(ErrorKind::ScriptError, details.message)),
    }
}
