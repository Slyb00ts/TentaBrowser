//! The converted relatedTarget wrapper remains rooted through later dictionary getters.

use rquickjs::{Ctx, Function};

use crate::Engine;

#[test]
fn related_target_conversion_survives_reentrant_native_gc_before_host_ownership() {
    let mut engine = Engine::new().expect("QuickJS runtime");
    engine.context.with(|ctx| {
        ctx.globals()
            .set(
                "forceNativeGc",
                Function::new(ctx.clone(), |ctx: Ctx<'_>| {
                    ctx.run_gc();
                    Ok::<_, rquickjs::Error>(())
                })
                .expect("test-only native GC function"),
            )
            .expect("install test-only native GC function");
    });
    let actual = engine
        .evaluate(
            r#"globalThis.reads=[];
               globalThis.mouse=new MouseEvent('x',{
                   get relatedTarget(){const node=document.createElement('div');
                       node.note='exact';globalThis.relatedRef=new WeakRef(node);
                       reads.push('related');return node},
                   get screenX(){reads.push('later');forceNativeGc();return 9}
               });
               [reads.join(','),mouse.relatedTarget===relatedRef.deref(),
                mouse.relatedTarget.note,mouse.screenX].join('|')"#,
        )
        .expect("constructor completes after native GC");
    assert_eq!(actual, "related,later|true|exact|9");
    assert_eq!(engine.state.world.native_failure.get(), None);
}
