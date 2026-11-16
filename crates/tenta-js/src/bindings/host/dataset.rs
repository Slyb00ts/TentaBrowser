//! `element.dataset`: HTML's `DOMStringMap` over the element's `data-*`
//! attributes
//! (<https://html.spec.whatwg.org/multipage/dom.html#domstringmap>).

use tenta_core::Atom;
use tenta_core::atom::atoms;
use tenta_html::QualName;

use super::{HostObject, encode_dataset};
use crate::bindings::{DOMStringMap, DOMStringMapOps, Handle, JsError};

/// The map's name for the attribute `data-<rest>`: each hyphen followed by an
/// ASCII lower-case letter is dropped and the letter upper-cased. An
/// attribute with an ASCII upper-case letter after `data-` has no name: it
/// could not be written back.
fn property_name(attribute: &str) -> Option<String> {
    let rest = attribute.strip_prefix("data-")?;
    if rest.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return None;
    }
    let mut name = String::with_capacity(rest.len());
    let mut chars = rest.chars().peekable();
    while let Some(ch) = chars.next() {
        match chars.peek() {
            Some(next) if ch == '-' && next.is_ascii_lowercase() => {
                name.push(next.to_ascii_uppercase());
                chars.next();
            }
            _ => name.push(ch),
        }
    }
    Some(name)
}

/// The attribute a map name stands for: `data-`, then the name with each
/// ASCII upper-case letter replaced by a hyphen and its lower-case form.
/// `None` for a name with a hyphen before an ASCII lower-case letter, which
/// no attribute maps to.
fn attribute_name(property: &str) -> Option<String> {
    let mut attribute = String::with_capacity(property.len() + 8);
    attribute.push_str("data-");
    let mut chars = property.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '-' && chars.peek().is_some_and(char::is_ascii_lowercase) {
            return None;
        }
        if ch.is_ascii_uppercase() {
            attribute.push('-');
            attribute.push(ch.to_ascii_lowercase());
        } else {
            attribute.push(ch);
        }
    }
    Some(attribute)
}

/// Whether `name` can be an attribute's local name: no ASCII whitespace, NUL,
/// `/`, `=` or `>`, as `setAttribute` requires of a valid attribute name.
fn is_valid_attribute_name(name: &str) -> bool {
    !name.is_empty()
        && !name
            .chars()
            .any(|ch| ch.is_ascii_whitespace() || matches!(ch, '\0' | '/' | '=' | '>'))
}

impl HostObject {
    /// The `dataset` of the element this host stands for.
    pub(super) fn dataset_handle(&self) -> Handle<DOMStringMap> {
        let (doc, id) = self.anchor();
        let owner = doc.borrow().node_ref(id);
        Handle::from_bits(encode_dataset(self.slot(), owner))
    }
}

impl DOMStringMapOps for HostObject {
    fn named_getter(&self, name: &str) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(attribute_name(name)
            .and_then(|attribute| doc.attribute_str(id, &attribute))
            .unwrap_or("")
            .to_owned())
    }

    fn named_setter(&self, name: &str, value: &str) -> Result<(), JsError> {
        let attribute = attribute_name(name).ok_or_else(|| {
            JsError::dom_exception(
                "SyntaxError",
                &format!("'{name}' is not a valid property name."),
            )
        })?;
        if !is_valid_attribute_name(&attribute) {
            return Err(JsError::dom_exception(
                "InvalidCharacterError",
                &format!("'{attribute}' is not a valid attribute name."),
            ));
        }
        let (_, id) = self.anchor();
        let qname = QualName::new(atoms::EMPTY, Atom::new(&attribute));
        self.context()
            .set_element_attribute(id, qname, value.to_owned())
    }

    fn named_deleter(&self, name: &str) -> Result<(), JsError> {
        if let Some(attribute) = attribute_name(name) {
            let (_, id) = self.anchor();
            let qname = QualName::new(atoms::EMPTY, Atom::new(&attribute));
            let _ = self.context().remove_element_attribute(id, qname);
        }
        Ok(())
    }

    fn supported_property_names(&self) -> Result<Vec<String>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let mut names: Vec<String> = Vec::new();
        for (attribute, _) in doc.attributes(id) {
            // Only attributes in no namespace are `data-*` attributes.
            if attribute.ns == atoms::EMPTY
                && let Some(name) = property_name(attribute.local.as_str())
                && !names.contains(&name)
            {
                names.push(name);
            }
        }
        Ok(names)
    }

    fn is_supported_property_name(&self, name: &str) -> Result<bool, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(attribute_name(name)
            .is_some_and(|attribute| doc.attribute_str(id, &attribute).is_some()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_convert_both_ways() {
        for (attribute, property) in [
            ("data-foo", "foo"),
            ("data-foo-bar", "fooBar"),
            ("data-foo-bar-baz", "fooBarBaz"),
            ("data-", ""),
            ("data-x-1", "x-1"),
            ("data--leading", "Leading"),
            ("data-trailing-", "trailing-"),
        ] {
            assert_eq!(
                property_name(attribute).as_deref(),
                Some(property),
                "{attribute}"
            );
            assert_eq!(
                attribute_name(property).as_deref(),
                Some(attribute),
                "{property}"
            );
        }
        assert_eq!(property_name("data-Upper"), None);
        assert_eq!(property_name("title"), None);
        assert_eq!(
            attribute_name("foo-bar"),
            None,
            "a hyphen before a lower-case letter"
        );
        assert_eq!(attribute_name("foo-Bar").as_deref(), Some("data-foo--bar"));
    }
}
