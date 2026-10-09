// Included by V8 and QuickJS adapters to exercise the same CSSOM behavior.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js::engine::PageEngine;

fn eval(source: &str) -> String {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = new_engine(document);
    engine.evaluate(source).expect("CSSOM script")
}

#[test]
fn constructed_style_sheet_exposes_typed_live_rules() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet(); \
             const list=sheet.cssRules;sheet.replaceSync('a {color:red} b {float:left}'); \
             const rule=list.item(1);const style=rule.style;const children=rule.cssRules; \
             [sheet instanceof CSSStyleSheet,sheet instanceof StyleSheet, \
             list===sheet.cssRules,list.length,list[1]===rule,rule instanceof CSSStyleRule, \
              rule instanceof CSSGroupingRule,rule instanceof CSSRule, \
             rule.constructor.name,rule.type===CSSRule.STYLE_RULE, \
             CSSRule.MEDIA_RULE===4,rule.parentStyleSheet===sheet, \
              rule.parentRule===null,style instanceof CSSStyleProperties, \
              style instanceof CSSStyleDeclaration,style.parentRule===rule, \
              style===rule.style,children===rule.cssRules,children.length, \
              style[0]===style.item(0),Object.keys(style).includes('0'), \
              typeof rule.insertRule,typeof rule.deleteRule,style.cssFloat].join('|')",
        ),
        "true|true|true|2|true|true|true|true|CSSStyleRule|true|true|true|true|true|true|true|true|true|0|true|true|function|function|left"
    );
}

#[test]
fn rule_and_style_identity_survive_index_shifts_and_deletion() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet(); \
             sheet.replaceSync('a {color:red} b {color:blue}'); \
             const list=sheet.cssRules;const rule=list.item(1);const style=rule.style; \
             sheet.insertRule('c {color:green}',0); \
             const shifted=list.item(2)===rule; \
             style.setProperty('float','left'); \
             const live=list.item(2).style===style && rule.cssText.includes('float'); \
             sheet.deleteRule(2); \
             style.cssFloat='right'; \
             [shifted,live,list.length,rule.parentStyleSheet===null, \
              style.parentRule===rule,style.cssFloat,rule.cssText.includes('right')].join('|')",
        ),
        "true|true|2|true|true|right|true"
    );
}

#[test]
fn rejected_rule_kinds_do_not_change_the_sheet() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet();sheet.replaceSync('a {color:red}'); \
             const first=sheet.cssRules.item(0);const list=sheet.cssRules; \
             let insert='none',replace='none',importResult='none',nested='none'; \
             try{sheet.insertRule('@media screen { b {color:blue} }',1)}catch(e){insert=e.name} \
             try{sheet.replaceSync('b {color:blue} @supports(display:grid){ c {color:red} }')}catch(e){replace=e.name} \
             try{sheet.insertRule('@import url(\"unused.css\")',0)}catch(e){importResult=e.name} \
             try{first.insertRule('@media screen { b {color:blue} }',0)}catch(e){nested=e.name} \
             [insert,replace,importResult,nested,list===sheet.cssRules,list.length, \
              list.item(0)===first,first.parentStyleSheet===sheet].join('|')",
        ),
        "NotSupportedError|NotSupportedError|SyntaxError|NotSupportedError|true|1|true|true"
    );
}

#[test]
fn style_assignment_forwards_and_css_float_uses_legacy_null_conversion() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet();sheet.replaceSync('a {color:red}'); \
             const rule=sheet.cssRules.item(0);const ruleStyle=rule.style; \
             rule.style='color: blue; float: left'; \
             const element=document.createElement('div');const inline=element.style; \
             element.style='color: green; float: right';inline.cssFloat=null; \
             [rule.style===ruleStyle,ruleStyle.getPropertyValue('color'), \
              ruleStyle.cssFloat,element.style===inline, \
              inline.getPropertyValue('color'),inline.cssFloat, \
              inline.getPropertyValue('float')].join('|')",
        ),
        "true|blue|left|true|green||"
    );
}

