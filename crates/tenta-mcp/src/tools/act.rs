//! The acting tools: `browser_click`, `browser_type`, `browser_fill` and
//! `browser_press_key`.
//!
//! Each resolves its ref to a node, runs the page-side interaction script
//! (focus, pointer and keyboard events with the activation behaviour browsers
//! give them), follows a navigation the action starts, and then settles the
//! page with the MCP tunables unless the caller opted out.

use serde::Deserialize;
use serde_json::{Value, json};
use tenta_browser::{Navigation, Page};

use super::dom::{
    self, Placement, Report, Resolved, check_supported, follow, is_text_entry, navigation_json,
    resolve_ref, run_script, script,
};
use super::{Tool, ToolOutput, handler};
use crate::error::{ErrorKind, ToolError};
use crate::keys;
use crate::session::{BrowserSession, Settled, read_doc, settle_after_action};

const MAX_TYPED_CHARS: usize = 20_000;

pub fn tools() -> Vec<Tool> {
    let settle = json!({
        "type": "boolean",
        "description": "Wait for the page to settle after the action (default true). Settling means load-driven work finished, not that content arriving later over sockets or workers has rendered; use browser_wait_for for that."
    });
    vec![
        Tool::new(
            "browser_click",
            "Click an element",
            "Clicks an element given by `ref` (from browser_snapshot) or a point given by `x` and `y` \
             (CSS pixels from the top-left of the page; needs the render tier). Dispatches the \
             pointer sequence a user causes (move, press, release, click) with focus changes and \
             default actions: following a link, toggling a checkbox or radio button, submitting a \
             form from a submit button, opening a <details>. With a ref the click lands on the \
             element's centre; if another element covers that point the call fails with `blocked` \
             and names the covering element. Fails with `element_not_found` for a stale ref or an \
             element with no layout box. Waits for the page to settle afterwards (set `settle` to \
             false to skip) and follows a navigation the click starts. Returns what was clicked, \
             any navigation and how the page settled; call browser_snapshot to see the result.",
            json!({
                "type": "object",
                "properties": {
                    "ref": {"type": "string", "description": "Element ref from browser_snapshot, such as `e12`."},
                    "x": {"type": "number", "description": "Horizontal position in CSS pixels. Use with `y` instead of `ref`."},
                    "y": {"type": "number", "description": "Vertical position in CSS pixels. Use with `x` instead of `ref`."},
                    "button": {"type": "string", "enum": ["left", "middle", "right"], "description": "Mouse button. Default left."},
                    "double": {"type": "boolean", "description": "Double-click."},
                    "modifiers": {
                        "type": "array",
                        "items": {"type": "string", "enum": ["Alt", "Control", "Meta", "Shift"]},
                        "maxItems": 4,
                        "description": "Modifier keys held during the click."
                    },
                    "settle": settle.clone()
                },
                "additionalProperties": false,
                "examples": [{"ref": "e12"}, {"x": 120, "y": 48, "button": "right"}]
            }),
            click_handler,
        ),
        Tool::new(
            "browser_type",
            "Type text into an element",
            "Focuses a text field (an <input> of a text-like type or a <textarea>) given by `ref` and \
             types `text` into it one character at a time at the caret, firing keydown, keypress, \
             input and keyup for each, as a user would; the field's existing text stays. Use it when \
             the page reacts to individual keystrokes (autocomplete, validation as you type); use \
             browser_fill to replace a field's content. With `submit` true, presses Enter afterwards \
             (submitting the form). Fails with `invalid_arguments` when the element does not take \
             text and with `element_not_found` for a stale ref. Waits for the page to settle \
             afterwards. Returns the field's value after typing, except for password fields.",
            json!({
                "type": "object",
                "properties": {
                    "ref": {"type": "string", "description": "Element ref of a text field."},
                    "text": {"type": "string", "maxLength": MAX_TYPED_CHARS, "description": "Text to type. A newline presses Enter."},
                    "submit": {"type": "boolean", "description": "Press Enter after typing."},
                    "settle": settle.clone()
                },
                "required": ["ref", "text"],
                "additionalProperties": false,
                "examples": [{"ref": "e3", "text": "tentabrowser"}, {"ref": "e3", "text": "query", "submit": true}]
            }),
            type_handler,
        ),
        Tool::new(
            "browser_fill",
            "Replace a field's content",
            "Focuses a text field (an <input> of a text-like type or a <textarea>) given by `ref`, \
             selects its content and replaces it with `value` (an empty string clears it), firing a \
             single input event. Prefer it to browser_type for forms: it is one step and does not \
             depend on the previous content. Fails with `invalid_arguments` when the element does \
             not take text and with `element_not_found` for a stale ref. Waits for the page to \
             settle afterwards. Returns the field's value after filling, except for password fields.",
            json!({
                "type": "object",
                "properties": {
                    "ref": {"type": "string", "description": "Element ref of a text field."},
                    "value": {"type": "string", "maxLength": 100000, "description": "The new content."},
                    "settle": settle.clone()
                },
                "required": ["ref", "value"],
                "additionalProperties": false,
                "examples": [{"ref": "e3", "value": "ada@example.com"}, {"ref": "e4", "value": ""}]
            }),
            fill_handler,
        ),
        Tool::new(
            "browser_press_key",
            "Press a key",
            "Presses and releases one key or chord on the focused element, or on the element given \
             by `ref` after focusing it. `key` is a character (`a`), a name (`Enter`, `Tab`, \
             `Escape`, `Backspace`, `Delete`, `ArrowUp`, `ArrowDown`, `ArrowLeft`, `ArrowRight`, \
             `Home`, `End`, `PageUp`, `PageDown`, `F1` to `F12`) or a chord with modifiers \
             (`Control+a`, `Shift+Tab`). Enter submits a form from a text field and activates a \
             focused button or link; Tab moves focus; editing keys edit the focused text field. \
             Waits for the page to settle afterwards and follows a navigation the key starts.",
            json!({
                "type": "object",
                "properties": {
                    "key": {"type": "string", "minLength": 1, "maxLength": 40, "description": "Key or chord, such as `Enter` or `Control+a`."},
                    "ref": {"type": "string", "description": "Element to focus first."},
                    "settle": settle.clone()
                },
                "required": ["key"],
                "additionalProperties": false,
                "examples": [{"key": "Enter"}, {"key": "Control+a", "ref": "e3"}]
            }),
            press_key_handler,
        ),
        Tool::new(
            "browser_select_option",
            "Choose an option of a select",
            "Selects options of a <select> element (a `combobox` or `listbox` in the snapshot, \
             whose line lists its `options`) given by `ref`. Each entry of `values` names one \
             option by its value or by its visible text; a single select takes one, a multiple \
             select takes any number and ends up with exactly those selected (an empty list \
             clears it). Fires `input` and `change` as a user's choice does, so pages that react \
             to the selection do. Fails with `invalid_arguments` when the element is not a \
             <select>, is disabled, or an entry names no option (the message lists the options), \
             and with `element_not_found` for a stale ref. Waits for the page to settle \
             afterwards. Returns the options now selected.",
            json!({
                "type": "object",
                "properties": {
                    "ref": {"type": "string", "description": "Element ref of a <select>."},
                    "values": {
                        "type": "array",
                        "items": {"type": "string", "maxLength": 1000},
                        "maxItems": 100,
                        "description": "The options to select, each by value or by visible text."
                    },
                    "settle": settle.clone()
                },
                "required": ["ref", "values"],
                "additionalProperties": false,
                "examples": [{"ref": "e7", "values": ["Poland"]}, {"ref": "e9", "values": ["a", "c"]}]
            }),
            select_option_handler,
        ),
        Tool::new(
            "browser_hover",
            "Move the pointer over an element",
            "Moves the pointer onto the element given by `ref`, firing `mouseover`, `mouseenter` \
             and `mousemove` (and `mouseout` on the element it leaves). Use it to open menus and \
             tooltips that appear on hover, then take a browser_snapshot. What a stylesheet's \
             `:hover` rule shows is not rendered: only script that listens for the pointer reacts. \
             Fails with `element_not_found` for a stale ref or an element with no layout box. \
             Waits for the page to settle afterwards.",
            json!({
                "type": "object",
                "properties": {
                    "ref": {"type": "string", "description": "Element ref from browser_snapshot."},
                    "settle": settle
                },
                "required": ["ref"],
                "additionalProperties": false,
                "examples": [{"ref": "e12"}]
            }),
            hover_handler,
        ),
    ]
}

