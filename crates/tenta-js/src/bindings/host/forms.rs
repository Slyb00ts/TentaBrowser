//! The script interface of forms and their controls: `HTMLFormElement`,
//! `HTMLInputElement`, `HTMLButtonElement`, `HTMLSelectElement`,
//! `HTMLTextAreaElement` and `HTMLOptionElement`.
//!
//! The state itself, and the submission algorithm, live in
//! `tenta_dom::forms`, where the extractors read the same answers. What is
//! here turns them into IDL members and adds what needs the script world:
//! the `submit` and `reset` events and the navigation request a submission
//! leaves for the page.
//!
//! Constraint validation (`tenta_dom::validation`) is here as the
//! `ValidityState` view and the `invalid` events of `checkValidity()`,
//! `reportValidity()` and an interactive submission.
//!
//! Not built: the `:invalid`/`:valid` selectors, `SubmitEvent` from the
//! engine (the event is a plain `Event` without `submitter`), the `select`
//! event, `labels`, and live `elements`/`options` collections (they are
//! snapshots).

use tenta_core::NodeId;
use tenta_dom::forms::{self, SelectionDirection};
use tenta_dom::validation;

use tenta_core::atom::atoms;

use super::{
    EventKind, EventRecord, HostObject, NodeOps, TAG_STATIC_HTML_COLLECTION, TAG_STATIC_NODE_LIST,
    dispatch, encode_node, encode_snapshot, encode_validity, node_argument,
};
use crate::bindings::{
    HTMLButtonElementOps, HTMLCollection, HTMLDetailsElementOps, HTMLElement, HTMLElementOrLong,
    HTMLFormElement, HTMLFormElementOps, HTMLInputElementOps, HTMLLabelElementOps,
    HTMLMeterElementOps, HTMLOptionElement, HTMLOptionElementOps, HTMLOutputElementOps,
    HTMLProgressElementOps, HTMLSelectElementOps, HTMLTextAreaElementOps, Handle, JsError,
    NodeList, ValidityState, ValidityStateOps,
};

/// A boundary for a `multipart/form-data` body, in the shape Chrome writes.
pub(super) fn multipart_boundary() -> String {
    const ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut random = [0u8; 16];
    // Without entropy the boundary is fixed; it only has to be absent from
    // the values, and a form whose value contains it is not a thing to defend.
    let _ = getrandom::fill(&mut random);
    let tail: String = random
        .iter()
        .map(|byte| ALPHABET[usize::from(*byte) % ALPHABET.len()] as char)
        .collect();
    format!("----WebKitFormBoundary{tail}")
}

impl HostObject {
    /// A snapshot `HTMLCollection` of `nodes`.
    pub(super) fn collection_of(
        &self,
        nodes: Vec<NodeId>,
    ) -> Result<Handle<HTMLCollection>, JsError> {
        Ok(Handle::from_bits(
            self.snapshot_bits(nodes, TAG_STATIC_HTML_COLLECTION)?,
        ))
    }

    /// A snapshot `NodeList` of `nodes`.
    fn node_list_of(&self, nodes: Vec<NodeId>) -> Result<Handle<NodeList>, JsError> {
        Ok(Handle::from_bits(
            self.snapshot_bits(nodes, TAG_STATIC_NODE_LIST)?,
        ))
    }

    fn snapshot_bits(&self, nodes: Vec<NodeId>, tag: u8) -> Result<u64, JsError> {
        let (doc, _) = self.anchor();
        let refs = {
            let doc = doc.borrow();
            nodes.into_iter().map(|node| doc.node_ref(node)).collect()
        };
        let key = self.context().snapshots.borrow_mut().insert(refs)?;
        Ok(encode_snapshot(self.slot(), tag, key))
    }

    /// The `<label>` elements whose labeled control is this element, in
    /// tree order.
    fn labels_of_control(&self) -> Result<Handle<NodeList>, JsError> {
        let labels = {
            let (doc, id) = self.anchor();
            let doc = doc.borrow();
            let root = doc.root_of(id);
            doc.descendants(root)
                .filter(|node| {
                    doc.tag(*node) == Some(atoms::LABEL)
                        && forms::label_control(&doc, *node) == Some(id)
                })
                .collect()
        };
        self.node_list_of(labels)
    }

    /// A reflected `double` attribute, `None` when it is missing or not a
    /// valid floating-point number.
    fn reflect_double(&self, name: &str) -> Option<f64> {
        let (doc, id) = self.anchor();
        doc.borrow()
            .attribute_str(id, name)
            .and_then(|value| validation::parse_float(value.trim()))
    }

    fn set_reflect_double(&self, name: &str, value: f64) {
        self.reflect_attribute(name, &format!("{value}"));
    }

