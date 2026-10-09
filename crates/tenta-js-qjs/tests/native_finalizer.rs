//! A native destructor must never unwind through QuickJS's C finalizer.
use std::cell::Cell;
use std::rc::Rc;
use tenta_js::bindings::QuickJsHost;

#[test]
fn a_panicking_native_finalizer_is_contained() {
    const CHILD: &str = "TENTA_QJS_FINALIZER_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "a_panicking_native_finalizer_is_contained",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .expect("run isolated finalizer probe");
        assert!(
            output.status.success(),
            "finalizer child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    struct PanickingHost;
    impl Drop for PanickingHost {
        fn drop(&mut self) {
            panic!("native finalizer probe");
        }
    }
    for initial_failure in [None, Some("earlier native callback")] {
        let runtime = rquickjs::Runtime::new().unwrap();
        let context = rquickjs::Context::full(&runtime).unwrap();
        let failure = Rc::new(Cell::new(initial_failure));
        let dead = Rc::new(Cell::new(Vec::new()));
        context.with(|ctx| {
            let instance = rquickjs::Class::instance(
                ctx,
                QuickJsHost {
                    host: Some(Rc::new(PanickingHost)),
                    handle: 17,
                    generation: 42,
                    interfaces: &[],
                    native_failure: failure.clone(),
                    dead: dead.clone(),
                },
            )
            .unwrap();
            drop(instance);
        });
        runtime.run_gc();
        assert_eq!(
            failure.get(),
            initial_failure.or(Some("native host finalizer"))
        );
        assert_eq!(dead.take(), vec![(17, 42)]);
    }
}
