//! The operations of a `Range` that change the tree (`deleteContents`,
//! `extractContents`, `cloneContents`, `insertNode`, `surroundContents`) and
//! `Text.splitText`, through the generated V8 glue.
//!
//! The algorithms are `tenta-dom`'s and their shapes are checked there
//! (`crates/tenta-dom/tests/range_contents.rs`). These tests are about what
//! script sees around them: the fragment it gets, where the range is left,
//! what other ranges and a mutation observer are told, and the refusals.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// `document -> html -> body`, and a script prologue that fills the body.
fn eval(source: &str) -> String {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let root = document.document_node();
    document
        .mutate(Mutation::Append {
            parent: root,
            child: html,
        })
        .expect("append <html>");
    document
        .mutate(Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append <body>");
    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    let prologue = "const body=document.body; \
        body.innerHTML='<p id=a>one<b id=b>two</b>three</p><p id=c>four<i id=i>five</i>six</p>'; \
        const $=id=>document.getElementById(id); \
        const at=r=>[r.startContainer.nodeName,r.startOffset,r.endContainer.nodeName,r.endOffset,r.collapsed].join(','); \
        const html=node=>{const d=document.createElement('div');d.appendChild(node);return d.innerHTML};";
    engine
        .evaluate(&format!("{prologue}{source}"))
        .expect("script should evaluate without throwing")
}

#[test]
fn extract_contents_returns_a_fragment_and_collapses_the_range() {
    assert_eq!(
        eval(
            "const range=document.createRange(); \
             range.setStart($('a').firstChild,1);range.setEnd($('i').firstChild,2); \
             const bold=$('b'); \
             const fragment=range.extractContents(); \
             [fragment instanceof DocumentFragment,fragment.childNodes.length, \
              fragment.firstChild.childNodes[1]===bold,at(range), \
              range.startContainer===body,body.innerHTML,html(fragment)].join('|')"
        ),
        "true|2|true|BODY,1,BODY,1,true|true|\
         <p id=\"a\">o</p><p id=\"c\"><i id=\"i\">ve</i>six</p>|\
         <p id=\"a\">ne<b id=\"b\">two</b>three</p><p id=\"c\">four<i id=\"i\">fi</i></p>"
    );
}

#[test]
fn clone_contents_copies_and_delete_contents_removes() {
    assert_eq!(
        eval(
            "const range=document.createRange(); \
             range.setStart($('a'),1);range.setEnd($('c'),1); \
             const before=body.innerHTML,bold=$('b'); \
             const copy=range.cloneContents(); \
             const untouched=[body.innerHTML===before,at(range),copy.firstChild.firstChild!==bold]; \
             range.deleteContents(); \
             [untouched.join(','),html(copy),at(range),body.innerHTML].join('|')"
        ),
        "true,P,1,P,1,false,true|\
         <p id=\"a\"><b id=\"b\">two</b>three</p><p id=\"c\">four</p>|\
         BODY,1,BODY,1,true|\
         <p id=\"a\">one</p><p id=\"c\"><i id=\"i\">five</i>six</p>"
    );
}

#[test]
fn a_range_in_one_text_node_and_a_collapsed_range() {
    assert_eq!(
        eval(
            "const text=$('a').firstChild,range=document.createRange(); \
             range.setStart(text,1);range.setEnd(text,2); \
             const copy=html(range.cloneContents()); \
             const cut=html(range.extractContents()); \
             const after=[text.data,at(range)]; \
             const empty=range.extractContents(); \
             range.deleteContents(); \
             [copy,cut,after.join(','),empty.childNodes.length,text.data].join('|')"
        ),
        "n|n|oe,#text,1,#text,1,true|0|oe"
    );
}

#[test]
fn insert_node_splits_text_and_extends_a_collapsed_range() {
    assert_eq!(
        eval(
            "const text=$('a').firstChild,range=document.createRange(); \
             range.setStart(text,2);range.collapse(true); \
             const mark=document.createElement('mark'); \
             range.insertNode(mark); \
             const collapsed=[$('a').innerHTML,at(range),range.startContainer===text, \
               range.endContainer===$('a'),$('a').childNodes.length]; \
             const wide=document.createRange(); \
             wide.setStart($('c'),0);wide.setEnd($('c'),3); \
             wide.insertNode(document.createElement('u')); \
             const fragment=document.createDocumentFragment(); \
             fragment.append('x',document.createElement('s')); \
             const end=document.createRange();end.selectNodeContents($('i'));end.collapse(false); \
             end.insertNode(fragment); \
             [collapsed.join(','),$('c').innerHTML,at(wide),at(end), \
              fragment.childNodes.length].join('|')"
        ),
        "on<mark></mark>e<b id=\"b\">two</b>three,#text,2,P,2,false,true,true,5|\
         <u></u>four<i id=\"i\">fivex<s></s></i>six|P,0,P,4,false|I,1,I,3,false|0"
    );
}

#[test]
fn insert_node_refuses_before_it_changes_anything() {
    assert_eq!(
        eval(
            "const text=$('a').firstChild,range=document.createRange(); \
             const attempt=(start,offset,node)=>{range.setStart(start,offset);range.collapse(true); \
               try{range.insertNode(node);return 'ok'}catch(e){return e.name}}; \
             const comment=$('a').appendChild(document.createComment('note')); \
             const detached=document.createTextNode('free'); \
             const results=[attempt(comment,1,document.createElement('u')), \
               attempt(text,1,text),attempt(text,1,body),attempt(detached,1,document.createElement('u')), \
               attempt(text,1,document)]; \
             let arity='none';try{range.insertNode()}catch(e){arity=e.name} \
             [results.join(','),arity,text.data,$('a').childNodes.length,detached.data].join('|')"
        ),
        "HierarchyRequestError,HierarchyRequestError,HierarchyRequestError,\
         HierarchyRequestError,HierarchyRequestError|TypeError|one|4|free"
    );
}

#[test]
fn surround_contents_wraps_the_range_and_selects_the_wrapper() {
    assert_eq!(
        eval(
            "const paragraph=$('a'),range=document.createRange(); \
             range.setStart(paragraph.firstChild,1);range.setEnd(paragraph.lastChild,2); \
             const wrapper=document.createElement('span'); \
             wrapper.textContent='old content goes'; \
             range.surroundContents(wrapper); \
             const wrapped=[paragraph.innerHTML,at(range),range.startContainer===paragraph, \
               paragraph.childNodes[range.startOffset]===wrapper]; \
             const bad=document.createRange(); \
             bad.setStart($('c').firstChild,1);bad.setEnd($('i').firstChild,2); \
             const attempt=(r,node)=>{try{r.surroundContents(node);return 'ok'}catch(e){return e.name}}; \
             const before=$('c').innerHTML; \
             const refusals=[attempt(bad,document.createElement('u')), \
               attempt(range,document.createDocumentFragment()),attempt(range,document), \
               attempt(bad,document.createDocumentFragment())]; \
             [wrapped.join(','),refusals.join(','),$('c').innerHTML===before].join('|')"
        ),
        "o<span>ne<b id=\"b\">two</b>th</span>ree,P,1,P,2,false,true,true|\
         InvalidStateError,InvalidNodeTypeError,InvalidNodeTypeError,InvalidStateError|true"
    );
}

#[test]
fn split_text_and_whole_text() {
    assert_eq!(
        eval(
            "const paragraph=$('a'),text=paragraph.firstChild; \
             const range=document.createRange();range.setStart(text,1);range.setEnd(text,3); \
             const after=document.createRange();after.setStart(paragraph,1);after.collapse(true); \
             const tail=text.splitText(2); \
             const split=[tail instanceof Text,text.data,tail.data,text.nextSibling===tail, \
               at(range),range.endContainer===tail,at(after),paragraph.childNodes.length]; \
             let past='none';try{text.splitText(3)}catch(e){past=e.name} \
             const free=document.createTextNode('abcd'),rest=free.splitText(1); \
             const whole=[text.wholeText,tail.wholeText,paragraph.lastChild.wholeText, \
               free.wholeText,rest.parentNode===null,rest.data]; \
             [split.join(','),past,whole.join(','),paragraph.innerHTML, \
              typeof Text.prototype.splitText].join('|')"
        ),
        "true,on,e,true,#text,1,#text,1,false,true,P,2,P,2,true,4|IndexSizeError|\
         one,one,three,a,true,bcd|one<b id=\"b\">two</b>three|function"
    );
}

#[test]
fn create_contextual_fragment_parses_in_the_context_of_the_range() {
    assert_eq!(
        eval(
            "const range=document.createRange(); \
             body.insertAdjacentHTML('beforeend','<table><tbody id=rows></tbody></table>'); \
             range.selectNodeContents($('rows')); \
             const rows=range.createContextualFragment('<tr><td>cell</td></tr>'); \
             range.selectNodeContents($('a')); \
             const inline=range.createContextualFragment('<tr><td>cell</td></tr><em>x</em>'); \
             range.setStart($('a').firstChild,1); \
             const text=range.createContextualFragment('<i>y</i>'); \
             const before=body.innerHTML; \
             range.selectNodeContents($('rows'));range.deleteContents(); \
             $('rows').appendChild(rows); \
             let arity='none';try{range.createContextualFragment()}catch(e){arity=e.name} \
             [rows instanceof DocumentFragment,inline.childNodes.length,html(inline),html(text), \
              body.innerHTML===before,$('rows').innerHTML,rows.childNodes.length,arity, \
              String(null)===html(range.createContextualFragment(null))].join('|')"
        ),
        "true|2|cell<em>x</em>|<i>y</i>|false|<tr><td>cell</td></tr>|0|TypeError|true"
    );
}

#[test]
fn other_ranges_and_an_observer_follow_an_extraction() {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let root = document.document_node();
    document
        .mutate(Mutation::Append {
            parent: root,
            child: html,
        })
        .expect("append <html>");
    document
        .mutate(Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append <body>");
    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    engine
        .evaluate(
            "const body=document.body; \
             body.innerHTML='<p id=a>one<b id=b>two</b>three</p><p id=c>four<i id=i>five</i>six</p>'; \
             const $=id=>document.getElementById(id); \
             globalThis.seen=[]; \
             new MutationObserver(records=>{for(const r of records){ \
               seen.push(r.type==='characterData'?'data:'+r.target.data \
                 :'list:'+r.target.nodeName+'-'+r.removedNodes.length+'+'+r.addedNodes.length)}}) \
               .observe(body,{subtree:true,childList:true,characterData:true}); \
             globalThis.moved=document.createRange(); \
             moved.setStart($('a').lastChild,1);moved.setEnd($('a').lastChild,4); \
             globalThis.kept=document.createRange(); \
             kept.setStart($('c').lastChild,1);kept.setEnd($('c').lastChild,2); \
             const range=document.createRange(); \
             range.setStart($('a').firstChild,1);range.setEnd($('i').firstChild,2); \
             globalThis.fragment=range.extractContents();",
        )
        .expect("script");
    engine.run_event_loop(16);
    assert_eq!(
        engine
            .evaluate(
                "[seen.join(' '),moved.startContainer.id,moved.startOffset,moved.collapsed, \
                  kept.startContainer.data,kept.startOffset,kept.endOffset].join('|')"
            )
            .expect("result"),
        // The start side from the bottom up (its text, then the siblings
        // that followed it, one record each), then the end side from the top
        // down (the sibling before `<i>`, then the text inside it).
        "data:o list:P-1+0 list:P-1+0 list:P-1+0 data:ve|a|1|true|six|1|2"
    );
}
