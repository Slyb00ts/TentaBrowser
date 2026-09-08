//! The `TreeSink` implementation (`docs/spec/04-dom-and-css.md` §3.3).
//!
//! The sink is driven directly here, exactly as the tree builder drives it,
//! so the contract is tested without the parser in between. End-to-end parses
//! live in `parse_integration.rs`; the golden tests in `extract_golden.rs`,
//! `selectors.rs` and `serialize.rs` all run through `tenta_html::Parser`.

mod support;

use tenta_core::atom::atoms;
use tenta_core::{Atom, NodeId};
use tenta_dom::{Document, NodeKind};
use tenta_html::{
    Attribute, ElementFlags, NodeOrText, ParseControl, QualName, QuirksMode, TreeSink,
};

fn element(document: &mut Document, local: &str, attrs: &[(&str, &str)]) -> NodeId {
    TreeSink::create_element(
        document,
        QualName::new(atoms::NS_HTML, Atom::new(local)),
        attrs
            .iter()
            .map(|(name, value)| Attribute {
                name: QualName::new(atoms::EMPTY, Atom::new(name)),
                value: (*value).to_owned(),
            })
            .collect(),
        ElementFlags::default(),
    )
}

#[test]
fn the_sink_builds_a_document_the_way_the_tree_builder_will() {
    let mut document = Document::new();
    let root = TreeSink::document(&mut document);
    document.append_doctype_to_document("html", "", "");
    let html = element(&mut document, "html", &[("lang", "en")]);
    document.append(&root, NodeOrText::Node(html));
    let head = element(&mut document, "head", &[]);
    document.append(&html, NodeOrText::Node(head));
    let body = element(&mut document, "body", &[]);
    document.append(&html, NodeOrText::Node(body));
    let paragraph = element(&mut document, "p", &[]);
    document.append(&body, NodeOrText::Node(paragraph));
    document.append(&paragraph, NodeOrText::Text("Hello "));
    document.append(&paragraph, NodeOrText::Text("world"));

    assert_eq!(document.document_element(), Some(html));
    assert_eq!(document.head(), Some(head));
    assert_eq!(document.body(), Some(body));
    assert_eq!(
        document.children(paragraph).count(),
        1,
        "the sink merges adjacent text runs"
    );
    assert_eq!(document.text_content(paragraph), "Hello world");
    assert_eq!(
        TreeSink::element_name(&document, &html),
        QualName::new(atoms::NS_HTML, atoms::HTML)
    );
    assert_eq!(
        document.doctype(document.first_child(root)).unwrap().name,
        "html"
    );
}

#[test]
fn append_before_sibling_foster_parents() {
    let mut document = Document::new();
    let root = TreeSink::document(&mut document);
    let table = element(&mut document, "table", &[]);
    document.append(&root, NodeOrText::Node(table));
    // The tree builder foster-parents misnested content before the table.
    document.append_before_sibling(&table, NodeOrText::Text("stray"));
    let orphan = element(&mut document, "b", &[]);
    document.append_before_sibling(&table, NodeOrText::Node(orphan));

    let children: Vec<NodeId> = document.children(root).collect();
    assert_eq!(children.len(), 3);
    assert_eq!(document.kind(children[0]), NodeKind::Text);
    assert_eq!(document.text_data(children[0]), Some("stray"));
    assert_eq!(children[1], orphan);
    assert_eq!(children[2], table);
}

#[test]
fn append_based_on_parent_node_follows_the_template_rule() {
    let mut document = Document::new();
    let root = TreeSink::document(&mut document);
    let parented = element(&mut document, "div", &[]);
    let previous = element(&mut document, "section", &[]);
    document.append(&root, NodeOrText::Node(parented));
    document.append(&root, NodeOrText::Node(previous));

    let a = element(&mut document, "i", &[]);
    document.append_based_on_parent_node(&parented, &previous, NodeOrText::Node(a));
    assert_eq!(
        document.next_sibling(a),
        parented,
        "inserted before the element"
    );

    let orphan = element(&mut document, "div", &[]);
    let b = element(&mut document, "u", &[]);
    document.append_based_on_parent_node(&orphan, &previous, NodeOrText::Node(b));
    assert_eq!(
        document.parent(b),
        previous,
        "fell back to the previous element"
    );
}

