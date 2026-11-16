//! Form extraction (`docs/spec/04-dom-and-css.md` §5): every form with its
//! action, method and fields, each field with its type, value and label text.

use tenta_core::NodeId;
use tenta_core::atom::atoms;

use crate::document::Document;
use crate::extract::text::collapsed_text;

/// One `<option>` of a `<select>`.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
    pub selected: bool,
    pub disabled: bool,
}

/// One form control.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize)]
pub struct Field {
    /// The control's element.
    #[serde(skip)]
    pub node: NodeId,
    /// Local name of the control element: `input`, `select`, `textarea`,
    /// `button`.
    pub element: String,
    /// The effective `type`: the `type` attribute for `<input>` and
    /// `<button>` (defaulting to `text` and `submit`), otherwise the element
    /// name.
    pub field_type: String,
    pub name: Option<String>,
    pub id: Option<String>,
    /// Current value: the `value` attribute, or a `<textarea>`'s text.
    pub value: Option<String>,
    /// Label text from `<label for>` or a wrapping `<label>`.
    pub label: Option<String>,
    pub placeholder: Option<String>,
    pub required: bool,
    pub disabled: bool,
    pub checked: bool,
    pub multiple: bool,
    /// Options of a `<select>`.
    pub options: Vec<SelectOption>,
}

/// One form.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize)]
pub struct Form {
    #[serde(skip)]
    pub node: NodeId,
    pub id: Option<String>,
    pub name: Option<String>,
    /// The `action`, resolved against the document base URL. A form without
    /// an action submits to the document URL.
    pub action: Option<String>,
    /// Upper-case HTTP method; `GET` when the attribute is missing.
    pub method: String,
    pub enctype: Option<String>,
    pub fields: Vec<Field>,
}

fn is_control(document: &Document, node: NodeId) -> bool {
    matches!(
        document.tag(node),
        Some(t)
            if t == atoms::INPUT
                || t == atoms::SELECT
                || t == atoms::TEXTAREA
                || t == atoms::BUTTON
    )
}

/// The label text for a control: a `<label for=id>` anywhere in the document,
/// otherwise the nearest wrapping `<label>`.
fn label_for(document: &Document, control: NodeId) -> Option<String> {
    if let Some(id) = document.attribute(control, atoms::ID)
        && let Some(label) = document
            .get_elements_by_tag_name("label")
            .into_iter()
            .find(|label| document.attribute(*label, atoms::FOR) == Some(id))
    {
        let text = collapsed_text(document, label);
        if !text.is_empty() {
            return Some(text);
        }
    }
    let wrapping = document
        .ancestors(control)
        .find(|a| document.tag(*a) == Some(atoms::LABEL))?;
    let text = collapsed_text(document, wrapping);
    (!text.is_empty()).then_some(text)
}

