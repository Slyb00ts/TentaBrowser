// Included by V8 and QuickJS adapters: the static operations of `URL`,
// `AbortSignal` and `Response`, which reach their world through the realm.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;

fn eval(source: &str) -> String {
    let mut document = Document::new();
    document.set_url(Some(
        "https://example.com/dir/page.html".parse().expect("test URL"),
    ));
    let mut engine = new_engine(Rc::new(RefCell::new(document)));
    engine.evaluate(source).expect("static operation script")
}

#[test]
fn url_can_parse_answers_what_the_constructor_would_do() {
    assert_eq!(
        eval(
            "[URL.canParse('https://a.example/x'),URL.canParse('/x'), \
              URL.canParse('/x','https://a.example/'),URL.canParse('x','not a base'), \
              URL.canParse('http://['),typeof URL.canParse, \
              (()=>{try{URL.canParse()}catch(e){return e.name}})()].join('|')"
        ),
        "true|false|true|false|false|function|TypeError"
    );
}

#[test]
fn url_parse_returns_a_url_or_null() {
    assert_eq!(
        eval(
            "const made=URL.parse('../y?q=1#h','https://a.example/dir/x'); \
             const other=URL.parse('https://a.example/y'); \
             [made instanceof URL,made.href,made.searchParams.get('q'),made!==other, \
              URL.parse('/x'),URL.parse('http://['),URL.parse('x','nope'), \
              (()=>{made.pathname='/z';return made.href})()].join('|')"
        ),
        "true|https://a.example/y?q=1#h|1|true|||\
         |https://a.example/z?q=1#h"
    );
}

#[test]
fn url_search_params_report_their_size() {
    assert_eq!(
        eval(
            "const p=new URLSearchParams('a=1&a=2&b=3');const before=p.size; \
             p.append('c','4');p.delete('a'); \
             const url=new URL('https://a.example/?x=1&y=2'); \
             [before,p.size,url.searchParams.size,new URLSearchParams().size].join('|')"
        ),
        "3|2|2|0"
    );
}

#[test]
fn abort_signal_abort_returns_a_signal_that_is_already_aborted() {
    assert_eq!(
        eval(
            "const plain=AbortSignal.abort();const why={};const given=AbortSignal.abort(why); \
             let fired=0;plain.addEventListener('abort',()=>fired++); \
             let thrown;try{given.throwIfAborted()}catch(e){thrown=e===why} \
             [plain instanceof AbortSignal,plain instanceof EventTarget,plain.aborted, \
              plain.reason instanceof DOMException,plain.reason.name,plain.reason===plain.reason, \
              given.aborted,given.reason===why,thrown,plain!==given,fired, \
              AbortSignal.abort(null).reason,AbortSignal.abort(0).reason, \
              AbortSignal.abort(undefined).reason.name].join('|')"
        ),
        "true|true|true|true|AbortError|true|true|true|true|true|0||0|AbortError"
    );
}

#[test]
fn response_error_is_a_network_error() {
    assert_eq!(
        eval(
            "const r=Response.error(); \
             [r instanceof Response,r.type,r.status,r.ok,r.statusText,r.url,r.redirected, \
              r.bodyUsed,[...r.headers].length,r!==Response.error()].join('|')"
        ),
        "true|error|0|false|||false|false|0|true"
    );
}

#[test]
fn response_redirect_names_the_parsed_url_and_takes_only_redirect_statuses() {
    assert_eq!(
        eval(
            "const attempt=f=>{try{return f()}catch(e){return e.name}}; \
             const r=Response.redirect('../next?a=1'); \
             const moved=Response.redirect('https://other.example/x',301); \
             [r.status,r.headers.get('location'),r.type,r.ok,r.bodyUsed, \
              moved.status,moved.headers.get('Location'), \
              [303,307,308].map(s=>Response.redirect('/x',s).status).join(','), \
              attempt(()=>Response.redirect('/x',200)),attempt(()=>Response.redirect('/x',304)), \
              attempt(()=>Response.redirect('http://[')),attempt(()=>Response.redirect())].join('|')"
        ),
        "302|https://example.com/next?a=1|default|false|false\
         |301|https://other.example/x|303,307,308|RangeError|RangeError|TypeError|TypeError"
    );
}

#[test]
fn css_namespace_operations_still_answer() {
    // The namespace's operations take the realm as every static one does.
    assert_eq!(
        eval(
            "[CSS.supports('display','grid'),CSS.supports('(display: nope)'), \
              CSS.escape('a b')].join('|')"
        ),
        "true|false|a\\ b"
    );
}

#[test]
fn text_comment_and_fragment_are_constructible() {
    assert_eq!(
        eval(
            "const text=new Text('a'),empty=new Text(),comment=new Comment('c'), \
               blank=new Comment(),fragment=new DocumentFragment(); \
             class Note extends Text{constructor(){super('sub')}get shout(){return this.data.toUpperCase()}} \
             const note=new Note();const host=document.createElement('div'); \
             fragment.append(text,comment,note);host.appendChild(fragment); \
             [text instanceof Text,text.nodeType,text.data,empty.data,text.ownerDocument===document, \
              comment instanceof Comment,comment.nodeType,comment.data,blank.data, \
              fragment instanceof DocumentFragment,fragment.nodeType,fragment.childNodes.length, \
              note instanceof Note,note instanceof Text,note.shout,host.lastChild===note, \
              host.firstChild===text,host.childNodes.length,host.innerHTML, \
              new Text(7).data,text.parentNode===host].join('|')"
        ),
        "true|3|a||true|true|8|c||true|11|0|true|true|SUB|true|true|3|a<!--c-->sub|7|true"
    );
}