#[test]
fn add_attrs_if_missing_never_overwrites() {
    let mut document = Document::new();
    let html = element(&mut document, "html", &[("lang", "en")]);
    document.add_attrs_if_missing(
        &html,
        vec![
            Attribute {
                name: QualName::new(atoms::EMPTY, atoms::LANG),
                value: "fr".to_owned(),
            },
            Attribute {
                name: QualName::new(atoms::EMPTY, atoms::CLASS),
                value: "added".to_owned(),
            },
        ],
    );
    assert_eq!(document.attribute(html, atoms::LANG), Some("en"));
    assert_eq!(document.attribute(html, atoms::CLASS), Some("added"));
}

#[test]
fn reparent_children_and_remove_from_parent() {
    let mut document = Document::new();
    let root = TreeSink::document(&mut document);
    let from = element(&mut document, "div", &[]);
    let to = element(&mut document, "div", &[]);
    document.append(&root, NodeOrText::Node(from));
    document.append(&root, NodeOrText::Node(to));
    let child = element(&mut document, "span", &[]);
    document.append(&from, NodeOrText::Node(child));

    document.reparent_children(&from, &to);
    assert_eq!(document.parent(child), to);

    document.remove_from_parent(&to);
    assert!(!document.parent(to).is_some());
    assert!(!document.is_connected(child));
}

#[test]
fn template_contents_are_a_separate_fragment() {
    let mut document = Document::new();
    let template = TreeSink::create_element(
        &mut document,
        QualName::new(atoms::NS_HTML, atoms::TEMPLATE),
        Vec::new(),
        ElementFlags {
            template: true,
            ..ElementFlags::default()
        },
    );
    let contents = document.get_template_contents(&template);
    assert_eq!(document.kind(contents), NodeKind::Fragment);
    let inside = element(&mut document, "p", &[]);
    document.append(&contents, NodeOrText::Node(inside));
    assert_eq!(document.children(template).count(), 0);
    assert_eq!(
        document.children(contents).collect::<Vec<_>>(),
        vec![inside]
    );
    assert_eq!(document.template_contents(template), Some(contents));
    assert_eq!(document.shadow_root(template), None);
}

#[test]
fn quirks_mode_and_parse_errors_are_recorded() {
    let mut document = Document::new();
    assert_eq!(document.quirks_mode(), QuirksMode::NoQuirks);
    TreeSink::set_quirks_mode(&mut document, QuirksMode::Quirks);
    assert_eq!(document.quirks_mode(), QuirksMode::Quirks);

    TreeSink::set_current_line(&mut document, 12);
    assert_eq!(document.parse_line(), 12);

    // Removing a node from nothing is fine; inserting into a text node is not.
    let text = document.create_text("x");
    let child = element(&mut document, "p", &[]);
    document.append(&text, NodeOrText::Node(child));
    assert_eq!(document.parse_errors().len(), 1);
    assert!(document.parse_errors()[0].contains("HierarchyRequestError"));
}

#[test]
fn mathml_integration_points_are_reported_back() {
    let mut document = Document::new();
    let plain = element(&mut document, "annotation-xml", &[]);
    assert!(!document.is_mathml_annotation_xml_integration_point(&plain));
    let integration = TreeSink::create_element(
        &mut document,
        QualName::new(atoms::NS_MATHML, Atom::new("annotation-xml")),
        Vec::new(),
        ElementFlags {
            mathml_annotation_xml_integration_point: true,
            ..ElementFlags::default()
        },
    );
    assert!(document.is_mathml_annotation_xml_integration_point(&integration));
}

