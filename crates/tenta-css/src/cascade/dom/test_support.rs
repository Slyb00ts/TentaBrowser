//! Shared fixtures for the adapter's unit tests.

use style::shared_lock::SharedRwLock;
use tenta_core::NodeId;
use tenta_dom::Document;
use tenta_html::{Parser, ParserOptions};

use super::{AtomMaps, FlushView, StyleStore};

/// Parses `html` into a document, the way a page load does.
pub(super) fn parse(html: &str) -> Document {
    let mut parser = Parser::new(Document::new(), ParserOptions::default());
    parser.feed_last(html.as_bytes());
    parser.finish()
}

/// Builds the store, atom maps and handle table for `doc` and runs `body`
/// against the resulting view.
pub(super) fn with_view<R>(doc: &Document, body: impl for<'a> FnOnce(&'a FlushView<'a>) -> R) -> R {
    let mut store = StyleStore::new();
    for slot in 0..doc.slots() {
        store.claim(doc.node_ref(NodeId(slot as u32)));
    }
    let mut atoms = AtomMaps::new();
    atoms.begin_flush(doc);
    let lock = SharedRwLock::new();
    let view = FlushView::new(doc, &atoms, &lock);
    let table = view.make_slots(&store);
    assert!(view.install_slots(&table));
    body(&view)
}
