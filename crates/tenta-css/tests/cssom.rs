#![cfg(feature = "stylo")]

use style::stylesheet_set::DocumentStylesheetSet;
use style::stylesheets::{CustomMediaMap, DocumentStyleSheet};
use tenta_css::{
    CssDeclarationBlock, CssPropertyRegistration, CssRuleType, CssStyleSheet, CssomError,
    SheetChangeKind, supported_property_names, supports_condition, supports_property,
};
use url::Url;

fn base_url() -> Url {
    Url::parse("https://example.test/app/").unwrap()
}

#[test]
fn stylo_parses_nested_rules_and_effective_declarations() {
    let sheet = CssStyleSheet::parse(
        "@layer theme { .card { color: red; color: blue !important; --accent: teal; } } \
         @supports (display: grid) { .grid { display: grid; } }",
        base_url(),
    );
    let rules = sheet.css_rules().unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].rule_type, CssRuleType::LayerBlock);
    assert_eq!(rules[0].children.len(), 1);
    assert_eq!(rules[0].children[0].rule_type, CssRuleType::Style);
    let block = rules[0].children[0].style.as_ref().unwrap();
    assert!(block.css_text.contains("color: blue !important"));
    assert!(
        block
            .properties
            .iter()
            .any(|property| property.name == "color" && property.important)
    );
    assert!(
        block
            .properties
            .iter()
            .any(|property| property.name == "--accent" && property.value == "teal")
    );
    assert_eq!(rules[1].rule_type, CssRuleType::Supports);
    assert_eq!(rules[1].children[0].rule_type, CssRuleType::Style);
}

#[test]
fn stylo_declarations_serialize_values_and_priority() {
    let block = CssDeclarationBlock::parse(
        "margin: 1px 2px; color: red; color: blue !important; --tone: teal; nonsense: 1",
        &base_url(),
    );
    assert_eq!(block.property_value("color"), "blue");
    assert!(block.is_important("color"));
    assert_eq!(block.property_value("--tone"), "teal");
    assert_eq!(block.property_value("nonsense"), "");
    assert_eq!(block.property_value("margin"), "1px 2px");
    assert!(block.snapshot().css_text.contains("color: blue !important"));
}

#[test]
fn declaration_mutations_use_stylo_values_and_preserve_cssom_order() {
    let mut block =
        CssDeclarationBlock::parse("color: red; --Tone: teal; margin: 1px 2px", &base_url());
    assert_eq!(block.item(0), "color");
    assert_eq!(block.item(block.len()), "");

    assert!(block.set_property("color", "blue", "IMPORTANT"));
    assert_eq!(block.item(0), "color", "an update keeps declaration order");
    assert_eq!(block.property_value("color"), "blue");
    assert!(block.is_important("color"));
    let before_invalid = block.snapshot();
    assert!(!block.set_property("color", "not-a-color", ""));
    assert!(!block.set_property("color", "green", "urgent"));
    assert!(!block.set_property("color", "green !important", ""));
    assert_eq!(block.snapshot(), before_invalid);

    assert!(block.set_property("margin", "3px 4px", ""));
    assert_eq!(block.property_value("margin"), "3px 4px");
    assert_eq!(block.remove_property("margin"), "3px 4px");
    assert_eq!(block.property_value("margin-top"), "");

    assert_eq!(block.property_value("--Tone"), "teal");
    assert_eq!(block.property_value("--tone"), "");
    assert!(block.set_property("--tone", "calc(1px + 2px)", ""));
    assert_ne!(block.property_value("--tone"), "");
    assert_eq!(block.property_value("--Tone"), "teal");
    assert_eq!(block.remove_property("--Tone"), "teal");
    assert_eq!(block.property_value("--Tone"), "");

    assert!(block.set_property("color", "", "invalid-priority"));
    assert_eq!(block.property_value("color"), "");
    assert!(!block.is_empty(), "the other custom property remains");
}

#[test]
fn cssom_insert_delete_checks_syntax_index_and_hierarchy() {
    let mut sheet = CssStyleSheet::parse(".one { color: red }", base_url());
    assert_eq!(sheet.revision(), 0);
    assert_eq!(sheet.insert_rule(".bad", 1).err(), Some(CssomError::Syntax));
    assert_eq!(
        sheet.insert_rule(".two { color: blue }", 3).err(),
        Some(CssomError::IndexSize)
    );
    assert_eq!(
        sheet.insert_rule("@import url(other.css);", 1).err(),
        Some(CssomError::Syntax),
        "a sheet without a loader refuses every @import before looking at its position"
    );
    assert_eq!(sheet.rule_count(), 1);
    assert_eq!(sheet.revision(), 0);

    let inserted = sheet.insert_rule(".zero { display: grid }", 0).unwrap();
    assert_eq!(inserted.kind, SheetChangeKind::Insertion);
    assert_eq!(inserted.index, Some(0));
    assert_eq!(inserted.revision, 1);
    assert!(sheet.css_rules().unwrap()[0].css_text.contains(".zero"));

    assert_eq!(sheet.delete_rule(2).err(), Some(CssomError::IndexSize));
    let removed = sheet.delete_rule(0).unwrap();
    assert_eq!(removed.kind, SheetChangeKind::Removal);
    assert_eq!(sheet.revision(), 2);
    assert_eq!(sheet.rule_count(), 1);
    assert!(sheet.css_rules().unwrap()[0].css_text.contains(".one"));
}

