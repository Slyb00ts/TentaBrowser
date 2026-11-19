// Shared real-engine CSSOM wrapper identity and collection probes.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tenta_dom::Document;
use tenta_js::engine::PageEngine;

fn cssom_engine() -> Engine {
    new_engine(Rc::new(RefCell::new(Document::new())))
}

fn cssom_eval(engine: &mut Engine, source: &str) -> String {
    PageEngine::evaluate(engine, source, Duration::from_secs(5))
        .expect("evaluate CSSOM wrapper state")
}

fn assert_retained(setup: &str, query: &str, expected: &str) {
    let mut engine = cssom_engine();
    cssom_eval(&mut engine, setup);
    let before = cssom_eval(&mut engine, query);
    for _ in 0..3 {
        PageEngine::trim_memory(&mut engine);
        assert!(
            engine.native_failure().is_none(),
            "native CSSOM collection failed"
        );
    }
    let after = cssom_eval(&mut engine, query);
    assert_eq!(before, expected, "state lost between ordinary script tasks");
    assert_eq!(after, expected, "state lost after explicit collection");
}

#[test]
fn held_sheet_keeps_same_object_rule_list_state() {
    assert_retained(
        "globalThis.sheet=new CSSStyleSheet();sheet.replaceSync('a{color:red}');\
         {const list=sheet.cssRules;const proto=Object.create(Object.getPrototypeOf(list));\
         proto.marker='prototype';Object.setPrototypeOf(list,proto);\
         list[Symbol.for('cssom-state')]=41;list.note='list';}",
        "[sheet.cssRules[Symbol.for('cssom-state')],sheet.cssRules.note,\
         sheet.cssRules.marker].join('|')",
        "41|list|prototype",
    );
}

#[test]
fn held_sheet_keeps_indexed_rule_state() {
    assert_retained(
        "globalThis.sheet=new CSSStyleSheet();sheet.replaceSync('a{color:red}');\
         sheet.cssRules[0].note='rule';",
        "sheet.cssRules[0].note",
        "rule",
    );
}

#[test]
fn held_rule_keeps_parent_sheet_state() {
    assert_retained(
        "globalThis.rule=(()=>{const sheet=new CSSStyleSheet();\
         sheet.replaceSync('a{color:red}');sheet.note='sheet';\
         return sheet.cssRules[0]})();",
        "rule.parentStyleSheet.note",
        "sheet",
    );
}

#[test]
fn held_rule_keeps_same_object_style_and_child_list_state() {
    assert_retained(
        "globalThis.rule=(()=>{const sheet=new CSSStyleSheet();\
         sheet.replaceSync('a{color:red;& b{color:blue}}');\
         return sheet.cssRules[0]})();\
         rule.style.note='style';rule.cssRules.note='children';",
        "[rule.style.note,rule.cssRules.note].join('|')",
        "style|children",
    );
}

#[test]
fn held_sheet_keeps_state_of_at_rules_and_what_hangs_off_them() {
    assert_retained(
        "globalThis.sheet=new CSSStyleSheet();\
         sheet.replaceSync('@media print{a{color:red}} @keyframes k{to{opacity:0}}');\
         {const media=sheet.cssRules[0];media.note='media';media.media.note='list';\
         media.cssRules.note='children';media.cssRules[0].note='nested';\
         sheet.cssRules[1].note='keyframes';}",
        "[sheet.cssRules[0].note,sheet.cssRules[0].media.note,\
         sheet.cssRules[0].cssRules.note,sheet.cssRules[0].cssRules[0].note,\
         sheet.cssRules[1].note].join('|')",
        "media|list|children|nested|keyframes",
    );
}

#[test]
fn held_sheet_keeps_state_of_keyframes_and_their_styles() {
    assert_retained(
        "globalThis.sheet=new CSSStyleSheet();\
         sheet.replaceSync('@keyframes k{from{opacity:0}to{opacity:1}} @font-face{font-family:X}');\
         {const rule=sheet.cssRules[0];rule.cssRules.note='list';rule[0].note='frame';\
         rule[1].style.note='style';sheet.cssRules[1].style.note='descriptors';}",
        "[sheet.cssRules[0].cssRules.note,sheet.cssRules[0][0].note,\
         sheet.cssRules[0].cssRules[1].style.note,sheet.cssRules[1].style.note].join('|')",
        "list|frame|style|descriptors",
    );
}

