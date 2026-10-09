//! `getComputedStyle` serialisation of computed values.
//!
//! The expected strings were read from headless Chrome 153 on the same markup
//! (`getComputedStyle(el).getPropertyValue`), so every row is a browser-visible
//! spelling. Used values (`width`, `margin: auto`, `transform-origin`, a
//! percentage `transform`, `min-width: auto`, ...) are deliberately absent:
//! they need layout.
#![cfg(feature = "test-util")]

mod cascade_support;

use cascade_support::*;
use tenta_css::cascade::{PseudoKind, StyleEngine};

/// One element per case, `id=t<N>`, in the order `ROWS` numbers them.
const FIXTURE: &str = r####"<!DOCTYPE html><html><head><base href="https://example.test/dir/x/"><style>body{font:14px/1.5 Arial,"Helvetica Neue",sans-serif}</style></head><body id=body>
<div id=t0 style='color: #336699; background: rgba(10, 20, 30, 0.5) url(a.png) no-repeat 5px 10px; margin: 1px 2px 3px 4px; padding: 5px; border: 2px dashed red; width: 50%; height: 20px; opacity: .5; z-index: 3; position: relative; top: 4px; display: inline-block; font-weight: bold; font-style: italic; letter-spacing: 2px; word-spacing: 1em; text-decoration: underline; text-transform: uppercase; box-shadow: 1px 2px 3px 4px blue; transform: rotate(10deg); transition: opacity 0.3s ease-in 1s; overflow: hidden; white-space: nowrap; cursor: pointer; border-radius: 4px 8px; line-height: 20px; vertical-align: middle; float: none; list-style: square; --x: foo bar'>x</div>
<div id=t1>x</div>
<ul><li id=t2>i</li></ul>
<p id=t3>x</p>
<a href=x id=t4>l</a>
<center id=t5>c</center>
<div style='text-align:right'><center id=t6>c</center></div>
<div align=center id=t7>z</div>
<div align=right id=t8>z</div>
<div align=left id=t9>z</div>
<div style='text-align: right' id=t10>z</div>
<div id=t11 style='background: red'>x</div>
<div id=t12 style='background: url(a.png) center / cover no-repeat fixed border-box, blue'>x</div>
<div id=t13 style='background: linear-gradient(red, blue) padding-box content-box'>x</div>
<div id=t14 style='background-repeat: repeat-x; background-clip: content-box;background-origin:content-box'>x</div>
<div id=t15 style='border: 1px solid red'>x</div>
<div id=t16 style='border-top: 3px dotted blue'>x</div>
<div id=t17 style='border-left: 2px solid; color: red'>x</div>
<div id=t18 style='outline: 2px solid green'>x</div>
<div id=t19 style='list-style: none'>x</div>
<div id=t20 style='list-style: inside url(a.png) square'>x</div>
<div id=t21 style='text-decoration: underline dotted red'>x</div>
<div id=t22 style='text-decoration: line-through'>x</div>
<div id=t23 style='text-decoration: none'>x</div>
<div id=t24 style='padding: 1px 2px'>x</div>
<div id=t25 style='inset: 1px 2px 3px 4px; position: absolute'>x</div>
<div id=t26 style='font: bold 12px/1.2 monospace'>x</div>
<div id=t27 style='font: 12px serif; font-variant: small-caps'>x</div>
<div id=t28 style='flex: 1'>x</div>
<div id=t29 style='overflow: hidden auto'>x</div>
<div id=t30 style='border-radius: 50%'>x</div>
<div id=t31 style='border-radius: 1px 2px 3px 4px / 5px 6px 7px 8px'>x</div>
<div id=t32 style='gap: 3px 4px'>x</div>
<div id=t33 style='transition: all 1s'>x</div>
<div id=t34 style='transition: opacity 1s, transform 2s ease'>x</div>
<div id=t35 style='animation: foo 1s'>x</div>
<div id=t36 style='place-items: center'>x</div>
<div id=t37 style='transform: translate(10px, 20px) scale(2)'>x</div>
<div id=t38 style='transform: translate3d(1px,2px,3px)'>x</div>
<div id=t39 style='transform: none'>x</div>
<div id=t40 style='transform: rotate(0.5turn) skew(10deg)'>x</div>
<div id=t41 style='color: red; border: 1px solid'>x</div>
<div id=t42 style='color: transparent; background-color: hsl(120 100% 25% / .3)'>x</div>
<div id=t43 style='color: rgb(0 0 0 / 50%); border-top-color: currentcolor'>x</div>
<div id=t44 style='display: flex; text-align: left'>x</div>
<div id=t45 style='font-size: 1.5em; line-height: normal'>x</div>
<div id=t46 style='z-index: auto; cursor: url(c.cur) 1 2, pointer'>x</div>
<div id=t47 style='will-change: transform; mix-blend-mode: multiply; filter: blur(2px)'>x</div>
<div id=t48 style='box-shadow: inset 0 0 1px red, 2px 2px blue'>x</div>
<div id=t49 style='text-shadow: 1px 1px red'>x</div>
<div id=t50 style="font-family: 'Times New Roman', serif">x</div>
<div id=t51 style='--a:  1px ; --b: {x}'>x</div>
<div id=t52 style='background-image: url(a.png)'>x</div>
<div id=t53 style='list-style-image: url(../b.png)'>x</div>
<div id=t54 style='margin-inline: 1px 2px; margin-block: 3px 4px; padding-block: 5px; border-inline-width: 6px; border-inline-style: solid; inset-inline: 7px; position: relative; scroll-margin-block: 1px; overflow: clip'>x</div>
</body></html>"####;