// ---- shared tail --------------------------------------------------------

/// What every acting tool does after the script ran.
struct Outcome {
    navigation: Option<Navigation>,
    settled: Option<Settled>,
}

async fn finish(
    session: &BrowserSession,
    page: &Page,
    report: &Report,
    settle: bool,
) -> Result<Outcome, ToolError> {
    check_supported(report)?;
    let navigation = match follow(session, page, report).await? {
        Some(navigation) => Some(navigation),
        // Nothing for the host to follow, but the page may have started a
        // navigation itself: a form submitted by POST, a handler that
        // assigned `location`.
        None => dom::page_navigation(session, page, report.document).await,
    };
    let settled = if settle {
        Some(settle_after_action(page, session).await?)
    } else {
        None
    };
    Ok(Outcome {
        navigation,
        settled,
    })
}

/// Adds the navigation an action caused to the session history.
fn record(session: &mut BrowserSession, outcome: &Outcome) {
    if let Some(navigation) = &outcome.navigation {
        session.history.visit(navigation.final_url.as_str());
    }
}

fn tail(outcome: &Outcome) -> String {
    let mut out = String::new();
    if let Some(navigation) = &outcome.navigation {
        out.push_str(&format!(
            " Navigated to {} (HTTP {}).",
            navigation.final_url, navigation.status
        ));
    }
    match outcome.settled {
        Some(settled) if settled.timed_out => {
            out.push_str(" The page had not settled when the deadline passed.");
        }
        Some(settled) if settled.busy => {
            out.push_str(" The page was still busy when the busy cap passed.");
        }
        Some(_) => out.push_str(" The page settled."),
        None => {}
    }
    out
}

