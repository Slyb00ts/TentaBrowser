// Include inside lib.rs::tests: inspect the uninterrupted script before
// Engine::run_inner could add its one task-end graph checkpoint.

fn run_pressure_without_task_end(engine: &mut Engine, source: &str) -> String {
    let _entry = engine.entered();
    engine.discard_stray_termination();
    engine.host.enter_script();
    let result = {
        v8::scope!(let handle_scope, &mut engine.isolate);
        let context = v8::Local::new(handle_scope, &engine.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        v8::tc_scope!(let tc, scope);
        compile_and_run(tc, source)
    };
    engine.host.exit_script();
    result.unwrap_or_else(|failure| panic!("wrapper pressure script: {}", failure.message))
}

#[test]
fn uninterrupted_css_adopt_and_rule_wrappers_collect_before_task_end() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    let records_before = engine.host_record_count();
    let epoch_before = engine
        .gc_counter
        .epoch
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        run_pressure_without_task_end(
            &mut engine,
            "for(let i=0;i<1600;i++){\
             const sheet=new CSSStyleSheet();sheet.replaceSync('a{color:red}');\
             const rule=sheet.cssRules[0];rule.style.note=i;} 'done'"
        ),
        "done"
    );
    // No outer Engine checkpoint has run yet. Both observations are from the
    // same uninterrupted JS task and therefore prove in-task pressure GC and
    // bounded native HostRecord retention, including constructor adoption.
    assert!(
        engine
            .gc_counter
            .epoch
            .load(std::sync::atomic::Ordering::Relaxed)
            > epoch_before,
        "CSS wrapper pressure did not collect inside the task"
    );
    assert!(
        engine.host_record_count() < records_before + 4096,
        "native CSS records grew past the wrapper hard bound"
    );
    assert_eq!(engine.native_failure(), None);
    {
        let _entry = engine.entered();
        assert!(engine.graph_checkpoint());
        engine.reclaim_dead_hosts();
    }
    assert_eq!(engine.host_record_count(), records_before);
}

#[test]
fn nested_css_constructors_refuse_before_native_record_publication() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    let records_before = engine.host_record_count();
    let result = engine
        .evaluate(
            "const node=document.createTextNode('x');\
             let created=0,refused=0;\
             try{new StaticRange({startContainer:node,get startOffset(){\
               for(let i=0;i<10000;i++){try{new CSSStyleSheet();created++;}\
                 catch(error){if(error.name!=='RangeError')throw error;refused++;}}\
               return 0;},endContainer:node,endOffset:0});}\
             catch(error){if(error.name!=='RangeError')throw error;}\
             [created,refused].join('|')",
        )
        .expect("nested constructor admission script");
    let (created, refused) = result.split_once('|').expect("two counters");
    let created: usize = created.parse().expect("created count");
    let refused: usize = refused.parse().expect("refused count");
    assert!(created > 0 && created < 4096);
    assert!(refused > 0);
    assert_eq!(created + refused, 10000);
    assert!(
        engine.host_record_count() <= records_before + created + 8,
        "refused constructors leaked native CSS records"
    );
    engine.low_memory_gc();
    engine.reclaim_dead_hosts();
    assert_eq!(engine.host_record_count(), records_before);
    assert_eq!(engine.native_failure(), None);
}
