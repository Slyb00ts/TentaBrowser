//! Pseudo-element styles, the `content` list layout reads, and the on-demand
//! resolve path for nodes the flush leaves unstyled.
#![cfg(feature = "test-util")]

mod cascade_support;

use cascade_support::*;
use style::values::generics::counters::{Content, ContentItem};
use tenta_css::cascade::{PseudoKind, ResolveError};
use tenta_html::{ElementFlags, QualName};

const PAGE: &str = "<!DOCTYPE html><html><head><style>
  #a { color: red }
  #a::before { content: \"x\" }
  #ml::before { content: \"x\"; display: none }
  #parts::after { content: attr(title) \" / \" counter(c, upper-roman) }
  #none::before { content: none }
  #normal::before { content: normal }
  #empty::before { content: \"\" }
  #inherit::before { content: inherit; display: inherit }
  #inherit { content: \"p\"; display: block }
  li.m::marker { color: blue }
  #hid { display: none; color: rgb(1, 2, 3) }
  #hid p::before { content: \"h\"; color: green }
  #hid p::marker { color: purple }
</style></head><body>
  <div id=a>a</div><div id=ml>m</div><div id=parts title=T>p</div>
  <div id=none>n</div><div id=normal>n</div><div id=empty>e</div><div id=inherit>i</div>
  <div id=plain>p</div>
  <q id=q>quoted</q>
  <ul><li class=m id=item>i</li><li id=bare>b</li></ul>
  <div id=hid><p id=hp>x<span id=deep>y</span></p></div>
</body></html>";

fn items(style: &tenta_css::cascade::Style) -> &[ContentItem<style::values::computed::Image>] {
    match &style.get_counters().content {
        Content::Items(items) => &items.items,
        other => panic!("not an item list: {other:?}"),
    }
}

#[test]
fn a_before_with_string_content_has_a_box_and_inherits() {
    let (doc, engine) = styled(PAGE);
    let style = engine
        .pseudo_style(element(&doc, "a"), PseudoKind::Before)
        .expect("::before generates a box");
    assert!(matches!(items(&style), [ContentItem::String(text)] if &**text == "x"));
    assert_eq!(computed(&style, "color"), "rgb(255, 0, 0)");
    assert_eq!(computed(&style, "display"), "inline");
    assert!(
        engine
            .pseudo_style(element(&doc, "a"), PseudoKind::After)
            .is_none()
    );
}

#[test]
fn content_items_keep_their_kind_and_order() {
    let (doc, engine) = styled(PAGE);
    let style = engine
        .pseudo_style(element(&doc, "parts"), PseudoKind::After)
        .expect("::after generates a box");
    assert!(matches!(
        items(&style),
        [ContentItem::Attr(_), ContentItem::String(sep), ContentItem::Counter(name, _)]
            if &**sep == " / " && &*name.0 == "c"
    ));
    assert_eq!(
        computed(&style, "content"),
        "attr(title) \" / \" counter(c, upper-roman)"
    );
}

#[test]
fn the_user_agent_quotes_are_open_and_close_quote_items() {
    let (doc, engine) = styled(PAGE);
    let q = element(&doc, "q");
    let before = engine
        .pseudo_style(q, PseudoKind::Before)
        .expect("q::before");
    let after = engine.pseudo_style(q, PseudoKind::After).expect("q::after");
    assert!(matches!(items(&before), [ContentItem::OpenQuote]));
    assert!(matches!(items(&after), [ContentItem::CloseQuote]));
}

#[test]
fn pseudo_elements_without_content_or_display_have_no_box() {
    let (doc, engine) = styled(PAGE);
    for id in ["ml", "none", "normal", "plain"] {
        assert!(
            engine
                .pseudo_style(element(&doc, id), PseudoKind::Before)
                .is_none(),
            "#{id}::before generates no box"
        );
    }
    let empty = engine.pseudo_style(element(&doc, "empty"), PseudoKind::Before);
    assert!(empty.is_some(), "an empty string is still content");
}