    /// The control's form owner as a handle.
    fn form_handle(&self) -> Option<Handle<HTMLFormElement>> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let form = forms::form_owner(&doc, id)?;
        Some(Handle::from_bits(encode_node(
            self.slot(),
            doc.node_ref(form),
        )))
    }

    /// A reflected `long` that is limited to non-negative numbers: -1 when
    /// the attribute is missing or is not one.
    fn reflect_length(&self, name: &str) -> i32 {
        let (doc, id) = self.anchor();
        doc.borrow()
            .attribute_str(id, name)
            .and_then(|value| value.trim().parse::<i32>().ok())
            .filter(|value| *value >= 0)
            .unwrap_or(-1)
    }

    /// A reflected `unsigned long` with a default for a missing, unparsable
    /// or zero value.
    fn reflect_count(&self, name: &str, default: u32) -> u32 {
        let (doc, id) = self.anchor();
        doc.borrow()
            .attribute_str(id, name)
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(default)
    }

    /// A reflected enumerated attribute limited to `known` keywords, read in
    /// lower case; `missing` when it is absent or names none of them.
    pub(super) fn reflect_keyword(
        &self,
        name: &str,
        known: &[&'static str],
        missing: &'static str,
    ) -> String {
        let (doc, id) = self.anchor();
        let given = doc
            .borrow()
            .attribute_str(id, name)
            .map(|value| value.trim().to_ascii_lowercase());
        given
            .and_then(|given| known.iter().copied().find(|keyword| *keyword == given))
            .unwrap_or(missing)
            .to_owned()
    }

    /// Fires a plain, bubbling, cancelable event at this element and reports
    /// whether it was left uncancelled.
    fn fire_plain_event(&self, kind: &str) -> Result<bool, JsError> {
        self.fire_simple_event(kind, true)
    }

    /// Fires a plain cancelable event at this element and reports whether it
    /// was left uncancelled.
    fn fire_simple_event(&self, kind: &str, bubbles: bool) -> Result<bool, JsError> {
        let ctx = self.context();
        let (doc, id) = self.anchor();
        let target = {
            let doc = doc.borrow();
            if !doc.is_live(id) {
                return Ok(true);
            }
            doc.node_ref(id)
        };
        let mut record = EventRecord::new(kind.to_owned(), EventKind::Plain, ctx.time_stamp());
        record.bubbles = bubbles;
        record.cancelable = true;
        record.flags.initialized = true;
        let key = ctx.events.borrow_mut().insert(record);
        let outcome = dispatch(ctx, target, key, false);
        let orphaned = ctx
            .events
            .borrow()
            .get(key)
            .is_some_and(|record| !record.has_wrapper);
        if orphaned {
            ctx.events.borrow_mut().remove(key);
        }
        outcome
    }

    /// Submits this form: the navigation its submission is becomes a request
    /// the page takes when the running script returns.
    fn submit_form(&self, submitter: Option<NodeId>) {
        let request = {
            let (doc, id) = self.anchor();
            forms::submission(&doc.borrow(), id, submitter, &multipart_boundary())
        };
        if let Some(request) = request {
            self.request_navigation(request);
        }
    }

    /// `requestSubmit()` for an already checked submitter: unless the form
    /// or the submitter turns validation off, the controls are validated
    /// interactively and an invalid one stops the submission; then `submit`
    /// fires at the form, and the form is submitted unless a listener
    /// cancelled it.
    pub(super) fn request_form_submission(&self, submitter: Option<NodeId>) -> Result<(), JsError> {
        let skips = {
            let (doc, id) = self.anchor();
            validation::skips_validation(&doc.borrow(), id, submitter)
        };
        if !skips && !self.validate_form(true)? {
            return Ok(());
        }
        if self.fire_plain_event("submit")? {
            self.submit_form(submitter);
        }
        Ok(())
    }

    /// `reset()`: `reset` fires at the form, and the controls go back to
    /// their defaults unless a listener cancelled it.
    pub(super) fn reset_form(&self) -> Result<(), JsError> {
        if self.fire_plain_event("reset")? {
            let (doc, id) = self.anchor();
            forms::reset(&mut doc.borrow_mut(), id);
        }
        Ok(())
    }

    // ---- constraint validation --------------------------------------------

    /// The host of another node of this host's document.
    pub(super) fn node_host(&self, node: NodeId) -> HostObject {
        let (doc, _) = self.anchor();
        let node = doc.borrow().node_ref(node);
        HostObject::Node {
            ctx: self.context().clone(),
            node,
        }
    }

    fn control_validity(&self) -> validation::Validity {
        let (doc, id) = self.anchor();
        validation::validity(&doc.borrow(), id)
    }

    /// A control's `checkValidity()`, and with `report` its
    /// `reportValidity()`: an invalid candidate gets an `invalid` event, and
    /// a report focuses it unless a listener cancelled the event.
    fn check_control(&self, report: bool) -> Result<bool, JsError> {
        let invalid = {
            let (doc, id) = self.anchor();
            let doc = doc.borrow();
            validation::will_validate(&doc, id) && !validation::validity(&doc, id).valid()
        };
        if !invalid {
            return Ok(true);
        }
        if self.fire_simple_event("invalid", false)? && report {
            self.focus_steps(None)?;
        }
        Ok(false)
    }

    /// Validates this form's controls: `invalid` fires at each that fails, in
    /// tree order. With `report`, the first whose event was not cancelled is
    /// focused, which is all of "report the problems" a headless page has.
    fn validate_form(&self, report: bool) -> Result<bool, JsError> {
        let invalid = {
            let (doc, id) = self.anchor();
            validation::invalid_controls(&doc.borrow(), id)
        };
        let mut unhandled = None;
        for control in &invalid {
            let host = self.node_host(*control);
            if host.fire_simple_event("invalid", false)? && unhandled.is_none() {
                unhandled = Some(host);
            }
        }
        if report && let Some(first) = unhandled {
            // A control that a listener removed or disabled is not focused.
            let focusable = {
                let (doc, id) = first.anchor();
                let doc = doc.borrow();
                doc.is_live(id) && validation::will_validate(&doc, id)
            };
            if focusable {
                first.focus_steps(None)?;
            }
        }
        Ok(invalid.is_empty())
    }

    // ---- the text selection of an input or a textarea ---------------------

    fn selection(&self) -> Option<forms::TextSelection> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        forms::selection_applies(&doc, id).then(|| forms::text_selection(&doc, id))
    }

    /// The selection to change, or the `InvalidStateError` a control the
    /// selection does not apply to answers with.
    fn selection_for_write(&self) -> Result<forms::TextSelection, JsError> {
        self.selection().ok_or_else(|| {
            JsError::dom_exception(
                "InvalidStateError",
                "The input element's type does not support selection.",
            )
        })
    }

    fn write_selection(&self, start: u32, end: u32, direction: SelectionDirection) {
        let (doc, id) = self.anchor();
        forms::set_selection_range(&mut doc.borrow_mut(), id, start, end, direction);
    }

    fn selection_start_value(&self) -> Option<u32> {
        self.selection().map(|selection| selection.start)
    }

    fn selection_end_value(&self) -> Option<u32> {
        self.selection().map(|selection| selection.end)
    }

    fn selection_direction_value(&self) -> Option<String> {
        self.selection()
            .map(|selection| selection.direction.as_str().to_owned())
    }

    fn assign_selection_start(&self, value: Option<u32>) -> Result<(), JsError> {
        let current = self.selection_for_write()?;
        let start = value.unwrap_or(0);
        self.write_selection(start, current.end.max(start), current.direction);
        Ok(())
    }

    fn assign_selection_end(&self, value: Option<u32>) -> Result<(), JsError> {
        let current = self.selection_for_write()?;
        self.write_selection(current.start, value.unwrap_or(0), current.direction);
        Ok(())
    }

    fn assign_selection_direction(&self, value: Option<&str>) -> Result<(), JsError> {
        let current = self.selection_for_write()?;
        self.write_selection(
            current.start,
            current.end,
            SelectionDirection::parse(value.unwrap_or("none")),
        );
        Ok(())
    }

    fn assign_selection_range(
        &self,
        start: u32,
        end: u32,
        direction: Option<&str>,
    ) -> Result<(), JsError> {
        self.selection_for_write()?;
        self.write_selection(
            start,
            end,
            SelectionDirection::parse(direction.unwrap_or("none")),
        );
        Ok(())
    }

    /// `select()`: the whole value is selected. A control without a text
    /// selection ignores it.
    fn select_all(&self) {
        if self.selection().is_some() {
            self.write_selection(0, u32::MAX, SelectionDirection::None);
        }
    }

    /// The activation behaviour `element.click()` owes after an uncancelled
    /// click: an enabled submit button submits its form and a reset button
    /// resets it; a link navigates. Checkboxes and radio buttons are toggled
    /// by the page-side interaction script, not here.
    pub(super) fn activate_after_click(&self) -> Result<(), JsError> {
        use tenta_core::atom::atoms;
        let (doc, id) = self.anchor();
        enum Activation {
            Submit(NodeId),
            Reset(NodeId),
            Follow(url::Url),
            Nothing,
        }
        let activation = {
            let doc = doc.borrow();
            let tag = doc.tag(id);
            let is_button = tag == Some(atoms::BUTTON) || tag == Some(atoms::INPUT);
            if is_button && !forms::is_disabled(&doc, id) {
                let kind = if tag == Some(atoms::BUTTON) {
                    forms::button_type(&doc, id)
                } else {
                    forms::input_type(&doc, id)
                };
                match (kind, forms::form_owner(&doc, id)) {
                    ("submit" | "image", Some(form)) => Activation::Submit(form),
                    ("reset", Some(form)) => Activation::Reset(form),
                    _ => Activation::Nothing,
                }
            } else {
                // The link the click landed in: the element or an ancestor.
                let link = std::iter::once(id).chain(doc.ancestors(id)).find(|node| {
                    matches!(doc.tag(*node), Some(tag) if tag == atoms::A || tag == atoms::AREA)
                        && doc.attribute_str(*node, "href").is_some()
                });
                let target = link.and_then(|link| {
                    let own = doc
                        .attribute_str(link, "target")
                        .map(|target| target.trim().to_ascii_lowercase())
                        .unwrap_or_default();
                    matches!(own.as_str(), "" | "_self" | "_top" | "_parent")
                        .then(|| doc.attribute_str(link, "href"))
                        .flatten()
                        .and_then(|href| doc.resolve_url(href.trim()))
                });
                target.map_or(Activation::Nothing, Activation::Follow)
            }
        };
        let form_host = |form: NodeId| {
            let node = doc.borrow().node_ref(form);
            HostObject::Node {
                ctx: self.context().clone(),
                node,
            }
        };
        match activation {
            Activation::Submit(form) => form_host(form).request_form_submission(Some(id)),
            Activation::Reset(form) => form_host(form).reset_form(),
            Activation::Follow(url) => {
                self.navigate_to(url, false);
                Ok(())
            }
            Activation::Nothing => Ok(()),
        }
    }
}

