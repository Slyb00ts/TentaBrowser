//! Script elements that script inserts (`tenta_js::scripts`): which of them
//! run, when, and what the engine leaves for the page to load.
//!
//! An inline classic script runs inside the DOM call that made it ready, so
//! everything about it is visible to one `evaluate`. An external or module
//! script is only handed over here (`Engine::take_inserted_scripts`); loading
//! and running it is the page's (`crates/tenta-browser/tests/inserted_scripts.rs`).

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js::scripts::{InsertedSource, ScriptType};
use tenta_js_v8::Engine;

/// `document -> html -> body` at `https://example.com/dir/page.html`.
fn engine() -> Engine {
    let mut document = Document::with_url("https://example.com/dir/page.html".parse().unwrap());
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
    Engine::with_document(Rc::new(RefCell::new(document)))
}

const PROLOGUE: &str = "globalThis.log=[]; \
    const make=(text,attrs)=>{const s=document.createElement('script'); \
      for(const k in (attrs||{}))s.setAttribute(k,attrs[k]); \
      if(text!==undefined)s.textContent=text;return s};";

fn eval(source: &str) -> String {
    let mut engine = engine();
    engine
        .evaluate(&format!("{PROLOGUE}{source}"))
        .expect("script should evaluate without throwing")
}

#[test]
fn an_inline_script_runs_inside_the_call_that_inserts_it() {
    assert_eq!(
        eval(
            "const s=make('log.push(\"script:\"+(document.currentScript===self_))'); \
             globalThis.self_=s; \
             log.push('before:'+document.currentScript); \
             const returned=document.body.appendChild(s); \
             log.push('after:'+document.currentScript,returned===s); \
             log.join(' ')"
        ),
        "before:null script:true after:null true"
    );
}

#[test]
fn a_script_runs_once_and_only_when_it_is_in_the_document() {
    assert_eq!(
        eval(
            "const s=make('log.push(\"ran\")'); \
             const holder=document.createElement('div'); \
             holder.appendChild(s);log.push('detached'); \
             document.body.appendChild(holder);log.push('connected'); \
             document.body.appendChild(s);holder.appendChild(s); \
             s.textContent='log.push(\"again\")';document.body.appendChild(s); \
             const copy=s.cloneNode(true);document.body.appendChild(copy); \
             log.join(' ')"
        ),
        "detached ran connected"
    );
}

#[test]
fn what_is_not_a_script_to_run_does_not_run() {
    assert_eq!(
        eval(
            "for(const type of ['text/template','application/json','importmap','text/javascript; charset=utf-8']){ \
               document.body.appendChild(make('log.push(\"wrong\")',{type}))} \
             document.body.appendChild(make('log.push(\"nomodule\")',{nomodule:''})); \
             document.body.appendChild(make('log.push(\"language\")',{language:'vbscript'})); \
             document.body.innerHTML+='<script>log.push(\"innerHTML\")<\\/script>'; \
             document.body.insertAdjacentHTML('beforeend','<script>log.push(\"adjacent\")<\\/script>'); \
             for(const type of ['','text/javascript','APPLICATION/ECMASCRIPT']){ \
               document.body.appendChild(make('log.push(\"ok\")',{type}))} \
             document.body.appendChild(make('log.push(\"js\")',{language:'JavaScript'})); \
             log.join(' ')"
        ),
        "ok ok ok js"
    );
}

#[test]
fn an_empty_connected_script_runs_when_it_is_given_text() {
    assert_eq!(
        eval(
            "const first=make();document.body.appendChild(first);log.push('empty'); \
             first.appendChild(document.createTextNode('log.push(\"filled\")')); \
             first.appendChild(document.createTextNode('log.push(\"never\")')); \
             const second=make();document.body.appendChild(second); \
             second.text='log.push(\"text:\"+(document.currentScript===second))'; \
             const third=make();document.body.appendChild(third); \
             third.textContent='log.push(\"textContent\")'; \
             [log.join(' '),second.text,first.text].join('|')"
        ),
        "empty filled text:true textContent|\
         log.push(\"text:\"+(document.currentScript===second))|\
         log.push(\"filled\")log.push(\"never\")"
    );
}

#[test]
fn scripts_nest_and_a_throwing_script_does_not_fail_the_insertion() {
    assert_eq!(
        eval(
            "const outer=make('log.push(\"outer-start\"); \
               const inner=document.createElement(\"script\"); \
               inner.textContent=\"log.push(\\\\\"inner:\\\\\"+(document.currentScript===this_inner))\"; \
               globalThis.this_inner=inner;document.body.appendChild(inner); \
               log.push(\"outer-end:\"+(document.currentScript===this_outer))'); \
             globalThis.this_outer=outer; \
             document.body.appendChild(outer); \
             let thrown='none'; \
             try{document.body.appendChild(make('log.push(\"throws\");throw new Error(\"boom\")'))} \
             catch(e){thrown=e.message} \
             try{document.body.appendChild(make('this is not javascript'))}catch(e){thrown=e.name} \
             log.push('after:'+thrown,String(document.currentScript)); \
             log.join(' ')"
        ),
        "outer-start inner:true outer-end:true throws after:none null"
    );
}