fn field(document: &Document, node: NodeId) -> Field {
    let tag = document.tag(node).unwrap_or(atoms::EMPTY);
    let field_type = if tag == atoms::INPUT {
        document
            .attribute(node, atoms::TYPE)
            .map(|t| t.trim().to_ascii_lowercase())
            .unwrap_or_else(|| "text".to_owned())
    } else if tag == atoms::BUTTON {
        document
            .attribute(node, atoms::TYPE)
            .map(|t| t.trim().to_ascii_lowercase())
            .unwrap_or_else(|| "submit".to_owned())
    } else {
        tag.as_str().to_owned()
    };
    // What the control holds now: a value script or input set wins over the
    // content attribute. A password is the exception: only the attribute is
    // reported, so a typed secret never leaves the page through an extraction.
    let value = if field_type == "password" {
        document.attribute(node, atoms::VALUE).map(str::to_owned)
    } else if tag == atoms::TEXTAREA {
        Some(crate::forms::control_value(document, node))
    } else if tag == atoms::INPUT {
        match document.dirty_value(node) {
            Some(dirty) => Some(dirty.to_owned()),
            None => document.attribute(node, atoms::VALUE).map(str::to_owned),
        }
    } else {
        document.attribute(node, atoms::VALUE).map(str::to_owned)
    };
    let options = if tag == atoms::SELECT {
        document
            .descendants(node)
            .filter(|child| document.tag(*child) == Some(atoms::OPTION))
            .map(|option| {
                let label = collapsed_text(document, option);
                SelectOption {
                    value: document
                        .attribute(option, atoms::VALUE)
                        .map(str::to_owned)
                        .unwrap_or_else(|| label.clone()),
                    label,
                    selected: crate::forms::is_selected(document, option),
                    disabled: crate::forms::is_option_disabled(document, option),
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    Field {
        node,
        element: tag.as_str().to_owned(),
        field_type,
        name: document.attribute(node, atoms::NAME).map(str::to_owned),
        id: document.attribute(node, atoms::ID).map(str::to_owned),
        value,
        label: label_for(document, node),
        placeholder: document
            .attribute(node, atoms::PLACEHOLDER)
            .map(str::to_owned),
        required: document.has_attribute(node, atoms::REQUIRED),
        disabled: crate::forms::is_disabled(document, node),
        checked: crate::forms::is_checked(document, node),
        multiple: document.has_attribute(node, atoms::MULTIPLE),
        options,
    }
}

fn should_include_field(document: &Document, node: NodeId) -> bool {
    // Exclude button type=button and type=reset (only submit buttons are fields)
    if document.tag(node) == Some(atoms::BUTTON) {
        let button_type = document
            .attribute(node, atoms::TYPE)
            .map(|t| t.trim().to_ascii_lowercase())
            .unwrap_or_else(|| "submit".to_owned());
        if button_type == "button" || button_type == "reset" {
            return false;
        }
    }
    true
}

/// Every form in the subtree, with its controls. Controls associated through
/// the `form="id"` attribute are included even when they sit outside the
/// form element.
pub fn forms(document: &Document, root: NodeId) -> Vec<Form> {
    let mut found = Vec::new();
    for node in document.descendants_inclusive(root) {
        if document.tag(node) != Some(atoms::FORM) {
            continue;
        }
        let id = document.attribute(node, atoms::ID).map(str::to_owned);
        let mut fields: Vec<Field> = document
            .descendants(node)
            .filter(|child| is_control(document, *child) && should_include_field(document, *child))
            .map(|child| field(document, child))
            .collect();
        if let Some(form_id) = id.as_deref() {
            let associated: Vec<NodeId> = document
                .descendants(document.document_node())
                .filter(|candidate| {
                    is_control(document, *candidate)
                        && should_include_field(document, *candidate)
                        && document.attribute_str(*candidate, "form") == Some(form_id)
                        && !document.ancestors(*candidate).any(|a| a == node)
                })
                .collect();
            fields.extend(
                associated
                    .into_iter()
                    .map(|control| field(document, control)),
            );
        }
        found.push(Form {
            node,
            id,
            name: document.attribute(node, atoms::NAME).map(str::to_owned),
            action: document
                .attribute(node, atoms::ACTION)
                .and_then(|action| document.resolve_url(action.trim()))
                // A `javascript:` action never submits over the network; drop it
                // to the document-URL fallback. The scheme is checked after URL
                // parsing so interior tab/CR/LF and leading C0 controls (which the
                // WHATWG parser strips) cannot smuggle the scheme past the filter.
                .filter(|url| url.scheme() != "javascript")
                .map(|url| url.to_string())
                .or_else(|| document.url().map(|url| url.to_string())),
            method: document
                .attribute(node, atoms::METHOD)
                .map(|m| m.trim().to_ascii_uppercase())
                .unwrap_or_else(|| "GET".to_owned()),
            enctype: document.attribute(node, atoms::ENCTYPE).map(str::to_owned),
            fields,
        });
    }
    found
}

impl Document {
    /// Every form in the document.
    pub fn forms(&self) -> Vec<Form> {
        forms(self, self.document_node())
    }
}
