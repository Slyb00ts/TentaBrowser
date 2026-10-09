// Shared integration control for native observer-world attachment and teardown.
// MutationObserver JavaScript delivery is outside this control's scope.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js::engine::PageEngine;

#[test]
fn destroying_either_world_preserves_the_other_world_and_native_mutations() {
    for destroy_first in [true, false] {
        let document = Rc::new(RefCell::new(Document::new()));
        let mut first = new_engine(document.clone());
        let mut second = new_engine(document.clone());
        first
            .evaluate(
                "document.appendChild(document.createElement('html')); \
                 document.documentElement.appendChild(document.createElement('body')); \
                 globalThis.kept = document.body.appendChild(document.createElement('div')); \
                 kept.id = 'retained'; kept.textContent = 'initial'; \
                 kept.worldLabel = 'first';",
            )
            .expect("create shared document content in the first world");
        assert_eq!(
            second
                .evaluate(
                    "globalThis.kept = document.getElementById('retained'); \
                     kept.worldLabel = 'second'; kept.textContent",
                )
                .expect("second world sees the same native node"),
            "initial"
        );

        let (mut survivor, expected_label) = if destroy_first {
            drop(first);
            (second, "second")
        } else {
            drop(second);
            (first, "first")
        };
        survivor
            .evaluate("document.body.removeChild(kept)")
            .expect("remaining world can detach its node");
        document.borrow_mut().sweep_detached();
        assert_eq!(
            survivor
                .evaluate(
                    "kept.textContent = 'survivor'; document.body.appendChild(kept); \
                     [kept.worldLabel, document.getElementById('retained') === kept].join('|')",
                )
                .expect("remaining wrapper pins its node and retains its own properties"),
            format!("{expected_label}|true")
        );

        {
            let mut document = document.borrow_mut();
            let body = document.body().expect("HTML body");
            document
                .append_text(body, "native")
                .expect("native mutation after another world's retirement");
        }
        assert_eq!(
            survivor
                .evaluate("document.body.textContent")
                .expect("remaining world sees the native mutation"),
            "survivornative"
        );
        // Generation-stamped handles do not retain native node pins.
        let (element, text) = {
            let document = document.borrow();
            let element = document
                .get_element_by_id("retained")
                .expect("retained element is connected");
            (
                document.node_ref(element),
                document.node_ref(document.first_child(element)),
            )
        };
        survivor
            .evaluate("document.body.removeChild(kept)")
            .expect("detach before releasing the last world");
        drop(survivor);
        document.borrow_mut().sweep_detached();
        assert!(
            document.borrow().resolve(element).is_err(),
            "the last world's detached element must be reclaimed"
        );
        assert!(
            document.borrow().resolve(text).is_err(),
            "the last world's detached text must be reclaimed"
        );
    }
}

#[test]
fn foreign_wrappers_preserve_per_world_state_during_collection() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut first = new_engine(document.clone());
    let mut second = new_engine(document.clone());
    first
        .evaluate(
            "document.appendChild(document.createElement('html')); \
             document.documentElement.appendChild(document.createElement('body')); \
             globalThis.kept = document.body.appendChild(document.createElement('div')); \
             kept.id = 'shared'; kept.note = 'first'; globalThis.hits = 0; \
             kept.addEventListener('probe', () => ++globalThis.hits)",
        )
        .unwrap();
    let node = {
        let document = document.borrow();
        document.node_ref(document.get_element_by_id("shared").unwrap())
    };
    second
        .evaluate(
            "globalThis.kept = document.getElementById('shared'); \
             kept.note = 'second'; document.body.removeChild(kept)",
        )
        .unwrap();
    first.evaluate("globalThis.kept = null").unwrap();
    for _ in 0..3 {
        PageEngine::trim_memory(&mut first);
        assert_eq!(first.native_failure(), None);
        assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 0);
    }
    assert_eq!(
        second
            .evaluate("document.body.appendChild(kept); kept.note")
            .unwrap(),
        "second"
    );
    assert_eq!(
        first
            .evaluate(
                "document.getElementById('shared').dispatchEvent(new Event('probe')); \
                 [document.getElementById('shared').note, hits].join('|')",
            )
            .unwrap(),
        "first|1",
        "the foreign wrapper retains this world's wrapper properties and listeners"
    );
    drop(second);
    first
        .evaluate("document.body.removeChild(document.getElementById('shared')); 'detached'")
        .unwrap();
    for _ in 0..3 {
        PageEngine::trim_memory(&mut first);
        assert_eq!(first.native_failure(), None);
        document.borrow_mut().sweep_detached();
    }
    assert!(
        document.borrow().resolve(node).is_err(),
        "ordinary single-engine collection resumes after the foreign world is gone"
    );
}

#[test]
fn foreign_node_list_retains_its_detached_owner_after_world_teardown() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut first = new_engine(document.clone());
    let mut second = new_engine(document.clone());
    first
        .evaluate(
            "document.appendChild(document.createElement('html')); \
             document.documentElement.appendChild(document.createElement('body')); \
             globalThis.kept = document.body.appendChild(document.createElement('div')); \
             kept.id = 'list-owner'; kept.textContent = 'before'; \
             globalThis.list = kept.childNodes",
        )
        .unwrap();
    let node = {
        let document = document.borrow();
        document.node_ref(document.get_element_by_id("list-owner").unwrap())
    };
    second
        .evaluate(
            "globalThis.list = document.getElementById('list-owner').childNodes; \
             list.note = 'second'; \
             document.body.removeChild(document.getElementById('list-owner'))",
        )
        .unwrap();
    drop(first);
    document.borrow_mut().sweep_detached();
    assert_eq!(
        second
            .evaluate(
                "list.item(0).parentNode.textContent = 'after'; \
                 [list.note, list.length, list.item(0).data, \
                  list.item(0).parentNode.childNodes === list].join('|')",
            )
            .unwrap(),
        "second|1|after|true"
    );
    drop(second);
    document.borrow_mut().sweep_detached();
    assert!(document.borrow().resolve(node).is_err());
}