/// (element index, property, expected value).
const ROWS: &[(usize, &str, &str)] = &[
    (0, "color", "rgb(51, 102, 153)"),
    (0, "background-color", "rgba(10, 20, 30, 0.5)"),
    (
        0,
        "background-image",
        "url(\"https://example.test/dir/x/a.png\")",
    ),
    (0, "background-repeat", "no-repeat"),
    (0, "background-position", "5px 10px"),
    (
        0,
        "background",
        "rgba(10, 20, 30, 0.5) url(\"https://example.test/dir/x/a.png\") no-repeat scroll 5px 10px / auto padding-box border-box",
    ),
    (0, "margin", "1px 2px 3px 4px"),
    (0, "margin-top", "1px"),
    (0, "padding", "5px"),
    (0, "border", "2px dashed rgb(255, 0, 0)"),
    (0, "border-top-width", "2px"),
    (0, "border-top-style", "dashed"),
    (0, "border-top-color", "rgb(255, 0, 0)"),
    (0, "border-radius", "4px 8px"),
    (0, "height", "20px"),
    (0, "opacity", "0.5"),
    (0, "z-index", "3"),
    (0, "position", "relative"),
    (0, "top", "4px"),
    (0, "display", "inline-block"),
    (0, "font-weight", "700"),
    (0, "font-style", "italic"),
    (0, "font-size", "14px"),
    (0, "font-family", "Arial, \"Helvetica Neue\", sans-serif"),
    (
        0,
        "font",
        "italic 700 14px / 20px Arial, \"Helvetica Neue\", sans-serif",
    ),
    (0, "line-height", "20px"),
    (0, "letter-spacing", "2px"),
    (0, "word-spacing", "14px"),
    (0, "text-decoration", "underline"),
    (0, "text-decoration-line", "underline"),
    (0, "text-transform", "uppercase"),
    (0, "box-shadow", "rgb(0, 0, 255) 1px 2px 3px 4px"),
    (
        0,
        "transform",
        "matrix(0.984808, 0.173648, -0.173648, 0.984808, 0, 0)",
    ),
    (0, "transition", "opacity 0.3s ease-in 1s"),
    (0, "transition-duration", "0.3s"),
    (0, "transition-property", "opacity"),
    (0, "overflow", "hidden"),
    (0, "white-space", "nowrap"),
    (0, "cursor", "pointer"),
    (0, "vertical-align", "middle"),
    (0, "float", "none"),
    (0, "list-style", "outside none square"),
    (0, "list-style-type", "square"),
    (0, "--x", "foo bar"),
    (0, "--nope", ""),
    (0, "text-align", "start"),
    (0, "content", "normal"),
    (0, "font-variant", "normal"),
    (0, "flex", "0 1 auto"),
    (0, "animation-name", "none"),
    (0, "outline", "rgb(51, 102, 153) none 3px"),
    (0, "outline-color", "rgb(51, 102, 153)"),
    (0, "border-color", "rgb(255, 0, 0)"),
    (0, "border-style", "dashed"),
    (0, "padding-inline", "5px"),
    (0, "background-size", "auto"),
    (0, "background-position-x", "5px"),
    (0, "visibility", "visible"),
    (0, "clip", "auto"),
    (0, "filter", "none"),
    (0, "text-shadow", "none"),
    (0, "max-width", "none"),
    (0, "pointer-events", "auto"),
    (1, "color", "rgb(0, 0, 0)"),
    (1, "background-color", "rgba(0, 0, 0, 0)"),
    (1, "font-family", "Arial, \"Helvetica Neue\", sans-serif"),
    (1, "font-size", "14px"),
    (1, "font-weight", "400"),
    (1, "line-height", "21px"),
    (1, "display", "block"),
    (1, "text-align", "start"),
    (1, "margin", "0px"),
    (1, "padding", "0px"),
    (1, "border", "0px none rgb(0, 0, 0)"),
    (1, "opacity", "1"),
    (1, "content", "normal"),
    (1, "transition", "all"),
    (
        1,
        "font",
        "14px / 21px Arial, \"Helvetica Neue\", sans-serif",
    ),
    (
        1,
        "background",
        "rgba(0, 0, 0, 0) none repeat scroll 0% 0% / auto padding-box border-box",
    ),
    (1, "border-width", "0px"),
    (1, "border-radius", "0px"),
    (1, "overflow", "visible"),
    (1, "flex", "0 1 auto"),
    (1, "animation", "none"),
    (1, "--x", ""),
    (2, "display", "list-item"),
    (2, "list-style", "outside none disc"),
    (2, "list-style-type", "disc"),
    (2, "padding-left", "0px"),
    (2, "margin", "0px"),
    (3, "margin", "14px 0px"),
    (3, "margin-top", "14px"),
    (3, "display", "block"),
    (4, "color", "rgb(0, 0, 238)"),
    (4, "text-decoration", "underline"),
    (4, "cursor", "pointer"),
    (4, "text-decoration-line", "underline"),
    (5, "text-align", "-webkit-center"),
    (6, "text-align", "-webkit-center"),
    (7, "text-align", "-webkit-center"),
    (8, "text-align", "-webkit-right"),
    (9, "text-align", "-webkit-left"),
    (10, "text-align", "right"),
    (
        11,
        "background",
        "rgb(255, 0, 0) none repeat scroll 0% 0% / auto padding-box border-box",
    ),
    (11, "background-color", "rgb(255, 0, 0)"),
    (
        12,
        "background",
        "url(\"https://example.test/dir/x/a.png\") no-repeat fixed 50% 50% / cover border-box border-box, rgb(0, 0, 255) none repeat scroll 0% 0% / auto padding-box border-box",
    ),
    (12, "background-position", "50% 50%, 0% 0%"),
    (12, "background-size", "cover, auto"),
    (12, "background-origin", "border-box, padding-box"),
    (12, "background-clip", "border-box, border-box"),
    (12, "background-attachment", "fixed, scroll"),
    (12, "background-repeat", "no-repeat, repeat"),
    (
        13,
        "background",
        "rgba(0, 0, 0, 0) linear-gradient(rgb(255, 0, 0), rgb(0, 0, 255)) repeat scroll 0% 0% / auto padding-box content-box",
    ),
    (
        14,
        "background",
        "rgba(0, 0, 0, 0) none repeat-x scroll 0% 0% / auto content-box content-box",
    ),
    (15, "border", "1px solid rgb(255, 0, 0)"),
    (15, "border-top", "1px solid rgb(255, 0, 0)"),
    (15, "border-left", "1px solid rgb(255, 0, 0)"),
    (15, "border-width", "1px"),
    (15, "border-style", "solid"),
    (15, "border-color", "rgb(255, 0, 0)"),
    (16, "border", ""),
    (16, "border-top", "3px dotted rgb(0, 0, 255)"),
    (16, "border-right", "0px none rgb(0, 0, 0)"),
    (
        16,
        "border-color",
        "rgb(0, 0, 255) rgb(0, 0, 0) rgb(0, 0, 0)",
    ),
    (16, "border-style", "dotted none none"),
    (16, "border-width", "3px 0px 0px"),
    (17, "border-left", "2px solid rgb(255, 0, 0)"),
    (17, "border-left-color", "rgb(255, 0, 0)"),
    (17, "color", "rgb(255, 0, 0)"),
    (18, "outline", "rgb(0, 128, 0) solid 2px"),
    (18, "outline-offset", "0px"),
    (19, "list-style", "outside none none"),
    (
        20,
        "list-style",
        "inside url(\"https://example.test/dir/x/a.png\") square",
    ),
    (21, "text-decoration", "underline dotted rgb(255, 0, 0)"),
    (21, "text-decoration-color", "rgb(255, 0, 0)"),
    (21, "text-decoration-style", "dotted"),
    (22, "text-decoration", "line-through"),
    (23, "text-decoration", "none"),
    (24, "padding", "1px 2px"),
    (24, "padding-top", "1px"),
    (25, "inset", "1px 2px 3px 4px"),
    (25, "top", "1px"),
    (26, "font", "700 12px / 14.4px monospace"),
    (26, "font-family", "monospace"),
    (26, "font-size", "12px"),
    (26, "line-height", "14.4px"),
    (26, "font-weight", "700"),
    (26, "font-variant", "normal"),
    (26, "font-stretch", "100%"),
    (27, "font", "small-caps 12px serif"),
    (27, "font-variant", "small-caps"),
    (28, "flex", "1 1 0%"),
    (28, "flex-grow", "1"),
    (28, "flex-basis", "0%"),
    (28, "flex-flow", "row nowrap"),
    (29, "overflow", "hidden auto"),
    (29, "overflow-x", "hidden"),
    (29, "overflow-y", "auto"),
    (30, "border-radius", "50%"),
    (30, "border-top-left-radius", "50%"),
    (31, "border-radius", "1px 2px 3px 4px / 5px 6px 7px 8px"),
    (32, "gap", "3px 4px"),
    (32, "row-gap", "3px"),
    (32, "column-gap", "4px"),
    (33, "transition", "1s"),
    (33, "transition-delay", "0s"),
    (33, "transition-timing-function", "ease"),
    (33, "transition-behavior", "normal"),
    (34, "transition", "opacity 1s, transform 2s"),
    (35, "animation", "1s foo"),
    (35, "animation-name", "foo"),
    (35, "animation-iteration-count", "1"),
    (35, "animation-duration", "1s"),
    (36, "place-items", "center"),
    (36, "align-items", "center"),
    (36, "justify-items", "center"),
    (37, "transform", "matrix(2, 0, 0, 2, 10, 20)"),
    (
        38,
        "transform",
        "matrix3d(1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 1, 2, 3, 1)",
    ),
    (39, "transform", "none"),
    (40, "transform", "matrix(-1, 0, -0.176327, -1, 0, 0)"),
    (41, "border-color", "rgb(255, 0, 0)"),
    (41, "border", "1px solid rgb(255, 0, 0)"),
    (42, "color", "rgba(0, 0, 0, 0)"),
    (42, "background-color", "rgba(0, 128, 0, 0.3)"),
    (43, "color", "rgba(0, 0, 0, 0.5)"),
    (43, "border-top-color", "rgba(0, 0, 0, 0.5)"),
    (44, "display", "flex"),
    (44, "text-align", "left"),
    (44, "text-align-last", "auto"),
    (45, "font-size", "21px"),
    (45, "line-height", "normal"),
    (46, "z-index", "auto"),
    (
        46,
        "cursor",
        "url(\"https://example.test/dir/x/c.cur\") 1 2, pointer",
    ),
    (47, "will-change", "transform"),
    (47, "mix-blend-mode", "multiply"),
    (47, "filter", "blur(2px)"),
    (
        48,
        "box-shadow",
        "rgb(255, 0, 0) 0px 0px 1px 0px inset, rgb(0, 0, 255) 2px 2px 0px 0px",
    ),
    (49, "text-shadow", "rgb(255, 0, 0) 1px 1px 0px"),
    (50, "font-family", "\"Times New Roman\", serif"),
    (51, "--a", "1px"),
    (51, "--b", "{x}"),
    (
        52,
        "background-image",
        "url(\"https://example.test/dir/x/a.png\")",
    ),
    (
        53,
        "list-style-image",
        "url(\"https://example.test/dir/b.png\")",
    ),
    (54, "margin-inline", "1px 2px"),
    (54, "margin-block", "3px 4px"),
    (54, "padding-block", "5px"),
    (54, "border-inline-width", "6px"),
    (54, "border-inline", "6px solid rgb(0, 0, 0)"),
    (54, "inset-inline", "7px"),
    (54, "overflow", "clip"),
    (54, "border-block-start-width", "0px"),
];