#[test]
fn style_put_forwards_observes_target_getter_and_assignment_errors() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet();sheet.replaceSync('a {color:red}'); \
             const rule=sheet.cssRules[0]; \
             const setter=Object.getOwnPropertyDescriptor(CSSStyleRule.prototype,'style').set; \
             const failures=[]; \
             let foreignRead=false; \
             const foreign={get style(){foreignRead=true;return {cssText:'untouched'}}}; \
             try{setter.call(foreign,'color:blue')}catch(e){failures.push(e.name)} \
             Object.defineProperty(rule,'style',{configurable:true,get(){throw new RangeError('getter')}}); \
             try{setter.call(rule,'color:blue')}catch(e){failures.push(e.name)} \
             Object.defineProperty(rule,'style',{configurable:true,get(){return 7}}); \
             try{setter.call(rule,'color:blue')}catch(e){failures.push(e.name)} \
             Object.defineProperty(rule,'style',{configurable:true,get(){return Object.freeze({cssText:''})}}); \
             let frozen='silent'; \
             try{setter.call(rule,'color:blue')}catch(e){frozen=e.name} \
             Object.defineProperty(rule,'style',{configurable:true,get(){return new Proxy({cssText:''},{set(){return false}})}}); \
             let falseTrap='silent'; \
             try{(()=>{'use strict';setter.call(rule,'color:blue')})()}catch(e){falseTrap=e.name} \
             let forwarded='none';const savedReflectSet=Reflect.set; \
             Reflect.set=()=>{throw new Error('tampered')}; \
             try{Object.defineProperty(rule,'style',{configurable:true,get(){return {set cssText(value){forwarded=value}}}}); \
                 setter.call(rule,'color:blue')}catch(e){forwarded=e.name}finally{Reflect.set=savedReflectSet} \
             Object.defineProperty(rule,'style',{configurable:true,get(){return new Proxy({cssText:''},{set(){throw new SyntaxError('setter')}})}}); \
             try{setter.call(rule,'color:blue')}catch(e){failures.push(e.name)} \
             delete rule.style; \
             [failures.join(','),foreignRead,frozen,falseTrap,forwarded,rule.style.getPropertyValue('color')].join('|')",
        ),
        "TypeError,RangeError,TypeError,SyntaxError|false|silent|silent|color:blue|red"
    );
}

#[test]
fn css_float_legacy_null_conversion_preserves_other_domstring_coercions() {
    assert_eq!(
        eval(
            "const style=document.createElement('div').style;const calls=[]; \
             style.cssFloat={toString(){calls.push('object');return 'left'}}; \
             const fromObject=style.cssFloat;style.cssFloat=undefined; \
             const fromUndefined=style.cssFloat;style.cssFloat=null; \
             [calls.join(','),fromObject,fromUndefined,style.cssFloat, \
              style.getPropertyValue('float')].join('|')",
        ),
        "object|left|left||"
    );
}

#[test]
fn set_property_null_value_removes_and_undefined_value_remains_invalid() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet();sheet.replaceSync('a {color:red}'); \
             const style=sheet.cssRules[0].style; \
             style.setProperty('color',null);const removed=style.getPropertyValue('color'); \
             style.setProperty('color','blue','important'); \
             style.setProperty('color','green',null); \
             const normal=[style.getPropertyValue('color'),style.getPropertyPriority('color')]; \
             style.setProperty('color','red',undefined); \
             const unchanged=style.getPropertyValue('color'); \
             style.setProperty('color',undefined); \
             [removed,normal.join(','),unchanged,style.getPropertyValue('color')].join('|')",
        ),
        "|green,|red|red"
    );
}

#[test]
fn insert_rule_reports_generic_syntax_before_list_index() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet();sheet.replaceSync('a {color:red}'); \
             let malformed='none',multiple='none',imported='none',unknown='none'; \
             try{sheet.insertRule('garbage',99)}catch(e){malformed=e.name} \
             try{sheet.insertRule('b {} c {}',99)}catch(e){multiple=e.name} \
             try{sheet.insertRule('@import url(\"unused.css\");',99)}catch(e){imported=e.name} \
             try{sheet.insertRule('@unsupported value;',99)}catch(e){unknown=e.name} \
             [malformed,multiple,imported,unknown,sheet.cssRules.length].join('|')",
        ),
        "SyntaxError|SyntaxError|SyntaxError|IndexSizeError|1"
    );
}

#[test]
fn replace_sync_strips_imports_and_detaches_prior_rule() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet();sheet.replaceSync('a {color:red}'); \
             const list=sheet.cssRules;const old=list.item(0); \
             sheet.replaceSync('@import url(\"unused.css\"); p {color:blue}'); \
             [list===sheet.cssRules,list.length,old.parentStyleSheet===null, \
              list.item(0)!==old,list.item(0).selectorText].join('|')",
        ),
        "true|1|true|true|p"
    );
}

#[test]
fn rule_and_style_recreate_sheet_wrapper_after_gc() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = new_engine(document);
    engine
        .evaluate(
            "globalThis.rule=(()=>{const sheet=new CSSStyleSheet(); \
             sheet.replaceSync('a {color:red}');return sheet.cssRules.item(0)})(); \
             globalThis.style=rule.style;",
        )
        .expect("retain rule and style without sheet wrapper");
    PageEngine::trim_memory(&mut engine);
    assert_eq!(
        engine
            .evaluate(
                "const sheet=rule.parentStyleSheet; \
                 [sheet instanceof CSSStyleSheet,sheet.cssRules.item(0)===rule, \
                  style.parentRule===rule,rule.style===style,style.getPropertyValue('color')].join('|')",
            )
            .expect("recreate canonical sheet and rule wrappers"),
        "true|true|true|true|red"
    );
}

