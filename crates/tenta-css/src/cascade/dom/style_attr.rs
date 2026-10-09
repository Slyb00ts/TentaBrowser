//! Parsed `style=""` blocks, keyed by node and attribute revision.
//!
//! The `style` attribute is the single source of truth for an element's inline
//! style. `element.style` in script reads and writes that attribute (it parses
//! the text with `CssDeclarationBlock` and serializes back to the attribute),
//! so the cascade sees exactly what script wrote by parsing the attribute here.
//! Nothing is shared between the two parsed forms except the text.
//!
//! The cache cells are `OnceCell`s: a flush is sequential and the store is not
//! `Sync`.

use servo_arc::{Arc, ArcBorrow};
use style::properties::{PropertyDeclarationBlock, parse_style_attribute};
use style::shared_lock::Locked;
use style::stylesheets::{CssRuleType, UrlExtraData};
use tenta_core::atom::atoms;
use tenta_dom::Document;
use url::Url;

use super::{FlushView, Slot};
use crate::cssom::configure_stylo;

/// The element's parsed `style` attribute, parsed with the engine's lock and
/// cached in the slot's store entry. `None` when the element has no `style`
/// attribute or it is empty.
///
/// Unitless lengths are accepted in quirks mode only, as the document's mode
/// dictates. [`StyleStore::refresh`](super::StyleStore::refresh) must have run
/// since the attribute last changed.
pub(crate) fn style_attribute<'a>(
    slot: &'a Slot<'a>,
) -> Option<ArcBorrow<'a, Locked<PropertyDeclarationBlock>>> {
    let view = slot.view;
    let doc = view.doc;
    assert_eq!(
        slot.entry.refreshed_generation.get(),
        doc.mutation_generation(),
        "StyleStore::refresh must run after the last document mutation",
    );
    let attr = &slot.entry.style_attr;
    let block = attr.block.get_or_init(|| {
        attr.source_generation.set(doc.mutation_generation());
        let text = doc.attribute(slot.node, atoms::STYLE)?;
        // Set together with the block; the cell is private to this function.
        let _ = attr.source.set(text.into());
        if text.trim().is_empty() {
            return None;
        }
        configure_stylo();
        let parsed =
            parse_style_attribute(text, &url_data(view), None, view.quirks, CssRuleType::Style);
        (!parsed.is_empty()).then(|| Arc::new(view.lock.wrap(parsed)))
    });
    debug_assert_eq!(
        attr.source.get().map(|text| &**text),
        doc.attribute(slot.node, atoms::STYLE),
        "the style store was not refreshed after an attribute change",
    );
    block.as_ref().map(Arc::borrow_arc)
}

/// The base URL against which `url()` in inline style and hints resolves: the
/// document base, then the document URL, then `about:blank`.
pub(super) fn document_base(doc: &Document) -> Url {
    doc.base_url()
        .or_else(|| doc.url())
        .cloned()
        .unwrap_or_else(|| Url::parse("about:blank").expect("about:blank is a valid URL"))
}

pub(super) fn url_data(view: &FlushView<'_>) -> UrlExtraData {
    UrlExtraData::from(document_base(view.doc))
}

#[cfg(test)]
mod tests {
    use style::properties::PropertyDeclaration;
    use style::shared_lock::SharedRwLock;
    use style::values::generics::image::Image;
    use tenta_core::NodeId;
    use tenta_core::atom::{Atom, atoms};
    use tenta_dom::{Document, Mutation};
    use tenta_html::QualName;

    use super::super::test_support::parse;
    use super::super::{AtomMaps, FlushView, StyleStore};

    fn target(doc: &Document) -> NodeId {
        (0..doc.slots() as u32)
            .map(NodeId)
            .find(|&node| doc.element_id(node) == Some("t"))
            .expect("the fixture has an element with id=t")
    }

    fn store_for(doc: &Document) -> StyleStore {
        let mut store = StyleStore::new();
        for slot in 0..doc.slots() {
            store.claim(doc.node_ref(NodeId(slot as u32)));
        }
        store.refresh(doc);
        store
    }

    /// The serialized `style` block of `node` and the identity of its cached
    /// allocation, as one flush would see them.
    fn read(
        doc: &Document,
        lock: &SharedRwLock,
        store: &StyleStore,
        node: NodeId,
    ) -> (Option<String>, usize) {
        let mut atoms = AtomMaps::new();
        atoms.begin_flush(doc);
        let view = FlushView::new(doc, &atoms, lock);
        let table = view.make_slots(store);
        assert!(view.install_slots(&table));
        let handle = view.handle(node).unwrap();
        match handle.style_attribute() {
            None => (None, 0),
            Some(block) => {
                let identity = std::ptr::from_ref(&*block) as usize;
                let guard = lock.read();
                let mut css = String::new();
                block.read_with(&guard).to_css(&mut css).unwrap();
                (Some(css), identity)
            }
        }
    }

    fn set_style(doc: &mut Document, node: NodeId, value: &str) {
        doc.mutate(Mutation::SetAttribute {
            element: node,
            name: QualName::new(atoms::EMPTY, Atom::new("style")),
            value: value.to_owned(),
        })
        .expect("set style attribute");
    }

