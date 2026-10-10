//! Matching through the adapter, checked against `tenta_dom`'s own engine.

use selectors::SelectorList;
use selectors::context::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, SelectorCaches,
};
use selectors::matching::{ElementSelectorFlags, matches_selector_list};
use style::dom::{NodeInfo, TDocument, TElement, TNode};
use style::selector_parser::{NonTSPseudoClass, SelectorImpl, SelectorParser};
use style::stylesheets::UrlExtraData;
use stylo_dom::ElementState;
use tenta_core::NodeId;
use tenta_dom::Document;
use url::Url;

use super::test_support::{parse, with_view};
use super::{FlushView, Handle};

fn compile(selector: &str) -> SelectorList<SelectorImpl> {
    crate::cssom::configure_stylo();
    let url = UrlExtraData::from(Url::parse("https://example.test/").unwrap());
    SelectorParser::parse_author_origin_no_namespace(selector, &url)
        .unwrap_or_else(|error| panic!("'{selector}' does not parse: {error:?}"))
}

fn matches<'a>(
    view: &'a FlushView<'a>,
    node: NodeId,
    list: &SelectorList<SelectorImpl>,
    flags: NeedsSelectorFlags,
) -> bool {
    let mut caches = SelectorCaches::default();
    let mut context = MatchingContext::new(
        MatchingMode::Normal,
        None,
        &mut caches,
        view.quirks,
        flags,
        MatchingForInvalidation::No,
    );
    matches_selector_list(list, &view.handle(node).unwrap(), &mut context)
}

fn query_all<'a>(view: &'a FlushView<'a>, selector: &str) -> Vec<NodeId> {
    let list = compile(selector);
    view.doc
        .descendants(view.doc.document_node())
        .filter(|node| view.doc.is_element(*node))
        .filter(|node| matches(view, *node, &list, NeedsSelectorFlags::No))
        .collect()
}

fn by_id(doc: &Document, id: &str) -> NodeId {
    doc.get_element_by_id(id)
        .unwrap_or_else(|| panic!("no element with id {id}"))
}

/// A deterministic page of about three hundred elements that exercises every
/// pseudo-class the adapter answers.
fn generated_page(doctype: bool) -> String {
    let mut html = String::from(if doctype { "<!DOCTYPE html>" } else { "" });
    html.push_str("<html lang=\"en\"><head><title>t</title></head><body>");
    let langs = ["", " lang=\"en-US\"", " lang=\"de\"", " lang=\"\""];
    for i in 0..30 {
        let extra = if i % 3 == 0 { " x" } else { "" };
        html.push_str(&format!(
            "<div id=\"d{i}\" class=\"c{} item{extra}\" data-k=\"v{}\"{}>",
            i % 5,
            i % 7,
            langs[i % 4],
        ));
        html.push_str(&format!("<p class=\"x\">text {i}</p><ul>"));
        html.push_str("<li class=\"x\">a</li><li>b</li><li class=\"x y\">c</li></ul>");
        let input = match i % 4 {
            0 if i % 8 == 0 => "<input type=checkbox checked class=in>",
            0 => "<input type=checkbox class=in>",
            1 => "<input type=text placeholder=p required class=in>",
            2 => "<input type=radio disabled class=in>",
            _ => "<input type=text value=v readonly class=in>",
        };
        html.push_str(input);
        html.push_str("<!-- c --><span></span><a href=\"/p");
        html.push_str(&format!("{i}\">link</a>"));
        if i % 6 == 0 {
            html.push_str(
                "<fieldset disabled><legend><input class=in></legend><input class=in>\
                 <textarea placeholder=t></textarea></fieldset>\
                 <select><optgroup disabled><option selected>o</option></optgroup>\
                 <option>p</option></select>",
            );
        }
        html.push_str("</div>");
    }
    html.push_str(
        "<svg><linearGradient id=g></linearGradient></svg><p id=ed contenteditable>e</p>\
         </body></html>",
    );
    html
}