#[test]
fn nested_style_rules_have_live_lists_parents_and_declarations() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet(); \
             sheet.replaceSync('.outer {color:red; & .child {color:blue}}'); \
             const parent=sheet.cssRules[0], list=parent.cssRules, child=list[0]; \
             const style=child.style; \
             const initial=[child instanceof CSSStyleRule, child.parentRule===parent, \
               child.parentStyleSheet===sheet,list===parent.cssRules, \
               style.parentRule===child,style.getPropertyValue('color')].join('|'); \
             child.selectorText='> .changed';style.setProperty('color','green'); \
             parent.insertRule('& .before {color: purple}',0); \
             const shifted=list[1]===child; \
             parent.deleteRule(1); \
             [initial,shifted,list.length,child.parentRule===null, \
               child.parentStyleSheet===null,style.getPropertyValue('color'), \
               child.selectorText.includes('.changed')].join('|')",
        ),
        "true|true|true|true|true|blue|true|1|true|true|green|true"
    );
}

#[test]
fn removing_parent_preserves_descendant_parent_rule_identity() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet(); \
             sheet.replaceSync('.outer {& .middle {& .leaf {color:red}}}'); \
             const parent=sheet.cssRules[0],middle=parent.cssRules[0],leaf=middle.cssRules[0]; \
             sheet.deleteRule(0); \
             const retained=parent.parentStyleSheet===null && \
               leaf.parentStyleSheet===sheet && middle.parentStyleSheet===sheet && \
               middle.parentRule===parent && \
               leaf.parentRule===middle; \
             middle.insertRule('& .other {color: blue}',0); \
             const other=middle.cssRules[0]; \
             parent.insertRule('& .late {& .nested {color: purple}}',1); \
             const late=parent.cssRules[1],nested=late.cssRules[0]; \
             const detachedInsertion=late.parentStyleSheet===null && \
               nested.parentStyleSheet===null && late.parentRule===parent; \
             parent.deleteRule(0); \
             const directRemoval=middle.parentStyleSheet===null && \
               middle.parentRule===null && leaf.parentStyleSheet===sheet && \
               leaf.parentRule===middle && other.parentStyleSheet===sheet; \
             middle.insertRule('& .new {color: green}',0); \
             const newChild=middle.cssRules[0]; \
             [retained,detachedInsertion,directRemoval, \
               newChild.parentStyleSheet===null,middle.cssRules[2]===leaf, \
               leaf.style.getPropertyValue('color'),sheet.cssRules.length].join('|')",
        ),
        "true|true|true|true|true|red|0"
    );
}

#[test]
fn replacement_detaches_prior_nested_rule_tree() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet(); \
             sheet.replaceSync('.outer {& .inner {color:red}}'); \
             const outer=sheet.cssRules[0],inner=outer.cssRules[0]; \
             sheet.replaceSync('.new {color:blue}'); \
             [outer.parentStyleSheet===null,inner.parentStyleSheet===null, \
               inner.parentRule===outer,sheet.cssRules[0].parentStyleSheet===sheet, \
               inner.style.getPropertyValue('color')].join('|')",
        ),
        "true|true|true|true|red"
    );
}

#[test]
fn unsupported_nested_kinds_leave_parent_tree_unchanged() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet();sheet.replaceSync('.outer {color:red}'); \
             const parent=sheet.cssRules[0],list=parent.cssRules; \
             let index='none',kind='none',descendant='none',deletion='none',replacement='none'; \
             try{parent.insertRule('& .child {}',9)}catch(e){index=e.name} \
             try{parent.insertRule('@media all {& .child {}}',0)}catch(e){kind=e.name} \
             try{parent.insertRule('& .child {@media all {& .grandchild {}}}',0)}catch(e){descendant=e.name} \
             try{parent.deleteRule(0)}catch(e){deletion=e.name} \
             try{sheet.replaceSync('.outer {@supports (display:grid) {& .child {}}}')}catch(e){replacement=e.name} \
             [index,kind,descendant,deletion,replacement,list===parent.cssRules,list.length, \
               sheet.cssRules[0]===parent,parent.parentStyleSheet===sheet].join('|')",
        ),
        "IndexSizeError|NotSupportedError|NotSupportedError|IndexSizeError|NotSupportedError|true|0|true|true"
    );
}