/// The members `HTMLInputElement` and `HTMLTextAreaElement` share: the text
/// selection.
macro_rules! text_selection_members {
    () => {
        fn selection_start(&self) -> Result<Option<u32>, JsError> {
            Ok(self.selection_start_value())
        }
        fn set_selection_start(&self, value: Option<u32>) -> Result<(), JsError> {
            self.assign_selection_start(value)
        }
        fn selection_end(&self) -> Result<Option<u32>, JsError> {
            Ok(self.selection_end_value())
        }
        fn set_selection_end(&self, value: Option<u32>) -> Result<(), JsError> {
            self.assign_selection_end(value)
        }
        fn selection_direction(&self) -> Result<Option<String>, JsError> {
            Ok(self.selection_direction_value())
        }
        fn set_selection_direction(&self, value: Option<&str>) -> Result<(), JsError> {
            self.assign_selection_direction(value)
        }
        fn set_selection_range(
            &self,
            start: u32,
            end: u32,
            direction: Option<&str>,
        ) -> Result<(), JsError> {
            self.assign_selection_range(start, end, direction)
        }
        fn select(&self) {
            self.select_all();
        }
    };
}

/// The constraint validation members every submittable element has.
macro_rules! constraint_validation_members {
    () => {
        fn will_validate(&self) -> bool {
            let (doc, id) = self.anchor();
            validation::will_validate(&doc.borrow(), id)
        }
        fn validity(&self) -> Result<Handle<ValidityState>, JsError> {
            let (doc, id) = self.anchor();
            let owner = doc.borrow().node_ref(id);
            Ok(Handle::from_bits(encode_validity(self.slot(), owner)))
        }
        fn validation_message(&self) -> Result<String, JsError> {
            let (doc, id) = self.anchor();
            Ok(validation::validation_message(&doc.borrow(), id))
        }
        fn check_validity(&self) -> Result<bool, JsError> {
            self.check_control(false)
        }
        fn report_validity(&self) -> Result<bool, JsError> {
            self.check_control(true)
        }
        fn set_custom_validity(&self, error: &str) {
            let (doc, id) = self.anchor();
            doc.borrow_mut().set_custom_validity(id, error.to_owned());
        }
    };
}

