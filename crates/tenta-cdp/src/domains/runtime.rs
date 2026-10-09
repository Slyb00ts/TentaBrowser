//! The `Runtime` domain: evaluation, remote objects, execution contexts,
//! console and exception events, bindings, and the target handshake.
//!
//! The page's realm does the work (`Page::evaluate` and its relatives); this
//! module is the protocol's view of it. A script that throws is a successful
//! command with `exceptionDetails`, never a JSON-RPC error. A command that
//! names a context or an object the page no longer has is `-32000`, with the
//! messages Chromium uses, which is what the clients' retry logic keys on.

mod context;
mod remote;

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tenta_browser::{
    CallArgument, Completion, EvalOptions, Evaluation, Page, PropertyQuery, RemoteValue,
};
use tenta_core::Error;

use self::context::RuntimeState;
pub(super) use self::context::utility_context;
use super::target::resume;
use crate::backend::Target;
use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry, parse_params};
use crate::error::CdpError;
use crate::page_runtime::World;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Runtime);
}

struct Runtime;

const NO_CONTEXT: &str = "Cannot find context with specified id";
const NO_OBJECT: &str = "Could not find object with given id";
/// What Chromium answers an awaited evaluation a navigation outlived.
const NAVIGATED_WHILE_AWAITING: &str = "Inspected target navigated or closed";
const CONTEXT_DESTROYED: &str = "Execution context was destroyed.";
/// How the realm script words a lookup of an id it does not hold.
const UNKNOWN_OBJECT_REASON: &str = "could not find an object";

