//! The cascade end to end: HTML and `<style>` in, computed values out.
#![cfg(feature = "test-util")]

mod cascade_support;

use cascade_support::*;
use tenta_css::cascade::test_util::style_html;
use tenta_css::cascade::{StyleEnv, StyleSource};

const STANDARDS: &str = "<!DOCTYPE html>";

fn page(head: &str, body: &str) -> String {
    format!("{STANDARDS}<html><head><style>{head}</style></head><body>{body}</body></html>")
}

fn env_with_viewport(width: f32, height: f32) -> StyleEnv {
    StyleEnv {
        viewport: (width, height),
        ..StyleEnv::default()
    }
}

#[test]
fn user_agent_defaults() {
    let (doc, engine) = styled(&format!(
        "{STANDARDS}<html id=root><head id=head><title id=title>t</title></head>\
         <body id=body><h1 id=h1>x</h1><p id=p>y</p></body></html>"
    ));
    assert_eq!(value(&doc, &engine, "body", "margin-top"), "8px");
    assert_eq!(value(&doc, &engine, "body", "margin-left"), "8px");
    assert_eq!(value(&doc, &engine, "head", "display"), "none");
    assert_eq!(value(&doc, &engine, "h1", "font-size"), "32px");
    assert_eq!(value(&doc, &engine, "p", "display"), "block");
    assert_eq!(value(&doc, &engine, "root", "font-size"), "16px");
}

#[test]
fn descendants_of_a_display_none_element_are_not_styled() {
    let (doc, engine) = styled(&format!(
        "{STANDARDS}<html><head id=head><title id=title>t</title></head><body></body></html>"
    ));
    assert!(engine.style(element(&doc, "head")).is_some());
    assert!(engine.style(element(&doc, "title")).is_none());
    assert!(
        engine.style(doc.document_node()).is_none(),
        "not an element"
    );
}

#[test]
fn the_initial_font_family_is_the_serif_generic() {
    let (doc, engine) = styled(&page("", "<p id=p>x</p>"));
    assert_eq!(value(&doc, &engine, "p", "font-family"), "serif");
}

#[test]
fn specificity_origin_important_and_source_order() {
    let (doc, engine) = styled(&page(
        "p { color: rgb(1, 0, 0) } .c { color: rgb(2, 0, 0) } #i { color: rgb(3, 0, 0) }\
         #order { color: rgb(4, 0, 0) } #order { color: rgb(5, 0, 0) }\
         .imp { color: rgb(6, 0, 0) !important } #imp { color: rgb(7, 0, 0) }\
         #element { display: inline }",
        "<p id=element>a</p><p id=cls class=c>b</p><p id=i class=c>c</p>\
         <p id=order>d</p><p id=imp class=imp>e</p>",
    ));
    assert_eq!(value(&doc, &engine, "element", "color"), "rgb(1, 0, 0)");
    assert_eq!(value(&doc, &engine, "cls", "color"), "rgb(2, 0, 0)");
    assert_eq!(value(&doc, &engine, "i", "color"), "rgb(3, 0, 0)");
    assert_eq!(value(&doc, &engine, "order", "color"), "rgb(5, 0, 0)");
    assert_eq!(value(&doc, &engine, "imp", "color"), "rgb(6, 0, 0)");
    assert_eq!(
        value(&doc, &engine, "element", "display"),
        "inline",
        "an author rule beats the user-agent sheet"
    );
}

#[test]
fn the_style_attribute_beats_author_rules_unless_they_are_important() {
    let (doc, engine) = styled(&page(
        "#a { color: rgb(1, 0, 0) } #b { color: rgb(1, 0, 0) !important }\
         #c { width: 320px !important } #d { width: 80px !important }",
        "<p id=a style='color: rgb(9, 0, 0)'>a</p>\
         <p id=b style='color: rgb(9, 0, 0)'>b</p>\
         <p id=c style='width: 10px'>c</p>\
         <p id=d style='width: 320px !important'>d</p>",
    ));
    assert_eq!(value(&doc, &engine, "a", "color"), "rgb(9, 0, 0)");
    assert_eq!(value(&doc, &engine, "b", "color"), "rgb(1, 0, 0)");
    assert_eq!(value(&doc, &engine, "c", "width"), "320px");
    assert_eq!(
        value(&doc, &engine, "d", "width"),
        "320px",
        "inline important beats stylesheet important"
    );
}