const SELECTORS: &[&str] = &[
    "div",
    ".x",
    "#d5",
    "[data-k]",
    "[data-k=v3]",
    "[data-k=\"V3\" i]",
    "[data-k^=v]",
    "[DATA-K=v1]",
    "[lang|=en]",
    "div > p",
    "div p",
    "p + ul",
    "p ~ a",
    "li:first-child",
    "li:last-child",
    "li:nth-child(2n+1)",
    "li:nth-child(2n+1 of .x)",
    "li:nth-last-child(2)",
    "li:first-of-type",
    "li:nth-of-type(odd)",
    "span:only-of-type",
    "span:empty",
    ":empty",
    ":root",
    "body > :first-child",
    "input:checked",
    "option:checked",
    "input:disabled",
    "option:disabled",
    "input:enabled",
    "input:required",
    "input:optional",
    "input:read-only",
    "input:read-write",
    "input:placeholder-shown",
    "textarea:placeholder-shown",
    "input[type=checkbox]:not(:checked)",
    "fieldset:disabled input",
    "a:link",
    "a:any-link",
    ":defined",
    "div:not(.c1)",
    ":is(p, li).x",
    ":where(ul, ol) > li",
    "div:has(> input:checked)",
    "div:has(~ div.c2)",
    "div:has(li.y) p",
    "div:nth-child(3) > ul > li:nth-child(even)",
    "a[href]:not([href^=\"/p1\"])",
    ":lang(en)",
    ":lang(de)",
    "p:read-write",
    "svg linearGradient",
    "svg lineargradient",
    ".C1",
    "#D5",
];

/// Selectors that legitimately select nothing, by mode.
fn may_be_empty(selector: &str, doctype: bool) -> bool {
    selector == "svg lineargradient" || (doctype && matches!(selector, ".C1" | "#D5"))
}

fn assert_agrees_with_the_dom_engine(doctype: bool) {
    let doc = parse(&generated_page(doctype));
    assert!(
        doc.descendants(doc.document_node())
            .filter(|node| doc.is_element(*node))
            .count()
            >= 300
    );
    with_view(&doc, |view| {
        for selector in SELECTORS {
            let expected = doc
                .query_selector_all(doc.document_node(), selector)
                .unwrap();
            let actual = query_all(view, selector);
            assert_eq!(actual, expected, "'{selector}' (doctype: {doctype})");
            assert!(
                !actual.is_empty() || may_be_empty(selector, doctype),
                "'{selector}' selects nothing, so it proves nothing (doctype: {doctype})"
            );
        }
    });
}

#[test]
fn selectors_agree_with_the_dom_engine_in_standards_mode() {
    assert_agrees_with_the_dom_engine(true);
}

#[test]
fn selectors_agree_with_the_dom_engine_in_quirks_mode() {
    assert_agrees_with_the_dom_engine(false);
}

#[test]
fn tree_navigation_visits_every_node_kind() {
    let doc = parse("<div id=a>t<!--c--><p id=b></p><span id=c></span></div>");
    with_view(&doc, |view| {
        let a = view.handle(by_id(&doc, "a")).unwrap().as_node();
        let kinds: Vec<_> = a
            .dom_children()
            .map(|child| (child.is_text_node(), child.is_element()))
            .collect();
        assert_eq!(
            kinds,
            [(true, false), (false, false), (false, true), (false, true)]
        );
        // Stylo's layout iterator drops the comment.
        assert_eq!(a.as_element().unwrap().traversal_children().count(), 3);

        let b = view.handle(by_id(&doc, "b")).unwrap();
        let c = view.handle(by_id(&doc, "c")).unwrap();
        assert_eq!(selectors::Element::next_sibling_element(&b), Some(c));
        assert_eq!(selectors::Element::prev_sibling_element(&c), Some(b));
        assert_eq!(selectors::Element::prev_sibling_element(&b), None);
        assert_eq!(
            selectors::Element::first_element_child(&a.as_element().unwrap()),
            Some(b)
        );
        assert_eq!(b.as_node().parent_node(), Some(a));
        assert_eq!(b.as_node().traversal_parent(), a.as_element());
        assert_eq!(a.last_child(), Some(c.as_node()));
        assert_eq!(a.first_child().unwrap().prev_sibling(), None);
        assert!(b.as_node().is_in_document());
        assert!(b.as_node().as_shadow_root().is_none());
        assert!(b.shadow_root().is_none() && b.containing_shadow().is_none());

        let document = b.as_node().owner_doc();
        assert!(document.as_node().as_document().is_some());
        assert_eq!(document.as_node().parent_node(), None);
        assert!(selectors::Element::is_root(
            &view.handle(doc.document_element().unwrap()).unwrap()
        ));
        assert!(!selectors::Element::is_root(&b));
    });
}