impl DomainHandler for Runtime {
    fn domain(&self) -> &'static str {
        "Runtime"
    }

    fn methods(&self) -> &'static [&'static str] {
        &[
            "enable",
            "disable",
            "evaluate",
            "callFunctionOn",
            "getProperties",
            "releaseObject",
            "releaseObjectGroup",
            "addBinding",
            "removeBinding",
            "discardConsoleEntries",
            "runIfWaitingForDebugger",
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
                "runIfWaitingForDebugger" => {
                    resume(cx.session());
                    Ok(json!({}))
                }
                "enable" => {
                    context::enable(cx.session(), cx.target()?).await;
                    Ok(json!({}))
                }
                "disable" => {
                    context::disable(cx.session()).await;
                    Ok(json!({}))
                }
                "evaluate" => evaluate(&cx, parse_params(params)?).await,
                "callFunctionOn" => call_function_on(&cx, parse_params(params)?).await,
                "getProperties" => get_properties(&cx, parse_params(params)?).await,
                "releaseObject" => release_object(&cx, parse_params(params)?).await,
                "releaseObjectGroup" => release_object_group(&cx, parse_params(params)?).await,
                "addBinding" => add_binding(&cx, parse_params(params)?).await,
                "removeBinding" => remove_binding(&cx, parse_params(params)?).await,
                // Console messages are events, not a stored list.
                "discardConsoleEntries" => Ok(json!({})),
                other => Err(CdpError::method_not_found(&format!("Runtime.{other}"))),
            }
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvaluateParams {
    expression: String,
    object_group: Option<String>,
    context_id: Option<u64>,
    unique_context_id: Option<String>,
    #[serde(default)]
    return_by_value: bool,
    #[serde(default)]
    generate_preview: bool,
    #[serde(default)]
    await_promise: bool,
    #[serde(default)]
    throw_on_side_effect: bool,
    timeout: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallFunctionParams {
    function_declaration: String,
    object_id: Option<String>,
    #[serde(default)]
    arguments: Vec<ArgumentParams>,
    object_group: Option<String>,
    execution_context_id: Option<u64>,
    unique_context_id: Option<String>,
    #[serde(default)]
    return_by_value: bool,
    #[serde(default)]
    generate_preview: bool,
    #[serde(default)]
    await_promise: bool,
    #[serde(default)]
    throw_on_side_effect: bool,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ArgumentParams {
    value: Option<Value>,
    unserializable_value: Option<String>,
    object_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PropertiesParams {
    object_id: String,
    #[serde(default)]
    own_properties: bool,
    #[serde(default)]
    accessor_properties_only: bool,
    #[serde(default)]
    generate_preview: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseParams {
    object_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseGroupParams {
    object_group: String,
}

#[derive(Deserialize)]
struct BindingParams {
    name: String,
}

fn no_context() -> CdpError {
    CdpError::server(NO_CONTEXT)
}

/// The realm number an object id starts with. Ids are `"<realm>.<n>"`.
fn realm_of(object_id: &str) -> Result<u64, CdpError> {
    object_id
        .split_once('.')
        .and_then(|(realm, _)| realm.parse().ok())
        .ok_or_else(|| CdpError::server("Invalid remote object id"))
}

/// A command that needs the page's script waits for a navigation in flight to
/// install its document. The wait is the command's own, so the session's next
/// command is released first and is not stuck behind a slow page; the
/// dispatcher's command timeout bounds it.
async fn after_navigation(cx: &CommandContext, page: &Page) {
    if page.navigation_in_flight() {
        cx.release_order();
        page.navigation_settled().await;
    }
}

/// Refuses a command aimed at a context, or at objects of a context, that is
/// no longer the page's. Only asks the page for its realm when the command
/// names something to compare.
async fn require_context(
    target: &Target,
    context_id: Option<u64>,
    unique_context_id: Option<&str>,
    object_ids: &[&str],
) -> Result<(), CdpError> {
    if context_id.is_some() && unique_context_id.is_some() {
        return Err(CdpError::invalid_params(
            "contextId and uniqueContextId are mutually exclusive",
        ));
    }
    let contexts = target.contexts();
    let wanted = match (context_id, unique_context_id) {
        (Some(id), _) => Some(contexts.resolve(id).ok_or_else(no_context)?),
        (None, Some(unique)) => Some(contexts.parse_unique_id(unique).ok_or_else(no_context)?),
        (None, None) => None,
    };
    let objects = object_ids
        .iter()
        .map(|id| realm_of(id))
        .collect::<Result<Vec<_>, _>>()?;
    if wanted.is_none() && objects.is_empty() {
        return Ok(());
    }
    let current = target
        .page()
        .execution_context()
        .await
        .map_err(|_| no_context())?;
    // A utility world runs in the main realm, so a context of either world is
    // stale exactly when its realm is.
    let stale = wanted.is_some_and(|(realm, _)| realm != current)
        || objects.iter().any(|realm| *realm != current);
    if stale { Err(no_context()) } else { Ok(()) }
}

/// The id the page's current context is known by, or 0 when it has none.
async fn current_context(target: &Target) -> u64 {
    match target.page().execution_context().await {
        Ok(realm) => target.contexts().id_for(realm, World::Main),
        Err(_) => 0,
    }
}

/// A failure of the page's realm operation, in Chromium's words.
fn map_error(error: Error) -> CdpError {
    match error {
        Error::Other { code, .. } if code == "navigated_while_awaiting" => {
            CdpError::server(NAVIGATED_WHILE_AWAITING)
        }
        Error::Other { code, .. } if code == "execution_context_destroyed" => {
            CdpError::server(CONTEXT_DESTROYED)
        }
        Error::Other { code, .. } if code == "handle_limit" => {
            CdpError::server("Too many remote objects are held")
        }
        Error::Other { message, .. } => CdpError::server(message),
        Error::InvalidArgument { reason } if reason.contains(UNKNOWN_OBJECT_REASON) => {
            CdpError::server(NO_OBJECT)
        }
        Error::Closed => CdpError::server("Target closed"),
        other => CdpError::server(other.to_string()),
    }
}

fn eval_options(
    object_group: Option<String>,
    return_by_value: bool,
    generate_preview: bool,
    await_promise: bool,
    timeout: Option<f64>,
) -> Result<EvalOptions, CdpError> {
    let timeout = match timeout {
        None => None,
        Some(ms) if ms.is_finite() && ms >= 0.0 => Some(Duration::from_secs_f64(ms / 1000.0)),
        Some(_) => {
            return Err(CdpError::invalid_params(
                "timeout must be a positive number",
            ));
        }
    };
    Ok(EvalOptions {
        await_promise,
        return_by_value,
        generate_preview,
        object_group,
        timeout,
        ..EvalOptions::default()
    })
}

/// What the page reports for a script a deadline stopped.
fn terminated(state: &RuntimeState, context: u64) -> Value {
    let exception = json!({
        "type": "object",
        "subtype": "error",
        "className": "Error",
        "description": "Error: Execution was terminated",
    });
    json!({
        "result": exception,
        "exceptionDetails": {
            "exceptionId": state.next_exception_id(),
            "text": "Uncaught",
            "lineNumber": 0,
            "columnNumber": 0,
            "exception": exception,
            "executionContextId": context,
        },
    })
}

/// The command result of an evaluation: the value, or the exception as
/// `exceptionDetails` next to the thrown value.
async fn finish(
    cx: &CommandContext,
    target: &Target,
    outcome: Result<Evaluation, Error>,
) -> Result<Value, CdpError> {
    let state = cx.session().state::<RuntimeState>();
    let evaluation = match outcome {
        Ok(evaluation) => evaluation,
        Err(Error::Timeout { .. }) => return Ok(terminated(&state, current_context(target).await)),
        Err(Error::Other { code, .. }) if code == "script_terminated" => {
            return Ok(terminated(&state, current_context(target).await));
        }
        Err(error) => return Err(map_error(error)),
    };
    match evaluation.into_completion() {
        Completion::Value(value) => Ok(json!({ "result": remote::remote_object(&value) })),
        Completion::Threw(details) => {
            let context = current_context(target).await;
            Ok(json!({
                "result": remote::remote_object(&details.exception),
                "exceptionDetails":
                    remote::exception_details(&details, state.next_exception_id(), context),
            }))
        }
    }
}

fn reject_side_effect_check(requested: bool) -> Result<(), CdpError> {
    if requested {
        return Err(CdpError::server("throwOnSideEffect is not supported"));
    }
    Ok(())
}

async fn evaluate(cx: &CommandContext, params: EvaluateParams) -> Result<Value, CdpError> {
    reject_side_effect_check(params.throw_on_side_effect)?;
    let target = cx.target()?;
    let page = target.page();
    after_navigation(cx, page).await;
    require_context(
        target,
        params.context_id,
        params.unique_context_id.as_deref(),
        &[],
    )
    .await?;
    let options = eval_options(
        params.object_group,
        params.return_by_value,
        params.generate_preview,
        params.await_promise,
        params.timeout,
    )?;
    // Past this point the script has run and only the settlement is awaited,
    // which the client may need other commands (a navigation) to end.
    let outcome = page
        .evaluate_notifying(&params.expression, options, || cx.release_order())
        .await;
    finish(cx, target, outcome).await
}

fn argument_of(argument: ArgumentParams) -> Result<CallArgument, CdpError> {
    if let Some(id) = argument.object_id {
        return Ok(CallArgument::Handle(id));
    }
    if let Some(spelling) = argument.unserializable_value {
        return Ok(CallArgument::Value(unserializable(&spelling)?));
    }
    Ok(CallArgument::Value(
        argument
            .value
            .as_ref()
            .map_or(RemoteValue::Undefined, from_json),
    ))
}

fn unserializable(spelling: &str) -> Result<RemoteValue, CdpError> {
    Ok(match spelling {
        "NaN" => RemoteValue::Number(f64::NAN),
        "Infinity" => RemoteValue::Number(f64::INFINITY),
        "-Infinity" => RemoteValue::Number(f64::NEG_INFINITY),
        "-0" => RemoteValue::Number(-0.0),
        big => match big.strip_suffix('n') {
            Some(digits) if !digits.is_empty() => RemoteValue::BigInt(digits.to_owned()),
            _ => {
                return Err(CdpError::invalid_params(format!(
                    "unserializableValue {big:?} is not recognised"
                )));
            }
        },
    })
}

fn from_json(value: &Value) -> RemoteValue {
    match value {
        Value::Null => RemoteValue::Null,
        Value::Bool(b) => RemoteValue::Bool(*b),
        Value::Number(n) => RemoteValue::Number(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => RemoteValue::String(s.clone()),
        Value::Array(items) => RemoteValue::Array(items.iter().map(from_json).collect()),
        Value::Object(members) => RemoteValue::Object(
            members
                .iter()
                .map(|(key, member)| (key.clone(), from_json(member)))
                .collect(),
        ),
    }
}

async fn call_function_on(
    cx: &CommandContext,
    params: CallFunctionParams,
) -> Result<Value, CdpError> {
    reject_side_effect_check(params.throw_on_side_effect)?;
    let has_context = params.execution_context_id.is_some() || params.unique_context_id.is_some();
    if params.object_id.is_some() && has_context {
        return Err(CdpError::server(
            "ObjectId must not be specified together with executionContextId",
        ));
    }
    if params.object_id.is_none() && !has_context {
        return Err(CdpError::server(
            "Either ObjectId or executionContextId must be specified",
        ));
    }
    let target = cx.target()?;
    let page = target.page();
    after_navigation(cx, page).await;
    let arguments = params
        .arguments
        .into_iter()
        .map(argument_of)
        .collect::<Result<Vec<_>, _>>()?;
    let named: Vec<&str> = params
        .object_id
        .as_deref()
        .into_iter()
        .chain(arguments.iter().filter_map(|argument| match argument {
            CallArgument::Handle(id) => Some(id.as_str()),
            CallArgument::Value(_) => None,
        }))
        .collect();
    if let Some(receiver) = params.object_id.as_deref() {
        let world = realm_of(receiver)?;
        if named
            .iter()
            .any(|id| realm_of(id).is_ok_and(|realm| realm != world))
        {
            return Err(CdpError::server(
                "Argument should belong to the same JavaScript world as target object",
            ));
        }
    }
    require_context(
        target,
        params.execution_context_id,
        params.unique_context_id.as_deref(),
        &named,
    )
    .await?;
    let options = eval_options(
        params.object_group,
        params.return_by_value,
        params.generate_preview,
        params.await_promise,
        None,
    )?;
    let outcome = page
        .call_function_on_id_notifying(
            params.object_id.as_deref(),
            &params.function_declaration,
            arguments,
            options,
            || cx.release_order(),
        )
        .await;
    finish(cx, target, outcome).await
}

async fn get_properties(cx: &CommandContext, params: PropertiesParams) -> Result<Value, CdpError> {
    let target = cx.target()?;
    let page = target.page();
    after_navigation(cx, page).await;
    require_context(target, None, None, &[params.object_id.as_str()]).await?;
    let query = PropertyQuery {
        own_only: params.own_properties,
        accessors_only: params.accessor_properties_only,
        preview: params.generate_preview,
    };
    let properties = page
        .get_properties(&params.object_id, query)
        .await
        .map_err(map_error)?;
    let result: Vec<Value> = properties.iter().map(remote::property_descriptor).collect();
    Ok(json!({ "result": result }))
}

/// Releasing an object of a context that is gone succeeds: the object is
/// gone with it, which is what the caller wanted. So does releasing while a
/// navigation is replacing the document, whose realm cannot be entered.
async fn release_object(cx: &CommandContext, params: ReleaseParams) -> Result<Value, CdpError> {
    let page = cx.target()?.page();
    if page.navigation_in_flight() {
        return Ok(json!({}));
    }
    page.release_object(&params.object_id)
        .await
        .map_err(map_error)?;
    Ok(json!({}))
}

async fn release_object_group(
    cx: &CommandContext,
    params: ReleaseGroupParams,
) -> Result<Value, CdpError> {
    let page = cx.target()?.page();
    if page.navigation_in_flight() {
        return Ok(json!({}));
    }
    page.release_object_group(&params.object_group)
        .await
        .map_err(map_error)?;
    Ok(json!({}))
}

async fn add_binding(cx: &CommandContext, params: BindingParams) -> Result<Value, CdpError> {
    cx.target()?
        .page()
        .add_binding(&params.name)
        .await
        .map_err(|error| CdpError::server(error.to_string()))?;
    cx.session()
        .state::<RuntimeState>()
        .add_binding(&params.name);
    Ok(json!({}))
}

async fn remove_binding(cx: &CommandContext, params: BindingParams) -> Result<Value, CdpError> {
    cx.target()?
        .page()
        .remove_binding(&params.name)
        .await
        .map_err(|error| CdpError::server(error.to_string()))?;
    cx.session()
        .state::<RuntimeState>()
        .remove_binding(&params.name);
    Ok(json!({}))
}
