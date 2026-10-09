//! Inspecting a page's realm from the host: evaluation with remote objects,
//! console and exception events, bindings.
//!
//! The inspection logic that has to see JavaScript values (describing an
//! object, copying one by value, walking a stack, the `console` object) is one
//! script, [`INSPECTOR_SOURCE`], that both backends run in their realm. Writing
//! it once is what keeps V8 and QuickJS answering identically; each backend only
//! supplies three things: compiling the script and handing it two native
//! functions, calling the dispatcher it returns, and reporting the uncaught
//! exceptions and unhandled rejections only the engine can see. Everything here
//! is the typed layer above that string protocol.
//!
//! # Handles
//!
//! An object that is not copied by value stays in the realm, in a table the
//! script owns, and the host holds its id ([`RemoteObject::id`]). The table is
//! the strong reference: the object lives until the id (or its object group) is
//! released or the realm ends, so no engine handle outlives the isolate or
//! context that owns it. Ids carry the realm they were issued in, which is how a
//! handle used after a navigation is told apart from one that was released.
//!
//! # Asynchronous results
//!
//! A promise the caller awaits is settled by a reaction the script attaches; the
//! reaction reports through [`InspectorSink::settled`] when the engine next runs
//! microtasks, so nothing here blocks the page thread.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::engine::{PageEngine, ScriptError, ScriptErrorKind};

/// The realm-side script. Evaluates to a function taking
/// `(post, listening, realm)` and returning the dispatcher.
pub const INSPECTOR_SOURCE: &str = include_str!("inspector.js");

/// How many remote objects a realm holds before it refuses to hand out more. A
/// host that needs more, such as one listing every element of a large page,
/// releases what it is done with; the objects themselves are bounded by the
/// page's heap budget, this bounds the bookkeeping and a host that never
/// releases.
pub const HANDLE_LIMIT: u32 = 50_000;

/// The object group console arguments are kept in.
pub const CONSOLE_GROUP: &str = "console";

/// The object group the values of uncaught exceptions are kept in.
pub const EXCEPTION_GROUP: &str = "exception";

/// A number for a realm, unique in this process, so an id issued by one realm is
/// recognisably foreign in another.
pub fn next_realm_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A value that crossed out of a realm.
///
/// Numbers keep their IEEE value, so `NaN`, the infinities and `-0` survive;
/// [`RemoteValue::unserializable`] names the ones JSON cannot carry. A value
/// copied by value is a structural copy: object members whose value is a
/// function or symbol are dropped, `undefined` members are kept.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum RemoteValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    /// The decimal digits of a `bigint`.
    BigInt(String),
    Array(Vec<RemoteValue>),
    Object(Vec<(String, RemoteValue)>),
    /// Milliseconds since the epoch; `None` for an invalid date.
    Date(Option<f64>),
    RegExp {
        source: String,
        flags: String,
    },
    /// An object that stayed in the realm.
    Handle(RemoteObject),
}

