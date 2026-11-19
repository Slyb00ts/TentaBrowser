#![cfg(feature = "stylo")]

use tenta_css::CssStyleSheet;
use url::Url;

fn sheet(css: &str) -> CssStyleSheet {
    CssStyleSheet::parse(css, Url::parse("https://example.test/app/").unwrap())
}

#[test]
fn graph_components_follow_live_parent_rule_membership() {
    let mut sheet = sheet("a { & b { color: red } }");
    let parent = sheet.rule_at(0).unwrap();
    let child = parent.child_at(0).unwrap();
    let top = sheet.graph_top_rule_identities();
    assert_eq!(top, vec![parent.graph_component_identity()]);
    assert_eq!(child.graph_component_identity(), top[0]);
    assert!(child.graph_component_root().same_rule(&parent));

    sheet.delete_rule(0).unwrap();
    assert!(sheet.graph_top_rule_identities().is_empty());
    assert_eq!(
        child.graph_component_identity(),
        parent.graph_component_identity()
    );
    assert!(parent.graph_reaches_parent_style_sheet());

    sheet.delete_nested_rule(&parent, 0).unwrap();
    assert_ne!(
        child.graph_component_identity(),
        parent.graph_component_identity()
    );
    assert!(child.graph_component_root().same_rule(&child));
    assert!(!parent.graph_reaches_parent_style_sheet());
}

#[test]
fn graph_sheet_links_end_at_replacement_epoch() {
    let mut sheet = sheet("a { & b { color: red } }");
    let parent = sheet.rule_at(0).unwrap();
    let child = parent.child_at(0).unwrap();
    sheet.delete_rule(0).unwrap();
    assert!(parent.graph_reaches_parent_style_sheet());

    sheet.replace_sync("c { color: blue }").unwrap();
    assert!(!parent.graph_reaches_parent_style_sheet());
    assert_eq!(
        child.graph_component_identity(),
        parent.graph_component_identity()
    );
    assert!(child.graph_component_root().same_rule(&parent));
}
