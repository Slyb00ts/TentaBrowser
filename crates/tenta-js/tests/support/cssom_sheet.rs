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
fn refused_rules_do_not_change_the_sheet() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet();sheet.replaceSync('a {color:red}'); \
             const first=sheet.cssRules.item(0);const list=sheet.cssRules; \
             let importResult='none',namespace='none',nested='none',unknown='none'; \
             try{sheet.insertRule('@import url(\"unused.css\")',0)}catch(e){importResult=e.name} \
             try{sheet.insertRule('@namespace svg url(x);',0)}catch(e){namespace=e.name} \
             try{first.insertRule('@font-face { font-family: X }',0)}catch(e){nested=e.name} \
             try{sheet.insertRule('@container (min-width:1px) { b {color:blue} }',1)}catch(e){unknown=e.name} \
             [importResult,namespace,nested,unknown,list===sheet.cssRules,list.length, \
              list.item(0)===first,first.cssRules.length,first.parentStyleSheet===sheet].join('|')",
        ),
        "SyntaxError|InvalidStateError|HierarchyRequestError|SyntaxError|true|1|true|0|true"
    );
}

#[test]
fn every_rule_is_an_object_of_the_interface_for_its_kind() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet(); \
             sheet.replaceSync('@namespace svg url(http://www.w3.org/2000/svg); \
               @layer reset, base; \
               @media screen and (min-width: 600px) { a {color:red} } \
               @supports (display: grid) { a {display:grid} } \
               @layer base { a {color:red} } \
               @scope (.card) to (.content) { a {color:red} } \
               @keyframes spin { to { rotate: 1turn } } \
               @font-face { font-family: X; src: url(x.woff2) } \
               @font-feature-values Y { @styleset { nice: 1 } } \
               @property --gap { syntax: \"<length>\"; inherits: false; initial-value: 4px } \
               a { color: red; & b {color:blue} float: left }'); \
             const rules=[...sheet.cssRules]; \
             const row=r=>[r.constructor.name,r.type,r instanceof CSSRule, \
               r instanceof CSSGroupingRule,r.parentStyleSheet===sheet,r.parentRule===null].join(','); \
             rules.map(row).join('|')",
        ),
        "CSSNamespaceRule,10,true,false,true,true|\
         CSSLayerStatementRule,0,true,false,true,true|\
         CSSMediaRule,4,true,true,true,true|\
         CSSSupportsRule,12,true,true,true,true|\
         CSSLayerBlockRule,0,true,true,true,true|\
         CSSScopeRule,0,true,true,true,true|\
         CSSKeyframesRule,7,true,false,true,true|\
         CSSFontFaceRule,5,true,false,true,true|\
         CSSFontFeatureValuesRule,14,true,false,true,true|\
         CSSPropertyRule,0,true,false,true,true|\
         CSSStyleRule,1,true,true,true,true"
    );
}

#[test]
fn at_rules_report_their_own_attributes() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet(); \
             sheet.replaceSync('@namespace svg url(http://www.w3.org/2000/svg); \
               @layer reset, base.widgets; \
               @media screen and (min-width: 600px), print { a {color:red} } \
               @supports (display: grid) { a {display:grid} } \
               @supports (display: nonsense) { a {display:grid} } \
               @layer base { a {color:red} } \
               @layer { a {color:red} } \
               @scope (.card) to (.content) { a {color:red} } \
               @scope { a {color:red} } \
               @keyframes spin { to { rotate: 1turn } } \
               @font-feature-values Y { @styleset { nice: 1 } } \
               @property --gap { syntax: \"<length>\"; inherits: false; initial-value: 4px } \
               @property --any { syntax: \"*\"; inherits: true }'); \
             const r=sheet.cssRules; \
             [r[0].prefix,r[0].namespaceURI,JSON.stringify(r[1].nameList), \
              Object.isFrozen(r[1].nameList),r[2].conditionText, \
              r[2] instanceof CSSConditionRule,r[3].conditionText,r[3].matches,r[4].matches, \
              r[5].name,JSON.stringify(r[6].name),r[7].start,r[7].end,r[8].start===null, \
              r[8].end===null,r[9].name,r[10].fontFamily,r[11].name,r[11].syntax, \
              r[11].inherits,r[11].initialValue,r[12].syntax,r[12].inherits, \
              r[12].initialValue===null, \
              r[9].cssText.replace(/\\s+/g,' '),r[2].cssRules[0].parentRule===r[2], \
              r[2].cssRules[0].cssText].join('|')",
        ),
        "svg|http://www.w3.org/2000/svg|[\"reset\",\"base.widgets\"]|true|\
         screen and (min-width: 600px), print|true|(display: grid)|true|false|\
         base|\"\"|.card|.content|true|true|spin|Y|--gap|<length>|false|4px|*|true|true|\
         @keyframes spin { 100% { rotate: 1turn; } }|true|a { color: red; }"
    );
}

