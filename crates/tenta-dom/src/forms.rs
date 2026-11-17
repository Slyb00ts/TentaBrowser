//! The live state of form controls and form submission
//! (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html>).
//!
//! Everything here reads what a control holds now: the dirty value and dirty
//! checkedness script and input set, and the selectedness of options, with
//! the content attributes as the defaults they are. The script bindings, the
//! extractors and form submission all answer from these functions, so they
//! cannot disagree about what a form contains.

use tenta_core::NodeId;
use tenta_core::atom::atoms;

use crate::document::Document;
use crate::navigation::{NavigationRequest, PostData};

fn has(doc: &Document, node: NodeId, name: &str) -> bool {
    doc.attribute_str(node, name).is_some()
}

fn lower(doc: &Document, node: NodeId, name: &str) -> String {
    doc.attribute_str(node, name)
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

/// The state of an `<input>`'s `type` attribute: the keyword in lower case,
/// `text` for a missing or unknown one.
pub fn input_type(doc: &Document, input: NodeId) -> &'static str {
    const TYPES: [&str; 22] = [
        "hidden",
        "text",
        "search",
        "tel",
        "url",
        "email",
        "password",
        "date",
        "month",
        "week",
        "time",
        "datetime-local",
        "number",
        "range",
        "color",
        "checkbox",
        "radio",
        "file",
        "submit",
        "image",
        "reset",
        "button",
    ];
    let given = lower(doc, input, "type");
    TYPES
        .iter()
        .copied()
        .find(|known| *known == given)
        .unwrap_or("text")
}

/// The state of a `<button>`'s `type` attribute: `submit` unless it says
/// `reset` or `button`.
pub fn button_type(doc: &Document, button: NodeId) -> &'static str {
    match lower(doc, button, "type").as_str() {
        "reset" => "reset",
        "button" => "button",
        _ => "submit",
    }
}

/// Whether `control` is a submit button: a `<button>` in the submit state or
/// an `<input>` of type `submit` or `image`.
pub fn is_submit_button(doc: &Document, control: NodeId) -> bool {
    match doc.tag(control) {
        Some(tag) if tag == atoms::BUTTON => button_type(doc, control) == "submit",
        Some(tag) if tag == atoms::INPUT => {
            matches!(input_type(doc, control), "submit" | "image")
        }
        _ => false,
    }
}

/// Whether the control is disabled: by its own `disabled` attribute, or by a
/// `<fieldset disabled>` ancestor unless it sits in that fieldset's first
/// `<legend>`.
pub fn is_disabled(doc: &Document, control: NodeId) -> bool {
    if has(doc, control, "disabled") {
        return true;
    }
    let mut below = control;
    for ancestor in doc.ancestors(control) {
        if doc.tag(ancestor) == Some(atoms::FIELDSET) && has(doc, ancestor, "disabled") {
            let first_legend = doc
                .children(ancestor)
                .find(|child| doc.tag(*child) == Some(atoms::LEGEND));
            if first_legend != Some(below) {
                return true;
            }
        }
        below = ancestor;
    }
    false
}

/// The form a control belongs to: the form its `form` attribute names by ID,
/// or its nearest `<form>` ancestor without that attribute.
pub fn form_owner(doc: &Document, control: NodeId) -> Option<NodeId> {
    if let Some(id) = doc.attribute_str(control, "form") {
        return doc
            .get_element_by_id(id)
            .filter(|form| doc.tag(*form) == Some(atoms::FORM));
    }
    doc.ancestors(control)
        .find(|ancestor| doc.tag(*ancestor) == Some(atoms::FORM))
}

fn is_listed(doc: &Document, node: NodeId) -> bool {
    matches!(
        doc.tag(node),
        Some(tag)
            if tag == atoms::INPUT
                || tag == atoms::SELECT
                || tag == atoms::TEXTAREA
                || tag == atoms::BUTTON
    )
}

/// The submittable controls whose form owner is `form`, in tree order:
/// descendants, and controls elsewhere that name the form with `form=`.
pub fn form_controls(doc: &Document, form: NodeId) -> Vec<NodeId> {
    let root = doc.document_node();
    doc.descendants(root)
        .filter(|node| is_listed(doc, *node) && form_owner(doc, *node) == Some(form))
        .collect()
}

