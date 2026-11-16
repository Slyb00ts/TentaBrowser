//! The live form model and form submission (`tenta_dom::forms`), over
//! documents the HTML parser built.

use tenta_core::NodeId;
use tenta_dom::forms::{self, Entry};
use tenta_dom::{Document, NavigationRequest};
use tenta_html::{ParseStatus, Parser, ParserOptions};

fn parse(html: &str) -> Document {
    let document = Document::with_url(
        "https://example.com/app/page.html?old=1#frag"
            .parse()
            .expect("the test URL"),
    );
    let mut parser = Parser::new(document, ParserOptions::default());
    assert_eq!(parser.feed(html.as_bytes()), ParseStatus::Continue);
    parser.finish()
}

fn by_id(doc: &Document, id: &str) -> NodeId {
    doc.get_element_by_id(id)
        .unwrap_or_else(|| panic!("no element with id {id}"))
}

fn pairs(entries: &[Entry]) -> Vec<(&str, &str)> {
    entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.value.as_str()))
        .collect()
}

const FORM: &str = r#"<!doctype html><body>
<form id="f" action="/submit" method="post">
  <input name="user" value="ada">
  <input name="password" type="password" value="secret">
  <input name="remember" type="checkbox" checked>
  <input name="news" type="checkbox">
  <input name="plan" type="radio" value="free">
  <input name="plan" type="radio" value="pro" checked>
  <input name="token" type="hidden" value="t0k">
  <input name="off" value="x" disabled>
  <input value="no name">
  <textarea name="note">line one
line two</textarea>
  <select name="color"><option value="r">Red<option value="g" selected>Green<option>Blue</select>
  <select name="tags" multiple><option selected>a<option>b<option selected>c</select>
  <fieldset disabled><legend><input name="in-legend" value="kept"></legend>
    <input name="in-fieldset" value="dropped"></fieldset>
  <input name="upload" type="file">
  <button id="save" name="action" value="save">Save</button>
  <button id="draft" name="action" value="draft" formaction="/draft" formmethod="get">Draft</button>
  <input id="img" type="image" name="pos">
  <button type="button" name="plain" value="never">Plain</button>
</form>
<input id="outside" name="outside" value="by form attribute" form="f">
<form id="other"><input name="mine" value="1"></form>
</body>"#;

#[test]
fn the_entry_list_holds_the_successful_controls_in_tree_order() {
    let doc = parse(FORM);
    let form = by_id(&doc, "f");
    let entries = forms::entry_list(&doc, form, Some(by_id(&doc, "save")));
    assert_eq!(
        pairs(&entries),
        [
            ("user", "ada"),
            ("password", "secret"),
            ("remember", "on"),
            ("plan", "pro"),
            ("token", "t0k"),
            ("note", "line one\r\nline two"),
            ("color", "g"),
            ("tags", "a"),
            ("tags", "c"),
            ("in-legend", "kept"),
            ("upload", ""),
            ("action", "save"),
            ("outside", "by form attribute"),
        ]
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.file && entry.name == "upload")
    );

    // Without a submitter no button contributes; an image button gives its
    // click coordinates.
    let none = forms::entry_list(&doc, form, None);
    assert!(!pairs(&none).iter().any(|(name, _)| *name == "action"));
    let image = forms::entry_list(&doc, form, Some(by_id(&doc, "img")));
    assert!(pairs(&image).contains(&("pos.x", "0")) && pairs(&image).contains(&("pos.y", "0")));
}

#[test]
fn the_entry_list_reads_what_the_controls_hold_now() {
    let mut doc = parse(FORM);
    let form = by_id(&doc, "f");
    let controls = forms::form_controls(&doc, form);
    let named = |doc: &Document, name: &str, nth: usize| {
        controls
            .iter()
            .copied()
            .filter(|control| doc.attribute_str(*control, "name") == Some(name))
            .nth(nth)
            .unwrap()
    };
    let user = named(&doc, "user", 0);
    doc.set_dirty_value(user, "grace".to_owned());
    let news = named(&doc, "news", 0);
    doc.set_dirty_checkedness(news, true);
    let remember = named(&doc, "remember", 0);
    doc.set_dirty_checkedness(remember, false);
    let color = named(&doc, "color", 0);
    forms::set_select_value(&mut doc, color, "Blue");
    let entries = forms::entry_list(&doc, form, None);
    let found = pairs(&entries);
    assert!(found.contains(&("user", "grace")), "{found:?}");
    assert!(found.contains(&("news", "on")), "{found:?}");
    assert!(
        !found.iter().any(|(name, _)| *name == "remember"),
        "{found:?}"
    );
    assert!(found.contains(&("color", "Blue")), "{found:?}");

    forms::reset(&mut doc, form);
    let found_after = forms::entry_list(&doc, form, None);
    let found_after = pairs(&found_after);
    assert!(found_after.contains(&("user", "ada")), "{found_after:?}");
    assert!(found_after.contains(&("remember", "on")), "{found_after:?}");
    assert!(found_after.contains(&("color", "g")), "{found_after:?}");
}