impl RemoteValue {
    /// The CDP `unserializableValue` spelling, for the values JSON cannot hold.
    pub fn unserializable(&self) -> Option<String> {
        match self {
            RemoteValue::Number(n) if n.is_nan() => Some("NaN".to_owned()),
            RemoteValue::Number(n) if *n == f64::INFINITY => Some("Infinity".to_owned()),
            RemoteValue::Number(n) if *n == f64::NEG_INFINITY => Some("-Infinity".to_owned()),
            RemoteValue::Number(n) if *n == 0.0 && n.is_sign_negative() => Some("-0".to_owned()),
            RemoteValue::BigInt(digits) => Some(format!("{digits}n")),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            RemoteValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            RemoteValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            RemoteValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The handle, if this value stayed in the realm.
    pub fn into_handle(self) -> Option<RemoteObject> {
        match self {
            RemoteValue::Handle(object) => Some(object),
            _ => None,
        }
    }

    fn to_argument(&self) -> Value {
        match self {
            RemoteValue::Undefined => json!({ "t": "undefined" }),
            RemoteValue::Null => json!({ "t": "null" }),
            RemoteValue::Bool(b) => json!({ "t": "bool", "v": b }),
            RemoteValue::Number(n) => match self.unserializable() {
                Some(spelling) => json!({ "t": "number", "u": spelling }),
                None => json!({ "t": "number", "v": n }),
            },
            RemoteValue::String(s) => json!({ "t": "string", "v": s }),
            RemoteValue::BigInt(digits) => json!({ "t": "bigint", "v": digits }),
            RemoteValue::Array(items) => {
                json!({ "t": "array", "v": items.iter().map(Self::to_argument).collect::<Vec<_>>() })
            }
            RemoteValue::Object(entries) => json!({
                "t": "object",
                "v": entries.iter().map(|(k, v)| json!([k, v.to_argument()])).collect::<Vec<_>>(),
            }),
            RemoteValue::Date(time) => json!({ "t": "date", "v": time }),
            RemoteValue::RegExp { source, flags } => {
                json!({ "t": "regexp", "s": source, "f": flags })
            }
            RemoteValue::Handle(object) => json!({ "t": "handle", "id": object.id }),
        }
    }

    fn from_json(value: &Value) -> Option<RemoteValue> {
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(match value.get("t")?.as_str()? {
            "undefined" => RemoteValue::Undefined,
            "null" => RemoteValue::Null,
            "bool" => RemoteValue::Bool(value.get("v")?.as_bool()?),
            "string" => RemoteValue::String(text("v")?),
            "bigint" => RemoteValue::BigInt(text("v")?),
            "number" => match value.get("u").and_then(Value::as_str) {
                Some("NaN") => RemoteValue::Number(f64::NAN),
                Some("Infinity") => RemoteValue::Number(f64::INFINITY),
                Some("-Infinity") => RemoteValue::Number(f64::NEG_INFINITY),
                Some(_) => RemoteValue::Number(-0.0),
                None => RemoteValue::Number(value.get("v")?.as_f64()?),
            },
            "array" => RemoteValue::Array(
                value
                    .get("v")?
                    .as_array()?
                    .iter()
                    .map(Self::from_json)
                    .collect::<Option<_>>()?,
            ),
            "object" => RemoteValue::Object(
                value
                    .get("v")?
                    .as_array()?
                    .iter()
                    .map(|entry| {
                        let pair = entry.as_array()?;
                        Some((
                            pair.first()?.as_str()?.to_owned(),
                            Self::from_json(pair.get(1)?)?,
                        ))
                    })
                    .collect::<Option<_>>()?,
            ),
            "date" => RemoteValue::Date(value.get("v")?.as_f64()),
            "regexp" => RemoteValue::RegExp {
                source: text("s")?,
                flags: text("f")?,
            },
            "handle" => RemoteValue::Handle(RemoteObject::from_json(value)?),
            _ => return None,
        })
    }
}

/// What kind of non-primitive a handle names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteKind {
    Object,
    Function,
    Symbol,
}

impl RemoteKind {
    /// The CDP `RemoteObject.type` spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            RemoteKind::Object => "object",
            RemoteKind::Function => "function",
            RemoteKind::Symbol => "symbol",
        }
    }
}

/// An object that stayed in the realm, named by an id the host can pass back.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteObject {
    pub id: String,
    pub kind: RemoteKind,
    /// The CDP subtype: `array`, `node`, `error`, `promise`, ...
    pub subtype: Option<String>,
    pub class_name: String,
    pub description: String,
    pub preview: Option<ObjectPreview>,
}

impl RemoteObject {
    fn from_json(value: &Value) -> Option<RemoteObject> {
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(RemoteObject {
            id: text("id")?,
            kind: match value.get("ty")?.as_str()? {
                "function" => RemoteKind::Function,
                "symbol" => RemoteKind::Symbol,
                _ => RemoteKind::Object,
            },
            subtype: text("st"),
            class_name: text("cn")?,
            description: text("d")?,
            preview: match value.get("p").filter(|p| !p.is_null()) {
                Some(preview) => Some(ObjectPreview::from_json(preview)?),
                None => None,
            },
        })
    }
}

/// A shallow summary of an object's own enumerable members.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectPreview {
    pub subtype: Option<String>,
    pub description: String,
    /// More members exist than the preview lists.
    pub overflow: bool,
    pub properties: Vec<PropertyPreview>,
}

impl ObjectPreview {
    fn from_json(value: &Value) -> Option<ObjectPreview> {
        Some(ObjectPreview {
            subtype: value
                .get("subtype")
                .and_then(Value::as_str)
                .map(str::to_owned),
            description: value.get("description")?.as_str()?.to_owned(),
            overflow: value.get("overflow")?.as_bool()?,
            properties: value
                .get("properties")?
                .as_array()?
                .iter()
                .map(|p| {
                    Some(PropertyPreview {
                        name: p.get("name")?.as_str()?.to_owned(),
                        kind: p.get("kind")?.as_str()?.to_owned(),
                        subtype: p.get("subtype").and_then(Value::as_str).map(str::to_owned),
                        value: p.get("value").and_then(Value::as_str).map(str::to_owned),
                    })
                })
                .collect::<Option<_>>()?,
        })
    }
}

