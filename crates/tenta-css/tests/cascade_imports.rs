#![cfg(feature = "stylo")]

use tenta_css::CssStyleSheet;
use tenta_css::cascade::import_urls;
use url::Url;

fn base() -> Url {
    Url::parse("http://fixture.test/css/main.css").unwrap()
}

fn urls(text: &str) -> Vec<String> {
    import_urls(text, &base())
        .iter()
        .map(Url::to_string)
        .collect()
}

#[test]
fn string_and_url_forms_resolve_against_the_sheet() {
    assert_eq!(
        urls("@import \"a.css\"; @import url(b.css) print; @import url('/c.css') layer(x);"),
        [
            "http://fixture.test/css/a.css",
            "http://fixture.test/css/b.css",
            "http://fixture.test/c.css"
        ]
    );
}

#[test]
fn comments_cdo_cdc_charset_and_layer_statements_precede_imports() {
    assert_eq!(
        urls("@charset \"utf-8\"; <!-- /* c */ --> @layer a, b; @import 'a.css';"),
        ["http://fixture.test/css/a.css"]
    );
}

#[test]
fn the_scan_stops_at_the_first_rule() {
    assert!(urls("p{} @import 'late.css';").is_empty());
    assert!(urls("@media print{} @import 'late.css';").is_empty());
    assert!(urls("@layer a { p{} } @import 'late.css';").is_empty());
}

#[test]
fn a_final_import_may_omit_its_semicolon() {
    assert_eq!(urls("@import 'a.css'"), ["http://fixture.test/css/a.css"]);
}

#[test]
fn a_semicolon_inside_a_string_or_url_does_not_end_the_rule() {
    assert_eq!(
        urls("@import 'a;b.css'; @import url(\"c;d.css\");"),
        [
            "http://fixture.test/css/a;b.css",
            "http://fixture.test/css/c;d.css"
        ]
    );
}

#[test]
fn malformed_and_lookalike_rules_are_skipped() {
    assert_eq!(
        urls("@import ; @import 'ok.css';"),
        ["http://fixture.test/css/ok.css"]
    );
    assert!(urls("@importer 'a.css';").is_empty());
}

#[test]
fn an_import_in_the_text_is_inert_until_the_engine_supplies_a_loader() {
    let sheet = CssStyleSheet::parse(
        "@import 'a.css'; p { color: red }",
        Url::parse("http://fixture.test/").unwrap(),
    );
    assert_eq!(
        sheet.rule_count(),
        1,
        "only the style rule survives AllowImportRules::No"
    );
}