/// A reflected DOMString attribute: getter and setter.
macro_rules! reflected_strings {
    ($($getter:ident, $setter:ident => $attribute:literal;)*) => {
        $(
            fn $getter(&self) -> Result<String, JsError> {
                Ok(self.reflect_string($attribute))
            }
            fn $setter(&self, value: &str) {
                self.reflect_attribute($attribute, value);
            }
        )*
    };
}

/// A reflected boolean attribute: getter and setter.
macro_rules! reflected_bools {
    ($($getter:ident, $setter:ident => $attribute:literal;)*) => {
        $(
            fn $getter(&self) -> bool {
                self.reflect_bool($attribute)
            }
            fn $setter(&self, value: bool) {
                self.set_reflect_bool($attribute, value);
            }
        )*
    };
}

const ENCTYPES: [&str; 3] = [
    "application/x-www-form-urlencoded",
    "multipart/form-data",
    "text/plain",
];

impl HTMLFormElementOps for HostObject {
    reflected_strings! {
        name, set_name => "name";
        target, set_target => "target";
    }
    reflected_bools! {
        no_validate, set_no_validate => "novalidate";
    }

    fn action(&self) -> Result<String, JsError> {
        Ok(self.reflect_url("action"))
    }
    fn set_action(&self, value: &str) {
        self.reflect_attribute("action", value);
    }
    fn method(&self) -> Result<String, JsError> {
        Ok(self.reflect_keyword("method", &["get", "post", "dialog"], "get"))
    }
    fn set_method(&self, value: &str) {
        self.reflect_attribute("method", value);
    }
    fn enctype(&self) -> Result<String, JsError> {
        Ok(self.reflect_keyword("enctype", &ENCTYPES, ENCTYPES[0]))
    }
    fn set_enctype(&self, value: &str) {
        self.reflect_attribute("enctype", value);
    }
    fn encoding(&self) -> Result<String, JsError> {
        <Self as HTMLFormElementOps>::enctype(self)
    }
    fn set_encoding(&self, value: &str) {
        self.reflect_attribute("enctype", value);
    }

    fn elements(&self) -> Result<Handle<HTMLCollection>, JsError> {
        let controls = {
            let (doc, id) = self.anchor();
            let doc = doc.borrow();
            // Image buttons are excluded "for historical reasons".
            forms::form_controls(&doc, id)
                .into_iter()
                .filter(|control| {
                    doc.tag(*control) != Some(tenta_core::atom::atoms::INPUT)
                        || forms::input_type(&doc, *control) != "image"
                })
                .collect()
        };
        self.collection_of(controls)
    }