fn outcome_json(outcome: &Outcome) -> Value {
    json!({
        "navigation": outcome.navigation.as_ref().map(navigation_json),
        "settled": outcome.settled.map(Settled::to_json),
    })
}

fn merge(mut base: Value, outcome: &Outcome) -> Value {
    if let (Some(base), Value::Object(extra)) = (base.as_object_mut(), outcome_json(outcome)) {
        base.extend(extra);
    }
    base
}

/// The element's current value, for confirming typed text. `None` once the
/// page has navigated: the node belongs to the document the action ran in,
/// and its id means nothing in the one that replaced it.
async fn value_of(page: &Page, node: tenta_core::NodeId, outcome: &Outcome) -> Option<String> {
    use tenta_core::atom::atoms;
    if outcome.navigation.is_some() {
        return None;
    }
    read_doc(page, move |doc| {
        if !doc.is_live(node)
            || (doc.tag(node) != Some(atoms::INPUT) && doc.tag(node) != Some(atoms::TEXTAREA))
        {
            return None;
        }
        let password = doc
            .attribute_str(node, "type")
            .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("password"));
        if password {
            return None;
        }
        doc.dirty_value(node)
            .map(str::to_owned)
            .or_else(|| doc.attribute_str(node, "value").map(str::to_owned))
            .or_else(|| (doc.tag(node) == Some(atoms::TEXTAREA)).then(|| doc.text_content(node)))
    })
    .await
    .ok()
    .flatten()
}

async fn text_target(
    session: &BrowserSession,
    page: &Page,
    reference: &str,
) -> Result<Resolved, ToolError> {
    dom::require_javascript(session)?;
    let target = resolve_ref(session, page, reference).await?;
    let node = target.node;
    let accepts = read_doc(page, move |doc| is_text_entry(doc, node)).await?;
    if accepts {
        Ok(target)
    } else {
        Err(ToolError::invalid(format!(
            "`{reference}` is {}, which does not take typed text; use browser_click for buttons, \
             checkboxes and links",
            target.description
        )))
    }
}

// ---- click --------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClickArgs {
    #[serde(rename = "ref")]
    reference: Option<String>,
    x: Option<f64>,
    y: Option<f64>,
    button: Option<String>,
    #[serde(default)]
    double: bool,
    #[serde(default)]
    modifiers: Vec<String>,
    settle: Option<bool>,
}

handler!(click_handler, ClickArgs, click);

