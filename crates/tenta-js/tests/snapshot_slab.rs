//! A failed static snapshot insertion must not leak native node handles.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js::bindings::host::SnapshotSlab;

#[test]
fn a_stale_later_result_rolls_back_earlier_node_pins() {
    let document = Rc::new(RefCell::new(Document::new()));
    let (live, stale) = {
        let mut document = document.borrow_mut();
        let live = document.create_text("live");
        let stale = document.create_text("stale");
        let live_reference = document.node_ref(live);
        let stale_reference = document.node_ref(stale);
        let external = document.handle(live).expect("retain the first result");
        assert_eq!(document.sweep_detached().nodes_freed, 1);
        document.release(external);
        (live_reference, stale_reference)
    };

    let mut snapshots = SnapshotSlab::new(document.clone());
    assert!(snapshots.insert(vec![live, stale]).is_err());
    assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 1);
}
