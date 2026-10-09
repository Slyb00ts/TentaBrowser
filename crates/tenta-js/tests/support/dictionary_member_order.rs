// Shared real-engine checks for Web IDL dictionary conversion order.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tenta_dom::Document;
use tenta_js::engine::PageEngine;

fn dictionary_engine() -> Engine {
    new_engine(Rc::new(RefCell::new(Document::new())))
}

fn dictionary_eval(engine: &mut Engine, source: &str) -> String {
    PageEngine::evaluate(engine, source, Duration::from_secs(10))
        .expect("dictionary conversion script")
}

#[test]
fn inherited_members_are_read_before_lexicographic_own_members() {
    let mut engine = dictionary_engine();
    assert_eq!(
        dictionary_eval(
            &mut engine,
            r#"const reads=[];
               document.addEventListener('x',null,{
                   get passive(){reads.push('passive');return false},
                   get once(){reads.push('once');return false},
                   get capture(){reads.push('capture');return false}
               });
               reads.join('|')"#
        ),
        "capture|once|passive"
    );
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn response_init_reads_its_own_members_lexicographically() {
    let mut engine = dictionary_engine();
    assert_eq!(
        dictionary_eval(
            &mut engine,
            r#"const reads=[];
               new Response(null,{
                   get status(){reads.push('status');return 200},
                   get statusText(){reads.push('statusText');return ''},
                   get headers(){reads.push('headers');return undefined}
               });
               reads.join('|')"#
        ),
        "headers|status|statusText"
    );
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn an_inherited_dictionary_getter_throw_stops_later_own_getters() {
    let mut engine = dictionary_engine();
    assert_eq!(
        dictionary_eval(
            &mut engine,
            r#"const marker={},reads=[];
               let same=false;
               try {
                   document.addEventListener('x',null,{
                       get passive(){reads.push('passive');return false},
                       get once(){reads.push('once');return false},
                       get capture(){reads.push('capture');throw marker}
                   });
               } catch(error) {same=error===marker}
               [same,reads.join(',')].join('|')"#
        ),
        "true|capture"
    );
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn a_derived_dictionary_getter_throw_stops_later_own_getters() {
    let mut engine = dictionary_engine();
    assert_eq!(
        dictionary_eval(
            &mut engine,
            r#"const marker={},reads=[];
               let same=false;
               try {
                   document.addEventListener('x',null,{
                       get passive(){reads.push('passive');return false},
                       get once(){reads.push('once');throw marker},
                       get capture(){reads.push('capture');return false}
                   });
               } catch(error) {same=error===marker}
               [same,reads.join(',')].join('|')"#
        ),
        "true|capture,once"
    );
    assert_eq!(engine.native_failure(), None);
}