#[test]
fn rule_handles_keep_identity_when_indexes_shift_and_after_deletion() {
    let mut sheet =
        CssStyleSheet::parse(".first { color: red } .second { color: blue }", base_url());
    let first = sheet.rule_at(0).unwrap();
    let second = sheet.rule_at(1).unwrap();
    let first_text = first.css_text().unwrap();
    let second_text = second.css_text().unwrap();
    assert!(first.is_attached());
    assert!(second.is_attached());
    assert!(sheet.rule_at(0).unwrap().same_rule(&first));
    assert!(!first.same_rule(&second));
    assert!(sheet.rule_at(2).is_none());

    sheet.insert_rule(".new { color: green }", 0).unwrap();
    assert!(sheet.rule_at(1).unwrap().same_rule(&first));
    assert!(sheet.rule_at(2).unwrap().same_rule(&second));
    assert_eq!(first.css_text().unwrap(), first_text);

    sheet.delete_rule(1).unwrap();
    assert!(!first.is_attached());
    assert!(second.is_attached());
    assert!(sheet.rule_at(1).unwrap().same_rule(&second));
    assert_eq!(first.css_text().unwrap(), first_text);
    assert_eq!(first.snapshot().unwrap().rule_type, CssRuleType::Style);
    assert_eq!(second.css_text().unwrap(), second_text);

    // A wrapper can keep a removed rule alive even after the sheet's Rust
    // owner goes away, just as a JavaScript CSSRule can outlive its list.
    drop(sheet);
    assert!(!first.is_attached());
    assert_eq!(first.css_text().unwrap(), first_text);
}

#[test]
fn nested_rule_handles_retain_identity_and_detach_with_parent() {
    let mut sheet = CssStyleSheet::parse("@media screen { .nested { display: grid } }", base_url());
    let media = sheet.rule_at(0).unwrap();
    let child = media.child_at(0).unwrap();
    let child_text = child.css_text().unwrap();
    assert_eq!(media.child_count(), 1);
    assert_eq!(child.rule_type(), CssRuleType::Style);
    assert!(media.child_at(0).unwrap().same_rule(&child));
    assert!(child.is_attached());

    sheet.insert_rule(".before { color: green }", 0).unwrap();
    assert!(sheet.rule_at(1).unwrap().same_rule(&media));
    assert!(
        sheet
            .rule_at(1)
            .unwrap()
            .child_at(0)
            .unwrap()
            .same_rule(&child)
    );

    sheet.delete_rule(1).unwrap();
    assert!(!media.is_attached());
    assert!(!child.is_attached());
    assert_eq!(media.child_count(), 1);
    assert_eq!(child.css_text().unwrap(), child_text);
}

#[test]
fn replacing_sheet_detaches_old_rules_without_reusing_identity() {
    let mut sheet = CssStyleSheet::parse(".same { color: red }", base_url());
    let old = sheet.rule_at(0).unwrap();
    let old_text = old.css_text().unwrap();

    sheet.replace_sync(".same { color: red }").unwrap();
    let replacement = sheet.rule_at(0).unwrap();
    assert!(!old.same_rule(&replacement));
    assert!(!old.is_attached());
    assert!(replacement.is_attached());
    assert_eq!(old.css_text().unwrap(), old_text);
}

#[test]
fn style_rule_declarations_mutate_the_retained_stylo_rule_and_notify() {
    let mut sheet = CssStyleSheet::parse(".card { color: red; margin: 1px 2px }", base_url());
    let rule = sheet.rule_at(0).unwrap();
    let mut set = DocumentStylesheetSet::<DocumentStyleSheet>::new();
    let custom_media = CustomMediaMap::default();
    sheet.attach_to_document(&mut set, None, &custom_media);
    set.flush_without_invalidation();

    assert_eq!(rule.selector_text().as_deref(), Some(".card"));
    assert_eq!(
        rule.style_property_value("margin").as_deref(),
        Some("1px 2px")
    );
    assert_eq!(rule.style_property_is_important("color"), Some(false));
    let changed = sheet
        .set_rule_style_property(&rule, "color", "blue", "IMPORTANT")
        .unwrap()
        .unwrap();
    assert_eq!(changed.kind, SheetChangeKind::StyleDeclarations);
    assert_eq!(changed.revision, 1);
    changed.notify(&mut set, None, &custom_media);
    assert!(set.has_changed());
    assert!(sheet.rule_at(0).unwrap().same_rule(&rule));
    assert_eq!(rule.style_property_value("color").as_deref(), Some("blue"));
    assert_eq!(rule.style_property_is_important("color"), Some(true));
    assert!(rule.css_text().unwrap().contains("color: blue !important"));

    let before_invalid = rule.style_declarations().unwrap();
    assert!(
        sheet
            .set_rule_style_property(&rule, "color", "not-a-color", "")
            .unwrap()
            .is_none()
    );
    assert!(
        sheet
            .set_rule_style_property(&rule, "color", "green", "urgent")
            .unwrap()
            .is_none()
    );
    assert_eq!(rule.style_declarations().unwrap(), before_invalid);
    assert_eq!(sheet.revision(), 1);

    let (old_margin, removed) = sheet.remove_rule_style_property(&rule, "margin").unwrap();
    assert_eq!(old_margin, "1px 2px");
    assert_eq!(removed.unwrap().kind, SheetChangeKind::StyleDeclarations);
    assert_eq!(rule.style_property_value("margin-top").as_deref(), Some(""));
    assert_eq!(sheet.revision(), 2);
}