// ---- values ---------------------------------------------------------------

/// The current value of an `<input>` or a `<textarea>`: the dirty value once
/// one was set; until then the `value` attribute, or a textarea's text.
pub fn control_value(doc: &Document, control: NodeId) -> String {
    if let Some(value) = doc.dirty_value(control) {
        return value.to_owned();
    }
    if doc.tag(control) == Some(atoms::TEXTAREA) {
        return doc.text_content(control);
    }
    doc.attribute_str(control, "value").unwrap_or("").to_owned()
}

/// The checkedness of a checkbox or radio button.
pub fn is_checked(doc: &Document, control: NodeId) -> bool {
    doc.dirty_checkedness(control)
        .unwrap_or_else(|| has(doc, control, "checked"))
}

// ---- text selection -------------------------------------------------------

/// Which end of a selection moves when it is extended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionDirection {
    #[default]
    None,
    Forward,
    Backward,
}

impl SelectionDirection {
    /// The keyword `selectionDirection` reports.
    pub fn as_str(self) -> &'static str {
        match self {
            SelectionDirection::None => "none",
            SelectionDirection::Forward => "forward",
            SelectionDirection::Backward => "backward",
        }
    }

    /// The direction a `selectionDirection` keyword names; anything that is
    /// not `forward` or `backward` is `none`.
    pub fn parse(keyword: &str) -> Self {
        match keyword {
            "forward" => SelectionDirection::Forward,
            "backward" => SelectionDirection::Backward,
            _ => SelectionDirection::None,
        }
    }
}

/// The selection of a text control, in UTF-16 code units of its value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextSelection {
    pub start: u32,
    pub end: u32,
    pub direction: SelectionDirection,
}

/// Whether the text selection API applies to `control`: a `<textarea>`, or
/// an `<input>` of type text, search, url, tel or password.
pub fn selection_applies(doc: &Document, control: NodeId) -> bool {
    match doc.tag(control) {
        Some(tag) if tag == atoms::TEXTAREA => true,
        Some(tag) if tag == atoms::INPUT => matches!(
            input_type(doc, control),
            "text" | "search" | "url" | "tel" | "password"
        ),
        _ => false,
    }
}

fn utf16_length(value: &str) -> u32 {
    u32::try_from(value.encode_utf16().count()).unwrap_or(u32::MAX)
}

/// The control's selection: the stored one clamped to the value it has now,
/// or a caret at the end of the value.
pub fn text_selection(doc: &Document, control: NodeId) -> TextSelection {
    let length = utf16_length(&control_value(doc, control));
    match doc.stored_text_selection(control) {
        Some(selection) => TextSelection {
            start: selection.start.min(length),
            end: selection.end.min(length),
            direction: selection.direction,
        },
        None => TextSelection {
            start: length,
            end: length,
            direction: SelectionDirection::None,
        },
    }
}

/// Sets the selection range: both ends are clamped to the value's length and
/// a start past the end collapses to the end.
pub fn set_selection_range(
    doc: &mut Document,
    control: NodeId,
    start: u32,
    end: u32,
    direction: SelectionDirection,
) {
    let length = utf16_length(&control_value(doc, control));
    let end = end.min(length);
    let start = start.min(end);
    doc.store_text_selection(
        control,
        TextSelection {
            start,
            end,
            direction,
        },
    );
}

// ---- select and option ----------------------------------------------------

/// The `<select>` an `<option>` belongs to: its parent, or the parent of its
/// `<optgroup>` parent.
pub fn option_select(doc: &Document, option: NodeId) -> Option<NodeId> {
    let parent = doc.parent(option);
    if parent == NodeId::NONE {
        return None;
    }
    if doc.tag(parent) == Some(atoms::SELECT) {
        return Some(parent);
    }
    if doc.tag(parent) == Some(atoms::OPTGROUP) {
        let grandparent = doc.parent(parent);
        if grandparent != NodeId::NONE && doc.tag(grandparent) == Some(atoms::SELECT) {
            return Some(grandparent);
        }
    }
    None
}