/// One member in an [`ObjectPreview`].
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyPreview {
    pub name: String,
    /// The CDP type: `number`, `string`, `object`, `function`, `accessor`, ...
    pub kind: String,
    pub subtype: Option<String>,
    /// The value as short text; absent for an accessor.
    pub value: Option<String>,
}

/// One frame of a captured stack. Lines and columns are zero-based, as in CDP.
#[derive(Clone, Debug, PartialEq)]
pub struct StackFrame {
    pub function_name: String,
    pub url: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl StackFrame {
    fn from_json(value: &Value) -> Option<StackFrame> {
        Some(StackFrame {
            function_name: value.get("fn")?.as_str()?.to_owned(),
            url: value.get("url")?.as_str()?.to_owned(),
            line: value.get("line").and_then(Value::as_u64).map(|n| n as u32),
            column: value.get("col").and_then(Value::as_u64).map(|n| n as u32),
        })
    }
}

/// What an exception, thrown or a promise rejection, looked like.
#[derive(Clone, Debug, PartialEq)]
pub struct ExceptionDetails {
    /// `Uncaught`, or `Uncaught (in promise)` for a rejection.
    pub text: String,
    /// The thrown value converted to a string, `Name: message` for an error.
    pub message: String,
    /// The error's `stack`, when it has one.
    pub stack: Option<String>,
    pub frames: Vec<StackFrame>,
    /// Where it was thrown, zero-based.
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// The thrown value; an object stays in the realm as a handle.
    pub exception: RemoteValue,
}

impl ExceptionDetails {
    fn from_json(value: &Value) -> Option<ExceptionDetails> {
        Some(ExceptionDetails {
            text: value.get("text")?.as_str()?.to_owned(),
            message: value.get("message")?.as_str()?.to_owned(),
            stack: value
                .get("stack")
                .and_then(Value::as_str)
                .map(str::to_owned),
            frames: value
                .get("frames")?
                .as_array()?
                .iter()
                .map(StackFrame::from_json)
                .collect::<Option<_>>()?,
            line: value.get("line").and_then(Value::as_u64).map(|n| n as u32),
            column: value.get("col").and_then(Value::as_u64).map(|n| n as u32),
            exception: RemoteValue::from_json(value.get("ex")?)?,
        })
    }
}

/// How an evaluation ended inside the realm.
#[derive(Clone, Debug, PartialEq)]
pub enum Completion {
    Value(RemoteValue),
    Threw(ExceptionDetails),
}

/// Why the host could not get a [`Completion`] at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectError {
    pub kind: InspectErrorKind,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InspectErrorKind {
    /// The handle was issued by a realm that has ended.
    StaleHandle,
    /// The handle was released, or never issued.
    UnknownHandle,
    /// The realm already holds the most handles it allows.
    HandleLimit,
    /// The result cannot be copied by value (a cycle, a function, too large).
    NotSerializable,
    /// The request was malformed.
    InvalidArgument,
    /// The engine stopped the script: a deadline or a budget.
    Terminated,
    Internal,
}

impl InspectError {
    fn new(kind: InspectErrorKind, message: impl Into<String>) -> InspectError {
        InspectError {
            kind,
            message: message.into(),
        }
    }

    fn from_code(code: &str, message: String) -> InspectError {
        let kind = match code {
            "stale_handle" => InspectErrorKind::StaleHandle,
            "unknown_handle" => InspectErrorKind::UnknownHandle,
            "handle_limit" => InspectErrorKind::HandleLimit,
            "not_serializable" => InspectErrorKind::NotSerializable,
            "invalid_handle" | "invalid_argument" => InspectErrorKind::InvalidArgument,
            _ => InspectErrorKind::Internal,
        };
        InspectError { kind, message }
    }