#[test]
fn held_keyframe_keeps_its_rule_and_sheet_state() {
    assert_retained(
        "globalThis.frame=(()=>{const sheet=new CSSStyleSheet();\
         sheet.replaceSync('@keyframes k{from{opacity:0}}');sheet.note='sheet';\
         const rule=sheet.cssRules[0];rule.note='rule';return rule[0]})();",
        "[frame.parentRule.note,frame.parentStyleSheet.note,frame.keyText].join('|')",
        "rule|sheet|0%",
    );
}

#[test]
fn held_media_list_keeps_the_rule_it_writes_to() {
    assert_retained(
        "globalThis.media=(()=>{const sheet=new CSSStyleSheet();\
         sheet.replaceSync('@media print{a{color:red}}');return sheet.cssRules[0].media})();\
         media.note='list';",
        "(()=>{media.appendMedium('screen');const text=media.mediaText;\
         media.deleteMedium('screen');return [media.note,text,media.mediaText].join('|')})()",
        "list|print, screen|print",
    );
}

#[test]
fn an_adopted_sheet_is_kept_by_the_list_it_is_in() {
    assert_retained(
        "{const sheet=new CSSStyleSheet();sheet.replaceSync('a{color:red}');\
         sheet.note='adopted';sheet.cssRules[0].note='rule';\
         document.adoptedStyleSheets=[sheet];}",
        "(()=>{const sheet=document.adoptedStyleSheets[0];\
         return [sheet.cssRules.length,sheet.cssRules[0].cssText].join('|')})()",
        "1|a { color: red; }",
    );
}

#[test]
fn held_style_keeps_parent_rule_state() {
    assert_retained(
        "globalThis.style=(()=>{const sheet=new CSSStyleSheet();\
         sheet.replaceSync('a{color:red}');const rule=sheet.cssRules[0];\
         rule.note='parent';return rule.style})();",
        "style.parentRule.note",
        "parent",
    );
}

#[test]
fn unreachable_sheet_rule_expando_cycle_collects() {
    let mut engine = cssom_engine();
    cssom_eval(
        &mut engine,
        "(()=>{const sheet=new CSSStyleSheet();sheet.replaceSync('a{color:red}');\
         const rule=sheet.cssRules[0];sheet.peer=rule;rule.peer=sheet;\
         globalThis.sheetRef=new WeakRef(sheet);globalThis.ruleRef=new WeakRef(rule)})();",
    );
    PageEngine::run_microtasks(&mut engine);
    for _ in 0..3 {
        PageEngine::trim_memory(&mut engine);
        assert!(
            engine.native_failure().is_none(),
            "native CSSOM collection failed"
        );
    }
    assert_eq!(
        cssom_eval(
            &mut engine,
            "sheetRef.deref()===undefined&&ruleRef.deref()===undefined"
        ),
        "true",
    );
}

#[test]
fn held_detached_rule_keeps_its_style_wrapper_state() {
    assert_retained(
        "globalThis.sheet=new CSSStyleSheet();sheet.replaceSync('a{color:red}');\
         globalThis.old=sheet.cssRules[0];old.style.note='detached';\
         sheet.replaceSync('b{color:blue}');",
        "[old.parentStyleSheet===null,old.style.note,sheet.cssRules[0]!==old].join('|')",
        "true|detached|true",
    );
}

#[test]
fn live_sheet_does_not_retain_removed_rule_wrapper() {
    let mut engine = cssom_engine();
    cssom_eval(
        &mut engine,
        "globalThis.sheet=new CSSStyleSheet();sheet.replaceSync('a{color:red}');\
         globalThis.oldRef=(()=>{const old=sheet.cssRules[0];\
         const weak=new WeakRef(old);sheet.replaceSync('b{color:blue}');return weak})();",
    );
    PageEngine::run_microtasks(&mut engine);
    for _ in 0..3 {
        PageEngine::trim_memory(&mut engine);
        assert!(
            engine.native_failure().is_none(),
            "native CSSOM collection failed"
        );
    }
    assert_eq!(
        cssom_eval(
            &mut engine,
            "sheet.cssRules.length===1&&oldRef.deref()===undefined"
        ),
        "true",
    );
}