#[test]
fn selector_and_css_text_setters_keep_style_rule_identity() {
    let mut sheet = CssStyleSheet::parse(".old { color: red; --tone: teal }", base_url());
    let rule = sheet.rule_at(0).unwrap();

    assert!(
        sheet
            .set_rule_selector_text(&rule, ".bad[")
            .unwrap()
            .is_none()
    );
    assert_eq!(rule.selector_text().as_deref(), Some(".old"));
    assert_eq!(sheet.revision(), 0);

    let selector_change = sheet
        .set_rule_selector_text(&rule, ".new, #target")
        .unwrap()
        .unwrap();
    assert_eq!(selector_change.kind, SheetChangeKind::Full);
    assert_eq!(rule.selector_text().as_deref(), Some(".new, #target"));
    assert_eq!(rule.style_property_value("color").as_deref(), Some("red"));
    assert!(sheet.rule_at(0).unwrap().same_rule(&rule));

    let block_change = sheet
        .set_rule_style_css_text(&rule, "color: blue; invalid-property: yes; --tone: gold")
        .unwrap()
        .unwrap();
    assert_eq!(block_change.kind, SheetChangeKind::StyleDeclarations);
    assert_eq!(rule.style_property_value("color").as_deref(), Some("blue"));
    assert_eq!(rule.style_property_value("--tone").as_deref(), Some("gold"));
    assert_eq!(
        rule.style_property_value("invalid-property").as_deref(),
        Some("")
    );
    assert!(sheet.rule_at(0).unwrap().same_rule(&rule));
    assert_eq!(sheet.revision(), 2);
}

#[test]
fn selector_setter_uses_the_stylesheets_namespace_map() {
    let mut sheet = CssStyleSheet::parse(
        "@namespace svg url('http://www.w3.org/2000/svg'); svg|rect { color: red }",
        base_url(),
    );
    let rule = sheet.rule_at(1).unwrap();
    assert_eq!(rule.selector_text().as_deref(), Some("svg|rect"));

    assert!(
        sheet
            .set_rule_selector_text(&rule, "unknown|circle")
            .unwrap()
            .is_none()
    );
    assert_eq!(rule.selector_text().as_deref(), Some("svg|rect"));
    assert_eq!(sheet.revision(), 0);

    assert_eq!(
        sheet
            .set_rule_selector_text(&rule, "svg|circle")
            .unwrap()
            .unwrap()
            .kind,
        SheetChangeKind::Full
    );
    assert_eq!(rule.selector_text().as_deref(), Some("svg|circle"));
    assert!(sheet.rule_at(1).unwrap().same_rule(&rule));
}

#[test]
fn detached_rule_stays_mutable_without_invalidating_its_former_sheet() {
    let mut sheet = CssStyleSheet::parse("@media all { .nested { color: red } }", base_url());
    let media = sheet.rule_at(0).unwrap();
    let nested = media.child_at(0).unwrap();
    let nested_change = sheet
        .set_rule_style_property(&nested, "color", "blue", "")
        .unwrap()
        .unwrap();
    assert_eq!(nested_change.kind, SheetChangeKind::Full);
    assert_eq!(
        nested.style_property_value("color").as_deref(),
        Some("blue")
    );

    sheet.delete_rule(0).unwrap();
    let revision = sheet.revision();
    assert!(!nested.is_attached());
    assert!(
        sheet
            .set_rule_style_property(&nested, "color", "green", "")
            .unwrap()
            .is_none()
    );
    assert!(
        sheet
            .set_rule_selector_text(&nested, ".detached")
            .unwrap()
            .is_none(),
        "a rule under a detached @media changes without invalidating the sheet"
    );
    assert_eq!(nested.selector_text().as_deref(), Some(".detached"));
    assert_eq!(
        nested.style_property_value("color").as_deref(),
        Some("green")
    );
    assert_eq!(sheet.revision(), revision);

    let mut another = CssStyleSheet::parse(".other { color: red }", base_url());
    assert_eq!(
        another
            .set_rule_style_property(&nested, "color", "black", "")
            .err(),
        Some(CssomError::InvalidState)
    );
    assert_eq!(
        nested.style_property_value("color").as_deref(),
        Some("green")
    );
    assert_eq!(media.selector_text(), None);
    assert_eq!(media.style_declarations(), None);

    let mut plain = CssStyleSheet::parse(".former { color: red }", base_url());
    let old = plain.rule_at(0).unwrap();
    plain.delete_rule(0).unwrap();
    let revision = plain.revision();
    assert!(
        plain
            .set_rule_selector_text(&old, ".detached")
            .unwrap()
            .is_none()
    );
    assert_eq!(old.selector_text().as_deref(), Some(".detached"));
    assert_eq!(plain.revision(), revision);
}

