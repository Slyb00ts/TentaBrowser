//! `:read-write`, `:placeholder-shown`, `:enabled`, `:link` and the
//! case-insensitive HTML attribute values, through `querySelectorAll`.

use tenta_dom::Document;
use tenta_html::{Parser, ParserOptions};

const PAGE: &str = "<!DOCTYPE html>\
    <input id=t1><input id=t2 type=TEXT placeholder=p><input id=cb type=checkbox placeholder=p>\
    <input id=ro type=text readonly placeholder=p><input id=dis type=text disabled>\
    <input id=num type=number placeholder=p value=3><input id=dt type=date placeholder=p>\
    <textarea id=ta placeholder=p></textarea><textarea id=ta2 placeholder=p>hi</textarea>\
    <div id=ce contenteditable></div><div id=cef contenteditable=false></div>\
    <select><optgroup id=og disabled><option id=o1>a</option></optgroup><option id=o2>b</option>\
    </select><a id=al href=x></a><area id=ar href=y><link id=lk href=z><a id=nl></a>";

fn ids(doc: &Document, selector: &str) -> Vec<String> {
    doc.query_selector_all(doc.document_node(), selector)
        .unwrap()
        .into_iter()
        .filter_map(|node| doc.element_id(node).map(str::to_owned))
        .collect()
}

fn page() -> Document {
    let mut parser = Parser::new(Document::new(), ParserOptions::default());
    parser.feed_last(PAGE.as_bytes());
    parser.finish()
}

#[test]
fn read_write_is_a_mutable_text_control_or_an_editing_host() {
    let doc = page();
    assert_eq!(
        ids(&doc, "[id]:read-write"),
        ["t1", "t2", "num", "dt", "ta", "ta2", "ce"]
    );
    let read_only = ids(&doc, "[id]:read-only");
    for id in ["cb", "ro", "dis", "cef", "al"] {
        assert!(read_only.contains(&id.to_owned()), "{id} is read-only");
    }
    assert!(!read_only.contains(&"ce".to_owned()));
}

#[test]
fn placeholder_shows_only_on_text_entry_with_an_empty_value() {
    let doc = page();
    assert_eq!(ids(&doc, ":placeholder-shown"), ["t2", "ro", "ta"]);
}

#[test]
fn options_take_part_in_enabled_and_disabled() {
    let doc = page();
    assert_eq!(ids(&doc, ":disabled"), ["dis", "og", "o1"]);
    assert_eq!(ids(&doc, "option:enabled"), ["o2"]);
}

#[test]
fn only_a_and_area_with_href_are_links() {
    let doc = page();
    assert_eq!(ids(&doc, ":link"), ["al", "ar"]);
    assert_eq!(ids(&doc, ":any-link"), ["al", "ar"]);
}

#[test]
fn the_type_attribute_value_is_ascii_case_insensitive() {
    let doc = page();
    assert_eq!(ids(&doc, "input[type=text]"), ["t2", "ro", "dis"]);
}

#[test]
fn script_state_overrides_the_attributes() {
    let mut doc = page();
    let ta = doc.get_element_by_id("ta").unwrap();
    doc.set_dirty_value(ta, "typed".to_owned());
    let cb = doc.get_element_by_id("cb").unwrap();
    doc.set_dirty_checkedness(cb, true);
    assert_eq!(ids(&doc, ":placeholder-shown"), ["t2", "ro"]);
    assert_eq!(ids(&doc, "input:checked"), ["cb"]);
}
