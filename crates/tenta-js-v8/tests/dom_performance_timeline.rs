//! The performance timeline on the V8 host (`tenta_js::performance_timeline`):
//! entries, `PerformanceObserver`, user timing and the navigation entry.
//!
//! A page's measuring code starts with `new PerformanceObserver(...)` and
//! `performance.getEntriesByType('navigation')[0]`, usually unguarded, so a
//! missing name stops the script that holds it.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js::engine::LifecycleEvent;
use tenta_js_v8::Engine;

fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some(
        "https://example.com/app/list?page=1"
            .parse()
            .expect("test URL"),
    ));
    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    engine.install_preludes();
    engine
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn the_interfaces_are_there_and_only_a_mark_can_be_constructed() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "const attempt=f=>{try{return String(f())}catch(e){return e.name}}; \
             const mark=new PerformanceMark('made',{detail:{a:1},startTime:5}); \
             [['PerformanceEntry','PerformanceMark','PerformanceMeasure', \
               'PerformanceResourceTiming','PerformanceNavigationTiming','PerformanceObserver', \
               'PerformanceObserverEntryList'].map(name=>typeof globalThis[name]).join(','), \
              Object.getPrototypeOf(PerformanceMark)===PerformanceEntry, \
              Object.getPrototypeOf(PerformanceNavigationTiming)===PerformanceResourceTiming, \
              attempt(()=>new PerformanceEntry()),attempt(()=>new PerformanceMeasure()), \
              attempt(()=>new PerformanceNavigationTiming()),attempt(()=>new PerformanceMark()), \
              mark instanceof PerformanceEntry,mark.name,mark.entryType,mark.startTime, \
              mark.duration,mark.detail.a,Object.prototype.toString.call(mark), \
              JSON.stringify(mark),performance.getEntriesByName('made').length, \
              attempt(()=>Object.getOwnPropertyDescriptor(PerformanceEntry.prototype,'name') \
                .get.call({})), \
              attempt(()=>new PerformanceMark('loadEventEnd')), \
              attempt(()=>new PerformanceMark('x',{startTime:-1})), \
              PerformanceObserver.supportedEntryTypes.join(','), \
              Object.isFrozen(PerformanceObserver.supportedEntryTypes)].join('|')"
        ),
        "function,function,function,function,function,function,function|true|true\
         |TypeError|TypeError|TypeError|TypeError|true|made|mark|5|0|1|[object PerformanceMark]\
         |{\"name\":\"made\",\"entryType\":\"mark\",\"startTime\":5,\"duration\":0,\"detail\":{\"a\":1}}\
         |0|TypeError|SyntaxError|TypeError|mark,measure,navigation|true"
    );
}

#[test]
fn a_measure_takes_marks_times_or_an_options_object() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "const attempt=f=>{try{return String(f())}catch(e){return e.name}}; \
             performance.mark('a',{startTime:10});performance.mark('b',{startTime:25}); \
             performance.mark('a',{startTime:12}); \
             const between=performance.measure('between','a','b'); \
             const options=performance.measure('options',{start:'a',end:40,detail:'d'}); \
             const lasting=performance.measure('lasting',{start:5,duration:7}); \
             const ending=performance.measure('ending',{end:'b',duration:5}); \
             const whole=performance.measure('whole'); \
             [between instanceof PerformanceMeasure,between.startTime,between.duration, \
              options.startTime,options.duration,options.detail,lasting.startTime, \
              lasting.duration,ending.startTime,ending.duration,whole.startTime, \
              whole.duration>=0,between.detail, \
              attempt(()=>performance.measure('x','missing')), \
              attempt(()=>performance.measure('x',{})), \
              attempt(()=>performance.measure('x',{start:1,end:2,duration:1})), \
              attempt(()=>performance.measure('x',{start:1},'b')), \
              attempt(()=>performance.measure('x',{start:-1})), \
              attempt(()=>performance.measure()), \
              performance.getEntriesByType('measure').map(e=>e.name).join(','), \
              performance.mark.name,performance.mark.length, \
              Object.keys(performance).includes('mark')].join('|')"
        ),
        "true|12|13|12|28|d|5|7|20|5|0|true|\
         |SyntaxError|TypeError|TypeError|TypeError|TypeError|TypeError\
         |whole,lasting,between,options,ending|mark|1|false"
    );
}