#[test]
fn the_scripts_of_a_contextual_fragment_run_when_it_is_inserted() {
    assert_eq!(
        eval(
            "const range=document.createRange();range.selectNodeContents(document.body); \
             const fragment=range.createContextualFragment( \
               '<p>x</p><script>log.push(\"one\")<\\/script><b><script>log.push(\"two\")<\\/script></b>'); \
             log.push('made');document.body.appendChild(fragment);log.push('inserted'); \
             log.join(' ')"
        ),
        "made one two inserted"
    );
}

/// An inserted script is entered at the depth of the call that inserted it,
/// so the wrapper collector still gets its checkpoints while the script runs:
/// a script that touches far more nodes than one checkpoint interval allows
/// is not stopped by the allocation limit.
#[test]
fn a_large_inserted_script_is_not_stopped_by_the_wrapper_limit() {
    assert_eq!(
        eval(
            "const s=make('let n=0;for(let i=0;i<20000;i++){ \
               const e=document.createElement(\"i\");document.body.appendChild(e); \
               n+=e.parentNode===document.body?1:0;e.remove()}globalThis.made=n'); \
             let thrown='none';try{document.body.appendChild(s)}catch(e){thrown=e.name} \
             [made,thrown].join('|')"
        ),
        "20000|none"
    );
}

#[test]
fn async_reads_as_true_until_script_assigns_it() {
    assert_eq!(
        eval(
            "document.body.innerHTML='<script id=parsed type=text/plain></script>'; \
             const created=document.createElement('script'),parsed=document.getElementById('parsed'); \
             const before=[created.async,created.hasAttribute('async'),parsed.async]; \
             created.async=false; \
             const cleared=[created.async,created.hasAttribute('async')]; \
             created.async=true; \
             [before.join(','),cleared.join(','),created.async,created.hasAttribute('async'), \
              document.createElement('script').cloneNode().async].join('|')"
        ),
        "true,false,false|false,false|true|true|true"
    );
}

#[test]
fn external_and_module_scripts_are_handed_to_the_page() {
    let mut engine = engine();
    engine
        .evaluate(&format!(
            "{PROLOGUE} \
             const add=(attrs,setup)=>{{const s=make(undefined,attrs);if(setup)setup(s); \
               document.body.appendChild(s);return s}}; \
             add({{src:'a.js'}}); \
             add({{src:'/b.js'}},s=>{{s.async=false}}); \
             add({{src:'c.js',async:''}},s=>{{s.async=false;s.async=true}}); \
             add({{src:'https://cdn.example/m.mjs',type:'module'}}); \
             add({{src:''}}); \
             add({{src:'d.js',type:'text/template'}}); \
             add({{src:'e.js',nomodule:''}}); \
             const inline=make('export const x = 1;',{{type:'module'}}); \
             document.body.appendChild(inline); \
             const late=make();document.body.appendChild(late);late.src='late.js'; \
             late.src='changed.js'; \
             document.body.appendChild(make('log.push(\"inline ran\")')); \
             log.join(' ')"
        ))
        .expect("script");
    let requests = engine.take_inserted_scripts();
    let summary: Vec<String> = requests
        .iter()
        .map(|request| {
            let source = match &request.source {
                InsertedSource::External(url) => url.to_string(),
                InsertedSource::Inline(text) => format!("inline({text})"),
                InsertedSource::Unusable => "unusable".to_owned(),
            };
            let kind = match request.kind {
                ScriptType::Classic => "classic",
                ScriptType::Module => "module",
            };
            format!("{kind} {source} ordered={}", request.ordered)
        })
        .collect();
    assert_eq!(
        summary,
        [
            "classic https://example.com/dir/a.js ordered=false",
            "classic https://example.com/b.js ordered=true",
            "classic https://example.com/dir/c.js ordered=false",
            "module https://cdn.example/m.mjs ordered=false",
            "classic unusable ordered=false",
            "module inline(export const x = 1;) ordered=false",
            "classic https://example.com/dir/late.js ordered=false",
        ]
    );
    // Taken once.
    assert!(engine.take_inserted_scripts().is_empty());
    assert_eq!(engine.evaluate("log.join(' ')").unwrap(), "inline ran");
}