#[test]
fn replace_and_disable_notify_the_native_stylo_document_set() {
    let mut sheet = CssStyleSheet::parse(".one { color: red }", base_url());
    let mut set = DocumentStylesheetSet::<DocumentStyleSheet>::new();
    let custom_media = CustomMediaMap::default();
    sheet.attach_to_document(&mut set, None, &custom_media);
    assert_eq!(set.len(), 1);
    assert!(set.has_changed());
    set.flush_without_invalidation();
    assert!(!set.has_changed());

    let inserted = sheet.insert_rule(".two { color: blue }", 1).unwrap();
    inserted.notify(&mut set, None, &custom_media);
    assert!(set.has_changed());
    set.flush_without_invalidation();
    assert!(!set.has_changed());

    let replaced = sheet.replace_sync(".new { color: green }").unwrap();
    assert_eq!(replaced.kind, SheetChangeKind::Full);
    replaced.notify(&mut set, None, &custom_media);
    assert!(set.has_changed());
    assert_eq!(sheet.rule_count(), 1);
    assert!(sheet.css_rules().unwrap()[0].css_text.contains(".new"));
    set.flush_without_invalidation();

    let disabled = sheet.set_disabled(true).unwrap();
    disabled.notify(&mut set, None, &custom_media);
    assert!(sheet.disabled());
    assert!(set.has_changed());
    assert!(sheet.set_disabled(true).is_none());
    sheet.detach_from_document(&mut set, None, &custom_media);
    assert_eq!(set.len(), 0);
}

#[test]
fn css_supports_uses_stylo_property_and_condition_grammar() {
    let base = base_url();
    assert!(supports_property("display", "grid", &base));
    assert!(!supports_property("display", "not-a-display", &base));
    assert!(!supports_property("not-a-property", "yes", &base));
    assert!(!supports_property("display: grid", "grid", &base));
    assert!(supports_condition(
        "(display: grid) and (color: red)",
        &base
    ));
    assert!(supports_condition("not (display: not-a-display)", &base));
    assert!(!supports_condition("(display: grid) trailing", &base));
}

#[test]
fn deeply_nested_css_rule_snapshot_fails_explicitly() {
    let source = format!(
        "{} .deep {{ color: red }} {}",
        "@media all {".repeat(129),
        "}".repeat(129)
    );
    let sheet = CssStyleSheet::parse(&source, base_url());
    assert_eq!(sheet.css_rules().err(), Some(CssomError::DepthLimit));
    let outer = sheet.rule_at(0).unwrap();
    assert_eq!(outer.css_text().err(), Some(CssomError::DepthLimit));
    assert_eq!(outer.snapshot().err(), Some(CssomError::DepthLimit));
    assert!(outer.is_attached(), "attachment checks walk iteratively");
}

#[test]
fn nested_style_children_keep_parent_identity_through_edits_and_detachment() {
    let mut sheet = CssStyleSheet::parse(".outer { color: red }", base_url());
    let parent = sheet.rule_at(0).unwrap();
    assert_eq!(parent.child_count(), 0);

    let change = sheet
        .insert_nested_rule(&parent, "& .first { color: blue }", 0)
        .unwrap()
        .unwrap();
    assert_eq!(change.kind, SheetChangeKind::Full);
    let first = parent.child_at(0).unwrap();
    assert!(first.parent_rule().unwrap().same_rule(&parent));
    assert!(first.is_attached());
    assert!(parent.has_parent_style_sheet());
    assert!(first.has_parent_style_sheet());

    sheet
        .insert_nested_rule(&parent, "& .before { color: green }", 0)
        .unwrap();
    assert!(parent.child_at(1).unwrap().same_rule(&first));
    let before = parent.child_at(0).unwrap();
    assert!(before.parent_rule().unwrap().same_rule(&parent));
    assert!(
        sheet
            .set_rule_selector_text(&first, "> .changed")
            .unwrap()
            .is_some()
    );
    assert!(first.selector_text().unwrap().contains(".changed"));

    sheet.delete_nested_rule(&parent, 1).unwrap();
    assert!(!first.is_attached());
    assert!(first.parent_rule().is_none());
    assert!(!first.has_parent_style_sheet());
    assert!(first.css_text().unwrap().contains(".changed"));
    assert!(before.parent_rule().unwrap().same_rule(&parent));

    sheet.delete_rule(0).unwrap();
    assert!(!parent.is_attached());
    assert!(!before.is_attached());
    assert!(!parent.has_parent_style_sheet());
    assert!(before.has_parent_style_sheet());
    assert!(before.parent_rule().unwrap().same_rule(&parent));
    sheet
        .insert_nested_rule(&parent, "& .late { & .inside { color: red } }", 1)
        .unwrap();
    let late = parent.child_at(1).unwrap();
    let inside = late.child_at(0).unwrap();
    assert!(!late.has_parent_style_sheet());
    assert!(!inside.has_parent_style_sheet());
    let revision = sheet.revision();
    assert!(
        sheet
            .set_rule_style_property(&before, "color", "purple", "")
            .unwrap()
            .is_none()
    );
    assert_eq!(sheet.revision(), revision);
    assert_eq!(
        before.style_property_value("color").as_deref(),
        Some("purple")
    );
}