    fn from_script(error: ScriptError) -> InspectError {
        let kind = match error.kind {
            ScriptErrorKind::Terminated => InspectErrorKind::Terminated,
            _ => InspectErrorKind::Internal,
        };
        InspectError {
            kind,
            message: error.message,
        }
    }
}

impl std::fmt::Display for InspectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for InspectError {}

/// One `console.*` call.
#[derive(Clone, Debug, PartialEq)]
pub struct ConsoleCall {
    /// The CDP `consoleAPICalled` type: `log`, `warning`, `error`, `table`,
    /// `startGroup`, ...
    pub kind: String,
    pub args: Vec<RemoteValue>,
    pub stack: Vec<StackFrame>,
    /// Milliseconds since the epoch.
    pub timestamp_ms: f64,
    /// The realm the call ran in.
    pub context: u64,
}

/// An exception no script caught, or a promise rejection nobody handled.
#[derive(Clone, Debug, PartialEq)]
pub struct ExceptionEvent {
    pub details: ExceptionDetails,
    pub unhandled_rejection: bool,
    pub timestamp_ms: f64,
    pub context: u64,
}

/// A page script called a function installed by `add_binding`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingCall {
    pub name: String,
    pub payload: String,
    pub context: u64,
}

/// Where a realm reports what happens in it.
///
/// Called on the page thread, from inside engine entries, so an implementation
/// must not re-enter the engine.
pub trait InspectorSink {
    /// Whether anything consumes console and exception events. A realm skips
    /// the work of describing their values when nothing does.
    fn listening(&self) -> bool;
    fn console(&self, call: ConsoleCall);
    fn exception(&self, event: ExceptionEvent);
    fn binding_called(&self, call: BindingCall);
    /// A promise an evaluation awaited has settled.
    fn settled(&self, ticket: u64, result: Result<Completion, InspectError>);
}

/// Routes one message the realm script posted to `sink`. This is the body of the
/// native `post` function both backends install.
pub fn deliver(sink: &dyn InspectorSink, realm: u64, kind: &str, payload: &str) {
    let Ok(message) = serde_json::from_str::<Value>(payload) else {
        return;
    };
    match kind {
        "console" => {
            let Some(call) = console_call(&message, realm) else {
                return;
            };
            sink.console(call);
        }
        "binding" => {
            if let (Some(name), Some(payload)) = (
                message.get("n").and_then(Value::as_str),
                message.get("p").and_then(Value::as_str),
            ) {
                sink.binding_called(BindingCall {
                    name: name.to_owned(),
                    payload: payload.to_owned(),
                    context: realm,
                });
            }
        }
        "settled" => {
            if let (Some(ticket), Some(out)) = (
                message.get("ticket").and_then(Value::as_u64),
                message.get("out"),
            ) {
                sink.settled(ticket, completion_of(out));
            }
        }
        _ => {}
    }
}

fn console_call(message: &Value, realm: u64) -> Option<ConsoleCall> {
    Some(ConsoleCall {
        kind: message.get("k")?.as_str()?.to_owned(),
        args: message
            .get("a")?
            .as_array()?
            .iter()
            .map(RemoteValue::from_json)
            .collect::<Option<_>>()?,
        stack: message
            .get("s")?
            .as_array()?
            .iter()
            .map(StackFrame::from_json)
            .collect::<Option<_>>()?,
        timestamp_ms: message.get("ts")?.as_f64()?,
        context: realm,
    })
}

fn failure_of(out: &Value) -> Option<InspectError> {
    let fail = out.get("fail")?;
    let code = fail
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("internal");
    let message = fail
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(InspectError::from_code(code, message.to_owned()))
}

fn completion_of(out: &Value) -> Result<Completion, InspectError> {
    let malformed = || InspectError::new(InspectErrorKind::Internal, "malformed inspector reply");
    if let Some(error) = failure_of(out) {
        return Err(error);
    }
    if let Some(thrown) = out.get("x") {
        return ExceptionDetails::from_json(thrown)
            .map(Completion::Threw)
            .ok_or_else(malformed);
    }
    out.get("r")
        .and_then(RemoteValue::from_json)
        .map(Completion::Value)
        .ok_or_else(malformed)
}

/// Builds the unhandled-exception event the backends report once they have
/// described the thrown value through the realm script.
pub fn exception_event(
    reply: &str,
    realm: u64,
    unhandled_rejection: bool,
    timestamp_ms: f64,
) -> Option<ExceptionEvent> {
    let reply: Value = serde_json::from_str(reply).ok()?;
    Some(ExceptionEvent {
        details: ExceptionDetails::from_json(reply.get("r")?)?,
        unhandled_rejection,
        timestamp_ms,
        context: realm,
    })
}