#[test]
fn a_detached_element_is_not_in_the_document() {
    let mut doc = parse("<p></p>");
    let detached = doc.create_html_element(tenta_core::atom::atoms::DIV);
    with_view(&doc, |view| {
        let node = view.handle(detached).unwrap().as_node();
        assert!(!node.is_in_document());
        assert_eq!(node.parent_node(), None);
        assert_eq!(node.next_sibling(), None);
    });
}

#[test]
fn handles_are_identified_by_slot() {
    use std::hash::{BuildHasher, RandomState};

    let doc = parse("<p id=a></p><p id=b></p>");
    with_view(&doc, |view| {
        let a1 = view.handle(by_id(&doc, "a")).unwrap();
        let a2 = view.handle(by_id(&doc, "a")).unwrap();
        let b = view.handle(by_id(&doc, "b")).unwrap();
        assert_eq!(a1, a2);
        assert_ne!(a1, b);
        let hasher = RandomState::new();
        assert_eq!(hasher.hash_one(a1), hasher.hash_one(a2));
        assert_eq!(
            selectors::Element::opaque(&a1),
            selectors::Element::opaque(&a2)
        );
        assert_ne!(
            selectors::Element::opaque(&a1),
            selectors::Element::opaque(&b)
        );
        assert_eq!(a1.as_node().opaque(), a2.as_node().opaque());
        assert_ne!(a1.as_node().opaque(), b.as_node().opaque());
        assert!(selectors::Element::is_same_type(&a1, &b));
    });
}

#[test]
fn names_and_namespaces_follow_the_element() {
    let doc = parse("<p id=h></p><svg><linearGradient id=g></linearGradient></svg>");
    with_view(&doc, |view| {
        let html = view.handle(by_id(&doc, "h")).unwrap();
        assert!(html.is_html_element() && !html.is_svg_element() && !html.is_mathml_element());
        assert!(selectors::Element::is_html_element_in_html_document(&html));
        assert_eq!(&**TElement::local_name(&html), "p");
        assert_eq!(
            &**TElement::namespace(&html),
            "http://www.w3.org/1999/xhtml"
        );

        let svg = view.handle(by_id(&doc, "g")).unwrap();
        assert!(svg.is_svg_element() && !svg.is_html_element());
        assert!(!selectors::Element::is_html_element_in_html_document(&svg));
        assert_eq!(
            &**TElement::local_name(&svg),
            "linearGradient",
            "case is kept outside HTML"
        );
        assert!(!selectors::Element::is_same_type(&html, &svg));
        assert!(!svg.is_html_document_body_element());
    });
}

#[test]
fn ids_classes_and_attribute_names_come_from_the_document() {
    let doc = parse("<p id=a class='x  y' data-k=1 title=t></p><p></p>");
    with_view(&doc, |view| {
        let a = view.handle(by_id(&doc, "a")).unwrap();
        assert_eq!(a.id().map(|id| &**id), Some("a"));
        assert!(
            std::ptr::eq(a.id().unwrap(), a.id().unwrap()),
            "converted once"
        );
        let mut classes = Vec::new();
        a.each_class(|class| classes.push(class.to_string()));
        assert_eq!(classes, ["x", "y"]);
        let mut names = Vec::new();
        a.each_attr_name(|name| names.push(name.to_string()));
        assert_eq!(names, ["id", "class", "data-k", "title"]);

        let plain = view.handle(doc.get_elements_by_tag_name("p")[1]).unwrap();
        assert!(plain.id().is_none());
        let mut count = 0;
        plain.each_class(|_| count += 1);
        assert_eq!(count, 0);
    });
}

#[test]
fn get_attr_reads_a_namespaced_attribute() {
    use style::{LocalName, Namespace};

    let doc = parse("<p id=a title=t></p>");
    with_view(&doc, |view| {
        let a = view.handle(by_id(&doc, "a")).unwrap();
        let none = Namespace::from("");
        assert_eq!(
            a.get_attr(&LocalName::from("title"), &none).as_deref(),
            Some("t")
        );
        assert_eq!(
            a.get_attr(&LocalName::from("missing-never-interned"), &none),
            None
        );
        assert_eq!(
            a.get_attr(&LocalName::from("title"), &Namespace::from("urn:other")),
            None
        );
    });
}