#[test]
fn nested_mutation_takes_any_rule_valid_there_and_refuses_atomically() {
    let mut sheet = CssStyleSheet::parse(".outer { color: red }", base_url());
    let parent = sheet.rule_at(0).unwrap();
    assert_eq!(
        sheet
            .insert_nested_rule(&parent, "& .child { color: red }", 1)
            .err(),
        Some(CssomError::IndexSize)
    );
    assert_eq!(
        sheet.delete_nested_rule(&parent, 0).err(),
        Some(CssomError::IndexSize)
    );
    assert_eq!(
        sheet
            .insert_nested_rule(&parent, "@font-face { font-family: X }", 0)
            .err(),
        Some(CssomError::HierarchyRequest),
        "a rule that cannot nest in a style rule is refused"
    );
    assert_eq!(parent.child_count(), 0);
    assert_eq!(sheet.revision(), 0);

    sheet
        .insert_nested_rule(&parent, "@media all { & .child { color: blue } }", 0)
        .unwrap();
    let media = parent.child_at(0).unwrap();
    assert_eq!(media.rule_type(), CssRuleType::Media);
    assert!(media.parent_rule().unwrap().same_rule(&parent));

    // The @media is a grouping rule in its own right, and a rule put in it
    // still sees the style rule above: `&` and a bare declaration are valid.
    sheet
        .insert_nested_rule(&media, "& .grandchild { color: green }", 1)
        .unwrap();
    let grandchild = media.child_at(1).unwrap();
    assert_eq!(grandchild.selector_text().as_deref(), Some("& .grandchild"));
    assert!(grandchild.parent_rule().unwrap().same_rule(&media));
    assert!(grandchild.has_parent_style_sheet());
    sheet
        .set_rule_selector_text(&grandchild, "& > .renamed")
        .unwrap();
    assert_eq!(grandchild.selector_text().as_deref(), Some("& > .renamed"));

    sheet.delete_nested_rule(&media, 0).unwrap();
    assert!(media.child_at(0).unwrap().same_rule(&grandchild));
    assert_eq!(media.child_count(), 1);

    let leaf = sheet.rule_at(0).unwrap().child_at(0).unwrap();
    assert_eq!(
        CssStyleSheet::parse("@font-face { font-family: X }", base_url())
            .rule_at(0)
            .map(|rule| rule.is_grouping()),
        Some(false)
    );
    assert!(leaf.is_grouping());
}

#[test]
fn a_top_level_group_takes_rules_parsed_without_a_style_ancestor() {
    let mut sheet = CssStyleSheet::parse(
        "@media (min-width: 10px) { .a { color: red } } @layer base { }",
        base_url(),
    );
    let media = sheet.rule_at(0).unwrap();
    assert_eq!(
        sheet
            .insert_nested_rule(&media, "& { color: red } junk", 0)
            .err(),
        Some(CssomError::Syntax)
    );
    sheet
        .insert_nested_rule(
            &media,
            "@supports (display: grid) { .b { display: grid } }",
            1,
        )
        .unwrap();
    sheet
        .insert_nested_rule(&media, ".c { color: blue }", 0)
        .unwrap();
    assert_eq!(media.child_count(), 3);
    assert_eq!(
        media.child_at(0).unwrap().selector_text().as_deref(),
        Some(".c")
    );
    assert_eq!(
        media.child_at(2).unwrap().condition_text().as_deref(),
        Some("(display: grid)")
    );
    assert_eq!(
        sheet
            .insert_nested_rule(&media, "@namespace svg url(x);", 0)
            .err(),
        Some(CssomError::HierarchyRequest)
    );

    let layer = sheet.rule_at(1).unwrap();
    assert_eq!(layer.layer_name().as_deref(), Some("base"));
    sheet
        .insert_nested_rule(&layer, ".d { color: red }", 0)
        .unwrap();
    assert_eq!(layer.child_count(), 1);
    assert!(layer.css_text().unwrap().contains(".d"));
}

