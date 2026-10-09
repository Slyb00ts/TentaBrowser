//! JSON-RPC framing: parsing inbound commands, building outbound messages.

use serde_json::{Map, Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::error::CdpError;

/// A syntactically valid command. `method` is still unchecked.
#[derive(Debug)]
pub(crate) struct Command {
    pub id: u64,
    pub session_id: Option<String>,
    pub method: String,
    pub params: Value,
}

#[derive(Debug)]
pub(crate) enum Inbound {
    Command(Command),
    /// The message cannot be a command; `id` and `session_id` are echoed when
    /// they could be recovered.
    Rejected {
        id: Option<u64>,
        session_id: Option<String>,
        error: CdpError,
    },
}

pub(crate) fn parse(text: &str) -> Inbound {
    let reject = |id, session_id, error| Inbound::Rejected {
        id,
        session_id,
        error,
    };
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return reject(None, None, CdpError::parse_error());
    };
    let Value::Object(mut object) = value else {
        return reject(
            None,
            None,
            CdpError::invalid_request("Message must be a JSON object"),
        );
    };
    // Chromium's ids are non-negative 32-bit integers; clients count up from
    // 1, so anything else is a malformed message.
    let Some(id) = object
        .get("id")
        .and_then(Value::as_u64)
        .filter(|id| i32::try_from(*id).is_ok())
    else {
        return reject(
            None,
            None,
            CdpError::invalid_request("Message must have integer 'id' property"),
        );
    };
    let session_id = match object.remove("sessionId") {
        None | Some(Value::Null) => None,
        Some(Value::String(session)) => Some(session),
        Some(_) => {
            return reject(
                Some(id),
                None,
                CdpError::invalid_request("'sessionId' must be a string"),
            );
        }
    };
    let Some(Value::String(method)) = object.remove("method") else {
        return reject(
            Some(id),
            session_id,
            CdpError::invalid_request("Message must have string 'method' property"),
        );
    };
    let params = match object.remove("params") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(params @ Value::Object(_)) => params,
        Some(_) => {
            return reject(
                Some(id),
                session_id,
                CdpError::invalid_params("'params' must be an object"),
            );
        }
    };
    Inbound::Command(Command {
        id,
        session_id,
        method,
        params,
    })
}

fn with_session(mut object: Map<String, Value>, session_id: Option<&str>) -> Message {
    if let Some(session) = session_id {
        object.insert("sessionId".to_owned(), Value::String(session.to_owned()));
    }
    Message::text(Value::Object(object).to_string())
}

fn into_object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(object) => object,
        _ => Map::new(),
    }
}

pub(crate) fn result_message(id: u64, session_id: Option<&str>, result: Value) -> Message {
    with_session(
        into_object(json!({ "id": id, "result": result })),
        session_id,
    )
}

pub(crate) fn error_message(
    id: Option<u64>,
    session_id: Option<&str>,
    error: &CdpError,
) -> Message {
    let mut body = json!({ "code": error.code, "message": error.message });
    if let (Some(data), Some(body)) = (&error.data, body.as_object_mut()) {
        body.insert("data".to_owned(), Value::String(data.clone()));
    }
    let mut object = into_object(json!({ "error": body }));
    if let Some(id) = id {
        object.insert("id".to_owned(), json!(id));
    }
    with_session(object, session_id)
}

pub(crate) fn event_message(method: &str, params: Value, session_id: Option<&str>) -> Message {
    with_session(
        into_object(json!({ "method": method, "params": params })),
        session_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_keeps_its_session_and_defaults_params_to_an_object() {
        let Inbound::Command(command) = parse(r#"{"id":7,"method":"Page.enable","sessionId":"S"}"#)
        else {
            panic!("expected a command");
        };
        assert_eq!(command.id, 7);
        assert_eq!(command.session_id.as_deref(), Some("S"));
        assert_eq!(command.params, json!({}));
    }

    #[test]
    fn rejections_echo_what_could_be_recovered() {
        let Inbound::Rejected { id, error, .. } = parse(r#"{"id":3,"params":{}}"#) else {
            panic!("expected a rejection");
        };
        assert_eq!(id, Some(3));
        assert_eq!(error.code, CdpError::INVALID_REQUEST);

        let Inbound::Rejected { id, error, .. } = parse("{not json") else {
            panic!("expected a rejection");
        };
        assert_eq!(id, None);
        assert_eq!(error.code, CdpError::PARSE_ERROR);

        let Inbound::Rejected { error, .. } = parse(r#"{"id":1,"method":"A.b","params":[]}"#)
        else {
            panic!("expected a rejection");
        };
        assert_eq!(error.code, CdpError::INVALID_PARAMS);
    }

    #[test]
    fn ids_that_are_not_non_negative_int32_are_refused() {
        for text in [
            r#"{"id":-1,"method":"A.b"}"#,
            r#"{"id":1.5,"method":"A.b"}"#,
            r#"{"id":2147483648,"method":"A.b"}"#,
        ] {
            assert!(matches!(parse(text), Inbound::Rejected { id: None, .. }));
        }
    }
}
