//! Flush behaviour: idempotence, restyle after mutation, environment changes,
//! the thread the traversal runs on, and recovery from a panic inside it.
#![cfg(feature = "test-util")]

mod cascade_support;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use cascade_support::*;
use style::thread_state::{self, ThreadState};
use tenta_core::NodeId;
use tenta_css::cascade::test_util::style_html;
use tenta_css::cascade::{
    FontMetrics, FontMetricsData, StyleDiff, StyleEngine, StyleEnv, StyleSource,
};
use tenta_dom::Document;
use tenta_html::{Parser, ParserOptions};

const PAGE: &str = "<!DOCTYPE html><html><head><style>\
    p { color: rgb(1, 0, 0) } .on { color: rgb(2, 0, 0) } div { font-size: 20px }\
    </style></head><body><div id=outer><p id=a>a</p><p id=b class=on>b</p></div></body></html>";

fn parse(html: &str) -> Document {
    let mut parser = Parser::new(Document::new(), ParserOptions::default());
    parser.feed_last(html.as_bytes());
    parser.finish()
}

fn source(text: &str) -> StyleSource {
    StyleSource {
        owner: None,
        url: url::Url::parse("https://example.test/a.css").unwrap(),
        text: text.to_owned(),
        media: None,
        disabled: false,
        imports: Vec::new(),
    }
}

#[test]
fn an_unchanged_flush_returns_none_without_traversing() {
    let (doc, mut engine) = styled(PAGE);
    assert_eq!(engine.traversal_count(), 1);
    assert_eq!(engine.flush(&doc), StyleDiff::None);
    assert_eq!(engine.flush(&doc), StyleDiff::None);
    assert_eq!(engine.traversal_count(), 1);
    assert_eq!(value(&doc, &engine, "a", "color"), "rgb(1, 0, 0)");
}

#[test]
fn a_mutation_restyles_the_whole_tree_and_reports_full() {
    let (mut doc, mut engine) = styled(PAGE);
    assert_eq!(value(&doc, &engine, "a", "color"), "rgb(1, 0, 0)");
    let a = element(&doc, "a");
    set_attribute(&mut doc, a, "class", "on");
    // The root's data is current from the first flush, so unless the flush
    // discards it Stylo would skip the whole tree and keep the old colour.
    assert_eq!(engine.flush(&doc), StyleDiff::Full);
    assert_eq!(engine.traversal_count(), 2);
    assert_eq!(value(&doc, &engine, "a", "color"), "rgb(2, 0, 0)");
    assert_eq!(value(&doc, &engine, "b", "color"), "rgb(2, 0, 0)");

    set_attribute(&mut doc, a, "class", "");
    assert_eq!(engine.flush(&doc), StyleDiff::Full);
    assert_eq!(value(&doc, &engine, "a", "color"), "rgb(1, 0, 0)");
    assert_eq!(engine.flush(&doc), StyleDiff::None);
}

#[test]
fn a_style_attribute_edit_is_picked_up() {
    let (mut doc, mut engine) = styled(PAGE);
    let a = element(&doc, "a");
    set_attribute(&mut doc, a, "style", "color: rgb(9, 9, 9)");
    engine.flush(&doc);
    assert_eq!(value(&doc, &engine, "a", "color"), "rgb(9, 9, 9)");
    set_attribute(&mut doc, a, "style", "color: rgb(8, 8, 8)");
    engine.flush(&doc);
    assert_eq!(value(&doc, &engine, "a", "color"), "rgb(8, 8, 8)");
}

#[test]
fn replacing_the_sources_restyles_an_unchanged_document() {
    let (doc, mut engine) = styled(PAGE);
    assert_eq!(value(&doc, &engine, "outer", "font-size"), "20px");
    engine.set_sources(vec![source("div { font-size: 30px }")]);
    assert_eq!(engine.flush(&doc), StyleDiff::Full);
    assert_eq!(value(&doc, &engine, "outer", "font-size"), "30px");
    assert_eq!(
        value(&doc, &engine, "a", "font-size"),
        "30px",
        "descendants are recomputed too"
    );
    assert_eq!(value(&doc, &engine, "a", "color"), "rgb(0, 0, 0)");
    assert_eq!(engine.sources().len(), 1);
}

