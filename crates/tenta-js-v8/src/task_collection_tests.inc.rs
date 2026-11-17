// Include inside lib.rs::tests: when the wrapper graph is collected at the
// end of a task (`Engine::collect_after_task`).

fn wrapper_debt(engine: &Engine) -> usize {
    engine
        .dom
        .as_ref()
        .expect("an engine with a document")
        .state
        .borrow()
        .wrapper_debt()
}

fn collections(engine: &Engine) -> u64 {
    engine
        .gc_counter
        .epoch
        .load(std::sync::atomic::Ordering::Relaxed)
}

#[test]
fn timer_tasks_share_collections_of_the_wrapper_graph() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    engine
        .evaluate("globalThis.ran=0;for(let i=0;i<200;i++)setTimeout(()=>{ran++},0);'set'")
        .unwrap();
    let before = collections(&engine);
    assert_eq!(engine.run_event_loop(1000), 200);
    let during = collections(&engine) - before;
    assert!(
        during < 100,
        "200 timer tasks ran {during} garbage collections; each task paid for its own"
    );
    assert_eq!(engine.evaluate("ran").unwrap(), "200");
}

#[test]
fn a_task_that_reaches_the_wrapper_bound_collects_at_its_end() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    // The script's own task end collects, so the timer starts without debt.
    engine
        .evaluate(
            "setTimeout(()=>{for(let i=0;i<12;i++)document.createElement('div')},0);\
             setTimeout(()=>{\
               const n=globalThis.tenta_soft_bound-12;\
               for(let i=0;i<n;i++)document.createElement('div')},1);'set'"
                .replace(
                    "globalThis.tenta_soft_bound",
                    &crate::world::SOFT_WRAPPER_MATERIALIZATIONS.to_string(),
                )
                .as_str(),
        )
        .unwrap();
    assert_eq!(wrapper_debt(&engine), 0);

    // Twelve wrappers are far from the bound and the last collection is
    // recent: the task ends without one and the debt carries over.
    assert_eq!(engine.run_event_loop(1), 1);
    assert_eq!(wrapper_debt(&engine), 12);

    // The second task brings the debt to the bound exactly, with no binding
    // call left to collect inside the task.
    assert_eq!(engine.run_event_loop(1), 1);
    assert_eq!(wrapper_debt(&engine), 0);
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn a_task_end_collects_once_the_last_collection_is_old() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    engine
        .evaluate("setTimeout(()=>{document.createElement('div')},0);'set'")
        .unwrap();
    assert_eq!(engine.run_event_loop(1), 1);
    assert_eq!(wrapper_debt(&engine), 1);
    // Another task end soon after changes nothing.
    engine.run_microtasks();
    assert_eq!(wrapper_debt(&engine), 1);

    let long_ago = std::time::Instant::now()
        .checked_sub(TASK_COLLECTION_INTERVAL)
        .expect("the machine has been up for longer than the interval");
    engine.host.last_graph_collection.set(Some(long_ago));
    engine.run_microtasks();
    assert_eq!(wrapper_debt(&engine), 0);
}

#[test]
fn trimming_memory_collects_without_waiting_for_a_task() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    let records_before = engine.host_record_count();
    engine
        .evaluate("setTimeout(()=>{for(let i=0;i<40;i++)new EventTarget()},0);'set'")
        .unwrap();
    assert_eq!(engine.run_event_loop(1), 1);
    assert_eq!(wrapper_debt(&engine), 40);
    assert_eq!(engine.host_record_count(), records_before + 40);

    tenta_js::engine::PageEngine::trim_memory(&mut engine);
    assert_eq!(wrapper_debt(&engine), 0);
    assert_eq!(engine.host_record_count(), records_before);
}
