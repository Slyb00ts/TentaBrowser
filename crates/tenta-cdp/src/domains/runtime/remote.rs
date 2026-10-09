//! The JSON shapes of the Runtime domain: remote objects, previews, stack
//! traces and exception details, built from what the page's realm reports.

use serde_json::{Map, Number, Value, json};
use tenta_browser::{
    ConsoleCall, ExceptionDetails, ObjectPreview, PropertyDescriptor, RemoteObject, RemoteValue,
    StackFrame,
};

/// Numbers that are whole and exactly representable are sent as integers, as
/// Chromium does; `1.0` would be a different token to a strict client.
fn number_json(n: f64) -> Value {
    const EXACT: f64 = 9_007_199_254_740_992.0;
    if n.fract() == 0.0 && n.abs() < EXACT {
        return json!(n as i64);
    }
    Number::from_f64(n).map_or(Value::Null, Value::Number)
}

fn number_description(n: f64) -> String {
    if n == 0.0 {
        "0".to_owned()
    } else {
        n.to_string()
    }
}

/// The JSON of a value copied out of the realm, as `returnByValue` sends it.
/// `NaN`, infinities and big integers have no JSON form below the top level
/// and become `null`; dates and regular expressions have no own members, so
/// they are `{}`.
fn by_value(value: &RemoteValue) -> Value {
    match value {
        RemoteValue::Bool(b) => Value::Bool(*b),
        RemoteValue::Number(n) => number_json(*n),
        RemoteValue::String(s) => Value::String(s.clone()),
        RemoteValue::Array(items) => Value::Array(items.iter().map(by_value).collect()),
        RemoteValue::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, member)| (key.clone(), by_value(member)))
                .collect(),
        ),
        RemoteValue::Date(_) | RemoteValue::RegExp { .. } => json!({}),
        // Undefined, null and big integers have no JSON form, and a handle is
        // never copied by value.
        _ => Value::Null,
    }
}

/// A `Runtime.RemoteObject`.
pub(in crate::domains) fn remote_object(value: &RemoteValue) -> Value {
    match value {
        RemoteValue::Undefined => json!({ "type": "undefined" }),
        RemoteValue::Null => json!({ "type": "object", "subtype": "null", "value": null }),
        RemoteValue::Bool(b) => json!({ "type": "boolean", "value": b }),
        RemoteValue::Number(n) => match value.unserializable() {
            Some(spelling) => json!({
                "type": "number",
                "description": if spelling == "-0" { "0" } else { spelling.as_str() },
                "unserializableValue": spelling,
            }),
            None => json!({
                "type": "number",
                "value": number_json(*n),
                "description": number_description(*n),
            }),
        },
        RemoteValue::String(s) => json!({ "type": "string", "value": s }),
        RemoteValue::BigInt(digits) => json!({
            "type": "bigint",
            "unserializableValue": format!("{digits}n"),
            "description": format!("{digits}n"),
        }),
        RemoteValue::Handle(object) => handle(object),
        copied => json!({ "type": "object", "value": by_value(copied) }),
    }
}

fn handle(object: &RemoteObject) -> Value {
    let mut out = Map::new();
    out.insert("type".into(), object.kind.as_str().into());
    if let Some(subtype) = &object.subtype {
        out.insert("subtype".into(), subtype.clone().into());
    }
    out.insert("className".into(), object.class_name.clone().into());
    out.insert("description".into(), object.description.clone().into());
    out.insert("objectId".into(), object.id.clone().into());
    if let Some(preview) = &object.preview {
        out.insert(
            "preview".into(),
            self::preview(object.kind.as_str(), preview),
        );
    }
    Value::Object(out)
}

fn preview(kind: &str, preview: &ObjectPreview) -> Value {
    let properties: Vec<Value> = preview
        .properties
        .iter()
        .map(|property| {
            let mut out = Map::new();
            out.insert("name".into(), property.name.clone().into());
            out.insert("type".into(), property.kind.clone().into());
            if let Some(value) = &property.value {
                out.insert("value".into(), value.clone().into());
            }
            if let Some(subtype) = &property.subtype {
                out.insert("subtype".into(), subtype.clone().into());
            }
            Value::Object(out)
        })
        .collect();
    let mut out = Map::new();
    out.insert("type".into(), kind.into());
    if let Some(subtype) = &preview.subtype {
        out.insert("subtype".into(), subtype.clone().into());
    }
    out.insert("description".into(), preview.description.clone().into());
    out.insert("overflow".into(), preview.overflow.into());
    out.insert("properties".into(), properties.into());
    Value::Object(out)
}

/// A `Runtime.StackTrace`; `None` when there is no frame to show.
pub(super) fn stack_trace(frames: &[StackFrame]) -> Option<Value> {
    if frames.is_empty() {
        return None;
    }
    let call_frames: Vec<Value> = frames
        .iter()
        .map(|frame| {
            json!({
                "functionName": frame.function_name,
                "scriptId": "0",
                "url": frame.url,
                "lineNumber": frame.line.unwrap_or(0),
                "columnNumber": frame.column.unwrap_or(0),
            })
        })
        .collect();
    Some(json!({ "callFrames": call_frames }))
}

/// A `Runtime.ExceptionDetails`.
pub(super) fn exception_details(
    details: &ExceptionDetails,
    exception_id: u64,
    context: u64,
) -> Value {
    let mut out = Map::new();
    out.insert("exceptionId".into(), exception_id.into());
    out.insert("text".into(), details.text.clone().into());
    out.insert("lineNumber".into(), details.line.unwrap_or(0).into());
    out.insert("columnNumber".into(), details.column.unwrap_or(0).into());
    if let Some(frame) = details.frames.first().filter(|frame| !frame.url.is_empty()) {
        out.insert("url".into(), frame.url.clone().into());
    }
    if let Some(trace) = stack_trace(&details.frames) {
        out.insert("stackTrace".into(), trace);
    }
    out.insert("exception".into(), remote_object(&details.exception));
    out.insert("executionContextId".into(), context.into());
    Value::Object(out)
}

/// The `Runtime.consoleAPICalled` parameters of one call.
pub(super) fn console_event(call: &ConsoleCall, context: u64) -> Value {
    let mut out = Map::new();
    out.insert("type".into(), call.kind.clone().into());
    out.insert(
        "args".into(),
        call.args
            .iter()
            .map(remote_object)
            .collect::<Vec<_>>()
            .into(),
    );
    out.insert("executionContextId".into(), context.into());
    out.insert("timestamp".into(), number_json(call.timestamp_ms));
    if let Some(trace) = stack_trace(&call.stack) {
        out.insert("stackTrace".into(), trace);
    }
    Value::Object(out)
}

/// A `Runtime.PropertyDescriptor`.
pub(super) fn property_descriptor(property: &PropertyDescriptor) -> Value {
    let mut out = Map::new();
    out.insert("name".into(), property.name.clone().into());
    if let Some(value) = &property.value {
        out.insert("value".into(), remote_object(value));
    }
    if let Some(writable) = property.writable {
        out.insert("writable".into(), writable.into());
    }
    if let Some(getter) = &property.getter {
        out.insert("get".into(), remote_object(getter));
    }
    if let Some(setter) = &property.setter {
        out.insert("set".into(), remote_object(setter));
    }
    out.insert("configurable".into(), property.configurable.into());
    out.insert("enumerable".into(), property.enumerable.into());
    out.insert("isOwn".into(), property.is_own.into());
    Value::Object(out)
}