async fn click(session: &mut BrowserSession, args: ClickArgs) -> Result<ToolOutput, ToolError> {
    let point = match (args.x, args.y) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some((x, y)),
        (None, None) => None,
        _ => {
            return Err(ToolError::invalid(
                "`x` and `y` must be given together, as finite numbers",
            ));
        }
    };
    if args.reference.is_some() == point.is_some() {
        return Err(ToolError::invalid(
            "give either `ref` or both `x` and `y`, not both and not neither",
        ));
    }
    let page = session.page().await?;
    super::snapshot::after_navigation(session, &page).await;

    let (target, at) = match (&args.reference, point) {
        (Some(reference), _) => {
            let target = resolve_ref(session, &page, reference).await?;
            click_point(&page, reference, target).await?
        }
        (None, Some((x, y))) => {
            let node = dom::hit(&page, x, y).await?.ok_or_else(|| {
                ToolError::element_not_found(format!("no element is at ({x}, {y})"))
            })?;
            let resolved = read_doc(&page, move |doc| dom::resolve_node(doc, node))
                .await?
                .ok_or_else(|| {
                    ToolError::element_not_found("the element at that point left the page")
                })?;
            (resolved, Some((x, y)))
        }
        (None, None) => unreachable!("checked above"),
    };

    let (button, mask) = match args.button.as_deref() {
        Some("middle") => (1, 4),
        Some("right") => (2, 2),
        _ => (0, 1),
    };
    let clicks = if args.double { 2 } else { 1 };
    let (x, y) = at.unwrap_or((0.0, 0.0));
    let has = |name: &str| args.modifiers.iter().any(|modifier| modifier == name);
    let body = format!(
        "var mods={{ctrlKey:{},shiftKey:{},altKey:{},metaKey:{}}};\
         function init(d,b){{return Object.assign({{x:{x},y:{y},button:{button},buttons:b,detail:d}},mods);}}\
         m(A.mouse(t,'mouseMoved',init(0,0)));\
         for(var i=1;i<={clicks};i++){{m(A.mouse(t,'mousePressed',init(i,{mask})));m(A.mouse(t,'mouseReleased',init(i,0)));}}\
         if({clicks}>1)t.dispatchEvent(new MouseEvent('dblclick',{{bubbles:true,cancelable:true,composed:true,view:globalThis,detail:2,clientX:{x},clientY:{y},button:{button}}}));",
        has("Control"),
        has("Shift"),
        has("Alt"),
        has("Meta"),
    );
    let report = run_script(session, &page, &script(&target.path, &body)).await?;
    let outcome = finish(session, &page, &report, args.settle.unwrap_or(true)).await?;
    record(session, &outcome);

    let place = at.map_or_else(String::new, |(x, y)| format!(" at ({x:.0}, {y:.0})"));
    let name = args.reference.as_deref().unwrap_or("the point");
    let summary = format!(
        "{} {name} {}{place}.{}",
        if args.double {
            "Double-clicked"
        } else {
            "Clicked"
        },
        target.description,
        tail(&outcome)
    );
    Ok(ToolOutput::text(
        summary,
        merge(
            json!({
                "action": "click",
                "target": {"ref": args.reference, "element": target.description},
                "point": at.map(|(x, y)| json!({"x": x, "y": y})),
            }),
            &outcome,
        ),
    ))
}

/// Where to click an element, and what actually sits there. Without a layout
/// the click goes to the element itself.
async fn click_point(
    page: &Page,
    reference: &str,
    target: Resolved,
) -> Result<(Resolved, Option<(f64, f64)>), ToolError> {
    let (x, y) = match dom::place(page, target.node).await? {
        Placement::Unavailable => return Ok((target, None)),
        Placement::Hidden => {
            return Err(ToolError::element_not_found(format!(
                "`{reference}` ({}) has no layout box: it is hidden or not rendered; take a new browser_snapshot",
                target.description
            )));
        }
        Placement::Box { x, y } => (x, y),
    };
    let Some(hit) = dom::hit(page, x, y).await? else {
        return Ok((target, Some((x, y))));
    };
    let node = target.node;
    let covered = read_doc(page, move |doc| {
        if doc.is_flat_tree_inclusive_ancestor(node, hit) {
            Ok(dom::resolve_node(doc, hit))
        } else {
            Err(dom::describe(doc, hit))
        }
    })
    .await?;
    match covered {
        Ok(Some(inside)) => Ok((inside, Some((x, y)))),
        Ok(None) => Ok((target, Some((x, y)))),
        Err(other) => Err(ToolError::new(
            ErrorKind::Blocked,
            format!(
                "{other} would receive the click at ({x:.0}, {y:.0}) instead of `{reference}` ({}): \
                 it covers the element; dismiss or scroll past it first",
                target.description
            ),
        )),
    }
}

