//! Which elements are replaced or engine-painted controls, and the text a
//! control shows.

use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_dom::Document;

use crate::types::{ControlKind, ReplacedKind};

/// A replaced element's kind and the text its control displays.
pub(super) struct ReplacedInfo {
    pub(super) kind: ReplacedKind,
    pub(super) text: Option<String>,
}

pub(super) fn classify(doc: &Document, node: NodeId) -> Option<ReplacedInfo> {
    let tag = doc.tag(node)?;
    let ns = doc.namespace(node)?;
    if ns == atoms::NS_SVG {
        return (tag == atoms::SVG).then_some(ReplacedInfo {
            kind: ReplacedKind::Svg,
            text: None,
        });
    }
    if ns != atoms::NS_HTML {
        return None;
    }
    let plain = |kind| Some(ReplacedInfo { kind, text: None });
    match tag {
        t if t == atoms::IMG => plain(ReplacedKind::Image),
        t if t == atoms::CANVAS => plain(ReplacedKind::Canvas),
        t if t == atoms::VIDEO || t == atoms::AUDIO => plain(ReplacedKind::Video),
        t if t == atoms::IFRAME || t == atoms::OBJECT || t == atoms::EMBED => {
            plain(ReplacedKind::Iframe)
        }
        t if t == atoms::PROGRESS => plain(ReplacedKind::Control(ControlKind::Progress)),
        t if t == atoms::METER => plain(ReplacedKind::Control(ControlKind::Meter)),
        t if t == atoms::TEXTAREA => Some(ReplacedInfo {
            kind: ReplacedKind::Control(ControlKind::TextArea),
            text: Some(textarea_text(doc, node)),
        }),
        t if t == atoms::SELECT => Some(ReplacedInfo {
            kind: ReplacedKind::Control(ControlKind::Select),
            text: Some(selected_option_label(doc, node)),
        }),
        t if t == atoms::INPUT => Some(input_info(doc, node)),
        _ => None,
    }
}

fn input_info(doc: &Document, node: NodeId) -> ReplacedInfo {
    let ty = doc
        .attribute(node, atoms::TYPE)
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let value = || {
        doc.dirty_value(node)
            .or_else(|| doc.attribute(node, atoms::VALUE))
            .unwrap_or("")
            .to_owned()
    };
    let (kind, text) = match ty.as_str() {
        "checkbox" => (ReplacedKind::Control(ControlKind::Checkbox), None),
        "radio" => (ReplacedKind::Control(ControlKind::Radio), None),
        "range" => (ReplacedKind::Control(ControlKind::Range), None),
        "color" => (ReplacedKind::Control(ControlKind::Color), None),
        "file" => (ReplacedKind::Control(ControlKind::File), None),
        "image" => (ReplacedKind::Image, None),
        "button" => (ReplacedKind::Control(ControlKind::Button), Some(value())),
        "submit" | "reset" => {
            let v = value();
            let label = match (v.is_empty(), ty.as_str()) {
                (false, _) => v,
                (true, "submit") => "Submit".to_owned(),
                (true, _) => "Reset".to_owned(),
            };
            (ReplacedKind::Control(ControlKind::Button), Some(label))
        }
        "password" => (
            ReplacedKind::Control(ControlKind::TextInput),
            Some(value().chars().map(|_| '\u{2022}').collect()),
        ),
        _ => (ReplacedKind::Control(ControlKind::TextInput), Some(value())),
    };
    ReplacedInfo { kind, text }
}

fn textarea_text(doc: &Document, node: NodeId) -> String {
    match doc.dirty_value(node) {
        Some(v) => v.to_owned(),
        None => doc
            .children(node)
            .filter_map(|child| doc.text_data(child))
            .collect(),
    }
}

/// The label of the option a `<select>` displays: the first `selected`
/// option, else the first option.
fn selected_option_label(doc: &Document, select: NodeId) -> String {
    let options: Vec<NodeId> = doc
        .descendants(select)
        .filter(|&n| doc.tag(n) == Some(atoms::OPTION))
        .collect();
    let chosen = options
        .iter()
        .copied()
        .find(|&o| doc.has_attribute(o, atoms::SELECTED))
        .or_else(|| options.first().copied());
    let Some(option) = chosen else {
        return String::new();
    };
    let raw = doc
        .attribute(option, atoms::LABEL)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            doc.children(option)
                .filter_map(|child| doc.text_data(child))
                .collect()
        });
    raw.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
}
