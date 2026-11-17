//! `@import` served from the bodies the host prefetched.
#![cfg(feature = "test-util")]

mod cascade_support;

use cascade_support::*;
use tenta_css::cascade::test_util::style_html;
use tenta_css::cascade::{ImportedSheet, StyleEnv, StyleSource};
use url::Url;

const BODY: &str = "<!DOCTYPE html><html><body><p id=p class=c>x</p></body></html>";

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

fn imported(at: &str, text: &str) -> ImportedSheet {
    ImportedSheet {
        url: url(at),
        final_url: url(at),
        text: text.to_owned(),
    }
}

fn root(text: &str, imports: Vec<ImportedSheet>) -> StyleSource {
    StyleSource {
        owner: None,
        scope: None,
        url: url("http://t.test/css/main.css"),
        text: text.to_owned(),
        media: None,
        disabled: false,
        imports,
    }
}

/// `property` of `#p` after styling the fixture with `sources` under `env`.
fn computed_p(sources: Vec<StyleSource>, env: StyleEnv, property: &str) -> String {
    let (doc, mut engine) = style_html(BODY, env);
    engine.set_sources(sources);
    engine.flush(&doc);
    value(&doc, &engine, "p", property)
}

fn color_of(sources: Vec<StyleSource>) -> String {
    computed_p(sources, StyleEnv::default(), "color")
}

#[test]
fn an_import_is_applied_from_the_prefetched_body() {
    let sources = vec![root(
        "@import 'a.css'; p { width: 5px }",
        vec![imported(
            "http://t.test/css/a.css",
            "p { color: rgb(1, 2, 3) }",
        )],
    )];
    assert_eq!(color_of(sources.clone()), "rgb(1, 2, 3)");
    assert_eq!(computed_p(sources, StyleEnv::default(), "width"), "5px");
}

#[test]
fn rules_after_an_import_win_over_it_at_equal_specificity() {
    let sources = vec![root(
        "@import url(a.css); p { color: rgb(9, 9, 9) }",
        vec![imported(
            "http://t.test/css/a.css",
            "p { color: rgb(1, 2, 3) }",
        )],
    )];
    assert_eq!(color_of(sources), "rgb(9, 9, 9)");
}

#[test]
fn a_missing_body_is_a_failed_import_and_the_rest_of_the_sheet_applies() {
    let sources = vec![root(
        "@import 'gone.css'; p { width: 5px }",
        vec![imported(
            "http://t.test/css/other.css",
            "p { color: rgb(1, 2, 3) }",
        )],
    )];
    assert_eq!(color_of(sources.clone()), "rgb(0, 0, 0)");
    assert_eq!(computed_p(sources, StyleEnv::default(), "width"), "5px");
}

#[test]
fn the_import_media_query_is_evaluated_against_the_environment() {
    let sources = vec![root(
        "@import 'a.css' print; @import 'b.css' (min-width: 600px);",
        vec![
            imported("http://t.test/css/a.css", "p { color: rgb(1, 0, 0) }"),
            imported("http://t.test/css/b.css", "p { width: 33px }"),
        ],
    )];
    let screen = StyleEnv::default();
    assert_eq!(
        computed_p(sources.clone(), screen.clone(), "color"),
        "rgb(0, 0, 0)"
    );
    assert_eq!(computed_p(sources.clone(), screen, "width"), "33px");

    let print = StyleEnv {
        print: true,
        viewport: (400.0, 300.0),
        ..StyleEnv::default()
    };
    assert_eq!(
        computed_p(sources.clone(), print.clone(), "color"),
        "rgb(1, 0, 0)"
    );
    assert_eq!(computed_p(sources, print, "width"), "auto");
}