#[test]
fn presentational_hints_sit_between_the_user_agent_sheet_and_author_rules() {
    let (doc, engine) = styled(&format!(
        "{STANDARDS}<html><head><style>#authored {{ background-color: rgb(0, 0, 255) }}</style></head>\
         <body link='#00ff00'>\
         <a id=link href='x'>l</a>\
         <table><tr><td id=hinted bgcolor='#ff0000'>h</td>\
         <td id=authored bgcolor='#ff0000'>a</td></tr></table></body></html>"
    ));
    assert_eq!(
        value(&doc, &engine, "link", "color"),
        "rgb(0, 255, 0)",
        "the body's link colour beats the user-agent a:link"
    );
    assert_eq!(
        value(&doc, &engine, "hinted", "background-color"),
        "rgb(255, 0, 0)"
    );
    assert_eq!(
        value(&doc, &engine, "authored", "background-color"),
        "rgb(0, 0, 255)",
        "author CSS beats a presentational hint"
    );
}

#[test]
fn custom_properties_cascade_with_importance_and_feed_var() {
    let (doc, engine) = styled(&page(
        ".custom-important { --case-width: 320px !important; --case-width: 80px; \
         width: var(--case-width) }\
         :root { --c: rgb(0, 128, 0) } #v { color: var(--c) } #f { color: var(--missing, rgb(7, 7, 7)) }",
        "<div id=w class=custom-important>w</div><p id=v>v</p><p id=f>f</p>",
    ));
    assert_eq!(value(&doc, &engine, "w", "width"), "320px");
    assert_eq!(value(&doc, &engine, "v", "color"), "rgb(0, 128, 0)");
    assert_eq!(value(&doc, &engine, "f", "color"), "rgb(7, 7, 7)");
}

#[test]
fn inheritance_keywords_and_relative_font_sizes() {
    let (doc, engine) = styled(&page(
        "#outer { font-size: 20px; color: rgb(1, 2, 3); border-top-width: 5px; border-top-style: solid }\
         #em { font-size: 1.5em } #pct { font-size: 50% } #rem { font-size: 2rem }\
         #kw { font-size: x-large } #inh { border-top-width: inherit; border-top-style: solid }\
         #ini { color: initial } #uns { color: unset; border-top-width: unset; border-top-style: solid }\
         #bigger { font-size: larger }",
        "<div id=outer><i id=em>a</i><i id=pct>b</i><i id=rem>c</i><i id=kw>d</i>\
         <i id=inh>e</i><i id=ini>f</i><i id=uns>g</i><i id=bigger>h</i></div>",
    ));
    assert_eq!(value(&doc, &engine, "em", "color"), "rgb(1, 2, 3)");
    assert_eq!(value(&doc, &engine, "em", "font-size"), "30px");
    assert_eq!(value(&doc, &engine, "pct", "font-size"), "10px");
    assert_eq!(value(&doc, &engine, "rem", "font-size"), "32px");
    assert_eq!(value(&doc, &engine, "kw", "font-size"), "24px");
    assert_eq!(value(&doc, &engine, "inh", "border-top-width"), "5px");
    assert_eq!(value(&doc, &engine, "ini", "color"), "rgb(0, 0, 0)");
    assert_eq!(value(&doc, &engine, "uns", "color"), "rgb(1, 2, 3)");
    assert_eq!(value(&doc, &engine, "uns", "border-top-width"), "3px");
    assert_eq!(value(&doc, &engine, "bigger", "font-size"), "24px");
}

#[test]
fn shorthands_expand() {
    let (doc, engine) = styled(&page(
        "#m { margin: 1px 2px 3px 4px } #f { font: italic bold 20px/30px monospace }\
         #b { background: rgb(1, 2, 3) }",
        "<div id=m>m</div><div id=f>f</div><div id=b>b</div>",
    ));
    assert_eq!(value(&doc, &engine, "m", "margin-top"), "1px");
    assert_eq!(value(&doc, &engine, "m", "margin-right"), "2px");
    assert_eq!(value(&doc, &engine, "m", "margin-bottom"), "3px");
    assert_eq!(value(&doc, &engine, "m", "margin-left"), "4px");
    assert_eq!(value(&doc, &engine, "f", "font-size"), "20px");
    assert_eq!(value(&doc, &engine, "f", "font-weight"), "700");
    assert_eq!(value(&doc, &engine, "f", "font-style"), "italic");
    assert_eq!(value(&doc, &engine, "f", "line-height"), "30px");
    assert_eq!(value(&doc, &engine, "f", "font-family"), "monospace");
    assert_eq!(
        value(&doc, &engine, "b", "background-color"),
        "rgb(1, 2, 3)"
    );
}