    fn length(&self) -> u32 {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let count = forms::form_controls(&doc, id)
            .into_iter()
            .filter(|control| {
                doc.tag(*control) != Some(tenta_core::atom::atoms::INPUT)
                    || forms::input_type(&doc, *control) != "image"
            })
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }

    fn submit(&self) -> Result<(), JsError> {
        self.submit_form(None);
        Ok(())
    }

    fn request_submit(
        &self,
        submitter: Option<Option<Handle<HTMLElement>>>,
    ) -> Result<(), JsError> {
        let submitter = match submitter.flatten() {
            Some(handle) => {
                let submitter = node_argument(Handle::from_bits(handle.bits()))?;
                let (doc, id) = self.anchor();
                let doc = doc.borrow();
                if !forms::is_submit_button(&doc, submitter) {
                    return Err(JsError::type_error(
                        "Failed to execute 'requestSubmit' on 'HTMLFormElement': The specified element is not a submit button.",
                    ));
                }
                if forms::form_owner(&doc, submitter) != Some(id) {
                    return Err(JsError::dom_exception(
                        "NotFoundError",
                        "The specified element is not owned by this form element.",
                    ));
                }
                Some(submitter)
            }
            None => None,
        };
        self.request_form_submission(submitter)
    }

    fn reset(&self) {
        // The standard's `reset()` has nowhere to report a listener's
        // exception either; the dispatch already reported it.
        let _ = self.reset_form();
    }

    fn check_validity(&self) -> Result<bool, JsError> {
        self.validate_form(false)
    }

    fn report_validity(&self) -> Result<bool, JsError> {
        self.validate_form(true)
    }
}

impl ValidityStateOps for HostObject {
    fn value_missing(&self) -> bool {
        self.control_validity().value_missing
    }
    fn type_mismatch(&self) -> bool {
        self.control_validity().type_mismatch
    }
    fn pattern_mismatch(&self) -> bool {
        self.control_validity().pattern_mismatch
    }
    fn too_long(&self) -> bool {
        self.control_validity().too_long
    }
    fn too_short(&self) -> bool {
        self.control_validity().too_short
    }
    fn range_underflow(&self) -> bool {
        self.control_validity().range_underflow
    }
    fn range_overflow(&self) -> bool {
        self.control_validity().range_overflow
    }
    fn step_mismatch(&self) -> bool {
        self.control_validity().step_mismatch
    }
    fn bad_input(&self) -> bool {
        self.control_validity().bad_input
    }
    fn custom_error(&self) -> bool {
        self.control_validity().custom_error
    }
    fn valid(&self) -> bool {
        self.control_validity().valid()
    }
}

impl HTMLInputElementOps for HostObject {
    reflected_strings! {
        name, set_name => "name";
        placeholder, set_placeholder => "placeholder";
        default_value, set_default_value => "value";
        min, set_min => "min";
        max, set_max => "max";
        step, set_step => "step";
        pattern, set_pattern => "pattern";
        autocomplete, set_autocomplete => "autocomplete";
        accept, set_accept => "accept";
        alt, set_alt => "alt";
    }
    reflected_bools! {
        disabled, set_disabled => "disabled";
        default_checked, set_default_checked => "checked";
        required, set_required => "required";
        read_only, set_read_only => "readonly";
        multiple, set_multiple => "multiple";
        form_no_validate, set_form_no_validate => "formnovalidate";
    }
    text_selection_members!();
    constraint_validation_members!();

    fn value(&self) -> Result<String, JsError> {
        Ok(self.control_value("value"))
    }
    fn set_value(&self, value: &str) {
        self.set_control_value(value);
    }
    fn checked(&self) -> bool {
        let (doc, id) = self.anchor();
        forms::is_checked(&doc.borrow(), id)
    }
    fn set_checked(&self, value: bool) {
        let (doc, id) = self.anchor();
        doc.borrow_mut().set_dirty_checkedness(id, value);
    }
    fn r#type(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(forms::input_type(&doc.borrow(), id).to_owned())
    }
    fn set_type(&self, value: &str) {
        self.reflect_attribute("type", value);
    }
    fn max_length(&self) -> i32 {
        self.reflect_length("maxlength")
    }
    fn set_max_length(&self, value: i32) {
        if value >= 0 {
            self.reflect_attribute("maxlength", &value.to_string());
        }
    }
    fn min_length(&self) -> i32 {
        self.reflect_length("minlength")
    }
    fn set_min_length(&self, value: i32) {
        if value >= 0 {
            self.reflect_attribute("minlength", &value.to_string());
        }
    }
    fn form(&self) -> Result<Option<Handle<HTMLFormElement>>, JsError> {
        Ok(self.form_handle())
    }
    fn value_as_number(&self) -> Result<f64, JsError> {
        let (doc, id) = self.anchor();
        Ok(validation::value_as_number(&doc.borrow(), id))
    }
    fn set_value_as_number(&self, value: f64) -> Result<(), JsError> {
        if value.is_infinite() {
            return Err(JsError::type_error(
                "Failed to set the 'valueAsNumber' property on 'HTMLInputElement': The value provided is infinite.",
            ));
        }
        let written = {
            let (doc, id) = self.anchor();
            validation::number_to_value(&doc.borrow(), id, value)
        };
        match written {
            Some(written) => {
                self.set_control_value(&written);
                Ok(())
            }
            None => Err(JsError::dom_exception(
                "InvalidStateError",
                "This input element does not support Number values.",
            )),
        }
    }
    fn step_up(&self, n: Option<i32>) -> Result<(), JsError> {
        self.step_by(n.unwrap_or(1))
    }
    fn step_down(&self, n: Option<i32>) -> Result<(), JsError> {
        self.step_by(-n.unwrap_or(1))
    }
    fn indeterminate(&self) -> bool {
        let (doc, id) = self.anchor();
        doc.borrow().is_indeterminate(id)
    }
    fn set_indeterminate(&self, value: bool) {
        let (doc, id) = self.anchor();
        doc.borrow_mut().set_indeterminate(id, value);
    }
    fn labels(&self) -> Result<Handle<NodeList>, JsError> {
        self.labels_of_control()
    }
}