#[test]
fn a_media_list_reads_and_rewrites_the_rule_it_belongs_to() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet(); \
             sheet.replaceSync('@media screen, print { a {color:red} }'); \
             const rule=sheet.cssRules[0],media=rule.media; \
             const before=[media instanceof MediaList,media===rule.media,media.length, \
               media[0],media.item(1),media.item(2)===null,media[2]===undefined, \
               String(media),media.mediaText]; \
             rule.media='(min-width: 10px)'; \
             const forwarded=[rule.media===media,media.mediaText,rule.conditionText]; \
             media.appendMedium('print');media.appendMedium('not valid here'); \
             const appended=media.mediaText; \
             media.deleteMedium('(min-width: 10px)'); \
             let missing='none';try{media.deleteMedium('screen')}catch(e){missing=e.name} \
             media.mediaText=null; \
             const emptied=[media.length,media.mediaText,rule.cssText.replace(/\\s+/g,' ')]; \
             media.mediaText='tv'; \
             [...before,...forwarded,appended,missing,...emptied,rule.conditionText, \
              Object.keys(media).join(',')].join('|')",
        ),
        "true|true|2|screen|print|true|true|screen, print|screen, print|\
         true|(min-width: 10px)|(min-width: 10px)|(min-width: 10px), print|NotFoundError|\
         0||@media { a { color: red; } }|tv|0"
    );
}

#[test]
fn group_rules_take_and_give_up_child_rules() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet(); \
             const at=sheet.insertRule('@media print { a {color:red} }'); \
             const media=sheet.cssRules[0],list=media.cssRules,first=list[0]; \
             const index=media.insertRule('@supports (display:grid) { b {display:grid} }',1); \
             media.insertRule('c {color:blue}'); \
             const supports=list[2]; \
             supports.insertRule('d {color:green}',1); \
             const order=[...list].map(r=>r.constructor.name).join(','); \
             let relative='none';try{media.insertRule('& e {color:red} f {}',0)}catch(e){relative=e.name} \
             let range='none';try{media.deleteRule(7)}catch(e){range=e.name} \
             media.deleteRule(1); \
             const layer=sheet.cssRules[sheet.insertRule('@layer base { }',1)]; \
             layer.insertRule('@keyframes k { from { opacity: 0 } }'); \
             [at,index,order,list===media.cssRules,list.length,first.parentRule===null, \
              first.parentStyleSheet===null,list[1]===supports,supports.parentRule===media, \
              supports.cssRules[1].selectorText,supports.cssRules[1].parentRule===supports, \
              relative,range,layer.cssRules[0].constructor.name,layer.cssRules[0].parentRule===layer, \
              sheet.cssRules.length].join('|')",
        ),
        "0|1|CSSStyleRule,CSSStyleRule,CSSSupportsRule|true|2|true|true|true|true|d|true|\
         SyntaxError|IndexSizeError|CSSKeyframesRule|true|2"
    );
}

#[test]
fn declarations_after_a_nested_rule_are_a_rule_with_a_live_style() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet(); \
             sheet.replaceSync('a { color: red; & b {color:blue} float: left; margin: 0 }'); \
             const outer=sheet.cssRules[0],rest=outer.cssRules[1],style=rest.style; \
             const before=[outer.cssRules.length,rest.constructor.name,rest.type, \
               rest instanceof CSSNestedDeclarations,rest instanceof CSSStyleRule, \
               style instanceof CSSStyleProperties,style===rest.style,style.parentRule===rest, \
               style.cssFloat,style.length,rest.cssText]; \
             style.setProperty('float','right');rest.style='color: green'; \
             [...before,rest.style===style,style.cssText,rest.cssText, \
              outer.style.color,typeof rest.selectorText,typeof rest.cssRules].join('|')",
        ),
        "2|CSSNestedDeclarations|0|true|false|true|true|true|left|5|float: left; margin: 0px;|\
         true|color: green;|color: green;|red|undefined|undefined"
    );
}