#[test]
fn media_queries_read_the_environment_viewport() {
    let html = page(
        "p { color: rgb(1, 0, 0) } @media (min-width: 600px) { p { color: rgb(2, 0, 0) } }\
         @media (width >= 832px) { #q { display: contents } }",
        "<p id=p>a</p><nav id=q>b</nav>",
    );
    let (doc, wide) = style_html(&html, env_with_viewport(900.0, 600.0));
    assert_eq!(value(&doc, &wide, "p", "color"), "rgb(2, 0, 0)");
    assert_eq!(value(&doc, &wide, "q", "display"), "contents");
    let (doc, narrow) = style_html(&html, env_with_viewport(500.0, 600.0));
    assert_eq!(value(&doc, &narrow, "p", "color"), "rgb(1, 0, 0)");
    assert_eq!(value(&doc, &narrow, "q", "display"), "block");
}

#[test]
fn media_type_and_color_scheme_come_from_the_environment() {
    let html = page(
        "@media print { p { color: rgb(1, 0, 0) } } @media screen { p { width: 11px } }\
         @media (prefers-color-scheme: dark) { p { height: 22px } }",
        "<p id=p>a</p>",
    );
    let (doc, screen) = style_html(&html, StyleEnv::default());
    assert_eq!(value(&doc, &screen, "p", "color"), "rgb(0, 0, 0)");
    assert_eq!(value(&doc, &screen, "p", "width"), "11px");
    assert_eq!(value(&doc, &screen, "p", "height"), "auto");
    let dark_print = StyleEnv {
        print: true,
        dark: true,
        ..StyleEnv::default()
    };
    let (doc, print) = style_html(&html, dark_print);
    assert_eq!(value(&doc, &print, "p", "color"), "rgb(1, 0, 0)");
    assert_eq!(value(&doc, &print, "p", "width"), "auto");
    assert_eq!(value(&doc, &print, "p", "height"), "22px");
}

#[test]
fn math_functions_resolve_against_the_viewport_and_fonts() {
    let (doc, engine) = styled(&page(
        "html, body { margin: 0; font-size: 16px }\
         .min-width { width: min(25vw, 350px) }\
         .min-margin { margin-left: min(25vw, 350px) }\
         .clamped { width: clamp(200px, 30vw, 320px) }\
         .calculated { margin-left: calc(10vw + 2rem) }\
         .padded { padding-left: max(5vw, 60px) }\
         .rounded { --media-width: 247px; width: calc(round(var(--media-width) * 1, 10px)) }\
         .floor { width: round(down, 29px, 10px) }",
        "<div id=a class=min-width></div><div id=b class=min-margin></div>\
         <div id=c class=clamped></div><div id=d class=calculated></div>\
         <div id=e class=padded></div><div id=f class=rounded></div>\
         <div id=g class=floor></div>",
    ));
    assert_eq!(value(&doc, &engine, "a", "width"), "200px");
    assert_eq!(value(&doc, &engine, "b", "margin-left"), "200px");
    assert_eq!(value(&doc, &engine, "c", "width"), "240px");
    assert_eq!(value(&doc, &engine, "d", "margin-left"), "112px");
    assert_eq!(value(&doc, &engine, "e", "padding-left"), "60px");
    assert_eq!(value(&doc, &engine, "f", "width"), "250px");
    assert_eq!(value(&doc, &engine, "g", "width"), "20px");
}

#[test]
fn supports_accepts_a_var_in_a_feature_query() {
    let (doc, engine) = styled(&page(
        "#probe { width: 100px } @supports (grid: var(--tw)) { #probe { width: 240px } }\
         @supports (not-a-property: 1) { #probe { height: 7px } }\
         @supports not (not-a-property: 1) { #probe { margin-left: 3px } }",
        "<div id=probe></div>",
    ));
    assert_eq!(value(&doc, &engine, "probe", "width"), "240px");
    assert_eq!(value(&doc, &engine, "probe", "height"), "auto");
    assert_eq!(value(&doc, &engine, "probe", "margin-left"), "3px");
}