fn assert_after_collection(setup: &str, query: &str, expected: &str) {
    let mut engine = cssom_engine();
    cssom_eval(&mut engine, setup);
    PageEngine::run_microtasks(&mut engine);
    for _ in 0..3 {
        PageEngine::trim_memory(&mut engine);
        assert!(
            engine.native_failure().is_none(),
            "native CSSOM collection failed"
        );
    }
    assert_eq!(cssom_eval(&mut engine, query), expected);
}

#[test]
fn held_nonempty_top_list_preserves_sheet_without_materializing_an_intermediate_rule() {
    assert_after_collection(
        "globalThis.list=(()=>{const sheet=new CSSStyleSheet();\
         sheet.replaceSync('a{color:red}');sheet.note='sheet';return sheet.cssRules})();",
        "list[0].parentStyleSheet.note",
        "sheet",
    );
}

#[test]
fn held_nonempty_child_list_preserves_parent_without_materializing_a_child_wrapper() {
    assert_after_collection(
        "globalThis.list=(()=>{const sheet=new CSSStyleSheet();\
         sheet.replaceSync('a{& b{color:red}}');const parent=sheet.cssRules[0];\
         parent.note='parent';return parent.cssRules})();",
        "list[0].parentRule.note",
        "parent",
    );
}

#[test]
fn held_empty_top_list_does_not_retain_its_sheet_wrapper() {
    assert_after_collection(
        "globalThis.list=(()=>{const sheet=new CSSStyleSheet();\
         globalThis.sheetRef=new WeakRef(sheet);return sheet.cssRules})();",
        "sheetRef.deref()===undefined&&list.length===0",
        "true",
    );
}

#[test]
fn held_empty_child_list_does_not_retain_its_parent_wrapper() {
    assert_after_collection(
        "globalThis.list=(()=>{const sheet=new CSSStyleSheet();\
         sheet.replaceSync('a{color:red}');const parent=sheet.cssRules[0];\
         globalThis.parentRef=new WeakRef(parent);return parent.cssRules})();",
        "parentRef.deref()===undefined&&list.length===0",
        "true",
    );
}

#[test]
fn held_parent_does_not_retain_a_deleted_child_wrapper() {
    assert_after_collection(
        "globalThis.parent=(()=>{const sheet=new CSSStyleSheet();\
         sheet.replaceSync('a{& b{color:red}}');const parent=sheet.cssRules[0];\
         const child=parent.cssRules[0];globalThis.childRef=new WeakRef(child);\
         parent.deleteRule(0);return parent})();",
        "childRef.deref()===undefined&&parent.cssRules.length===0",
        "true",
    );
}

#[test]
fn held_removed_ancestor_preserves_a_still_listed_descendant() {
    assert_after_collection(
        "globalThis.ancestor=(()=>{const sheet=new CSSStyleSheet();\
         sheet.replaceSync('a{& b{color:red}}');const ancestor=sheet.cssRules[0];\
         ancestor.cssRules[0].note='child';sheet.deleteRule(0);return ancestor})();",
        "[ancestor.cssRules[0].note,ancestor.cssRules[0].parentRule===ancestor].join('|')",
        "child|true",
    );
}

#[test]
fn held_descendant_preserves_removed_parent_and_its_live_sheet_link() {
    assert_after_collection(
        "globalThis.child=(()=>{const sheet=new CSSStyleSheet();\
         sheet.replaceSync('a{& b{color:red}}');sheet.note='sheet';\
         const ancestor=sheet.cssRules[0];ancestor.note='parent';\
         const child=ancestor.cssRules[0];sheet.deleteRule(0);return child})();",
        "[child.parentRule.note,child.parentStyleSheet.note].join('|')",
        "parent|sheet",
    );
}

#[test]
fn replaced_descendant_keeps_its_parent_but_not_the_old_sheet_wrapper() {
    assert_after_collection(
        "globalThis.child=(()=>{const sheet=new CSSStyleSheet();\
         sheet.replaceSync('a{& b{color:red}}');globalThis.sheetRef=new WeakRef(sheet);\
         const parent=sheet.cssRules[0];parent.note='parent';\
         const child=parent.cssRules[0];sheet.replaceSync('c{color:blue}');return child})();",
        "[child.parentRule.note,child.parentStyleSheet===null,\
         sheetRef.deref()===undefined].join('|')",
        "parent|true|true",
    );
}