#[test]
fn a_single_select_has_one_selected_option() {
    let mut doc = parse(
        r#"<select id="plain"><option>a<option>b<option>c</select>
           <select id="two"><option selected>a<option selected>b<option>c</select>
           <select id="first-disabled"><option disabled>a<option>b</select>
           <select id="list" size="3"><option>a<option>b</select>
           <select id="grouped"><optgroup disabled><option>a</optgroup><option>b</select>"#,
    );
    let value = |doc: &Document, id: &str| forms::select_value(doc, by_id(doc, id));
    assert_eq!(value(&doc, "plain"), "a", "the first option is the default");
    assert_eq!(value(&doc, "two"), "b", "the last `selected` wins");
    assert_eq!(value(&doc, "first-disabled"), "b");
    assert_eq!(
        value(&doc, "list"),
        "",
        "a list box selects nothing by default"
    );
    assert_eq!(
        value(&doc, "grouped"),
        "b",
        "an option in a disabled group is disabled"
    );
    assert_eq!(forms::selected_index(&doc, by_id(&doc, "list")), -1);

    let plain = by_id(&doc, "plain");
    let options = forms::select_options(&doc, plain);
    forms::set_selected(&mut doc, options[2], true);
    assert_eq!(forms::select_value(&doc, plain), "c");
    assert_eq!(forms::selected_index(&doc, plain), 2);
    assert!(!forms::is_selected(&doc, options[0]));
    forms::set_selected(&mut doc, options[2], false);
    assert_eq!(
        forms::select_value(&doc, plain),
        "",
        "script deselected the only selected option"
    );
    forms::set_select_value(&mut doc, plain, "no such value");
    assert_eq!(forms::selected_index(&doc, plain), -1);
    forms::set_selected_index(&mut doc, plain, 1);
    assert_eq!(forms::select_value(&doc, plain), "b");
}

#[test]
fn a_get_submission_replaces_the_query_and_a_post_carries_a_body() {
    let doc = parse(FORM);
    let form = by_id(&doc, "f");

    let post = forms::submission(&doc, form, Some(by_id(&doc, "save")), "B").expect("a POST");
    assert_eq!(post.url.as_str(), "https://example.com/submit");
    let body = post.post.expect("a body");
    assert_eq!(body.content_type, "application/x-www-form-urlencoded");
    let text = String::from_utf8(body.body).unwrap();
    assert!(
        text.starts_with("user=ada&password=secret&remember=on&plan=pro&token=t0k&note=line+one%0D%0Aline+two&color=g"),
        "{text}"
    );
    assert!(
        text.ends_with("action=save&outside=by+form+attribute"),
        "{text}"
    );

    // The submitter's formaction and formmethod override the form's.
    let draft = forms::submission(&doc, form, Some(by_id(&doc, "draft")), "B").expect("a GET");
    assert_eq!(draft.post, None);
    assert_eq!(draft.url.path(), "/draft");
    assert!(draft.url.query().unwrap().contains("action=draft"));

    // A form without an action submits to the document's URL; GET replaces
    // the query and keeps the fragment.
    let other = forms::submission(&doc, by_id(&doc, "other"), None, "B").expect("a GET");
    assert_eq!(
        other,
        NavigationRequest::get(
            "https://example.com/app/page.html?mine=1#frag"
                .parse()
                .unwrap()
        )
    );
}

#[test]
fn multipart_and_text_plain_bodies() {
    let doc = parse(
        r#"<form id="m" method="post" enctype="multipart/form-data" action="/up">
             <input name="a" value="1"><input name='q"x' value="two
lines"><input type="file" name="f"></form>
           <form id="t" method="post" enctype="text/plain" action="/plain">
             <input name="a" value="1"><input name="b" value="2"></form>
           <form id="d" method="dialog"><input name="a"></form>
           <form id="blank" target="_blank"><input name="a"></form>
           <form id="mail" action="mailto:someone@example.com"><input name="a"></form>"#,
    );
    let multipart = forms::submission(&doc, by_id(&doc, "m"), None, "BOUNDARY")
        .and_then(|request| request.post)
        .expect("a multipart body");
    assert_eq!(
        multipart.content_type,
        "multipart/form-data; boundary=BOUNDARY"
    );
    assert_eq!(
        String::from_utf8(multipart.body).unwrap(),
        "--BOUNDARY\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n\
         --BOUNDARY\r\nContent-Disposition: form-data; name=\"q%22x\"\r\n\r\ntwo\r\nlines\r\n\
         --BOUNDARY\r\nContent-Disposition: form-data; name=\"f\"; filename=\"\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n\r\n\
         --BOUNDARY--\r\n"
    );
    let plain = forms::submission(&doc, by_id(&doc, "t"), None, "B")
        .and_then(|request| request.post)
        .expect("a text/plain body");
    assert_eq!(plain.content_type, "text/plain");
    assert_eq!(plain.body, b"a=1\r\nb=2\r\n");
    for id in ["d", "blank", "mail"] {
        assert_eq!(
            forms::submission(&doc, by_id(&doc, id), None, "B"),
            None,
            "{id}"
        );
    }
}

#[test]
fn form_owner_follows_the_form_attribute_then_the_ancestors() {
    let doc = parse(FORM);
    let form = by_id(&doc, "f");
    assert_eq!(forms::form_owner(&doc, by_id(&doc, "outside")), Some(form));
    assert_eq!(forms::form_owner(&doc, by_id(&doc, "save")), Some(form));
    let stray = parse(r#"<input id="lost" form="nowhere"><p id="p" form="x">"#);
    assert_eq!(forms::form_owner(&stray, by_id(&stray, "lost")), None);
    assert!(forms::is_submit_button(&doc, by_id(&doc, "save")));
    assert!(forms::is_submit_button(&doc, by_id(&doc, "img")));
    assert!(!forms::is_submit_button(&doc, by_id(&doc, "outside")));
}