#[test]
fn at_rules_report_what_cssom_reads_from_them() {
    let sheet = CssStyleSheet::parse(
        "@namespace svg url(http://www.w3.org/2000/svg); \
         @layer reset, base.widgets; \
         @media screen and (min-width: 600px), print { a { color: red } } \
         @scope (.card) to (.content) { a { color: red } } \
         @scope { a { color: red } } \
         @keyframes spin { to { rotate: 1turn } } \
         @keyframes \"quoted name\" { to { rotate: 1turn } } \
         @layer { a { color: red } } \
         @property --gap { syntax: '<length>'; inherits: false; initial-value: 4px } \
         @supports (display: nonsense) { a { color: red } }",
        base_url(),
    );
    let rule = |index| sheet.rule_at(index).unwrap();
    assert_eq!(
        rule(0).namespace(),
        Some(("svg".to_owned(), "http://www.w3.org/2000/svg".to_owned()))
    );
    assert_eq!(
        rule(1).layer_names(),
        Some(vec!["reset".to_owned(), "base.widgets".to_owned()])
    );
    assert_eq!(
        rule(2).media_text().as_deref(),
        Some("screen and (min-width: 600px), print")
    );
    assert_eq!(
        rule(2).media_queries(),
        Some(vec![
            "screen and (min-width: 600px)".to_owned(),
            "print".to_owned()
        ])
    );
    assert_eq!(rule(2).condition_text(), rule(2).media_text());
    assert_eq!(
        rule(3).scope_bounds(),
        Some((Some(".card".to_owned()), Some(".content".to_owned())))
    );
    assert_eq!(rule(4).scope_bounds(), Some((None, None)));
    assert_eq!(rule(5).keyframes_name().as_deref(), Some("spin"));
    assert_eq!(rule(6).keyframes_name().as_deref(), Some("quoted name"));
    assert_eq!(rule(7).layer_name().as_deref(), Some(""));
    assert_eq!(
        rule(8).property_registration(),
        Some(CssPropertyRegistration {
            name: "--gap".to_owned(),
            syntax: Some("\"<length>\"".to_owned()),
            inherits: Some("false".to_owned()),
            initial_value: Some("4px".to_owned()),
        })
    );
    assert_eq!(rule(9).supports_matches(), Some(false));
    assert_eq!(
        rule(9).condition_text().as_deref(),
        Some("(display: nonsense)")
    );
    // Each getter answers for its own kind of rule only.
    assert_eq!(rule(2).layer_name(), None);
    assert_eq!(rule(5).condition_text(), None);
}

#[test]
fn a_media_list_is_rewritten_appended_to_and_deleted_from() {
    let mut sheet = CssStyleSheet::parse("@media screen { a { color: red } }", base_url());
    let media = sheet.rule_at(0).unwrap();
    let style = media.child_at(0).unwrap();
    assert_eq!(
        sheet.set_rule_media_text(&style, "print").err(),
        Some(CssomError::InvalidState)
    );

    let changed = sheet
        .set_rule_media_text(&media, "print, (width > 1px)")
        .unwrap();
    assert_eq!(
        changed.map(|change| change.kind),
        Some(SheetChangeKind::Full)
    );
    assert_eq!(media.media_text().as_deref(), Some("print, (width > 1px)"));

    sheet.append_rule_medium(&media, "print").unwrap();
    assert_eq!(media.media_text().as_deref(), Some("(width > 1px), print"));
    let revision = sheet.revision();
    assert!(
        sheet
            .append_rule_medium(&media, "and and")
            .unwrap()
            .is_none()
    );
    assert_eq!(sheet.revision(), revision);

    sheet.delete_rule_medium(&media, "(width > 1px)").unwrap();
    assert_eq!(media.media_text().as_deref(), Some("print"));
    assert_eq!(
        sheet.delete_rule_medium(&media, "screen").err(),
        Some(CssomError::NotFound)
    );

    sheet
        .set_rule_media_text(&media, "nonsense query here")
        .unwrap();
    assert_eq!(media.media_text().as_deref(), Some("not all"));
    assert!(media.css_text().unwrap().starts_with("@media not all {"));
}

#[test]
fn a_sheet_serializes_to_text_that_parses_back_to_its_rules() {
    let source = "@namespace svg url(http://www.w3.org/2000/svg); \
         @layer reset, base; \
         @media screen and (min-width: 600px) { a { color: red; & b { color: blue } } } \
         @supports (display: grid) { a { display: grid } } \
         @keyframes spin { from { rotate: 0turn } to { rotate: 1turn } } \
         @font-face { font-family: \"X Y\"; src: url(\"x;y.woff2\") } \
         @property --gap { syntax: \"<length>\"; inherits: false; initial-value: 4px } \
         a::before { content: \"}\"; --custom: { a: b } }";
    let mut sheet = CssStyleSheet::parse(source, base_url());
    assert_eq!(sheet.content_revision(), 0);
    let text = sheet.css_text().unwrap();
    let again = CssStyleSheet::parse(&text, base_url());
    assert_eq!(again.rule_count(), sheet.rule_count());
    assert_eq!(again.css_text().unwrap(), text, "serialization is stable");
    assert_eq!(again.css_rules().unwrap(), sheet.css_rules().unwrap());

    // Turning the sheet off and on is not a change to its rules; an edit is.
    sheet.set_disabled(true);
    sheet.set_disabled(false);
    assert_eq!(sheet.content_revision(), 0);
    assert_eq!(sheet.revision(), 2);
    let last = sheet.rule_count();
    sheet.insert_rule("b { color: green }", last).unwrap();
    let rule = sheet.rule_at(last).unwrap();
    sheet
        .set_rule_style_property(&rule, "color", "blue", "")
        .unwrap();
    sheet.delete_rule(last - 1).unwrap();
    assert_eq!(sheet.content_revision(), 3);
    assert!(
        sheet
            .css_text()
            .unwrap()
            .ends_with("\nb { color: blue; }\n")
    );
    assert!(!sheet.css_text().unwrap().contains("a::before"));

    let deep = format!(
        "{}a {{ color: red }}{}",
        "@media all {".repeat(200),
        "}".repeat(200)
    );
    assert_eq!(
        CssStyleSheet::parse(&deep, base_url()).css_text().err(),
        Some(CssomError::DepthLimit)
    );
}

