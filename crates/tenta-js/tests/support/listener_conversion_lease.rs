// Shared real-engine controls for listener callbacks acquired before options
// conversion. Both generated EventTarget methods and the Window path are used.

use tenta_js::engine::{PageEngine, ScriptKind};

#[test]
fn throwing_options_preserve_the_page_exception_without_installing_a_listener() {
    let mut engine = new_engine();
    engine
        .run_script(
            "globalThis.leaseMarker={};globalThis.leaseHits=0;\
             globalThis.leaseTarget=new EventTarget();\
             globalThis.leaseWeak=(()=>{const callback=()=>leaseHits++;\
               const weak=new WeakRef(callback);\
               for(const target of [leaseTarget,document,window]){\
                 let caught;try{target.addEventListener('lease',callback,\
                   {get capture(){throw leaseMarker}})}catch(error){caught=error;}\
                 if(caught!==leaseMarker)throw Error('original exception changed');\
                 target.dispatchEvent(new Event('lease'));\
               }\
               return weak;})();\
             if(leaseHits!==0)throw Error('failed add registered a listener');",
            ScriptKind::Classic,
        )
        .expect("failed options conversion preserves the page error");
    engine.run_microtasks();
    for _ in 0..3 {
        engine.trim_memory();
    }
    engine
        .run_script(
            "if(leaseWeak.deref()!==undefined)throw Error('failed add retained callback');",
            ScriptKind::Classic,
        )
        .expect("failed additions leave no native callback root");
}

#[test]
fn nested_options_getter_transfers_only_the_inner_registration() {
    let mut engine = new_engine();
    engine
        .run_script(
            "globalThis.leaseMarker={};globalThis.leaseHits=0;\
             globalThis.leaseTarget=new EventTarget();\
             globalThis.leaseCallback=()=>leaseHits++;\
             let caught;try{leaseTarget.addEventListener('lease',leaseCallback,\
               {get capture(){window.addEventListener('lease',leaseCallback);\
                 throw leaseMarker}})}catch(error){caught=error;}\
             if(caught!==leaseMarker)throw Error('nested error changed');\
             leaseTarget.dispatchEvent(new Event('lease'));\
             if(leaseHits!==0)throw Error('outer registration survived');\
             window.dispatchEvent(new Event('lease'));\
             if(leaseHits!==1)throw Error('inner registration was lost');\
             window.removeEventListener('lease',leaseCallback);\
             window.dispatchEvent(new Event('lease'));\
             if(leaseHits!==1)throw Error('inner removal failed');",
            ScriptKind::Classic,
        )
        .expect("inner callback ownership survives outer conversion failure");
}

#[test]
fn throwing_remove_options_keep_the_original_listener() {
    let mut engine = new_engine();
    engine
        .run_script(
            "globalThis.leaseMarker={};globalThis.leaseHits=0;\
             const callback=()=>leaseHits++;\
             for(const target of [new EventTarget(),document,window]){\
               target.addEventListener('lease',callback);\
               let caught;try{target.removeEventListener('lease',callback,\
                 {get capture(){throw leaseMarker}})}catch(error){caught=error;}\
               if(caught!==leaseMarker)throw Error('remove error changed');\
               target.dispatchEvent(new Event('lease'));\
               target.removeEventListener('lease',callback);\
               target.dispatchEvent(new Event('lease'));\
             }\
             if(leaseHits!==3)throw Error('registration ownership changed');",
            ScriptKind::Classic,
        )
        .expect("failed removal retains each existing listener");
}