#[test]
fn a_keyframes_rule_lists_finds_appends_and_deletes_its_keyframes() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet(); \
             sheet.replaceSync('@keyframes fade { from {opacity:0} 50%,75% {opacity:.5} to {opacity:1} }'); \
             const rule=sheet.cssRules[0],list=rule.cssRules,first=rule[0]; \
             const before=[rule.length,list.length,list===rule.cssRules,list instanceof CSSRuleList, \
               first===list[0],first===rule.findRule('from'),first===rule.findRule('0%'), \
               first.constructor.name,first instanceof CSSRule,first.type,first.keyText, \
               rule[1].keyText,rule[1].cssText,first.parentRule===rule, \
               first.parentStyleSheet===sheet,rule[3]===undefined,list[3]===undefined, \
               rule.findRule('33%')===null,[...rule].map(k=>k.keyText).join('/'), \
               [...list].length]; \
             rule.appendRule('to {opacity:.9}');rule.appendRule('garbage');rule.appendRule('to{} to{}'); \
             const last=rule.findRule('to'); \
             const appended=[rule.length,last===rule[3],last.style.opacity]; \
             rule.deleteRule('100%');rule.deleteRule('33%'); \
             rule.name='grow'; \
             [...before,...appended,rule.length,last.parentRule===null, \
              last.parentStyleSheet===null,last.cssText,rule.findRule('to').style.opacity, \
              rule.name,rule.cssText.replace(/\\s+/g,' ')].join('|')",
        ),
        "3|3|true|true|true|true|true|CSSKeyframeRule|true|8|0%|50%, 75%|\
         50%, 75% { opacity: 0.5; }|true|true|true|true|true|0%/50%, 75%/100%|3|\
         4|true|0.9|3|true|true|100% { opacity: 0.9; }|1|grow|\
         @keyframes grow { 0% { opacity: 0; } 50%, 75% { opacity: 0.5; } 100% { opacity: 1; } }"
    );
}

#[test]
fn a_keyframe_is_rekeyed_and_restyled_through_its_own_objects() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet(); \
             sheet.replaceSync('@keyframes fade { from {opacity:0} to {opacity:1} }'); \
             const rule=sheet.cssRules[0],frame=rule[0],style=frame.style; \
             frame.keyText='10%, 20%'; \
             let bad='none';try{frame.keyText='soon'}catch(e){bad=e.name} \
             style.setProperty('opacity','0.25');style.setProperty('color','red','important'); \
             style.top='1px';frame.style='left: 2px; opacity: 0.5'; \
             const removed=style.removeProperty('left'); \
             [frame.keyText,bad,style===frame.style,style instanceof CSSStyleProperties, \
              style.parentRule===frame,removed,style.cssText,style.length,style[0], \
              style.getPropertyPriority('opacity'),frame.cssText,rule.findRule('10%,20%')===frame, \
              rule.cssText.replace(/\\s+/g,' ')].join('|')",
        ),
        "10%, 20%|SyntaxError|true|true|true|2px|opacity: 0.5;|1|opacity||\
         10%, 20% { opacity: 0.5; }|true|\
         @keyframes fade { 10%, 20% { opacity: 0.5; } 100% { opacity: 1; } }"
    );
}