#[test]
fn import_layers_order_the_cascade_before_source_order_and_specificity() {
    let imports = vec![
        imported("http://t.test/css/one.css", "p { color: rgb(1, 0, 0) }"),
        imported("http://t.test/css/two.css", "p { color: rgb(2, 0, 0) }"),
    ];
    // Declared order of layers, not import order, decides between layers.
    let declared_backwards = vec![root(
        "@layer two, one; @import 'one.css' layer(one); @import 'two.css' layer(two);",
        imports.clone(),
    )];
    assert_eq!(color_of(declared_backwards), "rgb(1, 0, 0)");
    let in_import_order = vec![root(
        "@import 'one.css' layer(one); @import 'two.css' layer(two);",
        imports,
    )];
    assert_eq!(color_of(in_import_order), "rgb(2, 0, 0)");

    // Unlayered author rules beat layered ones whatever their specificity.
    let unlayered = vec![root(
        "@import 'a.css' layer(base); p { color: rgb(9, 9, 9) }",
        vec![imported(
            "http://t.test/css/a.css",
            "#p.c { color: rgb(1, 0, 0) }",
        )],
    )];
    assert_eq!(color_of(unlayered), "rgb(9, 9, 9)");
    let not_layered = vec![root(
        "@import 'a.css'; p { color: rgb(9, 9, 9) }",
        vec![imported(
            "http://t.test/css/a.css",
            "#p.c { color: rgb(1, 0, 0) }",
        )],
    )];
    assert_eq!(color_of(not_layered), "rgb(1, 0, 0)");
}

#[test]
fn import_supports_conditions_are_evaluated() {
    let imports = vec![imported(
        "http://t.test/css/a.css",
        "p { color: rgb(1, 0, 0) }",
    )];
    let supported = vec![root(
        "@import 'a.css' supports(display: grid);",
        imports.clone(),
    )];
    assert_eq!(color_of(supported), "rgb(1, 0, 0)");
    let unsupported = vec![root(
        "@import 'a.css' supports(not-a-property: 1);",
        imports,
    )];
    assert_eq!(color_of(unsupported), "rgb(0, 0, 0)");
}

#[test]
fn nested_imports_resolve_against_the_final_url_of_their_importer() {
    let outer = ImportedSheet {
        url: url("http://t.test/css/a.css"),
        final_url: url("http://cdn.test/v2/a.css"),
        text: "@import 'b.css'; p { width: 7px }".to_owned(),
    };
    let nested = imported("http://cdn.test/v2/b.css", "p { color: rgb(4, 5, 6) }");
    let sources = vec![root("@import 'a.css';", vec![outer, nested])];
    assert_eq!(color_of(sources.clone()), "rgb(4, 5, 6)");
    assert_eq!(computed_p(sources, StyleEnv::default(), "width"), "7px");
}

#[test]
fn urls_in_an_imported_sheet_resolve_against_its_final_url() {
    let sheet = ImportedSheet {
        url: url("http://t.test/css/a.css"),
        final_url: url("http://cdn.test/v2/a.css"),
        text: "p { background-image: url(img.png) }".to_owned(),
    };
    let sources = vec![root("@import 'a.css';", vec![sheet])];
    let image = computed_p(sources, StyleEnv::default(), "background-image");
    assert!(image.contains("http://cdn.test/v2/img.png"), "{image}");
}

#[test]
fn the_lookup_ignores_fragments() {
    let sources = vec![root(
        "@import 'a.css#section';",
        vec![imported(
            "http://t.test/css/a.css",
            "p { color: rgb(1, 0, 0) }",
        )],
    )];
    assert_eq!(color_of(sources), "rgb(1, 0, 0)");
}

#[test]
fn import_cycles_terminate() {
    let sources = vec![root(
        "@import 'a.css'; p { width: 3px }",
        vec![
            imported(
                "http://t.test/css/a.css",
                "@import 'b.css'; p { color: rgb(1, 0, 0) }",
            ),
            imported(
                "http://t.test/css/b.css",
                "@import 'a.css'; @import 'main.css'; p { height: 4px }",
            ),
        ],
    )];
    assert_eq!(color_of(sources.clone()), "rgb(1, 0, 0)");
    assert_eq!(
        computed_p(sources.clone(), StyleEnv::default(), "height"),
        "4px"
    );
    assert_eq!(computed_p(sources, StyleEnv::default(), "width"), "3px");
}

#[test]
fn a_disabled_source_does_not_load_its_imports() {
    let mut source = root(
        "@import 'a.css';",
        vec![imported(
            "http://t.test/css/a.css",
            "p { color: rgb(1, 0, 0) }",
        )],
    );
    source.disabled = true;
    assert_eq!(color_of(vec![source]), "rgb(0, 0, 0)");
}