#[test]
fn computed_values_match_chrome() {
    let (doc, engine) = styled(FIXTURE);
    let mismatches: Vec<String> = ROWS
        .iter()
        .filter_map(|&(index, property, expected)| {
            let node = element(&doc, &format!("t{index}"));
            let actual = engine.computed_value(node, None, property);
            (actual.as_deref() != Some(expected))
                .then(|| format!("t{index} {property}: expected {expected:?}, got {actual:?}"))
        })
        .collect();
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn properties_behind_the_unimplemented_pref_compute() {
    // Chrome 153: `user-select` is `auto` by default.
    let (doc, engine) = styled(FIXTURE);
    let node = element(&doc, "t0");
    assert_eq!(
        engine.computed_value(node, None, "user-select").as_deref(),
        Some("auto")
    );
    assert_eq!(
        engine
            .computed_value(node, None, "text-overflow")
            .as_deref(),
        Some("clip")
    );
}

#[test]
fn an_unknown_property_has_no_value() {
    let (doc, engine) = styled(FIXTURE);
    let node = element(&doc, "t0");
    assert_eq!(engine.computed_value(node, None, "bogus-prop"), None);
    assert_eq!(engine.computed_value(node, None, ""), None);
}

#[test]
fn a_node_without_a_stored_style_has_no_value() {
    let (doc, engine) = styled(FIXTURE);
    assert_eq!(
        engine.computed_value(doc.document_node(), None, "color"),
        None
    );
    assert_eq!(
        engine.computed_value(element(&doc, "t0"), Some(PseudoKind::Before), "content"),
        None,
        "t0 has no ::before rule, so the flush stored none"
    );
}

#[test]
fn legacy_text_align_values_report_their_webkit_names() {
    // Chrome has no `-moz-*` keywords: `<center>` and `align=` compute to
    // `-webkit-*` there, and the serialisation maps Stylo's Gecko names.
    let (doc, engine) = styled(FIXTURE);
    let align = |id: &str| engine.computed_value(element(&doc, id), None, "text-align");
    assert_eq!(align("t5").as_deref(), Some("-webkit-center"));
    assert_eq!(align("t7").as_deref(), Some("-webkit-center"));
    assert_eq!(align("t8").as_deref(), Some("-webkit-right"));
    assert_eq!(align("t9").as_deref(), Some("-webkit-left"));
    assert_eq!(align("t10").as_deref(), Some("right"), "CSS keeps its name");
}

#[test]
fn property_names_are_sorted_longhands_without_aliases() {
    let names = StyleEngine::computed_property_names();
    assert!(
        names.windows(2).all(|pair| pair[0] < pair[1]),
        "sorted, unique"
    );
    for longhand in [
        "color",
        "display",
        "font-size",
        "margin-top",
        "transition-duration",
        "counter-reset",
        "counter-increment",
        "text-overflow",
        "user-select",
    ] {
        assert!(names.contains(&longhand), "{longhand} is listed");
    }
    for other in ["margin", "font", "background", "--x", "word-wrap"] {
        assert!(
            !names.contains(&other),
            "{other} is a shorthand, alias or custom"
        );
    }
    assert!(names.len() > 150, "{} names", names.len());
}

const HIDDEN: &str = "<!DOCTYPE html><html><head></head><body>\
    <div id=hid style='display: none; color: rgb(1, 2, 3)'>\
      <p id=hp style='color: green; margin: 3px 4px'>x\
        <span id=deep style='font-weight: bold'>y</span></p>\
      <b id=plain>z</b>\
    </div>\
    <div id=shown>s</div></body></html>";

#[test]
fn elements_under_display_none_resolve_on_demand() {
    // The values are Chrome 153's `getComputedStyle` for the same markup.
    let (doc, engine) = styled(HIDDEN);
    let value = |id: &str, property: &str| {
        engine
            .computed_value_in(&doc, element(&doc, id), None, property)
            .unwrap()
    };
    assert_eq!(
        engine.computed_value(element(&doc, "hp"), None, "color"),
        None
    );
    assert_eq!(value("hp", "color").as_deref(), Some("rgb(0, 128, 0)"));
    assert_eq!(value("hp", "margin").as_deref(), Some("3px 4px"));
    assert_eq!(value("hp", "display").as_deref(), Some("block"));
    assert_eq!(value("hp", "font-size").as_deref(), Some("16px"));
    assert_eq!(
        value("deep", "color").as_deref(),
        Some("rgb(0, 128, 0)"),
        "inherited"
    );
    assert_eq!(value("deep", "font-weight").as_deref(), Some("700"));
    assert_eq!(value("deep", "display").as_deref(), Some("inline"));
    assert_eq!(value("plain", "color").as_deref(), Some("rgb(1, 2, 3)"));
    assert_eq!(value("plain", "font-weight").as_deref(), Some("700"));
    assert_eq!(value("hid", "display").as_deref(), Some("none"));
}

#[test]
fn a_styled_element_resolves_to_its_stored_value() {
    let (doc, engine) = styled(HIDDEN);
    let node = element(&doc, "shown");
    assert_eq!(
        engine
            .computed_value_in(&doc, node, None, "display")
            .unwrap(),
        engine.computed_value(node, None, "display")
    );
}

#[test]
fn pseudo_elements_without_a_box_report_browser_defaults() {
    // Chrome 153: `getComputedStyle(div, '::before')` for a div with no
    // generated content has `content: none`, and `::marker` of a non-list
    // element has `content: normal`.
    let (doc, engine) = styled(FIXTURE);
    let node = element(&doc, "t1");
    let value = |pseudo, property| {
        engine
            .computed_value_in(&doc, node, Some(pseudo), property)
            .unwrap()
    };
    assert_eq!(
        value(PseudoKind::Before, "content").as_deref(),
        Some("none")
    );
    assert_eq!(value(PseudoKind::After, "content").as_deref(), Some("none"));
    assert_eq!(
        value(PseudoKind::Before, "display").as_deref(),
        Some("inline")
    );
    assert_eq!(
        value(PseudoKind::Marker, "content").as_deref(),
        Some("normal")
    );
    assert_eq!(value(PseudoKind::FirstLine, "color"), None);
    assert_eq!(value(PseudoKind::FirstLetter, "color"), None);
}
