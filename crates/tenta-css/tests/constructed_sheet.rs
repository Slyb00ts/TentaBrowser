#![cfg(feature = "stylo")]

use tenta_css::{CssStyleSheet, CssomError};
use url::Url;

fn sheet(css: &str) -> CssStyleSheet {
    CssStyleSheet::parse(css, Url::parse("https://example.test/app/").unwrap())
}

#[test]
fn constructed_insert_accepts_style_rule_trees_atomically() {
    let mut sheet = sheet("a { color: red }");
    let original = sheet.rule_at(0).unwrap();
    let initial_revision = sheet.revision();
    assert_eq!(
        sheet
            .insert_supported_style_rule("@media all { b { color: blue } }", 1)
            .err(),
        Some(CssomError::UnsupportedRuleType)
    );
    assert_eq!(
        sheet
            .insert_supported_style_rule("@namespace svg url('http://www.w3.org/2000/svg');", 0)
            .err(),
        Some(CssomError::InvalidState),
        "Stylo rejects a namespace rule once the sheet contains a style rule"
    );
    let mut empty = self::sheet("");
    assert_eq!(
        empty
            .insert_supported_style_rule("@namespace svg url('http://www.w3.org/2000/svg');", 0)
            .err(),
        Some(CssomError::UnsupportedRuleType),
        "a valid namespace rule still has no concrete JS rule interface"
    );
    assert_eq!(empty.rule_count(), 0);
    assert_eq!(empty.revision(), 0);
    assert_eq!(
        sheet
            .insert_supported_style_rule("a { @media all { & b { color: blue } } }", 1)
            .err(),
        Some(CssomError::UnsupportedRuleType),
        "an unsupported nested rule kind must leave the tree unchanged"
    );
    assert_eq!(sheet.rule_count(), 1);
    assert_eq!(sheet.revision(), initial_revision);
    assert!(sheet.rule_at(0).unwrap().same_rule(&original));

    sheet
        .insert_supported_style_rule("b { float: left }", 0)
        .unwrap();
    assert_eq!(sheet.rule_count(), 2);
    assert!(sheet.rule_at(1).unwrap().same_rule(&original));

    sheet
        .insert_supported_style_rule("a { & b { color: blue } }", 2)
        .unwrap();
    let parent = sheet.rule_at(2).unwrap();
    let child = parent.child_at(0).unwrap();
    assert!(child.parent_rule().unwrap().same_rule(&parent));
    assert_eq!(sheet.rule_count(), 3);
}

#[test]
fn constructed_import_errors_obey_cssom_precedence() {
    let mut sheet = sheet("a { color: red }");
    // A constructed sheet rejects @import before invoking the list-insertion
    // algorithm, even for an index that would otherwise be out of range.
    for index in [0, 1, 2] {
        assert_eq!(
            sheet
                .insert_supported_style_rule("/* comment */ @IMPORT url('other.css');", index)
                .err(),
            Some(CssomError::Syntax)
        );
    }
    assert_eq!(
        sheet
            .insert_supported_style_rule("b { color: blue }", 2)
            .err(),
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
            sheet.insert_supported_style_rule(source, 99).err(),
            Some(CssomError::Syntax),
            "generic CSS Syntax must reject {source:?} before checking the list index"
        );
    }
    assert_eq!(
        sheet
            .insert_supported_style_rule("@unsupported value;", 99)
            .err(),
        Some(CssomError::IndexSize),
        "a generic rule reaches Stylo's index check before CSS grammar parsing"
    );
    assert_eq!(sheet.rule_count(), 1);
    assert_eq!(sheet.revision(), 0);
}

#[test]
fn constructed_replace_removes_imports_and_rejects_unsupported_rules_atomically() {
    let mut sheet = sheet("a { color: red }");
    let old = sheet.rule_at(0).unwrap();
    assert_eq!(
        sheet
            .replace_sync_supported_style_rules(
                "b { color: blue } @supports (display:grid) { c { display:grid } }"
            )
            .err(),
        Some(CssomError::UnsupportedRuleType)
    );
    assert_eq!(sheet.revision(), 0);
    assert!(sheet.rule_at(0).unwrap().same_rule(&old));

    sheet
        .replace_sync_supported_style_rules("@import url('other.css'); p { color: blue }")
        .unwrap();
    assert_eq!(sheet.rule_count(), 1);
    assert_eq!(sheet.revision(), 1);
    assert!(!old.is_attached());
    assert_eq!(
        sheet.rule_at(0).unwrap().selector_text().as_deref(),
        Some("p")
    );
}