#[test]
fn script_already_started_is_recorded() {
    let mut document = Document::new();
    let script = element(&mut document, "script", &[]);
    assert!(!document.script_already_started(script));
    document.mark_script_already_started(&script);
    assert!(document.script_already_started(script));
}

#[test]
fn pop_stops_the_parse_after_the_search_container_closes() {
    // The `search` fast path stops as soon as the results container closes
    // (`docs/spec/08-search.md` §3).
    let mut document = Document::with_url("https://example.test/search?q=rust".parse().unwrap());
    document.set_stop_after(Some(Box::new(|document: &Document, node: NodeId| {
        document.tag(node) == Some(atoms::DIV) && document.element_id(node) == Some("links")
    })));

    support::mini_html::parse_into(
        &mut document,
        "<html><body><div id=\"head\">before</div><div id=\"links\"><a href=\"/a\">A</a></div>\
         <div id=\"after\">never parsed</div></body></html>",
    );

    assert!(
        document.parse_stopped(),
        "the predicate must stop the parse"
    );
    assert!(document.get_element_by_id("links").is_some());
    assert!(
        document.get_element_by_id("after").is_none(),
        "nothing after the container is built"
    );
    assert!(!document.text().contains("never parsed"));
    assert_eq!(document.links().len(), 1);
}

#[test]
fn pop_without_a_predicate_always_continues() {
    let mut document = Document::new();
    let node = element(&mut document, "div", &[]);
    assert_eq!(document.pop(&node), ParseControl::Continue);
    assert!(!document.parse_stopped());
}

#[test]
fn a_selected_option_is_mirrored_into_selectedcontent() {
    // Customizable <select>: the parser reports the option's close and the
    // sink clones its children into the <selectedcontent>, so the mirroring is
    // already done by the time the parse finishes.
    let mut document = support::parse(
        "<html><body><select><button><selectedcontent></selectedcontent></button>\
         <option value=\"a\">Plain</option>\
         <option value=\"b\" selected>Rich <b>text</b></option></select></body></html>",
    );
    let selected_content = document
        .query_selector(document.document_node(), "selectedcontent")
        .unwrap()
        .expect("selectedcontent");
    assert_eq!(document.text_content(selected_content), "Rich text");
    assert_eq!(document.inner_html(selected_content), "Rich <b>text</b>");

    let option = document
        .query_selector(document.document_node(), "option[value=b]")
        .unwrap()
        .expect("the selected option");
    assert_eq!(
        document.children(option).count(),
        2,
        "the option keeps its own children"
    );

    // An option that is not selected changes nothing.
    let other = document
        .query_selector(document.document_node(), "option[value=a]")
        .unwrap()
        .expect("the other option");
    document.maybe_clone_option_into_selectedcontent(&other);
    assert_eq!(document.text_content(selected_content), "Rich text");

    // The hook is idempotent: re-running it for the selected option replaces
    // the mirrored children rather than appending a second copy.
    document.maybe_clone_option_into_selectedcontent(&option);
    assert_eq!(document.text_content(selected_content), "Rich text");
}

#[test]
fn cloning_copies_attributes_and_shares_text_chunks() {
    let mut document = support::parse(
        "<html><body><div id=\"a\" class=\"x y\"><p>text <em>here</em></p></div></body></html>",
    );
    let source = document.get_element_by_id("a").expect("the div");

    let shallow = document.shallow_clone(source).expect("clone");
    assert_eq!(
        document.attribute(shallow, tenta_core::atom::atoms::ID),
        Some("a")
    );
    assert_eq!(document.class_count(shallow), 2);
    assert_eq!(document.children(shallow).count(), 0);
    assert!(!document.is_connected(shallow));

    let deep = document.clone_node(source, true).expect("clone");
    assert_eq!(document.outer_html(deep), document.outer_html(source));
    assert_ne!(deep, source);

    // The clone is a detached root and is swept once nothing holds it.
    let before = document.len();
    let stats = document.sweep_detached();
    assert!(stats.nodes_freed >= 2);
    assert!(document.len() < before);
}