/// The list of options of a `<select>`: its `<option>` children and those of
/// its `<optgroup>` children, in tree order.
pub fn select_options(doc: &Document, select: NodeId) -> Vec<NodeId> {
    let mut options = Vec::new();
    for child in doc.children(select) {
        match doc.tag(child) {
            Some(tag) if tag == atoms::OPTION => options.push(child),
            Some(tag) if tag == atoms::OPTGROUP => options.extend(
                doc.children(child)
                    .filter(|option| doc.tag(*option) == Some(atoms::OPTION)),
            ),
            _ => {}
        }
    }
    options
}

/// Whether an option is disabled: by its own attribute or its `<optgroup>`'s.
pub fn is_option_disabled(doc: &Document, option: NodeId) -> bool {
    if has(doc, option, "disabled") {
        return true;
    }
    let parent = doc.parent(option);
    parent != NodeId::NONE
        && doc.tag(parent) == Some(atoms::OPTGROUP)
        && has(doc, parent, "disabled")
}

/// An option's text: its text content with ASCII whitespace stripped and
/// collapsed.
pub fn option_text(doc: &Document, option: NodeId) -> String {
    doc.text_content(option)
        .split_ascii_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// An option's value: its `value` attribute, or its text without one.
pub fn option_value(doc: &Document, option: NodeId) -> String {
    match doc.attribute_str(option, "value") {
        Some(value) => value.to_owned(),
        None => option_text(doc, option),
    }
}

/// The selectedness an option carries by itself: what script set, or the
/// `selected` attribute until script touches it.
fn raw_selectedness(doc: &Document, option: NodeId) -> bool {
    doc.dirty_selectedness(option)
        .unwrap_or_else(|| has(doc, option, "selected"))
}

fn is_multiple(doc: &Document, select: NodeId) -> bool {
    has(doc, select, "multiple")
}

/// The number of rows a `<select>` shows: its `size` attribute, 4 for a
/// multiple select without one, 1 otherwise.
pub fn display_size(doc: &Document, select: NodeId) -> u32 {
    doc.attribute_str(select, "size")
        .and_then(|size| size.trim().parse::<u32>().ok())
        .filter(|size| *size > 0)
        .unwrap_or(if is_multiple(doc, select) { 4 } else { 1 })
}

/// The options of `select` that are selected, in tree order.
///
/// A multiple select has whatever its options carry. A single select has at
/// most one: the last option that carries selectedness (so a later `selected`
/// attribute wins, as the reset the standard runs on insertion has it), and,
/// when none does and the select shows one row, its first option that is not
/// disabled, unless script has deselected options, which is how
/// `select.value = "no such value"` leaves nothing selected.
pub fn selected_options(doc: &Document, select: NodeId) -> Vec<NodeId> {
    let options = select_options(doc, select);
    if is_multiple(doc, select) {
        return options
            .into_iter()
            .filter(|option| raw_selectedness(doc, *option))
            .collect();
    }
    if let Some(last) = options
        .iter()
        .rev()
        .copied()
        .find(|option| raw_selectedness(doc, *option))
    {
        return vec![last];
    }
    let touched = options
        .iter()
        .any(|option| doc.dirty_selectedness(*option).is_some());
    if touched || display_size(doc, select) != 1 {
        return Vec::new();
    }
    options
        .into_iter()
        .find(|option| !is_option_disabled(doc, *option))
        .into_iter()
        .collect()
}

/// Whether `option` is selected, by the rules of its `<select>` when it has
/// one.
pub fn is_selected(doc: &Document, option: NodeId) -> bool {
    match option_select(doc, option) {
        Some(select) => selected_options(doc, select).contains(&option),
        None => raw_selectedness(doc, option),
    }
}

/// Sets an option's selectedness, as `option.selected = value` does. In a
/// single select, selecting an option deselects the others.
pub fn set_selected(doc: &mut Document, option: NodeId, selected: bool) {
    if selected
        && let Some(select) = option_select(doc, option)
        && !is_multiple(doc, select)
    {
        for other in select_options(doc, select) {
            doc.set_dirty_selectedness(other, other == option);
        }
        return;
    }
    doc.set_dirty_selectedness(option, selected);
}

/// The value of a `<select>`: the value of its first selected option, or the
/// empty string.
pub fn select_value(doc: &Document, select: NodeId) -> String {
    selected_options(doc, select)
        .first()
        .map(|option| option_value(doc, *option))
        .unwrap_or_default()
}

/// Selects the first option whose value is `value` and deselects every other
/// option; with no such option nothing is selected.
pub fn set_select_value(doc: &mut Document, select: NodeId, value: &str) {
    let options = select_options(doc, select);
    let chosen = options
        .iter()
        .copied()
        .find(|option| option_value(doc, *option) == value);
    for option in options {
        doc.set_dirty_selectedness(option, Some(option) == chosen);
    }
}

/// The index of the first selected option in the list of options, or -1.
pub fn selected_index(doc: &Document, select: NodeId) -> i32 {
    let selected = selected_options(doc, select);
    let Some(first) = selected.first() else {
        return -1;
    };
    select_options(doc, select)
        .iter()
        .position(|option| option == first)
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(-1)
}

/// Selects the option at `index` and deselects every other option; an index
/// outside the list deselects all.
pub fn set_selected_index(doc: &mut Document, select: NodeId, index: i32) {
    let options = select_options(doc, select);
    let chosen = usize::try_from(index)
        .ok()
        .and_then(|index| options.get(index).copied());
    for option in options {
        doc.set_dirty_selectedness(option, Some(option) == chosen);
    }
}

// ---- submission -----------------------------------------------------------

/// One entry of a form's entry list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub value: String,
    /// The entry stands for a file control. No file can be chosen yet, so it
    /// is always the empty file: an empty name and an empty body.
    pub file: bool,
}