#[test]
fn state_reports_what_the_selector_map_needs() {
    let mut doc = parse(
        "<a id=l href=/x></a><a id=nl></a>\
         <input id=c type=checkbox checked><input id=d disabled><input id=r readonly>\
         <fieldset id=fs disabled><legend><input id=lg></legend><input id=in></fieldset>\
         <input id=dirty type=checkbox checked><input id=ph placeholder=p>",
    );
    let dirty = by_id(&doc, "dirty");
    doc.set_dirty_checkedness(dirty, false);
    let ph = by_id(&doc, "ph");
    doc.set_dirty_value(ph, "typed".to_owned());
    with_view(&doc, |view| {
        let state = |id: &str| view.handle(by_id(&doc, id)).unwrap().state();
        assert!(state("l").contains(ElementState::UNVISITED | ElementState::DEFINED));
        assert!(!state("nl").intersects(ElementState::VISITED_OR_UNVISITED));
        assert!(state("c").contains(ElementState::CHECKED | ElementState::DEFAULT));
        assert!(state("d").contains(ElementState::DISABLED));
        assert!(!state("d").contains(ElementState::ENABLED));
        assert!(state("r").contains(ElementState::READONLY | ElementState::ENABLED));
        assert!(
            state("in").contains(ElementState::DISABLED),
            "inherited from the fieldset"
        );
        assert!(
            state("lg").contains(ElementState::ENABLED),
            "first legend is exempt"
        );
        assert!(
            !state("dirty").contains(ElementState::CHECKED),
            "script state wins"
        );
        assert!(
            !state("ph").contains(ElementState::PLACEHOLDER_SHOWN),
            "a typed value hides it"
        );
        assert!(
            !state("l")
                .intersects(ElementState::FOCUS | ElementState::HOVER | ElementState::ACTIVE)
        );
    });
}

#[test]
fn the_focused_element_reports_focus_and_its_ancestors_focus_within() {
    let mut doc = parse("<div id=o><p id=m><input id=f></p><input id=other></div>");
    let focused = by_id(&doc, "f");
    doc.set_focused_element(Some(focused), true);
    with_view(&doc, |view| {
        let state = |id: &str| view.handle(by_id(&doc, id)).unwrap().state();
        let all = ElementState::FOCUS | ElementState::FOCUSRING | ElementState::FOCUS_WITHIN;
        assert!(state("f").contains(all));
        for ancestor in ["m", "o"] {
            assert!(state(ancestor).contains(ElementState::FOCUS_WITHIN));
            assert!(!state(ancestor).intersects(ElementState::FOCUS | ElementState::FOCUSRING));
        }
        assert!(!state("other").intersects(all));
    });
    doc.set_focused_element(Some(focused), false);
    with_view(&doc, |view| {
        let state = view.handle(focused).unwrap().state();
        assert!(state.contains(ElementState::FOCUS));
        assert!(!state.contains(ElementState::FOCUSRING));
    });
}

#[test]
fn lang_is_inherited_and_matched_by_range() {
    let doc = parse(
        "<div lang=en-US><p id=a></p><p id=b lang=''></p><p id=c lang=de></p></div><p id=d></p>",
    );
    with_view(&doc, |view| {
        let handle = |id: &str| view.handle(by_id(&doc, id)).unwrap();
        let lang = |id: &str| handle(id).lang_attr().map(|lang| lang.to_string());
        assert_eq!(lang("a").as_deref(), Some("en-US"));
        assert_eq!(lang("b").as_deref(), Some(""), "empty declares unknown");
        assert_eq!(lang("d"), None);
        let en: style::selector_parser::Lang = "en".into();
        assert!(handle("a").match_element_lang(None, &en));
        assert!(!handle("b").match_element_lang(None, &en));
        assert!(!handle("c").match_element_lang(None, &en));
        assert!(!handle("d").match_element_lang(None, &en));
        assert!(handle("d").match_element_lang(Some(Some("en-GB".into())), &en));
        assert!(!handle("a").match_element_lang(Some(None), &en));
    });
}

