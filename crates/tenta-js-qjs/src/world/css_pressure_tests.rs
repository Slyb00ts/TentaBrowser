// Sample native CSS records inside one uninterrupted page task, before the
// Engine entry epilogue could hide a missing pressure checkpoint.
#[test]
fn uninterrupted_css_materialization_bounds_native_host_records() {
    let mut engine = crate::Engine::new().unwrap();
    engine.evaluate("0").unwrap();
    let world = engine.state.world.clone();
    let before = world.host.host_records.borrow().len();
    let epoch_before = world.gc_epoch.get();
    let observed = engine.context.with(|ctx| {
        let _active = crate::host::ActiveContext::new(&world, &ctx);
        ctx.eval::<(), _>(
            "for(let i=0;i<12000;i++){if(i&1){new CSSStyleSheet();}else{new CSSStyleSheet().cssRules;}}void 0;",
        )
        .unwrap();
        world.host.host_records.borrow().len()
    });
    assert_eq!(engine.native_failure(), None);
    assert!(
        observed < before + 4096,
        "uninterrupted CSS construction retained {observed} native host records"
    );
    assert!(
        world.gc_epoch.get() > epoch_before,
        "CSS-only materialization must reach a pressure checkpoint inside the task"
    );
    engine.trim_memory();
    assert_eq!(world.host.host_records.borrow().len(), before);
}