/// The argument of the realm script's `exception` operation.
pub fn exception_argument(text: &str, line: Option<u32>, column: Option<u32>) -> String {
    json!({ "text": text, "line": line, "col": column }).to_string()
}

/// How an evaluation or function call is to be reported.
#[derive(Clone, Debug, Default)]
pub struct Settings {
    /// Copy the result by value instead of returning a handle.
    pub by_value: bool,
    /// Attach a preview to returned handles.
    pub preview: bool,
    /// The object group a returned handle joins.
    pub group: Option<String>,
    /// Await the result when it is a promise, reporting through
    /// [`InspectorSink::settled`] under this ticket.
    pub await_ticket: Option<u64>,
}

impl Settings {
    fn to_json(&self) -> Value {
        json!({
            "value": self.by_value,
            "preview": self.preview,
            "group": self.group,
            "wait": self.await_ticket.is_some(),
            "ticket": self.await_ticket,
        })
    }
}

/// An argument of [`call_function`]: a value copied in, or an object of the
/// realm named by handle.
#[derive(Clone, Debug, PartialEq)]
pub enum CallArgument {
    Value(RemoteValue),
    Handle(String),
}

/// What an entry produced right away.
#[derive(Clone, Debug, PartialEq)]
// A value returned once and consumed at once, not stored: boxing the common
// variant would cost an allocation to save a move.
#[allow(clippy::large_enum_variant)]
pub enum Entered {
    Done(Completion),
    /// The result is a promise; its settlement arrives through
    /// [`InspectorSink::settled`].
    Pending,
}

fn invoke(
    engine: &mut dyn PageEngine,
    op: &str,
    argument: &Value,
    timeout: Duration,
) -> Result<Value, InspectError> {
    let reply = engine
        .inspector_call(op, &argument.to_string(), timeout)
        .map_err(InspectError::from_script)?;
    serde_json::from_str(&reply)
        .map_err(|_| InspectError::new(InspectErrorKind::Internal, "malformed inspector reply"))
}

fn run(
    engine: &mut dyn PageEngine,
    kind: &str,
    source: &str,
    argument: &Value,
    timeout: Duration,
) -> Result<Value, InspectError> {
    let reply = engine
        .inspector_run(kind, source, &argument.to_string(), timeout)
        .map_err(InspectError::from_script)?;
    serde_json::from_str(&reply)
        .map_err(|_| InspectError::new(InspectErrorKind::Internal, "malformed inspector reply"))
}

fn entered(reply: Value) -> Result<Entered, InspectError> {
    if reply.get("pending").and_then(Value::as_bool) == Some(true) {
        return Ok(Entered::Pending);
    }
    completion_of(&reply).map(Entered::Done)
}

/// Evaluates `expression` as a script in the realm's global scope.
pub fn evaluate(
    engine: &mut dyn PageEngine,
    expression: &str,
    settings: &Settings,
    timeout: Duration,
) -> Result<Entered, InspectError> {
    let reply = run(
        engine,
        "eval",
        expression,
        &json!({ "o": settings.to_json() }),
        timeout,
    )?;
    Ok(finish_entry(engine, settings, entered(reply)?))
}

/// Calls the function `function` (source text of a function expression or
/// declaration) with `this` bound to the object `target`, or the global object.
pub fn call_function(
    engine: &mut dyn PageEngine,
    target: Option<&str>,
    function: &str,
    arguments: &[CallArgument],
    settings: &Settings,
    timeout: Duration,
) -> Result<Entered, InspectError> {
    let args: Vec<Value> = arguments
        .iter()
        .map(|argument| match argument {
            CallArgument::Value(value) => value.to_argument(),
            CallArgument::Handle(id) => json!({ "t": "handle", "id": id }),
        })
        .collect();
    let reply = run(
        engine,
        "call",
        function,
        &json!({ "self": target, "args": args, "o": settings.to_json() }),
        timeout,
    )?;
    Ok(finish_entry(engine, settings, entered(reply)?))
}

/// A promise that was already settled when the entry returned has a reaction
/// queued, and nothing else would run it before the caller starts waiting.
fn finish_entry(engine: &mut dyn PageEngine, settings: &Settings, entered: Entered) -> Entered {
    if settings.await_ticket.is_some() && entered == Entered::Pending {
        engine.run_microtasks();
    }
    entered
}