// ---- type and fill ------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeArgs {
    #[serde(rename = "ref")]
    reference: String,
    text: String,
    #[serde(default)]
    submit: bool,
    settle: Option<bool>,
}

handler!(type_handler, TypeArgs, type_text);

async fn type_text(session: &mut BrowserSession, args: TypeArgs) -> Result<ToolOutput, ToolError> {
    let page = session.page().await?;
    super::snapshot::after_navigation(session, &page).await;
    let target = text_target(session, &page, &args.reference).await?;
    let inits: Vec<Value> = args
        .text
        .chars()
        .map(|character| keys::character_chord(character).init())
        .collect();
    let enter = keys::single("Enter").expect("Enter is a named key").init();
    let body = format!(
        "A.focus(t);var inits={};\
         for(var i=0;i<inits.length;i++){{m(A.key('keyDown',inits[i]));m(A.key('keyUp',inits[i]));}}\
         if({}){{var e={enter};m(A.key('keyDown',e));m(A.key('keyUp',e));}}",
        Value::Array(inits),
        args.submit,
    );
    let report = run_script(session, &page, &script(&target.path, &body)).await?;
    let outcome = finish(session, &page, &report, args.settle.unwrap_or(true)).await?;
    record(session, &outcome);
    let value = value_of(&page, target.node, &outcome).await;
    let summary = format!(
        "Typed {} characters into {} {}.{}",
        args.text.chars().count(),
        args.reference,
        target.description,
        tail(&outcome)
    );
    Ok(ToolOutput::text(
        summary,
        merge(
            json!({
                "action": "type",
                "target": {"ref": args.reference, "element": target.description},
                "characters": args.text.chars().count(),
                "value": value,
            }),
            &outcome,
        ),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FillArgs {
    #[serde(rename = "ref")]
    reference: String,
    value: String,
    settle: Option<bool>,
}

handler!(fill_handler, FillArgs, fill);

async fn fill(session: &mut BrowserSession, args: FillArgs) -> Result<ToolOutput, ToolError> {
    let page = session.page().await?;
    super::snapshot::after_navigation(session, &page).await;
    let target = text_target(session, &page, &args.reference).await?;
    let select_all = keys::parse("Control+a").expect("a valid chord").init();
    let backspace = keys::single("Backspace").expect("a named key").init();
    let replace = if args.value.is_empty() {
        format!("m(A.key('keyDown',{backspace}));")
    } else {
        format!("m(A.insertText({}));", Value::String(args.value.clone()))
    };
    let body = format!("A.focus(t);m(A.key('keyDown',{select_all}));{replace}");
    let report = run_script(session, &page, &script(&target.path, &body)).await?;
    let outcome = finish(session, &page, &report, args.settle.unwrap_or(true)).await?;
    record(session, &outcome);
    let value = value_of(&page, target.node, &outcome).await;
    let summary = format!(
        "Filled {} {} with {} characters.{}",
        args.reference,
        target.description,
        args.value.chars().count(),
        tail(&outcome)
    );
    Ok(ToolOutput::text(
        summary,
        merge(
            json!({
                "action": "fill",
                "target": {"ref": args.reference, "element": target.description},
                "characters": args.value.chars().count(),
                "value": value,
            }),
            &outcome,
        ),
    ))
}

// ---- press_key ----------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PressKeyArgs {
    key: String,
    #[serde(rename = "ref")]
    reference: Option<String>,
    settle: Option<bool>,
}

handler!(press_key_handler, PressKeyArgs, press_key);

async fn press_key(
    session: &mut BrowserSession,
    args: PressKeyArgs,
) -> Result<ToolOutput, ToolError> {
    let chord = keys::parse(&args.key).map_err(ToolError::invalid)?;
    dom::require_javascript(session)?;
    let page = session.page().await?;
    super::snapshot::after_navigation(session, &page).await;
    let (path, focus) = match &args.reference {
        Some(reference) => {
            let target = resolve_ref(session, &page, reference).await?;
            (target.path, "A.focus(t);")
        }
        None => (Vec::new(), ""),
    };
    let init = chord.init();
    let body = format!("{focus}m(A.key('keyDown',{init}));m(A.key('keyUp',{init}));");
    let report = run_script(session, &page, &script(&path, &body)).await?;
    let outcome = finish(session, &page, &report, args.settle.unwrap_or(true)).await?;
    record(session, &outcome);
    Ok(ToolOutput::text(
        format!("Pressed {}.{}", args.key, tail(&outcome)),
        merge(
            json!({"action": "press_key", "key": args.key, "ref": args.reference}),
            &outcome,
        ),
    ))
}

// ---- select and hover ---------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectOptionArgs {
    #[serde(rename = "ref")]
    reference: String,
    values: Vec<String>,
    settle: Option<bool>,
}

handler!(select_option_handler, SelectOptionArgs, select_option);

/// What the page-side selection reports: the options it could not find, or
/// the ones now selected.
#[derive(Default)]
struct Selection {
    missing: Vec<String>,
    available: Vec<String>,
    selected: Vec<(String, String)>,
    changed: bool,
}

fn strings(value: &tenta_browser::RemoteValue) -> Vec<String> {
    match value {
        tenta_browser::RemoteValue::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_selection(value: tenta_browser::RemoteValue) -> Selection {
    use tenta_browser::RemoteValue;
    let mut selection = Selection::default();
    let RemoteValue::Object(members) = value else {
        return selection;
    };
    for (name, value) in members {
        match name.as_str() {
            "missing" => selection.missing = strings(&value),
            "available" => selection.available = strings(&value),
            "changed" => selection.changed = value.as_bool().unwrap_or(false),
            "selected" => {
                if let RemoteValue::Array(items) = value {
                    selection.selected = items
                        .iter()
                        .filter_map(|item| match strings(item).as_slice() {
                            [value, text] => Some((value.clone(), text.clone())),
                            _ => None,
                        })
                        .collect();
                }
            }
            _ => {}
        }
    }
    selection
}

async fn select_option(
    session: &mut BrowserSession,
    args: SelectOptionArgs,
) -> Result<ToolOutput, ToolError> {
    dom::require_javascript(session)?;
    let page = session.page().await?;
    super::snapshot::after_navigation(session, &page).await;
    let target = resolve_ref(session, &page, &args.reference).await?;
    let node = target.node;
    let (is_select, multiple, disabled) = read_doc(&page, move |doc| {
        (
            doc.tag(node) == Some(tenta_core::atom::atoms::SELECT),
            doc.attribute_str(node, "multiple").is_some(),
            tenta_dom::forms::is_disabled(doc, node),
        )
    })
    .await?;
    if !is_select {
        return Err(ToolError::invalid(format!(
            "`{}` is {}, not a <select>; use browser_click for other controls",
            args.reference, target.description
        )));
    }
    if disabled {
        return Err(ToolError::invalid(format!(
            "`{}` ({}) is disabled",
            args.reference, target.description
        )));
    }
    if !multiple && args.values.len() != 1 {
        return Err(ToolError::invalid(format!(
            "`{}` is a single select: give exactly one value, not {}",
            args.reference,
            args.values.len()
        )));
    }

    // An option is named by its value, then by its text, then by its text
    // ignoring case and surrounding space. `input` and `change` fire only when
    // the selection changed, as they do for a person.
    let wanted = serde_json::to_string(&args.values).expect("a list of strings");
    let path = serde_json::to_string(&target.path).expect("a list of numbers");
    let expression = format!(
        "(function(){{var A=globalThis.__tentaCdp;\
         if(!A)throw new Error('the interaction script is not installed in this page');\
         var t=A.nodeAt({path});if(!t)throw new Error('the element left the page');\
         var W={wanted},O=[],i;for(i=0;i<t.options.length;i++)O.push(t.options[i]);\
         function norm(s){{return String(s).trim().toLowerCase();}}\
         function find(w){{return O.filter(function(o){{return o.value===w;}})[0]\
           ||O.filter(function(o){{return o.text===w;}})[0]\
           ||O.filter(function(o){{return norm(o.text)===norm(w);}})[0];}}\
         var chosen=[],missing=[];\
         W.forEach(function(w){{var o=find(w);if(o)chosen.push(o);else missing.push(w);}});\
         if(missing.length)return {{missing:missing,available:O.map(function(o){{return o.text;}})}};\
         var before=O.map(function(o){{return o.selected;}});\
         if(t.multiple)O.forEach(function(o){{o.selected=chosen.indexOf(o)!==-1;}});\
         else chosen[0].selected=true;\
         var changed=O.some(function(o,n){{return o.selected!==before[n];}});\
         if(changed){{t.dispatchEvent(new Event('input',{{bubbles:true,composed:true}}));\
           t.dispatchEvent(new Event('change',{{bubbles:true}}));}}\
         return {{changed:changed,selected:O.filter(function(o){{return o.selected;}})\
           .map(function(o){{return [o.value,o.text];}})}};}})()"
    );
    let document = dom::shown_document(&page);
    let options = tenta_browser::EvalOptions {
        return_by_value: true,
        ..tenta_browser::EvalOptions::default()
    };
    let selection = match page
        .evaluate_with(&expression, options)
        .await?
        .into_completion()
    {
        tenta_browser::Completion::Value(value) => parse_selection(value),
        tenta_browser::Completion::Threw(details) => {
            return Err(ToolError::new(
                ErrorKind::ScriptError,
                format!(
                    "the page could not carry out the action: {}",
                    details.message
                ),
            ));
        }
    };
    if !selection.missing.is_empty() {
        return Err(ToolError::invalid(format!(
            "`{}` has no option {}; its options are: {}",
            args.reference,
            selection
                .missing
                .iter()
                .map(|value| format!("{value:?}"))
                .collect::<Vec<_>>()
                .join(", "),
            selection
                .available
                .iter()
                .map(|text| format!("{text:?}"))
                .collect::<Vec<_>>()
                .join(", "),
        )));
    }
    let report = Report {
        document,
        ..Report::default()
    };
    let outcome = finish(session, &page, &report, args.settle.unwrap_or(true)).await?;
    record(session, &outcome);
    let labels: Vec<String> = selection
        .selected
        .iter()
        .map(|(_, text)| format!("{text:?}"))
        .collect();
    let summary = format!(
        "Selected {} in {} {}.{}{}",
        if labels.is_empty() {
            "nothing".to_owned()
        } else {
            labels.join(", ")
        },
        args.reference,
        target.description,
        if selection.changed {
            ""
        } else {
            " The selection was already that."
        },
        tail(&outcome)
    );
    Ok(ToolOutput::text(
        summary,
        merge(
            json!({
                "action": "select_option",
                "target": {"ref": args.reference, "element": target.description},
                "selected": selection.selected.iter().map(|(value, text)| json!({
                    "value": value,
                    "text": text,
                })).collect::<Vec<_>>(),
                "changed": selection.changed,
            }),
            &outcome,
        ),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HoverArgs {
    #[serde(rename = "ref")]
    reference: String,
    settle: Option<bool>,
}

handler!(hover_handler, HoverArgs, hover);

async fn hover(session: &mut BrowserSession, args: HoverArgs) -> Result<ToolOutput, ToolError> {
    let page = session.page().await?;
    super::snapshot::after_navigation(session, &page).await;
    let target = resolve_ref(session, &page, &args.reference).await?;
    let (target, at) = click_point(&page, &args.reference, target).await?;
    let (x, y) = at.unwrap_or((0.0, 0.0));
    let body = format!(
        "m(A.mouse(t,'mouseMoved',{{x:{x},y:{y},button:0,buttons:0,detail:0,\
         ctrlKey:false,shiftKey:false,altKey:false,metaKey:false}}));"
    );
    let report = run_script(session, &page, &script(&target.path, &body)).await?;
    let outcome = finish(session, &page, &report, args.settle.unwrap_or(true)).await?;
    record(session, &outcome);
    let place = at.map_or_else(String::new, |(x, y)| format!(" at ({x:.0}, {y:.0})"));
    Ok(ToolOutput::text(
        format!(
            "Moved the pointer over {} {}{place}.{}",
            args.reference,
            target.description,
            tail(&outcome)
        ),
        merge(
            json!({
                "action": "hover",
                "target": {"ref": args.reference, "element": target.description},
                "point": at.map(|(x, y)| json!({"x": x, "y": y})),
            }),
            &outcome,
        ),
    ))
}