    #[test]
    fn the_style_attribute_is_parsed_into_a_block() {
        let doc = parse("<!DOCTYPE html><p id=t style='color: red; width: 10px; bogus: 1'>");
        let store = store_for(&doc);
        let (css, _) = read(&doc, &SharedRwLock::new(), &store, target(&doc));
        assert_eq!(css.as_deref(), Some("color: red; width: 10px;"));
    }

    #[test]
    fn an_absent_or_empty_attribute_has_no_block() {
        for html in [
            "<!DOCTYPE html><p id=t>",
            "<!DOCTYPE html><p id=t style=''>",
            "<!DOCTYPE html><p id=t style='  '>",
            "<!DOCTYPE html><p id=t style='nonsense'>",
        ] {
            let doc = parse(html);
            let store = store_for(&doc);
            assert_eq!(
                read(&doc, &SharedRwLock::new(), &store, target(&doc)).0,
                None,
                "{html}"
            );
        }
    }

    #[test]
    fn unitless_lengths_parse_in_quirks_mode_only() {
        let quirks = parse("<p id=t style='width: 10'>");
        let standards = parse("<!DOCTYPE html><p id=t style='width: 10'>");
        assert_eq!(
            read(
                &quirks,
                &SharedRwLock::new(),
                &store_for(&quirks),
                target(&quirks)
            )
            .0
            .as_deref(),
            Some("width: 10px;"),
        );
        assert_eq!(
            read(
                &standards,
                &SharedRwLock::new(),
                &store_for(&standards),
                target(&standards)
            )
            .0,
            None
        );
    }

    #[test]
    fn the_cache_is_reused_until_the_attribute_changes() {
        let mut doc = parse("<!DOCTYPE html><p id=t style='color: red'><i id=other>");
        let node = target(&doc);
        let mut store = store_for(&doc);
        let lock = SharedRwLock::new();
        let (first, identity) = read(&doc, &lock, &store, node);
        assert_eq!(first.as_deref(), Some("color: red;"));
        assert_eq!(
            read(&doc, &lock, &store, node).1,
            identity,
            "second flush reuses the block"
        );

        // A mutation elsewhere moves the generation but not the text.
        let other = (0..doc.slots() as u32)
            .map(NodeId)
            .find(|&n| doc.element_id(n) == Some("other"))
            .unwrap();
        set_style(&mut doc, other, "color: blue");
        store.refresh(&doc);
        assert_eq!(
            read(&doc, &lock, &store, node).1,
            identity,
            "unchanged text keeps the block"
        );

        set_style(&mut doc, node, "color: blue; margin: 0");
        store.refresh(&doc);
        let (changed, _) = read(&doc, &lock, &store, node);
        assert_eq!(changed.as_deref(), Some("color: blue; margin: 0px;"));
        let (other_css, _) = read(&doc, &lock, &store, other);
        assert_eq!(other_css.as_deref(), Some("color: blue;"));
    }

    #[test]
    fn removing_the_attribute_drops_the_block() {
        let mut doc = parse("<!DOCTYPE html><p id=t style='color: red'>");
        let node = target(&doc);
        let mut store = store_for(&doc);
        let lock = SharedRwLock::new();
        assert!(read(&doc, &lock, &store, node).0.is_some());
        doc.mutate(Mutation::RemoveAttribute {
            element: node,
            name: QualName::new(atoms::EMPTY, Atom::new("style")),
        })
        .expect("remove style attribute");
        store.refresh(&doc);
        assert_eq!(read(&doc, &lock, &store, node).0, None);
    }

    #[test]
    #[should_panic(expected = "StyleStore::refresh must run")]
    fn reading_without_a_refresh_after_a_mutation_panics() {
        let mut doc = parse("<!DOCTYPE html><p id=t style='color: red'>");
        let node = target(&doc);
        let store = store_for(&doc);
        set_style(&mut doc, node, "color: blue");
        read(&doc, &SharedRwLock::new(), &store, node);
    }

    #[test]
    fn url_values_resolve_against_the_base_and_follow_a_base_change() {
        let mut doc = parse("<!DOCTYPE html><p id=t style='background-image: url(a.png)'>");
        doc.set_url(Some("http://example.test/dir/page.html".parse().unwrap()));
        let node = target(&doc);
        let mut store = store_for(&doc);
        let lock = SharedRwLock::new();
        let resolved = |doc: &Document, store: &StyleStore| {
            let mut atoms = AtomMaps::new();
            atoms.begin_flush(doc);
            let view = FlushView::new(doc, &atoms, &lock);
            let table = view.make_slots(store);
            assert!(view.install_slots(&table));
            let block = view.handle(node).unwrap().style_attribute().unwrap();
            let guard = lock.read();
            let declarations = block.read_with(&guard).declarations();
            let PropertyDeclaration::BackgroundImage(images) = &declarations[0] else {
                panic!("expected a background-image declaration");
            };
            match &images.0[0] {
                Image::Url(url) => url.url().unwrap().to_string(),
                other => panic!("expected a url image: {other:?}"),
            }
        };
        assert_eq!(resolved(&doc, &store), "http://example.test/dir/a.png");

        doc.set_url(Some("http://other.test/x/page.html".parse().unwrap()));
        store.refresh(&doc);
        assert_eq!(resolved(&doc, &store), "http://other.test/x/a.png");
    }
}