#[test]
fn changing_the_environment_restyles_an_unchanged_document() {
    let html = "<!DOCTYPE html><html><head><style>\
        @media (min-width: 600px) { div { width: 50px } }</style></head>\
        <body><div id=d></div></body></html>";
    let (doc, mut engine) = styled(html);
    assert_eq!(value(&doc, &engine, "d", "width"), "50px");
    engine.set_env(StyleEnv {
        viewport: (400.0, 300.0),
        ..engine.env().clone()
    });
    assert_eq!(engine.flush(&doc), StyleDiff::Full);
    assert_eq!(value(&doc, &engine, "d", "width"), "auto");
    assert_eq!(engine.flush(&doc), StyleDiff::None);
}

#[test]
fn the_document_decides_the_quirks_mode() {
    let doc = parse("<!DOCTYPE html><html><body><p id=p style='width: 10'>x</p></body></html>");
    let quirky = StyleEnv {
        quirks: true,
        ..StyleEnv::default()
    };
    let (_, mut engine) = style_html("", quirky.clone());
    assert!(engine.env().quirks, "an empty document has no doctype");
    engine.flush(&doc);
    assert!(!engine.env().quirks);
    assert_eq!(value(&doc, &engine, "p", "width"), "auto");
}

#[test]
fn elements_inserted_after_a_flush_are_styled_by_the_next_one() {
    let (mut doc, mut engine) = styled(PAGE);
    let outer = element(&doc, "outer");
    let fresh = doc
        .create_element(
            tenta_html::QualName::new(
                tenta_core::atom::atoms::NS_HTML,
                tenta_core::atom::Atom::new("p"),
            ),
            Vec::new(),
            tenta_html::ElementFlags::default(),
        )
        .unwrap();
    assert!(engine.style(fresh).is_none());
    doc.mutate(tenta_dom::Mutation::Append {
        parent: outer,
        child: fresh,
    })
    .unwrap();
    assert_eq!(engine.flush(&doc), StyleDiff::Full);
    let style = engine.style(fresh).expect("the new element is styled");
    assert_eq!(computed(&style, "color"), "rgb(1, 0, 0)");
    assert_eq!(computed(&style, "font-size"), "20px");
}

/// Records which thread asks for font metrics and fails on request.
#[derive(Default)]
struct Probe {
    fail: AtomicBool,
    threads: Mutex<Vec<ThreadId>>,
}

impl FontMetrics for Probe {
    fn metrics(&self, _: &str, _: u16, _: bool, size_px: f32) -> FontMetricsData {
        self.threads.lock().unwrap().push(thread::current().id());
        assert!(
            !self.fail.load(Ordering::SeqCst),
            "the metrics source failed"
        );
        FontMetricsData {
            x_height: Some(size_px * 0.5),
            ascent: size_px,
            ..FontMetricsData::default()
        }
    }
}

fn ex_page(count: usize) -> String {
    let items: String = (0..count)
        .map(|i| format!("<div id=d{i} style='width: {}ex'></div>", i + 1))
        .collect();
    format!("<!DOCTYPE html><html><body>{items}</body></html>")
}

#[test]
fn the_traversal_runs_on_the_calling_thread() {
    let probe = Arc::new(Probe::default());
    let mut engine = StyleEngine::new(StyleEnv::default(), probe.clone());
    let doc = parse(&ex_page(200));
    engine.flush(&doc);
    assert_eq!(value(&doc, &engine, "d19", "width"), "160px");
    let threads = probe.threads.lock().unwrap();
    assert!(!threads.is_empty());
    assert!(threads.iter().all(|&id| id == thread::current().id()));
}