#[test]
fn display_contents_follows_the_media_query() {
    let html = page(
        ".desktop { display: none } @media (width >= 832px) { .desktop { display: contents } }",
        "<nav><div id=d class=desktop><div id=i></div></div></nav>",
    );
    let (doc, narrow) = styled(&html);
    assert_eq!(value(&doc, &narrow, "d", "display"), "none");
    assert!(
        narrow.style(element(&doc, "i")).is_none(),
        "the subtree of a display:none element is not styled"
    );
    let (doc, wide) = style_html(&html, env_with_viewport(900.0, 600.0));
    assert_eq!(value(&doc, &wide, "d", "display"), "contents");
    assert_eq!(value(&doc, &wide, "i", "display"), "block");
}

#[test]
fn unitless_lengths_are_valid_in_quirks_mode_only() {
    let body = "<p id=p style='margin-left: 7'>x</p>";
    let sheet = "p { width: 10; height: 20px }";
    let quirks = format!("<html><head><style>{sheet}</style></head><body>{body}</body></html>");
    let (doc, engine) = styled(&quirks);
    assert_eq!(value(&doc, &engine, "p", "width"), "10px");
    assert_eq!(value(&doc, &engine, "p", "margin-left"), "7px");

    let (doc, engine) = styled(&page(sheet, body));
    assert_eq!(value(&doc, &engine, "p", "width"), "auto");
    assert_eq!(value(&doc, &engine, "p", "height"), "20px");
    assert_eq!(value(&doc, &engine, "p", "margin-left"), "0px");
}

#[test]
fn ex_and_ch_units_use_the_font_metrics() {
    let (doc, engine) = styled(&page(
        "#a { width: 2ex; height: 4ch; font-size: 20px }",
        "<div id=a></div>",
    ));
    assert_eq!(value(&doc, &engine, "a", "width"), "20px");
    assert_eq!(value(&doc, &engine, "a", "height"), "40px");
}

#[test]
fn a_disabled_source_and_a_mismatched_media_attribute_contribute_nothing() {
    let (doc, mut engine) = styled(&page("", "<p id=p>x</p>"));
    let source = |text: &str, media: Option<&str>, disabled: bool| StyleSource {
        owner: None,
        url: url::Url::parse("https://example.test/a.css").unwrap(),
        text: text.to_owned(),
        media: media.map(str::to_owned),
        disabled,
        imports: Vec::new(),
    };
    engine.set_sources(vec![
        source("p { width: 1px }", None, true),
        source("p { height: 2px }", Some("print"), false),
        source(
            "p { margin-left: 3px }",
            Some("screen and (min-width: 100px)"),
            false,
        ),
    ]);
    engine.flush(&doc);
    assert_eq!(value(&doc, &engine, "p", "width"), "auto");
    assert_eq!(value(&doc, &engine, "p", "height"), "auto");
    assert_eq!(value(&doc, &engine, "p", "margin-left"), "3px");
}

#[test]
fn a_lone_monospace_generic_family_scales_medium_to_thirteen_pixels() {
    let (doc, engine) = styled(&page(
        "#fixed { font-size: 16px } #list { font-family: monospace, serif }\
         #named { font-family: 'Courier New', monospace } #em { font-size: 2em }",
        "<pre id=pre>a</pre><code id=code>b</code>\
         <div id=div style='font-family: monospace'>c</div>\
         <pre id=fixed>d</pre><div id=list>e</div><div id=named>f</div>\
         <div style='font-family: monospace'><i id=nested>g</i><b id=em>h</b></div>",
    ));
    assert_eq!(value(&doc, &engine, "pre", "font-size"), "13px");
    assert_eq!(value(&doc, &engine, "code", "font-size"), "13px");
    assert_eq!(value(&doc, &engine, "div", "font-size"), "13px");
    assert_eq!(value(&doc, &engine, "nested", "font-size"), "13px");
    assert_eq!(value(&doc, &engine, "em", "font-size"), "26px");
    assert_eq!(
        value(&doc, &engine, "fixed", "font-size"),
        "16px",
        "an explicit size is not scaled"
    );
    // Stylo scales only when the list is exactly the generic.
    assert_eq!(value(&doc, &engine, "list", "font-size"), "16px");
    assert_eq!(value(&doc, &engine, "named", "font-size"), "16px");
}