#[test]
fn a_font_face_rule_reads_its_descriptors_through_style() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet(); \
             sheet.replaceSync('@font-face { font-family: \"My Font\"; \
               src: url(\"a;b.woff2\") format(\"woff2\"); font-weight: 400 700; font-display: swap }'); \
             const rule=sheet.cssRules[0],style=rule.style; \
             style.setProperty('font-weight','900');style.cssText='font-family: Other'; \
             const removed=style.removeProperty('src'); \
             [style===rule.style,style.constructor.name,style instanceof CSSStyleDeclaration, \
              style.parentRule===rule,style.length,[...style].join(','), \
              style.getPropertyValue('font-family'),style.getPropertyValue('SRC'), \
              style.getPropertyValue('font-weight'),style.getPropertyValue('color')==='', \
              style.getPropertyPriority('src')==='',removed==='',style.item(9)==='', \
              style.cssText].join('|')",
        ),
        "true|CSSStyleDeclaration|true|true|4|font-family,src,font-weight,font-display|\
         \"My Font\"|url(\"a;b.woff2\") format(\"woff2\")|400 700|true|true|true|true|\
         font-family: \"My Font\"; src: url(\"a;b.woff2\") format(\"woff2\"); \
         font-weight: 400 700; font-display: swap;"
    );
}

#[test]
fn interfaces_without_rules_are_still_interface_objects() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet();sheet.replaceSync('a {color:red}'); \
             const rule=sheet.cssRules[0]; \
             [typeof CSSImportRule,typeof CSSPageRule,rule instanceof CSSImportRule, \
              rule instanceof CSSPageRule,Object.getPrototypeOf(CSSImportRule.prototype)===CSSRule.prototype, \
              Object.getPrototypeOf(CSSPageRule.prototype)===CSSGroupingRule.prototype, \
              Object.getPrototypeOf(CSSMediaRule.prototype)===CSSConditionRule.prototype, \
              Object.getPrototypeOf(CSSConditionRule.prototype)===CSSGroupingRule.prototype, \
              typeof CSSContainerRule,CSSRule.KEYFRAMES_RULE,CSSRule.SUPPORTS_RULE, \
              CSSRule.FONT_FEATURE_VALUES_RULE].join('|')",
        ),
        "function|function|false|false|true|true|true|true|undefined|7|12|14"
    );
}

#[test]
fn a_getter_of_one_rule_kind_refuses_a_rule_of_another() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet(); \
             sheet.replaceSync('@media print { a {color:red} } @supports (display:grid) { } b {}'); \
             const [media,supports,style]=sheet.cssRules; \
             const get=(proto,name)=>Object.getOwnPropertyDescriptor(proto,name).get; \
             const attempt=(f,on)=>{try{f.call(on);return 'ok'}catch(e){return e.name}}; \
             [attempt(get(CSSMediaRule.prototype,'media'),supports), \
              attempt(get(CSSSupportsRule.prototype,'matches'),media), \
              attempt(get(CSSStyleRule.prototype,'selectorText'),media), \
              attempt(get(CSSConditionRule.prototype,'conditionText'),style), \
              attempt(get(CSSConditionRule.prototype,'conditionText'),supports), \
              attempt(get(CSSGroupingRule.prototype,'cssRules'),style), \
              attempt(get(MediaList.prototype,'mediaText'),media), \
              attempt(get(CSSRule.prototype,'cssText'),media.media)].join('|')",
        ),
        "TypeError|TypeError|TypeError|TypeError|ok|ok|TypeError|TypeError"
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
fn a_style_rule_takes_group_rules_and_refuses_bad_indices_whole() {
    assert_eq!(
        eval(
            "const sheet=new CSSStyleSheet();sheet.replaceSync('.outer {color:red}'); \
             const parent=sheet.cssRules[0],list=parent.cssRules; \
             let index='none',deletion='none'; \
             try{parent.insertRule('& .child {}',9)}catch(e){index=e.name} \
             try{parent.deleteRule(0)}catch(e){deletion=e.name} \
             const empty=list.length; \
             parent.insertRule('@media all {& .child {color:blue}}',0); \
             const media=list[0],child=media.cssRules[0]; \
             sheet.replaceSync('.outer {@supports (display:grid) {& .child {}}}'); \
             [index,deletion,empty,list===parent.cssRules,list.length,media.constructor.name, \
              media.parentRule===parent,child.selectorText,child.parentRule===media, \
              parent.parentStyleSheet===null,media.parentStyleSheet===null, \
              sheet.cssRules[0].cssRules[0].constructor.name].join('|')",
        ),
        "IndexSizeError|IndexSizeError|0|true|1|CSSMediaRule|true|& .child|true|true|true|CSSSupportsRule"
    );
}