#[test]
fn keyframes_are_read_found_appended_and_deleted() {
    let mut sheet = CssStyleSheet::parse(
        "@keyframes fade { from { opacity: 0 } 50%, 75% { opacity: .5 } to { opacity: 1 } } \
         a { color: red }",
        base_url(),
    );
    let rule = sheet.rule_at(0).unwrap();
    assert_eq!(rule.keyframe_count(), Some(3));
    assert_eq!(sheet.rule_at(1).unwrap().keyframe_count(), None);
    assert!(sheet.rule_at(1).unwrap().keyframe_at(0).is_none());

    let first = rule.keyframe_at(0).unwrap();
    let middle = rule.keyframe_at(1).unwrap();
    assert_eq!(first.key_text(), "0%");
    assert_eq!(middle.key_text(), "50%, 75%");
    assert_eq!(middle.css_text(), "50%, 75% { opacity: 0.5; }");
    assert_eq!(middle.style_property_value("opacity"), "0.5");
    assert!(first.parent_rule().unwrap().same_rule(&rule));
    assert!(first.has_parent_style_sheet());
    assert!(rule.keyframe_at(3).is_none());

    // The same keyframe is the same allocation however it is reached.
    assert_eq!(
        rule.find_keyframe("from").unwrap().identity(),
        first.identity()
    );
    assert_eq!(
        rule.find_keyframe(" 75% , 50% ").map(|k| k.identity()),
        None
    );
    assert_eq!(
        rule.find_keyframe("50%,75%").unwrap().identity(),
        middle.identity()
    );
    assert!(rule.find_keyframe("nonsense").is_none());

    let appended = sheet.append_keyframe(&rule, "to { opacity: .9 }").unwrap();
    assert_eq!(
        appended.map(|change| change.kind),
        Some(SheetChangeKind::Full)
    );
    assert_eq!(rule.keyframe_count(), Some(4));
    let revision = sheet.revision();
    assert!(
        sheet
            .append_keyframe(&rule, "not a keyframe")
            .unwrap()
            .is_none()
    );
    assert!(
        sheet
            .append_keyframe(&rule, "to {} to {}")
            .unwrap()
            .is_none()
    );
    assert_eq!(sheet.revision(), revision);

    // findRule and deleteRule take the last keyframe with the selector.
    let last = rule.find_keyframe("100%").unwrap();
    assert_eq!(last.style_property_value("opacity"), "0.9");
    sheet.delete_keyframe(&rule, "to").unwrap();
    assert_eq!(rule.keyframe_count(), Some(3));
    assert!(last.parent_rule().is_none());
    assert!(!last.has_parent_style_sheet());
    assert_eq!(last.css_text(), "100% { opacity: 0.9; }");
    assert_eq!(
        rule.find_keyframe("to")
            .unwrap()
            .style_property_value("opacity"),
        "1"
    );
    assert!(sheet.delete_keyframe(&rule, "33%").unwrap().is_none());

    assert_eq!(
        sheet
            .append_keyframe(&sheet.rule_at(1).unwrap(), "to {}")
            .err(),
        Some(CssomError::InvalidState)
    );
}

