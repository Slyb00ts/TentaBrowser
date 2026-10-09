//! JSON-RPC 2.0 framing for MCP: parsing a message, building replies, and
//! the error codes the protocol uses.

use serde_json::{Map, Value, json};

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
/// MCP 2026-07-28: the HTTP headers disagree with the body.
pub const HEADER_MISMATCH: i64 = -32020;
/// MCP 2026-07-28: the request names a protocol version this server lacks.
pub const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// A protocol-level failure, as opposed to a tool that ran and failed.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(METHOD_NOT_FOUND, format!("method not found: {method}"))
    }

    pub fn to_value(&self) -> Value {
        let mut error = Map::new();
        error.insert("code".to_owned(), json!(self.code));
        error.insert("message".to_owned(), json!(self.message));
        if let Some(data) = &self.data {
            error.insert("data".to_owned(), data.clone());
        }
        Value::Object(error)
    }
}

/// A message a client may send.
#[derive(Debug)]
pub enum Message {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// A response to a request the server never sends. It is accepted and
    /// ignored: this server does not issue requests.
    Response,
}

/// Classifies a decoded JSON value. The error carries the id to answer with
/// (`null` when there is none).
pub fn classify(value: Value) -> Result<Message, (Value, RpcError)> {
    let Value::Object(mut object) = value else {
        return Err((
            Value::Null,
            RpcError::new(
                INVALID_REQUEST,
                "a JSON-RPC message must be a JSON object (batches are not supported)",
            ),
        ));
    };
    let id = object.remove("id");
    if let Some(id) = &id
        && !matches!(id, Value::String(_) | Value::Number(_))
    {
        return Err((
            Value::Null,
            RpcError::new(INVALID_REQUEST, "the id must be a string or a number"),
        ));
    }
    let Some(method) = object.remove("method") else {
        return if id.is_some() && (object.contains_key("result") || object.contains_key("error")) {
            Ok(Message::Response)
        } else {
            Err((
                id.unwrap_or(Value::Null),
                RpcError::new(INVALID_REQUEST, "the message has no method"),
            ))
        };
    };
    let Value::String(method) = method else {
        return Err((
            id.unwrap_or(Value::Null),
            RpcError::new(INVALID_REQUEST, "the method must be a string"),
        ));
    };
    let params = object.remove("params").unwrap_or(Value::Null);
    if !matches!(params, Value::Null | Value::Object(_)) {
        return Err((
            id.unwrap_or(Value::Null),
            RpcError::invalid_params("params must be an object"),
        ));
    }
    Ok(match id {
        Some(id) => Message::Request { id, method, params },
        None => Message::Notification { method, params },
    })
}

pub fn success(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn failure(id: &Value, error: &RpcError) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": error.to_value() })
}

/// The reply to bytes that are not JSON at all.
pub fn parse_failure(detail: &str) -> Value {
    failure(
        &Value::Null,
        &RpcError::new(PARSE_ERROR, format!("parse error: {detail}")),
    )
}