#[test]
fn matching_records_selector_flags_only_when_asked() {
    let doc = parse("<ul id=u><li id=a></li><li id=b></li></ul>");
    let list = compile("li:nth-child(2n+1)");
    with_view(&doc, |view| {
        let parent = view.handle(by_id(&doc, "u")).unwrap();
        assert!(matches(
            view,
            by_id(&doc, "a"),
            &list,
            NeedsSelectorFlags::No
        ));
        assert!(!parent.has_selector_flags(ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS));

        assert!(matches(
            view,
            by_id(&doc, "a"),
            &list,
            NeedsSelectorFlags::Yes
        ));
        assert!(parent.has_selector_flags(ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS));
        let item = view.handle(by_id(&doc, "a")).unwrap();
        assert!(!item.has_selector_flags(ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS));
        assert!(item.relative_selector_search_direction().is_empty());

        let empty = compile("li:empty");
        assert!(matches(
            view,
            by_id(&doc, "a"),
            &empty,
            NeedsSelectorFlags::Yes
        ));
        assert!(item.has_selector_flags(ElementSelectorFlags::HAS_EMPTY_SELECTOR));
        assert!(!parent.has_selector_flags(ElementSelectorFlags::HAS_EMPTY_SELECTOR));
    });
}

#[test]
fn has_marks_its_anchor_with_a_search_direction() {
    let doc = parse("<div id=a><p><b id=t></b></p></div>");
    let list = compile("div:has(b)");
    with_view(&doc, |view| {
        assert!(matches(
            view,
            by_id(&doc, "a"),
            &list,
            NeedsSelectorFlags::Yes
        ));
        let anchor = view.handle(by_id(&doc, "a")).unwrap();
        assert!(anchor.has_selector_flags(ElementSelectorFlags::ANCHORS_RELATIVE_SELECTOR));
    });
}

#[test]
fn servo_and_custom_state_pseudo_classes_have_dom_answers() {
    let doc =
        parse("<table id=b border=2></table><table id=z border=0></table><table id=n></table>");
    with_view(&doc, |view| {
        let mut caches = SelectorCaches::default();
        let mut context = MatchingContext::new(
            MatchingMode::Normal,
            None,
            &mut caches,
            view.quirks,
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );
        let mut ask = |id: &str, class: &NonTSPseudoClass| {
            selectors::Element::match_non_ts_pseudo_class(
                &view.handle(by_id(&doc, id)).unwrap(),
                class,
                &mut context,
            )
        };
        assert!(ask("b", &NonTSPseudoClass::ServoNonZeroBorder));
        assert!(!ask("z", &NonTSPseudoClass::ServoNonZeroBorder));
        assert!(!ask("n", &NonTSPseudoClass::ServoNonZeroBorder));
        assert!(!ask("b", &NonTSPseudoClass::Hover));
        assert!(!ask("b", &NonTSPseudoClass::Visited));
        assert!(ask("b", &NonTSPseudoClass::Defined));
    });
}

#[test]
fn the_document_view_reports_quirks_lock_and_html_flavour() {
    let standards = parse("<!DOCTYPE html><p></p>");
    with_view(&standards, |view| {
        let document = view
            .handle(standards.document_node())
            .unwrap()
            .as_node()
            .as_document()
            .unwrap();
        assert_eq!(document.quirks_mode(), style::context::QuirksMode::NoQuirks);
        assert!(document.is_html_document());
        assert!(std::ptr::eq(document.shared_lock(), view.lock));
    });
    let quirks = parse("<p></p>");
    with_view(&quirks, |view| {
        let document = view
            .handle(quirks.document_node())
            .unwrap()
            .as_node()
            .as_document()
            .unwrap();
        assert_eq!(document.quirks_mode(), style::context::QuirksMode::Quirks);
    });
}

#[test]
fn only_the_documents_body_is_the_body_element() {
    let doc = parse("<!DOCTYPE html><html><body id=b><div id=d></div></body></html>");
    with_view(&doc, |view| {
        let handle = |id: &str| -> Handle<'_> { view.handle(by_id(&doc, id)).unwrap() };
        assert!(handle("b").is_html_document_body_element());
        assert!(!handle("d").is_html_document_body_element());
    });
}

#[test]
fn element_data_follows_ensure_and_clear() {
    let doc = parse("<p id=a></p>");
    with_view(&doc, |view| {
        let a = view.handle(by_id(&doc, "a")).unwrap();
        assert!(!a.has_data() && a.borrow_data().is_none() && a.mutate_data().is_none());
        // SAFETY: single-threaded test; no borrow of this element's data is held.
        unsafe {
            drop(a.ensure_data());
        }
        assert!(a.has_data() && a.borrow_data().is_some());
        assert!(a.mutate_data().is_some());
        // SAFETY: the guards above were dropped.
        unsafe {
            a.clear_data();
        }
        assert!(!a.has_data());

        assert!(!a.has_dirty_descendants());
        // SAFETY: Cell writes on a single thread.
        unsafe {
            a.set_dirty_descendants();
        }
        assert!(a.has_dirty_descendants());
        // SAFETY: as above.
        unsafe {
            a.unset_dirty_descendants();
        }
        assert!(!a.has_dirty_descendants());
    });
}