impl Entry {
    fn text(name: &str, value: &str) -> Self {
        Self {
            name: normalise_newlines(name),
            value: normalise_newlines(value),
            file: false,
        }
    }
}

/// Lone CR and lone LF become CRLF, as form submission encodes line breaks.
fn normalise_newlines(text: &str) -> String {
    if !text.contains(['\r', '\n']) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str("\r\n");
            }
            '\n' => out.push_str("\r\n"),
            other => out.push(other),
        }
    }
    out
}

/// Constructs the entry list of `form` for a submission by `submitter`
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constructing-the-form-data-set>).
pub fn entry_list(doc: &Document, form: NodeId, submitter: Option<NodeId>) -> Vec<Entry> {
    let mut entries = Vec::new();
    for control in form_controls(doc, form) {
        let tag = doc.tag(control);
        let is_input = tag == Some(atoms::INPUT);
        let kind = if is_input {
            input_type(doc, control)
        } else {
            ""
        };
        let is_button =
            tag == Some(atoms::BUTTON) || matches!(kind, "submit" | "image" | "reset" | "button");
        if is_disabled(doc, control)
            || (is_button && Some(control) != submitter)
            || (matches!(kind, "checkbox" | "radio") && !is_checked(doc, control))
            || doc
                .ancestors(control)
                .any(|ancestor| doc.tag(ancestor) == Some(atoms::DATALIST))
        {
            continue;
        }
        let name = doc.attribute_str(control, "name").unwrap_or("");
        if kind == "image" {
            // The coordinates of the click on the image; a synthetic
            // submission has none, which is the top left corner.
            let prefix = if name.is_empty() {
                String::new()
            } else {
                format!("{name}.")
            };
            entries.push(Entry::text(&format!("{prefix}x"), "0"));
            entries.push(Entry::text(&format!("{prefix}y"), "0"));
            continue;
        }
        if name.is_empty() {
            continue;
        }
        if tag == Some(atoms::SELECT) {
            for option in selected_options(doc, control) {
                if !is_option_disabled(doc, option) {
                    entries.push(Entry::text(name, &option_value(doc, option)));
                }
            }
        } else if matches!(kind, "checkbox" | "radio") {
            let value = doc.attribute_str(control, "value").unwrap_or("on");
            entries.push(Entry::text(name, value));
        } else if kind == "file" {
            entries.push(Entry {
                name: normalise_newlines(name),
                value: String::new(),
                file: true,
            });
        } else if kind == "hidden" && name.eq_ignore_ascii_case("_charset_") {
            entries.push(Entry::text(name, "UTF-8"));
        } else if tag == Some(atoms::BUTTON) {
            let value = doc.attribute_str(control, "value").unwrap_or("");
            entries.push(Entry::text(name, value));
        } else {
            entries.push(Entry::text(name, &control_value(doc, control)));
        }
    }
    entries
}