#[test]
fn content_and_display_that_depend_on_the_parent_are_checked_after_inheritance() {
    // `content: inherit` takes the originating element's "p": Stylo keeps the
    // style because it cannot tell yet, and the box test decides.
    let (doc, engine) = styled(PAGE);
    let style = engine.pseudo_style(element(&doc, "inherit"), PseudoKind::Before);
    assert!(style.is_some_and(|style| computed(&style, "display") == "block"));
}

#[test]
fn first_line_first_letter_and_marker_are_not_served_without_the_document() {
    let (doc, engine) = styled(PAGE);
    for pseudo in [
        PseudoKind::FirstLine,
        PseudoKind::FirstLetter,
        PseudoKind::Marker,
    ] {
        assert!(engine.pseudo_style(element(&doc, "item"), pseudo).is_none());
    }
    for pseudo in [PseudoKind::FirstLine, PseudoKind::FirstLetter] {
        assert!(
            engine
                .resolve_pseudo_style(&doc, element(&doc, "item"), pseudo)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn marker_styles_follow_the_marker_rules() {
    // Chrome 153: li.m::marker has the blue of the rule, any other element
    // answers with the default black and `content: normal`.
    let (doc, engine) = styled(PAGE);
    let marker = |id: &str| {
        engine
            .resolve_pseudo_style(&doc, element(&doc, id), PseudoKind::Marker)
            .unwrap()
            .expect("::marker is answered for every element")
    };
    assert_eq!(computed(&marker("item"), "color"), "rgb(0, 0, 255)");
    assert_eq!(computed(&marker("bare"), "color"), "rgb(0, 0, 0)");
    assert_eq!(computed(&marker("bare"), "unicode-bidi"), "isolate");
    assert_eq!(computed(&marker("plain"), "content"), "normal");
}

#[test]
fn a_pseudo_element_that_generates_no_box_still_resolves_for_get_computed_style() {
    // Chrome 153: `#ml::before` reports display none and its content.
    let (doc, engine) = styled(PAGE);
    let resolve = |id: &str, pseudo| {
        engine
            .resolve_pseudo_style(&doc, element(&doc, id), pseudo)
            .unwrap()
            .unwrap_or_else(|| panic!("#{id} has a computed pseudo style"))
    };
    let hidden = resolve("ml", PseudoKind::Before);
    assert_eq!(computed(&hidden, "display"), "none");
    assert_eq!(computed(&hidden, "content"), "\"x\"");
    // No rule at all: cascaded from nothing, inheriting from the element.
    let plain = resolve("a", PseudoKind::After);
    assert_eq!(computed(&plain, "color"), "rgb(255, 0, 0)");
    assert_eq!(computed(&plain, "content"), "none");
    // A box-generating one is the stored style itself.
    let stored = engine
        .pseudo_style(element(&doc, "a"), PseudoKind::Before)
        .unwrap();
    assert!(std::ptr::eq(&*resolve("a", PseudoKind::Before), &*stored));
}

#[test]
fn nodes_under_display_none_resolve_without_being_stored() {
    let (doc, engine) = styled(PAGE);
    let traversals = engine.traversal_count();
    let (hp, deep) = (element(&doc, "hp"), element(&doc, "deep"));
    assert!(engine.style(hp).is_none(), "the flush leaves it unstyled");

    let style = engine
        .resolve_style(&doc, hp)
        .unwrap()
        .expect("resolves on demand");
    assert_eq!(
        computed(&style, "color"),
        "rgb(1, 2, 3)",
        "from the display:none parent"
    );
    assert_eq!(computed(&style, "display"), "block");
    let child = engine
        .resolve_style(&doc, deep)
        .unwrap()
        .expect("a grandchild resolves too");
    assert_eq!(computed(&child, "color"), "rgb(1, 2, 3)");
    assert_eq!(computed(&child, "display"), "inline");

    assert!(engine.style(hp).is_none(), "nothing was stored");
    assert!(engine.pseudo_style(hp, PseudoKind::Before).is_none());
    assert_eq!(engine.traversal_count(), traversals);
}

#[test]
fn pseudo_elements_of_unstyled_nodes_resolve_with_their_rules() {
    let (doc, engine) = styled(PAGE);
    let hp = element(&doc, "hp");
    let before = engine
        .resolve_pseudo_style(&doc, hp, PseudoKind::Before)
        .unwrap()
        .expect("p::before under display:none");
    assert_eq!(computed(&before, "content"), "\"h\"");
    assert_eq!(computed(&before, "color"), "rgb(0, 128, 0)");
    let marker = engine
        .resolve_pseudo_style(&doc, hp, PseudoKind::Marker)
        .unwrap()
        .expect("p::marker under display:none");
    assert_eq!(computed(&marker, "color"), "rgb(128, 0, 128)");
}

#[test]
fn resolving_repeatedly_leaves_the_engine_usable() {
    let (mut doc, mut engine) = styled(PAGE);
    let hp = element(&doc, "hp");
    let first = computed(&engine.resolve_style(&doc, hp).unwrap().unwrap(), "color");
    for _ in 0..3 {
        let again = engine.resolve_style(&doc, hp).unwrap().unwrap();
        assert_eq!(computed(&again, "color"), first);
    }
    let a = element(&doc, "a");
    set_attribute(&mut doc, a, "class", "changed");
    engine.flush(&doc);
    assert_eq!(value(&doc, &engine, "a", "color"), "rgb(255, 0, 0)");
    assert_eq!(
        computed(&engine.resolve_style(&doc, hp).unwrap().unwrap(), "color"),
        first
    );
}

#[test]
fn staleness_is_reported_before_anything_else_on_every_path() {
    let (mut doc, mut engine) = styled(PAGE);
    // A styled element, a pseudo-element stored by the flush, an unstyled node
    // and a non-element: all are "stale", none is "no style".
    let (a, hp) = (element(&doc, "a"), element(&doc, "hp"));
    set_attribute(&mut doc, a, "title", "changed");
    for node in [a, hp, doc.document_node()] {
        assert_eq!(
            engine.resolve_style(&doc, node).map(|s| s.is_some()),
            Err(ResolveError::Stale)
        );
        for pseudo in [
            PseudoKind::Before,
            PseudoKind::Marker,
            PseudoKind::FirstLine,
        ] {
            assert_eq!(
                engine
                    .resolve_pseudo_style(&doc, node, pseudo)
                    .map(|s| s.is_some()),
                Err(ResolveError::Stale)
            );
        }
        assert_eq!(
            engine.computed_value_in(&doc, node, None, "color"),
            Err(ResolveError::Stale)
        );
        assert_eq!(
            engine.computed_value_in(&doc, node, Some(PseudoKind::Before), "content"),
            Err(ResolveError::Stale)
        );
    }
    engine.flush(&doc);
    assert!(engine.resolve_style(&doc, a).unwrap().is_some());
    assert!(engine.resolve_style(&doc, hp).unwrap().is_some());
}

#[test]
fn a_document_that_was_never_flushed_is_stale() {
    let (doc, engine) = styled(PAGE);
    let other = {
        let (other, _) = styled("<!DOCTYPE html><p id=p>x</p>");
        other
    };
    let p = element(&other, "p");
    assert_eq!(
        engine.resolve_style(&other, p).map(|s| s.is_some()),
        Err(ResolveError::Stale)
    );
    assert!(
        engine
            .resolve_style(&doc, element(&doc, "a"))
            .unwrap()
            .is_some()
    );
}

#[test]
fn detached_nodes_and_non_elements_have_no_style() {
    let (mut doc, mut engine) = styled(PAGE);
    assert_eq!(
        engine
            .resolve_style(&doc, doc.document_node())
            .map(|s| s.is_some()),
        Ok(false)
    );
    let detached = doc
        .create_element(
            QualName::new(
                tenta_core::atom::atoms::NS_HTML,
                tenta_core::atom::Atom::new("div"),
            ),
            Vec::new(),
            ElementFlags::default(),
        )
        .unwrap();
    engine.flush(&doc);
    assert_eq!(
        engine.resolve_style(&doc, detached).map(|s| s.is_some()),
        Ok(false)
    );
    assert_eq!(
        engine.computed_value_in(&doc, detached, None, "color"),
        Ok(None)
    );
}

#[test]
fn an_empty_string_content_generates_a_box_but_an_empty_list_does_not() {
    // The clearfix idiom: `content: ''` with a display and no text.
    let (doc, engine) = styled(
        "<!DOCTYPE html><html><head><style>
           .cf::after { content: ''; display: table; clear: both }
           .dq::before { content: \"\" }
           .none::before { content: none }
           .normal::before { content: normal }
         </style></head><body>
           <div class=cf id=cf>x</div><div class=dq id=dq>x</div>
           <div class=none id=none>x</div><div class=normal id=normal>x</div>
         </body></html>",
    );
    let after = engine
        .pseudo_style(element(&doc, "cf"), PseudoKind::After)
        .expect("clearfix box");
    assert!(matches!(items(&after), [ContentItem::String(text)] if text.is_empty()));
    assert_eq!(computed(&after, "display"), "table");
    assert!(
        engine
            .pseudo_style(element(&doc, "dq"), PseudoKind::Before)
            .is_some()
    );
    for id in ["none", "normal"] {
        assert!(
            engine
                .pseudo_style(element(&doc, id), PseudoKind::Before)
                .is_none()
        );
    }
    let resolved = engine.resolve_pseudo_style(&doc, element(&doc, "cf"), PseudoKind::After);
    assert!(
        resolved
            .unwrap()
            .is_some_and(|style| generates_empty_string(&style))
    );
}

fn generates_empty_string(style: &tenta_css::cascade::Style) -> bool {
    matches!(items(style), [ContentItem::String(text)] if text.is_empty())
}

#[test]
fn counters_parse_and_reach_content() {
    // Chrome 153 serialises the counter properties as below.
    let (doc, engine) = styled(
        "<!DOCTYPE html><html><head><style>
           #c { counter-reset: c; counter-increment: a 2 b }
           #c::before { content: counter(c) \".\" counters(a, \"-\") }
         </style></head><body><div id=c>x</div></body></html>",
    );
    let c = element(&doc, "c");
    assert_eq!(
        engine.computed_value(c, None, "counter-reset").as_deref(),
        Some("c 0")
    );
    assert_eq!(
        engine
            .computed_value(c, None, "counter-increment")
            .as_deref(),
        Some("a 2 b 1")
    );
    let before = engine
        .pseudo_style(c, PseudoKind::Before)
        .expect("::before");
    assert!(matches!(
        items(&before),
        [ContentItem::Counter(c, _), ContentItem::String(dot), ContentItem::Counters(a, sep, _)]
            if &*c.0 == "c" && &**dot == "." && &*a.0 == "a" && &**sep == "-"
    ));
}

#[test]
fn pseudo_element_values_match_chrome() {
    // Rows read from headless Chrome 153 on the markup of `PAGE`.
    let (doc, engine) = styled(PAGE);
    let rows: &[(&str, PseudoKind, &str, &str)] = &[
        ("a", PseudoKind::Before, "content", "\"x\""),
        ("a", PseudoKind::Before, "color", "rgb(255, 0, 0)"),
        ("a", PseudoKind::Before, "display", "inline"),
        ("ml", PseudoKind::Before, "content", "\"x\""),
        ("ml", PseudoKind::Before, "display", "none"),
        (
            "parts",
            PseudoKind::After,
            "content",
            "\"T / \" counter(c, upper-roman)",
        ),
        ("inherit", PseudoKind::Before, "display", "block"),
        ("q", PseudoKind::Before, "content", "open-quote"),
        ("q", PseudoKind::After, "content", "close-quote"),
        ("empty", PseudoKind::Before, "content", "\"\""),
        ("item", PseudoKind::Marker, "color", "rgb(0, 0, 255)"),
        ("bare", PseudoKind::Marker, "color", "rgb(0, 0, 0)"),
        ("bare", PseudoKind::Marker, "unicode-bidi", "isolate"),
        ("plain", PseudoKind::Marker, "content", "normal"),
        ("a", PseudoKind::After, "content", "none"),
        ("a", PseudoKind::After, "color", "rgb(255, 0, 0)"),
        ("hp", PseudoKind::Before, "content", "\"h\""),
        ("hp", PseudoKind::Before, "color", "rgb(0, 128, 0)"),
        ("hp", PseudoKind::Marker, "color", "rgb(128, 0, 128)"),
    ];
    for &(id, pseudo, property, expected) in rows {
        let actual = engine
            .computed_value_in(&doc, element(&doc, id), Some(pseudo), property)
            .unwrap();
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "#{id} {pseudo:?} {property}"
        );
    }
}

#[test]
fn attr_in_generated_content_is_read_from_the_element_for_get_computed_style() {
    // Chrome 153 joins strings and attribute values into one quoted string and
    // leaves counters and quotes as written.
    let (doc, engine) = styled(
        "<!DOCTYPE html><html><head><style>
           #m::before { content: attr(foo) }
           #n::before { content: \"a\" attr(t) \"b\" counter(c) attr(t) open-quote attr(zz) }
           #e::before { content: attr(t) attr(t) }
           #q::before { content: \"a\\\"b\\\\ c\" attr(t) }
         </style></head><body>
           <div id=m>m</div><div id=n t=T>n</div><div id=e t=E>e</div><div id=q t='x\"y'>q</div>
         </body></html>",
    );
    let content = |id: &str| {
        engine
            .computed_value_in(&doc, element(&doc, id), Some(PseudoKind::Before), "content")
            .unwrap()
    };
    assert_eq!(content("m").as_deref(), Some("\"\""));
    assert_eq!(
        content("n").as_deref(),
        Some("\"aTb\" counter(c) \"T\" open-quote \"\"")
    );
    assert_eq!(content("e").as_deref(), Some("\"EE\""));
    assert_eq!(content("q").as_deref(), Some("\"a\\\"b\\\\ cx\\\"y\""));
    // The stored style keeps `attr()` for layout to resolve.
    assert_eq!(
        engine
            .computed_value(element(&doc, "e"), Some(PseudoKind::Before), "content")
            .as_deref(),
        Some("attr(t) attr(t)")
    );
}

#[test]
fn author_rules_for_a_pseudo_element_are_detected_by_origin() {
    let has =
        |engine: &tenta_css::cascade::StyleEngine| engine.has_author_rules(PseudoKind::Marker);
    let (_, engine) = styled("<!DOCTYPE html><ul><li>i</li></ul>");
    assert!(!has(&engine), "the user-agent ::marker rule does not count");

    for css in [
        "li::marker { color: red }",
        "@media (min-width: 1px) { .x::marker { color: red } }",
        "@layer base { .x::marker { color: red } }",
    ] {
        let (_, engine) = styled(&format!(
            "<!DOCTYPE html><style>{css}</style><ul><li class=x>i</li></ul>"
        ));
        assert!(has(&engine), "{css}");
    }

    let (_, engine) =
        styled("<!DOCTYPE html><style>.map-marker { color: red }</style><ul><li>i</li></ul>");
    assert!(!has(&engine));
    assert!(!engine.has_author_rules(PseudoKind::FirstLine));

    let (_, engine) = styled("<!DOCTYPE html><style>p::before { content: 'x' }</style>");
    assert!(engine.has_author_rules(PseudoKind::Before));
    assert!(!engine.has_author_rules(PseudoKind::After));
}

#[test]
fn author_rules_in_an_imported_sheet_are_detected() {
    use tenta_css::cascade::{ImportedSheet, StyleSource};
    let (doc, mut engine) = styled("<!DOCTYPE html><ul><li>i</li></ul>");
    let base = url::Url::parse("https://example.test/").unwrap();
    let imported = base.join("m.css").unwrap();
    engine.set_sources(vec![StyleSource {
        owner: None,
        scope: None,
        url: base,
        text: "@import url(m.css);".to_owned(),
        media: None,
        disabled: false,
        imports: vec![ImportedSheet {
            url: imported.clone(),
            final_url: imported,
            text: "li::marker { color: red }".to_owned(),
        }],
    }]);
    engine.flush(&doc);
    assert!(engine.has_author_rules(PseudoKind::Marker));
}