const FORM_PAGE: &str = "<!DOCTYPE html>\
    <input id=t1><input id=t2 type=TEXT placeholder=p><input id=cb type=checkbox placeholder=p>\
    <input id=ro type=text readonly placeholder=p><input id=dis type=text disabled>\
    <input id=num type=number placeholder=p value=3><input id=dt type=date placeholder=p>\
    <textarea id=ta placeholder=p></textarea><textarea id=ta2 placeholder=p>hi</textarea>\
    <div id=ce contenteditable></div><div id=cef contenteditable=false></div>\
    <select><optgroup id=og disabled><option id=o1>a</option></optgroup><option id=o2>b</option>\
    </select><a id=al href=x></a><area id=ar href=y><link id=lk href=z><a id=nl></a>";

fn ids<'a>(view: &'a FlushView<'a>, selector: &str) -> Vec<String> {
    query_all(view, selector)
        .into_iter()
        .filter_map(|node| view.doc.element_id(node).map(str::to_owned))
        .collect()
}

#[test]
fn form_and_link_pseudo_classes_follow_html() {
    let doc = parse(FORM_PAGE);
    with_view(&doc, |view| {
        assert_eq!(
            ids(view, "[id]:read-write"),
            ["t1", "t2", "num", "dt", "ta", "ta2", "ce"]
        );
        assert!(ids(view, "[id]:read-only").contains(&"cef".to_owned()));
        assert_eq!(ids(view, ":placeholder-shown"), ["t2", "ro", "ta"]);
        assert_eq!(ids(view, ":disabled"), ["dis", "og", "o1"]);
        assert_eq!(ids(view, "option:enabled"), ["o2"]);
        assert_eq!(ids(view, ":link"), ["al", "ar"]);
        assert_eq!(ids(view, ":any-link"), ["al", "ar"]);
    });
}

#[test]
fn html_attribute_values_such_as_type_ignore_ascii_case() {
    let doc = parse(FORM_PAGE);
    with_view(&doc, |view| {
        assert_eq!(ids(view, "input[type=text]"), ["t2", "ro", "dis"]);
        assert_eq!(ids(view, "input[type=\"TEXT\"]"), ["t2", "ro", "dis"]);
        assert_eq!(ids(view, "input[data-x=A]"), Vec::<String>::new());
    });
}

#[test]
fn nth_last_of_type_counts_from_the_end() {
    let doc = parse("<div><p id=a></p><b></b><p id=b></p><p id=c></p></div>");
    with_view(&doc, |view| {
        assert_eq!(ids(view, "p:nth-last-of-type(2)"), ["b"]);
        assert_eq!(ids(view, "p:nth-last-of-type(1)"), ["c"]);
    });
}

#[test]
fn scope_is_the_root_element_unless_a_scope_element_is_set() {
    let doc = parse("<div id=a><p id=b></p></div>");
    with_view(&doc, |view| {
        assert_eq!(query_all(view, ":scope"), [doc.document_element().unwrap()]);
        let list = compile(":scope > p");
        let mut caches = SelectorCaches::default();
        let mut context = MatchingContext::new(
            MatchingMode::Normal,
            None,
            &mut caches,
            view.quirks,
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );
        let div = view.handle(by_id(&doc, "a")).unwrap();
        context.scope_element = Some(selectors::Element::opaque(&div));
        let p = view.handle(by_id(&doc, "b")).unwrap();
        assert!(matches_selector_list(&list, &p, &mut context));
    });
}

#[test]
fn namespaced_attribute_selectors_distinguish_the_namespace() {
    let doc = parse("<svg><use id=u xlink:href=\"#a\"></use><use id=v href=\"#b\"></use></svg>");
    with_view(&doc, |view| {
        assert_eq!(ids(view, "use[*|href]"), ["u", "v"]);
        assert_eq!(ids(view, "use[|href]"), ["v"]);
    });
}
