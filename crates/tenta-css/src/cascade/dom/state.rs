//! Pseudo-class state of an element, derived from the document on demand.
//!
//! The form and link rules are `tenta_dom::selectors`' own functions, so that
//! `querySelector` and the cascade cannot disagree on what `:checked` or
//! `:disabled` match.

use std::sync::LazyLock;

use stylo_dom::ElementState;
use tenta_core::NodeId;
use tenta_core::atom::{Atom, atoms};
use tenta_dom::{Document, selectors};

static INDETERMINATE: LazyLock<Atom> = LazyLock::new(|| Atom::new("indeterminate"));
pub(super) static PART: LazyLock<Atom> = LazyLock::new(|| Atom::new("part"));
pub(super) static EXPORTPARTS: LazyLock<Atom> = LazyLock::new(|| Atom::new("exportparts"));
pub(super) static BORDER: LazyLock<Atom> = LazyLock::new(|| Atom::new("border"));

/// `:link` and `:any-link`.
pub(super) fn is_link(doc: &Document, node: NodeId) -> bool {
    selectors::is_link(doc, node)
}

/// Every state flag `match_non_ts_pseudo_class` can be asked about.
///
/// The selector map skips whole rule buckets (for instance `:link` rules)
/// unless this reports the matching flag, so it must be complete, not just
/// consistent with the matcher.
pub(super) fn element_state(doc: &Document, node: NodeId) -> ElementState {
    let tag = doc.tag(node);
    let mut state = ElementState::DEFINED;
    if is_link(doc, node) {
        state |= ElementState::UNVISITED;
    }
    if selectors::is_checked(doc, node) {
        state |= ElementState::CHECKED;
    }
    if is_default(doc, node, tag) {
        state |= ElementState::DEFAULT;
    }
    if selectors::is_disabled(doc, node) {
        state |= ElementState::DISABLED;
    } else if selectors::is_enabled(doc, node) {
        state |= ElementState::ENABLED;
    }
    if matches!(tag, Some(t) if t == atoms::INPUT || t == atoms::SELECT || t == atoms::TEXTAREA) {
        state |= if doc.has_attribute(node, atoms::REQUIRED) {
            ElementState::REQUIRED
        } else {
            ElementState::OPTIONAL_
        };
    }
    state |= if selectors::is_read_write(doc, node) {
        ElementState::READWRITE
    } else {
        ElementState::READONLY
    };
    if doc.has_attribute(node, *INDETERMINATE) {
        state |= ElementState::INDETERMINATE;
    }
    if selectors::is_placeholder_shown(doc, node) {
        state |= ElementState::PLACEHOLDER_SHOWN;
    }
    if selectors::is_focus_within(doc, node) {
        state |= ElementState::FOCUS_WITHIN;
        if selectors::is_focused(doc, node) {
            state |= ElementState::FOCUS;
            if doc.focus_visible() {
                state |= ElementState::FOCUSRING;
            }
        }
    }
    state
}

fn is_default(doc: &Document, node: NodeId, tag: Option<Atom>) -> bool {
    match tag {
        Some(t) if t == atoms::OPTION => doc.has_attribute(node, atoms::SELECTED),
        Some(t) if t == atoms::INPUT => {
            let kind = doc
                .attribute(node, atoms::TYPE)
                .map_or_else(|| "text".to_owned(), |t| t.trim().to_ascii_lowercase());
            matches!(kind.as_str(), "checkbox" | "radio") && doc.has_attribute(node, atoms::CHECKED)
        }
        _ => false,
    }
}

/// `:-servo-nonzero-border`: a `border` attribute that is not the number
/// zero. An unparsable value counts as non-zero, as it does for the table
/// border the attribute maps to.
pub(super) fn has_nonzero_border(doc: &Document, node: NodeId) -> bool {
    let Some(value) = doc.attribute(node, *BORDER) else {
        return false;
    };
    let digits: String = value
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.is_empty() || !digits.trim_start_matches('0').is_empty()
}

/// The `lang` of the nearest inclusive ancestor that declares one. `lang`
/// wins over `xml:lang`; an empty value is a declaration ("unknown") and
/// stops the search.
pub(super) fn declared_language(doc: &Document, node: NodeId) -> Option<&str> {
    std::iter::once(node)
        .chain(doc.ancestors(node))
        .find_map(|candidate| {
            doc.attribute(candidate, atoms::LANG)
                .or_else(|| doc.attribute_ns(candidate, atoms::NS_XML, atoms::LANG))
        })
}

/// RFC 4647 extended filtering of one language range against a language
/// tag, which is what `:lang()` of Selectors 3 asks for.
pub(super) fn language_matches(tag: &str, range: &str) -> bool {
    let mut tag_parts = tag.split('-');
    let mut range_parts = range.split('-');
    let (Some(tag_first), Some(range_first)) = (tag_parts.next(), range_parts.next()) else {
        return false;
    };
    if range_first == "*" {
        if range.len() == 1 {
            return !tag.is_empty();
        }
    } else if !tag_first.eq_ignore_ascii_case(range_first) {
        return false;
    }
    'ranges: for wanted in range_parts {
        if wanted == "*" {
            continue;
        }
        for candidate in tag_parts.by_ref() {
            if candidate.eq_ignore_ascii_case(wanted) {
                continue 'ranges;
            }
            if candidate.len() == 1 {
                return false;
            }
        }
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cascade::dom::test_support::parse;

    #[test]
    fn language_ranges_match_by_subtag_prefix() {
        assert!(language_matches("en", "en"));
        assert!(language_matches("en-US", "en"));
        assert!(language_matches("EN-us", "en-US"));
        assert!(language_matches("de-Latn-DE", "de-DE"), "skips the script");
        assert!(language_matches("de-DE", "*-DE"));
        assert!(language_matches("fr", "*"));
        assert!(!language_matches("", "*"));
        assert!(!language_matches("english", "en"));
        assert!(!language_matches("en", "en-US"));
        assert!(
            !language_matches("de-x-DE", "de-DE"),
            "stops at a singleton"
        );
        assert!(!language_matches("", "en"));
    }

    #[test]
    fn nonzero_border_follows_the_leading_digits() {
        let doc = parse(
            "<table border=0></table><table border=00></table><table border=1></table>\
             <table border></table><table border=x></table><table border=0x></table><table></table>",
        );
        let verdicts: Vec<bool> = doc
            .get_elements_by_tag_name("table")
            .into_iter()
            .map(|table| has_nonzero_border(&doc, table))
            .collect();
        assert_eq!(verdicts, [false, false, true, true, true, false, false]);
    }
}