/// Releases one handle. Reports whether it existed.
pub fn release(
    engine: &mut dyn PageEngine,
    id: &str,
    timeout: Duration,
) -> Result<bool, InspectError> {
    let reply = invoke(engine, "release", &json!({ "id": id }), timeout)?;
    Ok(reply.get("r").and_then(Value::as_bool).unwrap_or(false))
}

/// Releases every handle of one object group.
pub fn release_group(
    engine: &mut dyn PageEngine,
    group: &str,
    timeout: Duration,
) -> Result<(), InspectError> {
    invoke(engine, "release_group", &json!({ "group": group }), timeout).map(|_| ())
}

/// How many handles the realm holds.
pub fn live_handles(engine: &mut dyn PageEngine, timeout: Duration) -> Result<usize, InspectError> {
    let reply = invoke(engine, "live", &Value::Null, timeout)?;
    Ok(reply.get("r").and_then(Value::as_u64).unwrap_or(0) as usize)
}

/// Installs a global function `name` that reports its one string argument as a
/// [`BindingCall`].
pub fn bind(
    engine: &mut dyn PageEngine,
    name: &str,
    timeout: Duration,
) -> Result<(), InspectError> {
    invoke(engine, "bind", &json!({ "name": name }), timeout).map(|_| ())
}

/// Removes a function installed by [`bind`].
pub fn unbind(
    engine: &mut dyn PageEngine,
    name: &str,
    timeout: Duration,
) -> Result<(), InspectError> {
    invoke(engine, "unbind", &json!({ "name": name }), timeout).map(|_| ())
}

/// Which members [`properties`] lists.
#[derive(Clone, Copy, Debug, Default)]
pub struct PropertyQuery {
    /// Stop at the object itself instead of walking its prototype chain.
    pub own_only: bool,
    /// List accessors only.
    pub accessors_only: bool,
    /// Attach a preview to the handles of object values.
    pub preview: bool,
}

/// One member of an object, as `Runtime.getProperties` describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyDescriptor {
    pub name: String,
    /// The value of a data property.
    pub value: Option<RemoteValue>,
    pub getter: Option<RemoteValue>,
    pub setter: Option<RemoteValue>,
    /// Set for a data property only.
    pub writable: Option<bool>,
    pub configurable: bool,
    pub enumerable: bool,
    /// Defined on the object itself rather than inherited.
    pub is_own: bool,
}

impl PropertyDescriptor {
    fn from_json(value: &Value) -> Option<PropertyDescriptor> {
        // A member that is present but unreadable makes the entry malformed.
        let remote = |key: &str| match value.get(key) {
            None => Some(None),
            Some(raw) => RemoteValue::from_json(raw).map(Some),
        };
        Some(PropertyDescriptor {
            name: value.get("n")?.as_str()?.to_owned(),
            value: remote("v")?,
            getter: remote("g")?,
            setter: remote("s")?,
            writable: value.get("w").and_then(Value::as_bool),
            configurable: value.get("c")?.as_bool()?,
            enumerable: value.get("e")?.as_bool()?,
            is_own: value.get("own")?.as_bool()?,
        })
    }
}

/// The handle `id` names has members; lists them without running any getter
/// of the page. A symbol has none.
pub fn properties(
    engine: &mut dyn PageEngine,
    id: &str,
    query: PropertyQuery,
    timeout: Duration,
) -> Result<Vec<PropertyDescriptor>, InspectError> {
    let reply = invoke(
        engine,
        "props",
        &json!({
            "id": id,
            "own": query.own_only,
            "accessors": query.accessors_only,
            "preview": query.preview,
        }),
        timeout,
    )?;
    if let Some(error) = failure_of(&reply) {
        return Err(error);
    }
    let malformed = || InspectError::new(InspectErrorKind::Internal, "malformed inspector reply");
    reply
        .get("r")
        .and_then(Value::as_array)
        .ok_or_else(malformed)?
        .iter()
        .map(|entry| PropertyDescriptor::from_json(entry).ok_or_else(malformed))
        .collect()
}

/// The number of the realm the engine runs, the one ids and events carry.
pub fn realm(engine: &mut dyn PageEngine, timeout: Duration) -> Result<u64, InspectError> {
    let reply = invoke(engine, "realm", &Value::Null, timeout)?;
    reply
        .get("r")
        .and_then(Value::as_u64)
        .ok_or_else(|| InspectError::new(InspectErrorKind::Internal, "malformed inspector reply"))
}