#[test]
fn an_observer_is_told_of_new_entries_in_a_later_task() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "globalThis.log=[]; \
             globalThis.observer=new PerformanceObserver(function(list,self,extra){ \
               log.push([this===observer,self===observer,list instanceof PerformanceObserverEntryList, \
                 list.getEntries().map(e=>e.name).join('+'), \
                 list.getEntriesByType('measure').length,list.getEntriesByName('one').length, \
                 extra?extra.droppedEntriesCount:'none'].join(','))}); \
             observer.observe({entryTypes:['mark','measure','paint']}); \
             performance.mark('one');performance.measure('span','one');performance.mark('two'); \
             [log.length,Object.prototype.toString.call(observer)].join('|')"
        ),
        "0|[object PerformanceObserver]"
    );
    engine.run_event_loop(8);
    assert_eq!(
        eval(
            &mut engine,
            "performance.mark('three');const taken=observer.takeRecords(); \
             observer.disconnect();performance.mark('four'); \
             [log.join(' / '),taken.map(e=>e.name).join(',')].join('|')"
        ),
        "true,true,true,one+span+two,1,1,0|three"
    );
    engine.run_event_loop(8);
    assert_eq!(eval(&mut engine, "log.length"), "1");
}

#[test]
fn a_buffered_observer_gets_what_was_recorded_before_it() {
    let mut engine = engine();
    eval(
        &mut engine,
        "const attempt=f=>{try{return String(f())}catch(e){return e.name}}; \
         globalThis.log=[];performance.mark('early'); \
         const observer=new PerformanceObserver(list=>log.push( \
           list.getEntries().map(e=>e.entryType+':'+e.name).join('+'))); \
         observer.observe({type:'mark',buffered:true}); \
         observer.observe({type:'navigation',buffered:true}); \
         observer.observe({type:'largest-contentful-paint',buffered:true}); \
         globalThis.refused=[attempt(()=>observer.observe({entryTypes:['mark']})), \
           attempt(()=>observer.observe()),attempt(()=>observer.observe({})), \
           attempt(()=>observer.observe({entryTypes:['mark'],type:'mark'})), \
           attempt(()=>new PerformanceObserver()),attempt(()=>PerformanceObserver(()=>{})), \
           attempt(()=>{const quiet=new PerformanceObserver(()=>{}); \
             quiet.observe({entryTypes:['paint']});return quiet.takeRecords().length})].join(','); \
         'observed'",
    );
    engine.run_event_loop(8);
    assert_eq!(
        eval(&mut engine, "[log.join(' / '),refused].join('|')"),
        "mark:early+navigation:https://example.com/app/list?page=1\
         |InvalidModificationError,TypeError,TypeError,TypeError,TypeError,TypeError,0"
    );
}

#[test]
fn the_navigation_entry_reports_the_lifecycle_events() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "globalThis.entry=performance.getEntriesByType('navigation')[0]; \
             [performance.getEntriesByType('navigation').length, \
              entry===performance.getEntries()[0],entry instanceof PerformanceNavigationTiming, \
              entry instanceof PerformanceResourceTiming,entry.name,entry.entryType,entry.type, \
              entry.startTime,entry.initiatorType,entry.redirectCount, \
              entry.domContentLoadedEventStart,entry.loadEventEnd,entry.duration, \
              entry.serverTiming.length,performance.getEntriesByType('resource').length, \
              performance.getEntriesByType('paint').length].join('|')"
        ),
        "1|true|true|true|https://example.com/app/list?page=1|navigation|navigate|0|navigation|0\
         |0|0|0|0|0|0"
    );
    eval(
        &mut engine,
        "globalThis.seen=[]; \
         document.addEventListener('DOMContentLoaded',()=>seen.push( \
           entry.domContentLoadedEventStart>0,entry.domContentLoadedEventEnd,entry.loadEventStart)); \
         addEventListener('load',()=>seen.push(entry.loadEventStart>0,entry.loadEventEnd)); \
         setTimeout(()=>{},5);'listening'",
    );
    engine.run_event_loop(8);
    engine.dispatch_lifecycle(LifecycleEvent::DomContentLoaded);
    engine.dispatch_lifecycle(LifecycleEvent::Load);
    assert_eq!(
        eval(
            &mut engine,
            "const json=entry.toJSON(); \
             [seen.join(','),entry.domContentLoadedEventEnd>=entry.domContentLoadedEventStart, \
              entry.domInteractive===entry.domContentLoadedEventStart, \
              entry.loadEventStart>=entry.domContentLoadedEventEnd, \
              entry.loadEventEnd>=entry.loadEventStart,entry.domComplete===entry.loadEventStart, \
              entry.duration===entry.loadEventEnd,json.loadEventEnd===entry.loadEventEnd, \
              json.type,Object.keys(json).length].join('|')"
        ),
        "true,0,0,true,0|true|true|true|true|true|true|true|navigate|36"
    );
}