impl HostObject {
    fn step_by(&self, n: i32) -> Result<(), JsError> {
        let stepped = {
            let (doc, id) = self.anchor();
            validation::stepped_value(&doc.borrow(), id, n)
        };
        match stepped {
            Ok(Some(value)) => {
                self.set_control_value(&value);
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(validation::StepError::NotApplicable) => Err(JsError::dom_exception(
                "InvalidStateError",
                "This form element does not have an allowed value step.",
            )),
        }
    }
}

impl HTMLButtonElementOps for HostObject {
    reflected_strings! {
        name, set_name => "name";
        value, set_value => "value";
    }
    reflected_bools! {
        disabled, set_disabled => "disabled";
        form_no_validate, set_form_no_validate => "formnovalidate";
    }
    constraint_validation_members!();

    fn r#type(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(forms::button_type(&doc.borrow(), id).to_owned())
    }
    fn set_type(&self, value: &str) {
        self.reflect_attribute("type", value);
    }
    fn form(&self) -> Result<Option<Handle<HTMLFormElement>>, JsError> {
        Ok(self.form_handle())
    }
}

impl HTMLSelectElementOps for HostObject {
    reflected_strings! {
        name, set_name => "name";
    }
    reflected_bools! {
        disabled, set_disabled => "disabled";
        multiple, set_multiple => "multiple";
        required, set_required => "required";
    }
    constraint_validation_members!();

    fn r#type(&self) -> Result<String, JsError> {
        Ok(if self.reflect_bool("multiple") {
            "select-multiple"
        } else {
            "select-one"
        }
        .to_owned())
    }
    fn size(&self) -> u32 {
        self.reflect_count("size", 0)
    }
    fn set_size(&self, value: u32) {
        self.reflect_attribute("size", &value.to_string());
    }
    fn form(&self) -> Result<Option<Handle<HTMLFormElement>>, JsError> {
        Ok(self.form_handle())
    }
    fn options(&self) -> Result<Handle<HTMLCollection>, JsError> {
        let options = {
            let (doc, id) = self.anchor();
            forms::select_options(&doc.borrow(), id)
        };
        self.collection_of(options)
    }
    fn selected_options(&self) -> Result<Handle<HTMLCollection>, JsError> {
        let options = {
            let (doc, id) = self.anchor();
            forms::selected_options(&doc.borrow(), id)
        };
        self.collection_of(options)
    }
    fn length(&self) -> u32 {
        let (doc, id) = self.anchor();
        u32::try_from(forms::select_options(&doc.borrow(), id).len()).unwrap_or(u32::MAX)
    }
    fn selected_index(&self) -> i32 {
        let (doc, id) = self.anchor();
        forms::selected_index(&doc.borrow(), id)
    }
    fn set_selected_index(&self, value: i32) {
        let (doc, id) = self.anchor();
        forms::set_selected_index(&mut doc.borrow_mut(), id, value);
    }
    fn value(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(forms::select_value(&doc.borrow(), id))
    }
    fn set_value(&self, value: &str) {
        let (doc, id) = self.anchor();
        forms::set_select_value(&mut doc.borrow_mut(), id, value);
    }
    fn add(
        &self,
        element: Handle<HTMLElement>,
        before: Option<Option<HTMLElementOrLong>>,
    ) -> Result<(), JsError> {
        let element = node_argument(Handle::from_bits(element.bits()))?;
        let (doc, select) = self.anchor();
        {
            let doc = doc.borrow();
            if !matches!(doc.tag(element), Some(tag) if tag == atoms::OPTION || tag == atoms::OPTGROUP)
            {
                return Err(JsError::type_error(
                    "Failed to execute 'add' on 'HTMLSelectElement': The provided value is not of type '(HTMLOptGroupElement or HTMLOptionElement)'.",
                ));
            }
            if doc.ancestors(select).any(|ancestor| ancestor == element) || element == select {
                return Err(JsError::dom_exception(
                    "HierarchyRequestError",
                    "The new child element contains the parent.",
                ));
            }
        }
        let reference = match before.flatten() {
            Some(HTMLElementOrLong::HTMLElement(handle)) => {
                let reference = node_argument(Handle::from_bits(handle.bits()))?;
                if !doc
                    .borrow()
                    .ancestors(reference)
                    .any(|ancestor| ancestor == select)
                {
                    return Err(JsError::dom_exception(
                        "NotFoundError",
                        "The node before which the new node is to be inserted is not a child of this node.",
                    ));
                }
                Some(reference)
            }
            Some(HTMLElementOrLong::Long(index)) => usize::try_from(index).ok().and_then(|index| {
                forms::select_options(&doc.borrow(), select)
                    .get(index)
                    .copied()
            }),
            None => None,
        };
        let (parent, before) = match reference {
            Some(reference) => (doc.borrow().parent(reference), reference),
            None => (select, NodeId::NONE),
        };
        let mutation = if before == NodeId::NONE {
            tenta_dom::Mutation::Append {
                parent,
                child: element,
            }
        } else {
            tenta_dom::Mutation::InsertBefore {
                parent,
                child: element,
                before,
            }
        };
        doc.borrow_mut()
            .mutate(mutation)
            .map_err(|error| super::mutation_error(&error))
    }
    fn item(&self, index: u32) -> Result<Option<Handle<HTMLOptionElement>>, JsError> {
        let (doc, select) = self.anchor();
        let doc = doc.borrow();
        Ok(forms::select_options(&doc, select)
            .get(index as usize)
            .map(|option| Handle::from_bits(encode_node(self.slot(), doc.node_ref(*option)))))
    }
    fn named_item(&self, name: &str) -> Result<Option<Handle<HTMLOptionElement>>, JsError> {
        let (doc, select) = self.anchor();
        let doc = doc.borrow();
        Ok(forms::select_options(&doc, select)
            .into_iter()
            .find(|option| {
                doc.attribute_str(*option, "id") == Some(name)
                    || doc.attribute_str(*option, "name") == Some(name)
            })
            .map(|option| Handle::from_bits(encode_node(self.slot(), doc.node_ref(option)))))
    }
    fn labels(&self) -> Result<Handle<NodeList>, JsError> {
        self.labels_of_control()
    }
}