#[test]
fn a_panic_in_the_traversal_is_cleaned_up_after_and_the_next_flush_works() {
    let probe = Arc::new(Probe::default());
    let mut engine = StyleEngine::new(StyleEnv::default(), probe.clone());
    let doc = parse(&ex_page(10));

    probe.fail.store(true, Ordering::SeqCst);
    let outcome = catch_unwind(AssertUnwindSafe(|| engine.flush(&doc)));
    assert!(outcome.is_err(), "the panic is handed back to the caller");
    assert!(
        !thread_state::get().contains(ThreadState::LAYOUT),
        "the layout state is left on unwind"
    );
    assert!(
        (0..doc.slots() as u32).all(|slot| engine.style(NodeId(slot)).is_none()),
        "no half-computed style is served"
    );

    probe.fail.store(false, Ordering::SeqCst);
    assert_eq!(
        engine.flush(&doc),
        StyleDiff::Full,
        "an unmutated document is restyled after a failed flush"
    );
    assert_eq!(value(&doc, &engine, "d4", "width"), "40px");
    assert_eq!(engine.flush(&doc), StyleDiff::None);

    // An unchanged document is not traversed, so the failing source is not asked.
    probe.fail.store(true, Ordering::SeqCst);
    assert_eq!(engine.flush(&doc), StyleDiff::None);
    // A second failure: the sharing cache was left empty by the first.
    engine.set_env(StyleEnv {
        viewport: (1.0, 1.0),
        ..engine.env().clone()
    });
    assert!(catch_unwind(AssertUnwindSafe(|| engine.flush(&doc))).is_err());
    probe.fail.store(false, Ordering::SeqCst);
    engine.flush(&doc);
    assert_eq!(value(&doc, &engine, "d9", "width"), "80px");
}

#[test]
fn every_kind_of_style_input_is_read_under_the_engines_one_lock() {
    // Inline style, a presentational hint, the UA sheet and an author sheet
    // are all read in one traversal; Stylo panics if their locks differ.
    let (doc, engine) = styled(
        "<!DOCTYPE html><html><head><style>p { margin-left: 4px }</style></head>\
         <body text='#00ff00'><p id=p style='margin-top: 2px'>x</p></body></html>",
    );
    assert_eq!(value(&doc, &engine, "p", "margin-left"), "4px");
    assert_eq!(value(&doc, &engine, "p", "margin-top"), "2px");
    assert_eq!(value(&doc, &engine, "p", "color"), "rgb(0, 255, 0)");
}

const FOCUS_PAGE: &str = "<!DOCTYPE html><html><head><style>\
    input { color: rgb(1, 0, 0) } input:focus { color: rgb(2, 0, 0) }\
    input:focus-visible { background-color: rgb(4, 0, 0) } #wrap:focus-within { color: rgb(3, 0, 0) }\
    </style></head><body><div id=wrap><input id=f><input id=g></div></body></html>";

#[test]
fn focus_moves_the_focus_pseudo_classes_in_the_computed_styles() {
    let (mut doc, mut engine) = styled(FOCUS_PAGE);
    assert_eq!(value(&doc, &engine, "f", "color"), "rgb(1, 0, 0)");
    assert_eq!(value(&doc, &engine, "wrap", "color"), "rgb(0, 0, 0)");

    let f = element(&doc, "f");
    doc.set_focused_element(Some(f), true);
    assert_eq!(engine.flush(&doc), StyleDiff::Full);
    assert_eq!(value(&doc, &engine, "f", "color"), "rgb(2, 0, 0)");
    assert_eq!(value(&doc, &engine, "g", "color"), "rgb(1, 0, 0)");
    assert_eq!(
        value(&doc, &engine, "f", "background-color"),
        "rgb(4, 0, 0)"
    );
    assert_eq!(value(&doc, &engine, "wrap", "color"), "rgb(3, 0, 0)");

    doc.set_focused_element(Some(f), false);
    assert_eq!(engine.flush(&doc), StyleDiff::Full);
    assert_eq!(value(&doc, &engine, "f", "color"), "rgb(2, 0, 0)");
    assert_eq!(
        value(&doc, &engine, "f", "background-color"),
        "rgb(255, 255, 255)"
    );

    doc.set_focused_element(None, false);
    assert_eq!(engine.flush(&doc), StyleDiff::Full);
    assert_eq!(value(&doc, &engine, "f", "color"), "rgb(1, 0, 0)");
    assert_eq!(value(&doc, &engine, "wrap", "color"), "rgb(0, 0, 0)");
}
