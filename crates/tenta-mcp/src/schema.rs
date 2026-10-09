//! The subset of JSON Schema the tool input schemas use, as a validator.
//!
//! The schemas a server advertises must be the schemas it enforces, so every
//! `tools/call` argument object is checked against the tool's own
//! `inputSchema` before a handler sees it. Supported keywords: `type`,
//! `properties`, `required`, `additionalProperties: false`, `enum`,
//! `minimum`, `maximum`, `minLength`, `maxLength`, `maxItems` and `items`.

use serde_json::Value;

/// Checks `value` against `schema`. The error names the offending path.
pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    check(schema, value, "arguments")
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn matches_type(expected: &str, value: &Value) -> bool {
    match expected {
        "number" => value.is_number(),
        "integer" => {
            value.is_i64()
                || value.is_u64()
                || value
                    .as_f64()
                    .is_some_and(|number| number.fract() == 0.0 && number.is_finite())
        }
        other => type_name(value) == other,
    }
}

fn check(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    if let Some(expected) = schema.get("type").and_then(Value::as_str)
        && !matches_type(expected, value)
    {
        return Err(format!(
            "{path}: expected {expected}, got {}",
            type_name(value)
        ));
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        let names: Vec<String> = allowed.iter().map(Value::to_string).collect();
        return Err(format!("{path}: must be one of {}", names.join(", ")));
    }
    if let Some(number) = value.as_f64() {
        if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
            && number < minimum
        {
            return Err(format!("{path}: must be at least {minimum}"));
        }
        if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64)
            && number > maximum
        {
            return Err(format!("{path}: must be at most {maximum}"));
        }
    }
    if let Some(text) = value.as_str() {
        let length = text.chars().count() as u64;
        if let Some(minimum) = schema.get("minLength").and_then(Value::as_u64)
            && length < minimum
        {
            return Err(format!("{path}: must have at least {minimum} characters"));
        }
        if let Some(maximum) = schema.get("maxLength").and_then(Value::as_u64)
            && length > maximum
        {
            return Err(format!("{path}: must have at most {maximum} characters"));
        }
    }
    if let Some(items) = value.as_array() {
        if let Some(maximum) = schema.get("maxItems").and_then(Value::as_u64)
            && items.len() as u64 > maximum
        {
            return Err(format!("{path}: must have at most {maximum} items"));
        }
        if let Some(item_schema) = schema.get("items") {
            for (index, item) in items.iter().enumerate() {
                check(item_schema, item, &format!("{path}[{index}]"))?;
            }
        }
    }
    if let Some(object) = value.as_object() {
        let properties = schema.get("properties").and_then(Value::as_object);
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    return Err(format!("{path}: missing required property `{name}`"));
                }
            }
        }
        for (name, member) in object {
            match properties.and_then(|properties| properties.get(name)) {
                Some(property) => check(property, member, &format!("{path}.{name}"))?,
                None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    return Err(format!("{path}: unknown property `{name}`"));
                }
                None => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "ref": {"type": "string", "minLength": 2},
                "n": {"type": "integer", "minimum": 1, "maximum": 5},
                "mode": {"enum": ["a", "b"]},
                "keys": {"type": "array", "items": {"type": "string"}, "maxItems": 2},
            },
            "required": ["ref"],
            "additionalProperties": false,
        })
    }

    #[test]
    fn accepts_a_conforming_object() {
        let value = json!({"ref": "e1", "n": 3, "mode": "a", "keys": ["x"]});
        assert_eq!(validate(&schema(), &value), Ok(()));
    }

    #[test]
    fn names_the_path_of_each_violation() {
        let cases = [
            (json!({}), "missing required property `ref`"),
            (json!({"ref": 3}), "arguments.ref: expected string"),
            (json!({"ref": "e"}), "at least 2 characters"),
            (json!({"ref": "e1", "n": 9}), "at most 5"),
            (json!({"ref": "e1", "n": 1.5}), "expected integer"),
            (json!({"ref": "e1", "mode": "c"}), "must be one of"),
            (json!({"ref": "e1", "keys": [1]}), "arguments.keys[0]"),
            (
                json!({"ref": "e1", "keys": ["a", "b", "c"]}),
                "at most 2 items",
            ),
            (
                json!({"ref": "e1", "extra": true}),
                "unknown property `extra`",
            ),
        ];
        for (value, expected) in cases {
            let error = validate(&schema(), &value).expect_err("rejected");
            assert!(error.contains(expected), "{error} vs {expected}");
        }
    }
}