impl HTMLTextAreaElementOps for HostObject {
    reflected_strings! {
        name, set_name => "name";
        placeholder, set_placeholder => "placeholder";
    }
    reflected_bools! {
        disabled, set_disabled => "disabled";
        required, set_required => "required";
        read_only, set_read_only => "readonly";
    }
    text_selection_members!();
    constraint_validation_members!();

    fn value(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(forms::control_value(&doc.borrow(), id))
    }
    fn set_value(&self, value: &str) {
        self.set_control_value(value);
    }
    fn r#type(&self) -> Result<String, JsError> {
        Ok("textarea".to_owned())
    }
    fn default_value(&self) -> Result<String, JsError> {
        Ok(<Self as NodeOps>::text_content(self)?.unwrap_or_default())
    }
    fn set_default_value(&self, value: &str) {
        let _ = <Self as NodeOps>::set_text_content(self, Some(value));
    }
    fn rows(&self) -> u32 {
        self.reflect_count("rows", 2)
    }
    fn set_rows(&self, value: u32) {
        self.reflect_attribute("rows", &value.to_string());
    }
    fn cols(&self) -> u32 {
        self.reflect_count("cols", 20)
    }
    fn set_cols(&self, value: u32) {
        self.reflect_attribute("cols", &value.to_string());
    }
    fn max_length(&self) -> i32 {
        self.reflect_length("maxlength")
    }
    fn set_max_length(&self, value: i32) {
        if value >= 0 {
            self.reflect_attribute("maxlength", &value.to_string());
        }
    }
    fn min_length(&self) -> i32 {
        self.reflect_length("minlength")
    }
    fn set_min_length(&self, value: i32) {
        if value >= 0 {
            self.reflect_attribute("minlength", &value.to_string());
        }
    }
    fn text_length(&self) -> u32 {
        let (doc, id) = self.anchor();
        let value = forms::control_value(&doc.borrow(), id);
        u32::try_from(value.encode_utf16().count()).unwrap_or(u32::MAX)
    }
    fn form(&self) -> Result<Option<Handle<HTMLFormElement>>, JsError> {
        Ok(self.form_handle())
    }
}

impl HTMLOptionElementOps for HostObject {
    reflected_bools! {
        disabled, set_disabled => "disabled";
        default_selected, set_default_selected => "selected";
    }

    fn selected(&self) -> bool {
        let (doc, id) = self.anchor();
        forms::is_selected(&doc.borrow(), id)
    }
    fn set_selected(&self, value: bool) {
        let (doc, id) = self.anchor();
        forms::set_selected(&mut doc.borrow_mut(), id, value);
    }
    fn value(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(forms::option_value(&doc.borrow(), id))
    }
    fn set_value(&self, value: &str) {
        self.reflect_attribute("value", value);
    }
    fn text(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(forms::option_text(&doc.borrow(), id))
    }
    fn set_text(&self, value: &str) {
        let _ = <Self as NodeOps>::set_text_content(self, Some(value));
    }
    fn label(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(match doc.attribute_str(id, "label") {
            Some(label) => label.to_owned(),
            None => forms::option_text(&doc, id),
        })
    }
    fn set_label(&self, value: &str) {
        self.reflect_attribute("label", value);
    }
    fn index(&self) -> i32 {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        forms::option_select(&doc, id)
            .and_then(|select| {
                forms::select_options(&doc, select)
                    .iter()
                    .position(|option| *option == id)
            })
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(0)
    }
    fn form(&self) -> Result<Option<Handle<HTMLFormElement>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        // An option's form owner is its select's.
        Ok(forms::option_select(&doc, id)
            .and_then(|select| forms::form_owner(&doc, select))
            .map(|form| Handle::from_bits(encode_node(self.slot(), doc.node_ref(form)))))
    }
}

