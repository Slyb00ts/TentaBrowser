// Included by V8 and QuickJS adapters to exercise CSSStyleDeclaration's
// per-property attributes (camel-cased, dashed and webkit-cased) on both engines.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;

fn eval(source: &str) -> String {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = new_engine(document);
    engine.evaluate(source).expect("CSSStyleProperties script")
}

#[test]
fn camel_cased_attributes_read_and_write_the_style_attribute() {
    assert_eq!(
        eval(
            "const d=document.createElement('div'); \
             d.style.backgroundColor='red'; \
             d.style.width='10px'; \
             [d.style.backgroundColor,d.style.getPropertyValue('background-color'), \
              d.style.width,d.style.length,d.getAttribute('style')].join('|')"
        ),
        "red|red|10px|2|background-color: red; width: 10px;"
    );
}

#[test]
fn dashed_attributes_are_reachable_by_bracket_access() {
    assert_eq!(
        eval(
            "const d=document.createElement('div'); \
             d.style['background-color']='blue'; \
             [d.style['background-color'],d.style.backgroundColor, \
              d.getAttribute('style')].join('|')"
        ),
        "blue|blue|background-color: blue;"
    );
}

#[test]
fn css_float_and_the_float_property_agree() {
    assert_eq!(
        eval(
            "const d=document.createElement('div'); \
             d.style.cssFloat='left'; \
             const first=[d.style.cssFloat,d.style.getPropertyValue('float'), \
                          d.getAttribute('style')].join(','); \
             d.style.setProperty('float','right'); \
             [first,d.style.cssFloat].join('|')"
        ),
        "left,left,float: left;|right"
    );
}

#[test]
fn webkit_prefixed_properties_have_both_cased_attributes() {
    assert_eq!(
        eval(
            "const d=document.createElement('div'); \
             d.style.webkitUserSelect='none'; \
             const lower=[d.style.webkitUserSelect,d.style.WebkitUserSelect, \
                          d.style['-webkit-user-select']].join(','); \
             d.style.WebkitUserSelect='auto'; \
             [lower,d.style.webkitUserSelect, \
              'webkitUserSelect' in CSSStyleProperties.prototype, \
              'WebkitUserSelect' in CSSStyleProperties.prototype].join('|')"
        ),
        "none,none,none|auto|true|true"
    );
}

#[test]
fn an_empty_value_removes_the_declaration() {
    assert_eq!(
        eval(
            "const d=document.createElement('div'); \
             d.style.color='red'; d.style.width='1px'; \
             d.style.color=''; \
             const removed=[d.style.color,d.style.length,d.getAttribute('style')].join(','); \
             d.style.width=null; \
             [removed,d.style.length,d.getAttribute('style')].join('|')"
        ),
        ",1,width: 1px;|0|"
    );
}

#[test]
fn invalid_values_and_priorities_are_ignored() {
    assert_eq!(
        eval(
            "const d=document.createElement('div'); \
             d.style.color='red'; \
             d.style.color='definitely-not-a-color'; \
             d.style.color='blue !important'; \
             d.style.width='-'; \
             [d.style.color,d.style.getPropertyPriority('color'),d.style.width, \
              d.getAttribute('style')].join('|')"
        ),
        "red|||color: red;"
    );
}

#[test]
fn assignment_resets_the_priority_like_set_property() {
    assert_eq!(
        eval(
            "const d=document.createElement('div'); \
             d.style.setProperty('color','red','important'); \
             const before=d.style.getPropertyPriority('color'); \
             d.style.color='blue'; \
             [before,d.style.getPropertyPriority('color'),d.style.color, \
              d.getAttribute('style')].join('|')"
        ),
        "important||blue|color: blue;"
    );
}

#[test]
fn shorthand_assignment_expands_to_longhands() {
    assert_eq!(
        eval(
            "const d=document.createElement('div'); \
             d.style.margin='1px 2px'; \
             const expanded=[d.style.marginTop,d.style.marginRight, \
                             d.style.getPropertyValue('margin-left'),d.style.length]; \
             d.style.marginTop='9px'; \
             [expanded.join(','),d.style.margin,d.getAttribute('style')].join('|')"
        ),
        "1px,2px,2px,4|9px 2px 1px|margin: 9px 2px 1px;"
    );
}

#[test]
fn attributes_live_on_the_prototype_not_the_instance() {
    assert_eq!(
        eval(
            "const s=document.createElement('div').style; \
             const names=['backgroundColor','background-color','cssFloat','marginTop']; \
             const own=Object.getOwnPropertyNames(s).filter(n=>isNaN(n)); \
             const descriptor=Object.getOwnPropertyDescriptor( \
                 CSSStyleProperties.prototype,'backgroundColor'); \
             [names.map(n=>n in s).join(','), \
              names.map(n=>Object.hasOwn(CSSStyleProperties.prototype,n)).join(','), \
              own.length,typeof descriptor.get,typeof descriptor.set, \
              descriptor.enumerable,descriptor.configurable, \
              'color' in CSSStyleDeclaration.prototype, \
              'color' in CSSStyleProperties.prototype,'float' in s, \
              'notAProperty' in s, s.notAProperty].join('|')"
        ),
        "true,true,true,true|true,true,true,true|0|function|function|true|true|false|true|false|false|"
    );
}

#[test]
fn receivers_that_are_not_declarations_are_refused() {
    assert_eq!(
        eval(
            "document.createElement('div').style; \
             const get=Object.getOwnPropertyDescriptor( \
                 CSSStyleProperties.prototype,'color').get; \
             const set=Object.getOwnPropertyDescriptor( \
                 CSSStyleProperties.prototype,'color').set; \
             const results=[{},document.createElement('div'),CSSStyleProperties.prototype] \
                 .map(r=>{ try { get.call(r); set.call(r,'red'); return 'ok'; } \
                           catch (e) { return e instanceof TypeError; } }); \
             results.join(',')"
        ),
        "true,true,true"
    );
}

#[test]
fn style_rule_declarations_share_the_attributes() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet(); sheet.replaceSync('a {color:red}'); \
             const rule=sheet.cssRules[0]; \
             rule.style.backgroundColor='blue'; \
             rule.style.color=''; \
             [rule.style.backgroundColor,rule.style.color,rule.style.cssFloat, \
              rule.cssText].join('|')"
        ),
        "blue|||a { background-color: blue; }"
    );
}

#[test]
fn properties_enabled_for_unimplemented_layout_are_attributes() {
    assert_eq!(
        eval(
            "const d=document.createElement('div'); \
             d.style.textOverflow='ellipsis'; d.style.userSelect='none'; \
             [d.style.textOverflow,d.style['user-select'],d.getAttribute('style')].join('|')"
        ),
        "ellipsis|none|text-overflow: ellipsis; user-select: none;"
    );
}