/// The `application/x-www-form-urlencoded` serialisation of an entry list.
pub fn urlencoded(entries: &[Entry]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for entry in entries {
        serializer.append_pair(&entry.name, &entry.value);
    }
    serializer.finish()
}

/// A name or filename inside a multipart header: line breaks and the quote
/// are percent-encoded, as the standard's multipart encoding has it.
pub fn multipart_name(text: &str) -> String {
    text.replace('\n', "%0A")
        .replace('\r', "%0D")
        .replace('"', "%22")
}

/// The `multipart/form-data` encoding of an entry list with `boundary`.
pub fn multipart(entries: &[Entry], boundary: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in entries {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        let name = multipart_name(&entry.name);
        if entry.file {
            out.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"\"\r\n\
                     Content-Type: application/octet-stream\r\n\r\n"
                )
                .as_bytes(),
            );
        } else {
            out.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
            );
            out.extend_from_slice(entry.value.as_bytes());
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

/// The `text/plain` encoding of an entry list.
pub fn text_plain(entries: &[Entry]) -> String {
    let mut out = String::new();
    for entry in entries {
        out.push_str(&entry.name);
        out.push('=');
        out.push_str(&entry.value);
        out.push_str("\r\n");
    }
    out
}

/// The attribute of a submission: the submitter's `form*` override when it
/// has one, the form's own attribute otherwise.
fn submission_attribute<'a>(
    doc: &'a Document,
    form: NodeId,
    submitter: Option<NodeId>,
    name: &str,
) -> Option<&'a str> {
    submitter
        .and_then(|submitter| doc.attribute_str(submitter, &format!("form{name}")))
        .or_else(|| doc.attribute_str(form, name))
}

/// The navigation a submission of `form` by `submitter` is: the action with
/// the entry list as its query for GET, or as its body for POST, encoded as
/// the form's `enctype` says. `boundary` separates the parts of a multipart
/// body and must not occur in the form's values.
///
/// `None` when nothing is to be loaded here: a `dialog` method, a `target`
/// that names another browsing context, or an action that is not an http(s)
/// URL.
pub fn submission(
    doc: &Document,
    form: NodeId,
    submitter: Option<NodeId>,
    boundary: &str,
) -> Option<NavigationRequest> {
    let attribute = |name: &str| submission_attribute(doc, form, submitter, name);
    let method = attribute("method")
        .map(|method| method.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if method == "dialog" {
        return None;
    }
    let target = attribute("target")
        .map(|target| target.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(target.as_str(), "" | "_self" | "_top" | "_parent") {
        return None;
    }
    let document_url = doc.url()?;
    let action = attribute("action").map(str::trim).unwrap_or("");
    let mut url = if action.is_empty() {
        document_url.clone()
    } else {
        doc.base_url().unwrap_or(document_url).join(action).ok()?
    };
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let entries = entry_list(doc, form, submitter);
    if method != "post" {
        // "Mutate action URL": the entry list replaces the query.
        url.set_query(Some(&urlencoded(&entries)));
        return Some(NavigationRequest::get(url));
    }
    let enctype = attribute("enctype")
        .map(|enctype| enctype.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let post = match enctype.as_str() {
        "multipart/form-data" => PostData {
            content_type: format!("multipart/form-data; boundary={boundary}"),
            body: multipart(&entries, boundary),
        },
        "text/plain" => PostData {
            content_type: "text/plain".to_owned(),
            body: text_plain(&entries).into_bytes(),
        },
        _ => PostData {
            content_type: "application/x-www-form-urlencoded".to_owned(),
            body: urlencoded(&entries).into_bytes(),
        },
    };
    Some(NavigationRequest {
        url,
        post: Some(post),
        replace: false,
    })
}

/// Resets a form: every control goes back to what its content attributes
/// say, by dropping the dirty value, the dirty checkedness and the option
/// selectedness script and input set.
pub fn reset(doc: &mut Document, form: NodeId) {
    for control in form_controls(doc, form) {
        doc.clear_dirty_value(control);
        doc.clear_dirty_checkedness(control);
        if doc.tag(control) == Some(atoms::SELECT) {
            for option in select_options(doc, control) {
                doc.clear_dirty_selectedness(option);
            }
        }
    }
}