impl HTMLLabelElementOps for HostObject {
    fn html_for(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("for"))
    }
    fn set_html_for(&self, value: &str) {
        self.reflect_attribute("for", value);
    }
    fn control(&self) -> Result<Option<Handle<HTMLElement>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(forms::label_control(&doc, id)
            .map(|control| Handle::from_bits(encode_node(self.slot(), doc.node_ref(control)))))
    }
    fn form(&self) -> Result<Option<Handle<HTMLFormElement>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(forms::label_control(&doc, id)
            .and_then(|control| forms::form_owner(&doc, control))
            .map(|form| Handle::from_bits(encode_node(self.slot(), doc.node_ref(form)))))
    }
}

impl HTMLOutputElementOps for HostObject {
    reflected_strings! {
        name, set_name => "name";
    }
    fn r#type(&self) -> Result<String, JsError> {
        Ok("output".to_owned())
    }
    fn value(&self) -> Result<String, JsError> {
        Ok(<Self as NodeOps>::text_content(self)?.unwrap_or_default())
    }
    fn set_value(&self, value: &str) {
        let _ = <Self as NodeOps>::set_text_content(self, Some(value));
    }
    fn default_value(&self) -> Result<String, JsError> {
        Ok(<Self as NodeOps>::text_content(self)?.unwrap_or_default())
    }
    fn set_default_value(&self, value: &str) {
        let _ = <Self as NodeOps>::set_text_content(self, Some(value));
    }
    fn form(&self) -> Result<Option<Handle<HTMLFormElement>>, JsError> {
        Ok(self.form_handle())
    }
}

impl HTMLProgressElementOps for HostObject {
    fn value(&self) -> f64 {
        let max = <Self as HTMLProgressElementOps>::max(self);
        self.reflect_double("value")
            .filter(|value| *value >= 0.0)
            .map_or(0.0, |value| value.min(max))
    }
    fn set_value(&self, value: f64) {
        self.set_reflect_double("value", value);
    }
    fn max(&self) -> f64 {
        self.reflect_double("max")
            .filter(|max| *max > 0.0)
            .unwrap_or(1.0)
    }
    fn set_max(&self, value: f64) {
        if value > 0.0 {
            self.set_reflect_double("max", value);
        }
    }
    fn position(&self) -> f64 {
        // Without a valid `value` the bar is indeterminate.
        if self.reflect_double("value").is_none() {
            return -1.0;
        }
        <Self as HTMLProgressElementOps>::value(self) / <Self as HTMLProgressElementOps>::max(self)
    }
}

impl HostObject {
    /// The meter's minimum, maximum, value, low, high and optimum, each
    /// clamped by the standard's rules.
    fn meter_points(&self) -> [f64; 6] {
        let min = self.reflect_double("min").unwrap_or(0.0);
        let max = self.reflect_double("max").unwrap_or(1.0).max(min);
        let value = self.reflect_double("value").unwrap_or(0.0).clamp(min, max);
        let low = self.reflect_double("low").unwrap_or(min).clamp(min, max);
        let high = self.reflect_double("high").unwrap_or(max).clamp(low, max);
        let optimum = self
            .reflect_double("optimum")
            .unwrap_or((min + max) / 2.0)
            .clamp(min, max);
        [min, max, value, low, high, optimum]
    }
}

impl HTMLMeterElementOps for HostObject {
    fn value(&self) -> f64 {
        self.meter_points()[2]
    }
    fn set_value(&self, value: f64) {
        self.set_reflect_double("value", value);
    }
    fn min(&self) -> f64 {
        self.meter_points()[0]
    }
    fn set_min(&self, value: f64) {
        self.set_reflect_double("min", value);
    }
    fn max(&self) -> f64 {
        self.meter_points()[1]
    }
    fn set_max(&self, value: f64) {
        self.set_reflect_double("max", value);
    }
    fn low(&self) -> f64 {
        self.meter_points()[3]
    }
    fn set_low(&self, value: f64) {
        self.set_reflect_double("low", value);
    }
    fn high(&self) -> f64 {
        self.meter_points()[4]
    }
    fn set_high(&self, value: f64) {
        self.set_reflect_double("high", value);
    }
    fn optimum(&self) -> f64 {
        self.meter_points()[5]
    }
    fn set_optimum(&self, value: f64) {
        self.set_reflect_double("optimum", value);
    }
}

impl HTMLDetailsElementOps for HostObject {
    reflected_strings! {
        name, set_name => "name";
    }
    reflected_bools! {
        open, set_open => "open";
    }
}
