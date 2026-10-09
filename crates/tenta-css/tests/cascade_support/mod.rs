//! Shared helpers of the `cascade_*` integration tests.
#![allow(dead_code)]

use style::Atom;
use style::properties::{PropertyDeclarationId, PropertyId};
use tenta_core::NodeId;
use tenta_core::atom::{Atom as TentaAtom, atoms};
use tenta_css::cascade::test_util::style_html;
use tenta_css::cascade::{Style, StyleEngine, StyleEnv};
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;

/// Styles `html` with the default environment.
pub fn styled(html: &str) -> (Document, StyleEngine) {
    style_html(html, StyleEnv::default())
}

/// The element with id `id`.
pub fn element(doc: &Document, id: &str) -> NodeId {
    doc.get_element_by_id(id)
        .unwrap_or_else(|| panic!("the fixture has no #{id}"))
}

/// The computed style of `#id`.
pub fn style_of(doc: &Document, engine: &StyleEngine, id: &str) -> Style {
    engine
        .style(element(doc, id))
        .unwrap_or_else(|| panic!("#{id} has no style"))
}

/// The serialized computed value of `property` (a longhand or `--custom`).
pub fn computed(style: &Style, property: &str) -> String {
    if let Some(custom) = property.strip_prefix("--") {
        let name = Atom::from(custom);
        return style.computed_value_to_string(PropertyDeclarationId::Custom(&name));
    }
    let id = PropertyId::parse_enabled_for_all_content(property)
        .unwrap_or_else(|_| panic!("unknown property {property}"));
    let PropertyId::NonCustom(id) = id else {
        panic!("{property} is not a standard property");
    };
    let longhand = id
        .as_longhand()
        .unwrap_or_else(|| panic!("{property} is a shorthand"));
    style.computed_value_to_string(PropertyDeclarationId::Longhand(longhand))
}

/// The computed value of `property` on `#id`.
pub fn value(doc: &Document, engine: &StyleEngine, id: &str, property: &str) -> String {
    computed(&style_of(doc, engine, id), property)
}

/// Sets the attribute `name` of `node`.
pub fn set_attribute(doc: &mut Document, node: NodeId, name: &str, value: &str) {
    doc.mutate(Mutation::SetAttribute {
        element: node,
        name: QualName::new(atoms::EMPTY, TentaAtom::new(name)),
        value: value.to_owned(),
    })
    .expect("set attribute");
}