#[test]
fn a_keyframe_is_renamed_rekeyed_and_restyled_in_place() {
    let mut sheet = CssStyleSheet::parse(
        "@keyframes fade { from { opacity: 0 } to { opacity: 1 } }",
        base_url(),
    );
    let rule = sheet.rule_at(0).unwrap();
    sheet.set_keyframes_name(&rule, "grow").unwrap();
    assert_eq!(rule.keyframes_name().as_deref(), Some("grow"));
    sheet.set_keyframes_name(&rule, "two words").unwrap();
    assert_eq!(rule.keyframes_name().as_deref(), Some("two words"));
    // A name that is not an identifier serializes escaped.
    assert!(
        rule.css_text()
            .unwrap()
            .starts_with("@keyframes two\\ words {")
    );

    let from = rule.keyframe_at(0).unwrap();
    sheet.set_keyframe_key_text(&from, "10%, 20%").unwrap();
    assert_eq!(from.key_text(), "10%, 20%");
    assert_eq!(
        sheet.set_keyframe_key_text(&from, "soon").err(),
        Some(CssomError::Syntax)
    );
    assert_eq!(from.key_text(), "10%, 20%");

    sheet
        .set_keyframe_style_property(&from, "opacity", "0.25", "")
        .unwrap();
    assert!(
        sheet
            .set_keyframe_style_property(&from, "color", "red", "important")
            .unwrap()
            .is_none(),
        "a keyframe's declaration cannot be important"
    );
    sheet
        .set_keyframe_style_property(&from, "color", "red", "")
        .unwrap();
    assert_eq!(
        from.style_declarations().css_text,
        "opacity: 0.25; color: red;"
    );
    let (removed, _) = sheet
        .remove_keyframe_style_property(&from, "opacity")
        .unwrap();
    assert_eq!(removed, "0.25");
    sheet
        .set_keyframe_style_css_text(&from, "top: 1px; left: 2px !important")
        .unwrap();
    assert_eq!(from.style_declarations().css_text, "top: 1px;");
    assert_eq!(
        rule.css_text().unwrap(),
        "@keyframes two\\ words {\n10%, 20% { top: 1px; }\n100% { opacity: 1; }\n}"
    );

    // A keyframe that left its rule is still its own to change.
    sheet.delete_keyframe(&rule, "10%, 20%").unwrap();
    let revision = sheet.revision();
    assert!(
        sheet
            .set_keyframe_style_property(&from, "top", "3px", "")
            .unwrap()
            .is_none()
    );
    assert_eq!(sheet.revision(), revision);
    assert_eq!(from.style_property_value("top"), "3px");
}

#[test]
fn font_face_descriptors_read_back_as_name_and_value() {
    let sheet = CssStyleSheet::parse(
        "@font-face { font-family: \"Semi; Colon\"; \
           src: url(\"a;b.woff2\") format(\"woff2\"), local(Arial); \
           font-weight: 400 700; font-display: swap; unicode-range: U+0-7F } \
         a { color: red }",
        base_url(),
    );
    let descriptors = sheet.rule_at(0).unwrap().font_face_descriptors().unwrap();
    let get = |name: &str| {
        descriptors
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(get("font-family"), Some("\"Semi; Colon\""));
    assert_eq!(
        get("src"),
        Some("url(\"a;b.woff2\") format(\"woff2\"), local(Arial)")
    );
    assert_eq!(get("font-weight"), Some("400 700"));
    assert_eq!(get("font-display"), Some("swap"));
    assert_eq!(get("unicode-range"), Some("U+0-7F"));
    assert_eq!(descriptors.len(), 5);
    assert_eq!(sheet.rule_at(1).unwrap().font_face_descriptors(), None);
}

#[test]
fn parsed_nested_style_tree_is_exposed_with_live_lineage() {
    let mut sheet = CssStyleSheet::parse("", base_url());
    sheet
        .replace_sync(".outer { color: red; & .middle { color: blue; & .leaf { color: green } } }")
        .unwrap();
    let parent = sheet.rule_at(0).unwrap();
    let middle = parent.child_at(0).unwrap();
    let leaf = middle.child_at(0).unwrap();
    assert!(parent.has_parent_style_sheet());
    assert!(middle.has_parent_style_sheet());
    assert!(leaf.has_parent_style_sheet());
    assert!(middle.parent_rule().unwrap().same_rule(&parent));
    assert!(leaf.parent_rule().unwrap().same_rule(&middle));
    assert_eq!(leaf.style_property_value("color").as_deref(), Some("green"));

    sheet.delete_rule(0).unwrap();
    assert!(!parent.has_parent_style_sheet());
    assert!(middle.has_parent_style_sheet());
    assert!(leaf.has_parent_style_sheet());
    assert!(!leaf.is_attached());
    assert!(leaf.parent_rule().unwrap().same_rule(&middle));
    assert!(middle.parent_rule().unwrap().same_rule(&parent));

    sheet.replace_sync(".replacement { color: black }").unwrap();
    assert!(!middle.has_parent_style_sheet());
    assert!(!leaf.has_parent_style_sheet());
    assert!(sheet.rule_at(0).unwrap().has_parent_style_sheet());
}

#[test]
fn retained_parent_lineage_does_not_cycle_with_the_sheet() {
    let mut sheet = CssStyleSheet::parse(".outer { color: red }", base_url());
    let parent = sheet.rule_at(0).unwrap();
    sheet
        .insert_nested_rule(&parent, "& .child { color: blue }", 0)
        .unwrap();
    let child = parent.child_at(0).unwrap();
    let retained = sheet.document_sheet();
    assert!(!retained.0.is_unique());
    drop(sheet);
    drop(parent);
    drop(child);
    assert!(retained.0.is_unique());
}

#[test]
fn supported_property_names_cover_longhands_shorthands_and_gated_properties() {
    let names = supported_property_names();
    let base = base_url();
    for expected in [
        "color",
        "background-color",
        "margin",
        "float",
        "user-select",
    ] {
        assert!(names.contains(&expected), "{expected} is not listed");
    }
    assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(!names.contains(&"--custom"));
    assert!(
        names
            .iter()
            .all(|name| supports_property(name, "inherit", &base)),
        "every listed name must be accepted by setProperty's parser"
    );
}
