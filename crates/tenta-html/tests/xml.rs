//! XML parsing: namespaces, entities, CDATA, processing instructions and
//! well-formedness errors.

mod common;

use common::tree::{NodeData, Tree, serialize};
use tenta_html::sink::TreeSink;
use tenta_html::xml::{Parser, XmlOptions};

fn parse(text: &str) -> Result<Tree, tenta_html::xml::XmlError> {
    let mut parser = Parser::new(Tree::new(), XmlOptions::default());
    parser.feed_str(text);
    parser.finish()
}

fn parse_bytes(bytes: &[u8]) -> Result<Tree, tenta_html::xml::XmlError> {
    let mut parser = Parser::new(Tree::new(), XmlOptions::default());
    parser.feed(bytes);
    parser.finish()
}

fn tree_of(text: &str) -> String {
    let tree = parse(text).expect("well-formed document");
    serialize(&tree, 0, true)
}

#[test]
fn elements_text_and_attributes() {
    assert_eq!(
        tree_of(r#"<root a="1" b='2'>text</root>"#),
        "| <root>\n|   a=\"1\"\n|   b=\"2\"\n|   \"text\"\n"
    );
}

#[test]
fn namespaces_bind_elements_and_attributes() {
    let tree = parse(
        r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">
  <a xlink:href="#target" id="link"/>
</svg>"##,
    )
    .expect("well-formed document");

    let root = tree.nodes[0].children[0];
    let name = tree.element_name(&root);
    assert_eq!(name.ns.as_str(), "http://www.w3.org/2000/svg");
    assert_eq!(name.local.as_str(), "svg");
    assert!(name.prefix.is_none());

    let anchor = tree.nodes[root]
        .children
        .iter()
        .copied()
        .find(|&node| matches!(tree.nodes[node].data, NodeData::Element { .. }))
        .expect("element child");
    let NodeData::Element { name, attrs, .. } = &tree.nodes[anchor].data else {
        unreachable!()
    };
    // The default namespace is inherited by the element but never by an
    // unprefixed attribute.
    assert_eq!(name.ns.as_str(), "http://www.w3.org/2000/svg");
    let href = &attrs[0];
    assert_eq!(href.name.ns.as_str(), "http://www.w3.org/1999/xlink");
    assert_eq!(href.name.prefix.map(|p| p.as_str()), Some("xlink"));
    assert_eq!(href.name.local.as_str(), "href");
    assert_eq!(href.value, "#target");
    let id = &attrs[1];
    assert_eq!(id.name.ns.as_str(), "");
    assert!(id.name.prefix.is_none());
}

#[test]
fn namespace_scope_ends_with_the_element() {
    let error = parse("<r xmlns:p=\"urn:x\"><p:a/></r><p:b/>").unwrap_err();
    assert!(error.message.contains("root"), "{error}");

    let error = parse("<r><b xmlns:p=\"urn:x\"><p:a/></b><p:c/></r>").unwrap_err();
    assert_eq!(error.message, "unbound namespace prefix p");
}

#[test]
fn xml_prefix_is_predefined() {
    let tree = parse(r#"<r xml:lang="pl"/>"#).expect("well-formed document");
    let root = tree.nodes[0].children[0];
    let NodeData::Element { attrs, .. } = &tree.nodes[root].data else {
        unreachable!()
    };
    assert_eq!(
        attrs[0].name.ns.as_str(),
        "http://www.w3.org/XML/1998/namespace"
    );
}

#[test]
fn predefined_and_numeric_entities() {
    assert_eq!(
        tree_of("<r>&lt;&gt;&amp;&quot;&apos;&#65;&#x1F600;</r>"),
        "| <r>\n|   \"<>&\"'A\u{1F600}\"\n"
    );
    assert_eq!(tree_of(r#"<r a="&lt;&#38;"/>"#), "| <r>\n|   a=\"<&\"\n");
}

#[test]
fn undeclared_entity_is_an_error() {
    let error = parse("<r>&nbsp;</r>").unwrap_err();
    assert_eq!(error.message, "undeclared entity &nbsp;");
    assert_eq!((error.line, error.column), (1, 4));
}

#[test]
fn cdata_is_text() {
    assert_eq!(
        tree_of("<r><![CDATA[a < b & <tag>]]>c</r>"),
        "| <r>\n|   \"a < b & <tag>c\"\n"
    );
}

#[test]
fn comments_and_processing_instructions() {
    assert_eq!(
        tree_of("<?target data?><!-- hi --><r><?pi?></r>"),
        "| <?target data?>\n| <!--  hi  -->\n| <r>\n|   <?pi ?>\n"
    );
}

#[test]
fn doctype_with_an_internal_subset_is_skipped() {
    let tree = parse(
        "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd\" [ <!ENTITY x \"y\"> ]>\n<html xmlns=\"http://www.w3.org/1999/xhtml\"/>",
    )
    .expect("well-formed document");
    let NodeData::Doctype {
        name,
        public_id,
        system_id,
    } = &tree.nodes[tree.nodes[0].children[0]].data
    else {
        panic!("expected a doctype");
    };
    assert_eq!(name, "html");
    assert_eq!(public_id, "-//W3C//DTD XHTML 1.0 Strict//EN");
    assert_eq!(
        system_id,
        "http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd"
    );
}

#[test]
fn newlines_are_normalized() {
    let tree = parse("<r a=\"x\r\ny\">a\r\nb\rc</r>").expect("well-formed document");
    let root = tree.nodes[0].children[0];
    let NodeData::Element { attrs, .. } = &tree.nodes[root].data else {
        unreachable!()
    };
    assert_eq!(attrs[0].value, "x y");
    let NodeData::Text(text) = &tree.nodes[tree.nodes[root].children[0]].data else {
        unreachable!()
    };
    assert_eq!(text, "a\nb\nc");
}

#[test]
fn well_formedness_errors_carry_a_position() {
    let cases: &[(&str, &str, u64, u64)] = &[
        ("<a></b>", "</b> does not close <a>", 1, 4),
        ("<a>", "end of input inside <a>", 1, 4),
        ("</a>", "</a> without a start tag", 1, 1),
        ("<a/><b/>", "more than one root element", 1, 5),
        ("   ", "no root element", 1, 4),
        ("text<a/>", "text outside the root element", 1, 1),
        ("<a x=\"1\" x=\"2\"/>", "duplicate attribute x", 1, 10),
        ("<a x=1/>", "attribute value must be quoted", 1, 6),
        ("<a x/>", "attribute without a value", 1, 5),
        ("<a>&#xZ;</a>", "invalid character reference &#xZ;", 1, 4),
        ("<a>]]></a>", "\"]]>\" in character data", 1, 4),
        ("<a><!-- -- --></a>", "\"--\" inside a comment", 1, 4),
        (
            "<a><?xml v?></a>",
            "\"xml\" is a reserved processing instruction target",
            1,
            4,
        ),
        ("<a b=\"<\"/>", "\"<\" in an attribute value", 1, 7),
        ("<a>\n<b>&x;</b>\n</a>", "undeclared entity &x;", 2, 4),
    ];
    for (input, message, line, column) in cases {
        let error = parse(input).unwrap_err();
        assert_eq!(&error.message, message, "input {input:?}");
        assert_eq!(
            (error.line, error.column),
            (*line, *column),
            "input {input:?}"
        );
    }
}

#[test]
fn xhtml_document_builds_the_html_namespace() {
    let tree = parse(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>T</title></head>
<body><p class="x">hi</p></body>
</html>"#,
    )
    .expect("well-formed document");
    let root = tree.nodes[0].children[0];
    assert_eq!(
        tree.element_name(&root).ns.as_str(),
        "http://www.w3.org/1999/xhtml"
    );
    assert_eq!(tree.element_count(), 5);
}

#[test]
fn the_xml_declaration_selects_the_encoding() {
    let mut bytes: Vec<u8> = br#"<?xml version="1.0" encoding="ISO-8859-2"?><r>"#.to_vec();
    // "Ł" is 0xA3 in ISO-8859-2 and would be invalid UTF-8.
    bytes.push(0xA3);
    bytes.extend_from_slice(b"</r>");
    let tree = parse_bytes(&bytes).expect("well-formed document");
    let root = tree.nodes[0].children[0];
    let NodeData::Text(text) = &tree.nodes[tree.nodes[root].children[0]].data else {
        unreachable!()
    };
    assert_eq!(text, "Ł");
}

#[test]
fn a_byte_order_mark_selects_the_encoding() {
    let mut bytes: Vec<u8> = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice("<r>ą</r>".as_bytes());
    let tree = parse_bytes(&bytes).expect("well-formed document");
    let root = tree.nodes[0].children[0];
    let NodeData::Text(text) = &tree.nodes[tree.nodes[root].children[0]].data else {
        unreachable!()
    };
    assert_eq!(text, "ą");
}

#[test]
fn the_sink_can_stop_the_parse() {
    let mut sink = Tree::new();
    sink.stop_on = Some(("stop".to_owned(), "here".to_owned()));
    let mut parser = Parser::new(sink, XmlOptions::default());
    parser.feed_str(r#"<r><stop id="here">a</stop><after/></r>"#);
    let tree = parser.finish().expect("well-formed document");
    assert!(tree.pops_after_stop.is_empty());
    assert_eq!(tree.element_count(), 2);
}
