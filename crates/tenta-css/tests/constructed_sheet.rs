#![cfg(feature = "stylo")]

use tenta_css::{CssRuleType, CssStyleSheet, CssomError};
use url::Url;

fn sheet(css: &str) -> CssStyleSheet {
    CssStyleSheet::parse(css, Url::parse("https://example.test/app/").unwrap())
}

#[test]
fn insert_accepts_every_rule_kind_and_refuses_atomically() {
    let mut sheet = sheet("a { color: red }");
    let original = sheet.rule_at(0).unwrap();
    assert_eq!(
        sheet
            .insert_rule("@namespace svg url('http://www.w3.org/2000/svg');", 0)
            .err(),
        Some(CssomError::InvalidState),
        "Stylo rejects a namespace rule once the sheet contains a style rule"
    );
    let mut empty = self::sheet("");
    assert_eq!(
        empty
            .insert_rule("@namespace svg url('http://www.w3.org/2000/svg');", 0)
            .err(),
        Some(CssomError::InvalidState),
        "a namespace rule cannot be inserted until the sheet's namespace map follows it"
    );
    assert_eq!(empty.rule_count(), 0);
    assert_eq!(empty.revision(), 0);
    assert_eq!(sheet.rule_count(), 1);
    assert_eq!(sheet.revision(), 0);

    sheet
        .insert_rule("@media all { b { color: blue } }", 1)
        .unwrap();
    let media = sheet.rule_at(1).unwrap();
    assert_eq!(media.rule_type(), CssRuleType::Media);
    assert_eq!(media.condition_text().as_deref(), Some("all"));
    assert_eq!(media.child_count(), 1);
    assert!(sheet.rule_at(0).unwrap().same_rule(&original));

    sheet
        .insert_rule("a { @media all { & b { color: blue } } }", 1)
        .unwrap();
    let nested = sheet.rule_at(1).unwrap().child_at(0).unwrap();
    assert_eq!(nested.rule_type(), CssRuleType::Media);
    assert_eq!(
        nested.child_at(0).unwrap().selector_text().as_deref(),
        Some("& b")
    );

    for (source, kind) in [
        (
            "@keyframes spin { to { rotate: 1turn } }",
            CssRuleType::Keyframes,
        ),
        (
            "@font-face { font-family: X; src: url(x.woff2) }",
            CssRuleType::FontFace,
        ),
        (
            "@supports (display: grid) { c { display: grid } }",
            CssRuleType::Supports,
        ),
        ("@layer base { c { color: red } }", CssRuleType::LayerBlock),
        ("@layer reset, base;", CssRuleType::LayerStatement),
        ("@scope (.card) { c { color: red } }", CssRuleType::Scope),
        (
            "@property --x { syntax: '<length>'; inherits: false; initial-value: 0px }",
            CssRuleType::Property,
        ),
    ] {
        let index = sheet.rule_count();
        sheet.insert_rule(source, index).unwrap();
        assert_eq!(sheet.rule_at(index).unwrap().rule_type(), kind, "{source}");
    }

    // Stylo's Servo configuration has no container queries, so the rule is
    // unknown to it and refused as any unknown at-rule is.
    let count = sheet.rule_count();
    let revision = sheet.revision();
    assert_eq!(
        sheet
            .insert_rule("@container (min-width: 10px) { c { color: red } }", count)
            .err(),
        Some(CssomError::Syntax)
    );
    assert_eq!(sheet.rule_count(), count);
    assert_eq!(sheet.revision(), revision);

    sheet.insert_rule("b { float: left }", 0).unwrap();
    assert!(sheet.rule_at(1).unwrap().same_rule(&original));

    let count = sheet.rule_count();
    sheet
        .insert_rule("a { & b { color: blue } }", count)
        .unwrap();
    let parent = sheet.rule_at(count).unwrap();
    let child = parent.child_at(0).unwrap();
    assert!(child.parent_rule().unwrap().same_rule(&parent));
}

#[test]
fn constructed_import_errors_obey_cssom_precedence() {
    let mut sheet = sheet("a { color: red }");
    // A constructed sheet rejects @import before invoking the list-insertion
    // algorithm, even for an index that would otherwise be out of range.
    for index in [0, 1, 2] {
        assert_eq!(
            sheet
                .insert_rule("/* comment */ @IMPORT url('other.css');", index)
                .err(),
            Some(CssomError::Syntax)
        );
    }
    assert_eq!(
        sheet.insert_rule("b { color: blue }", 2).err(),
        Some(CssomError::IndexSize)
    );
    assert_eq!(sheet.rule_count(), 1);
    assert_eq!(sheet.revision(), 0);
}

#[test]
fn generic_rule_syntax_precedes_insert_index_but_css_grammar_follows_it() {
    let mut sheet = sheet("a { color: red }");
    for source in ["garbage", "b {} c {}"] {
        assert_eq!(
            sheet.insert_rule(source, 99).err(),
            Some(CssomError::Syntax),
            "generic CSS Syntax must reject {source:?} before checking the list index"
        );
    }
    assert_eq!(
        sheet.insert_rule("@unsupported value;", 99).err(),
        Some(CssomError::IndexSize),
        "a generic rule reaches Stylo's index check before CSS grammar parsing"
    );
    assert_eq!(sheet.rule_count(), 1);
    assert_eq!(sheet.revision(), 0);
}

#[test]
fn constructed_replace_removes_imports_and_keeps_every_other_rule() {
    let mut sheet = sheet("a { color: red }");
    let old = sheet.rule_at(0).unwrap();
    sheet
        .replace_sync(
            "@import url('other.css'); b { color: blue } \
             @supports (display:grid) { c { display:grid } }",
        )
        .unwrap();
    assert_eq!(sheet.rule_count(), 2);
    assert_eq!(sheet.revision(), 1);
    assert!(!old.is_attached());
    assert_eq!(
        sheet.rule_at(0).unwrap().selector_text().as_deref(),
        Some("b")
    );
    let supports = sheet.rule_at(1).unwrap();
    assert_eq!(supports.rule_type(), CssRuleType::Supports);
    // Stylo keeps a supports condition as it was written.
    assert_eq!(supports.condition_text().as_deref(), Some("(display:grid)"));
    assert_eq!(supports.supports_matches(), Some(true));
}

#[test]
fn a_replacement_nested_too_deeply_to_read_back_is_refused_whole() {
    let mut sheet = sheet("a { color: red }");
    let old = sheet.rule_at(0).unwrap();
    let deep = format!(
        "{}a {{ color: red }}{}",
        "@media all {".repeat(200),
        "}".repeat(200)
    );
    assert_eq!(
        sheet.replace_sync(&deep).err(),
        Some(CssomError::DepthLimit)
    );
    assert_eq!(
        sheet.insert_rule(&deep, 0).err(),
        Some(CssomError::DepthLimit)
    );
    assert_eq!(sheet.revision(), 0);
    assert!(sheet.rule_at(0).unwrap().same_rule(&old));
}
