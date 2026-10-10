//! `MutationObserver` and `MutationRecord` through the generated V8 glue
//! (DOM Standard §4.3). The native registrations and queues are covered in
//! `tenta-dom`; these tests cover what a page can observe.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;
use tenta_js_v8::Engine;

/// `<html><body><div id=target>text</div></body></html>`.
fn test_document() -> Rc<RefCell<Document>> {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let div = document.create_html_element(atoms::DIV);
    let text = document.create_text("text");
    let root = document.document_node();
    for (parent, child) in [(root, html), (html, body), (body, div), (div, text)] {
        document
            .mutate(Mutation::Append { parent, child })
            .expect("build the fixture tree");
    }
    document
        .mutate(Mutation::SetAttribute {
            element: div,
            name: QualName::new(atoms::EMPTY, atoms::ID),
            value: "target".to_owned(),
        })
        .expect("set the id");
    Rc::new(RefCell::new(document))
}

/// Runs `setup`, drains the microtask queue, and returns the page's `log`.
fn run(setup: &str) -> String {
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(&format!("globalThis.log = []; {setup}"))
        .expect("the setup script evaluates without throwing");
    engine.run_microtasks();
    engine
        .evaluate("log.join('|')")
        .expect("the log is readable")
}

#[test]
fn a_child_list_mutation_is_delivered_in_a_microtask_with_both_node_lists() {
    assert_eq!(
        run(r#"
            const target = document.getElementById('target');
            const mo = new MutationObserver((records, observer) => {
                log.push('callback:' + records.length + ':' + (observer === mo));
                const r = records[0];
                log.push(r.type, r.target === target, r.addedNodes.length, r.removedNodes.length,
                         r.previousSibling === target.firstChild, r.nextSibling === null, r.attributeName === null,
                         r.oldValue === null, r.attributeNamespace === null);
            });
            mo.observe(target, {childList: true});
            const child = document.createElement('span');
            target.appendChild(child);
            log.push('sync');
        "#),
        "sync|callback:1:true|childList|true|1|0|true|true|true|true|true"
    );
}

#[test]
fn the_notification_is_ordered_with_promise_reactions_queued_after_the_mutation() {
    assert_eq!(
        run(r#"
            const target = document.getElementById('target');
            new MutationObserver(() => log.push('mo')).observe(target, {attributes: true});
            target.setAttribute('a', '1');
            Promise.resolve().then(() => log.push('promise'));
        "#),
        "mo|promise"
    );
}

#[test]
fn attribute_records_carry_name_namespace_and_old_value() {
    assert_eq!(
        run(r#"
            const target = document.getElementById('target');
            const mo = new MutationObserver((records) => {
                for (const r of records) log.push(r.type + ':' + r.attributeName + ':' + r.attributeNamespace + ':' + r.oldValue);
            });
            mo.observe(target, {attributeOldValue: true});
            target.setAttribute('a', '1');
            target.setAttribute('a', '2');
            target.setAttributeNS('urn:x', 'p:b', 'v');
            target.removeAttribute('a');
        "#),
        "attributes:a:null:null|attributes:a:null:1|attributes:b:urn:x:null|attributes:a:null:2"
    );
}

#[test]
fn character_data_and_subtree_observation() {
    assert_eq!(
        run(r#"
            const target = document.getElementById('target');
            const text = target.firstChild;
            const mo = new MutationObserver((records) => {
                for (const r of records) log.push(r.type + ':' + (r.target === text) + ':' + r.oldValue);
            });
            mo.observe(target, {characterDataOldValue: true, subtree: true});
            text.data = 'changed';
            text.appendData('!');
        "#),
        "characterData:true:text|characterData:true:changed"
    );
}

#[test]
fn take_records_empties_the_queue_but_still_delivers_the_microtask_without_records() {
    assert_eq!(
        run(r#"
            const target = document.getElementById('target');
            const mo = new MutationObserver(() => log.push('called'));
            mo.observe(target, {childList: true});
            target.appendChild(document.createElement('b'));
            const taken = mo.takeRecords();
            log.push('taken:' + taken.length + ':' + mo.takeRecords().length);
        "#),
        "taken:1:0"
    );
}

#[test]
fn disconnect_discards_queued_records() {
    assert_eq!(
        run(r#"
            const target = document.getElementById('target');
            const mo = new MutationObserver(() => log.push('called'));
            mo.observe(target, {childList: true});
            target.appendChild(document.createElement('b'));
            mo.disconnect();
            target.appendChild(document.createElement('i'));
            log.push('done');
        "#),
        "done"
    );
}

#[test]
fn an_observer_with_no_page_reference_still_notifies() {
    assert_eq!(
        run(r#"
            (function () {
                const target = document.getElementById('target');
                new MutationObserver(() => log.push('notified')).observe(target, {childList: true});
            })();
            document.getElementById('target').appendChild(document.createElement('b'));
        "#),
        "notified"
    );
}

#[test]
fn removing_a_subtree_keeps_a_transient_registration_for_the_removed_nodes() {
    assert_eq!(
        run(r#"
            const target = document.getElementById('target');
            const inner = target.appendChild(document.createElement('p'));
            const leaf = inner.appendChild(document.createElement('i'));
            const mo = new MutationObserver((records) => {
                for (const r of records) log.push(r.type + ':' + r.target.localName + ':' + r.removedNodes.length + ':' + r.addedNodes.length);
            });
            mo.observe(target, {childList: true, subtree: true});
            target.removeChild(inner);
            leaf.appendChild(document.createElement('u'));
        "#),
        "childList:div:1:0|childList:i:0:1"
    );
}

#[test]
fn options_are_validated_with_type_errors() {
    assert_eq!(
        run(r#"
            const target = document.getElementById('target');
            const mo = new MutationObserver(() => {});
            const attempt = (options) => {
                try { mo.observe(target, options); return 'ok'; }
                catch (e) { return e.constructor.name; }
            };
            log.push(attempt({}), attempt({attributes: false, attributeOldValue: true}),
                     attempt({characterData: false, characterDataOldValue: true}),
                     attempt({attributeFilter: ['a'], attributes: false}),
                     attempt({childList: true}));
            try { new MutationObserver(5); } catch (e) { log.push(e.constructor.name); }
            try { mo.observe({}, {childList: true}); } catch (e) { log.push(e.constructor.name); }
        "#),
        "TypeError|TypeError|TypeError|TypeError|ok|TypeError|TypeError"
    );
}

#[test]
fn text_content_and_inner_html_queue_one_record_with_the_whole_replacement() {
    assert_eq!(
        run(r#"
            const target = document.getElementById('target');
            const mo = new MutationObserver((records) => {
                for (const r of records) log.push(r.type + ':' + r.addedNodes.length + ':' + r.removedNodes.length);
            });
            mo.observe(target, {childList: true});
            target.textContent = 'fresh';
            target.innerHTML = '<a></a><b></b>';
        "#),
        "childList:1:1|childList:2:1"
    );
}

#[test]
fn a_mutation_made_by_a_callback_is_delivered_in_a_later_notification() {
    assert_eq!(
        run(r#"
            const target = document.getElementById('target');
            let first = true;
            const mo = new MutationObserver((records) => {
                log.push('records:' + records.length);
                if (first) { first = false; target.appendChild(document.createElement('s')); }
            });
            mo.observe(target, {childList: true});
            target.appendChild(document.createElement('b'));
        "#),
        "records:1|records:1"
    );
}

/// Collects what the page dropped and returns `(host records, stored values)`.
fn settled(engine: &mut Engine) -> (usize, usize) {
    engine.run_microtasks();
    engine.low_memory_gc();
    engine.reclaim_dead_hosts();
    (engine.host_record_count(), engine.stored_value_count())
}

#[test]
fn observers_and_records_the_page_drops_are_reclaimed() {
    let mut engine = Engine::with_document(test_document());
    let (records_before, values_before) = settled(&mut engine);
    engine
        .evaluate(
            r#"
            globalThis.target = document.getElementById('target');
            (function () {
                // Never observing: collectable at once.
                for (let i = 0; i < 20; i++) new MutationObserver(() => {});
                // Observing, then disconnected: collectable once released.
                const mo = new MutationObserver(() => {});
                mo.observe(target, {childList: true});
                target.appendChild(document.createElement('b'));
                mo.takeRecords();
                mo.disconnect();
                // A delivered record the callback keeps no reference to.
                new MutationObserver(() => {}).observe(target, {attributes: true});
                target.setAttribute('x', '1');
            })();
        "#,
        )
        .expect("create and drop observers");
    // The last observer still observes `target`, which the page holds, so it
    // and its record stay; everything else goes.
    let (records, values) = settled(&mut engine);
    assert_eq!(
        (records - records_before, values - values_before),
        (1, 2),
        "only the observing observer (and its callback) survive"
    );
    engine
        .evaluate("document.getElementById('target').remove(); globalThis.target = null;")
        .unwrap();
}

#[test]
fn an_observer_stays_reachable_for_as_long_as_it_observes() {
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(
            r#"
            globalThis.log = [];
            (function () {
                new MutationObserver((r) => log.push('seen:' + r.length))
                    .observe(document.getElementById('target'), {childList: true});
            })();
        "#,
        )
        .unwrap();
    settled(&mut engine);
    engine
        .evaluate("document.getElementById('target').appendChild(document.createElement('b'))")
        .unwrap();
    engine.run_microtasks();
    assert_eq!(engine.evaluate("log.join('|')").unwrap(), "seen:1");
}

// The scenarios below were checked against Chrome by the JS1 review; the
// expected logs are Chrome's.

fn scenario(body: &str) -> String {
    run(&format!(
        "const t = document.getElementById('target'); \
         const d = document.createElement('p'); document.body.appendChild(d); {body}"
    ))
}

#[test]
fn observers_are_notified_in_creation_order_not_queue_order() {
    assert_eq!(
        scenario(
            "const a = new MutationObserver(r => log.push('a' + r.length)); \
             const b = new MutationObserver(r => log.push('b' + r.length)); \
             a.observe(t, {attributes: true}); b.observe(d, {attributes: true}); \
             d.setAttribute('x', 1); t.setAttribute('y', 1);"
        ),
        "a1|b1"
    );
}

#[test]
fn an_observer_takes_its_records_just_before_its_own_callback() {
    assert_eq!(
        scenario(
            "const a = new MutationObserver(r => { log.push('a' + r.length); d.setAttribute('z', 1); }); \
             const b = new MutationObserver(r => log.push('b' + r.length)); \
             a.observe(t, {attributes: true}); b.observe(d, {attributes: true}); \
             t.setAttribute('q', 1); d.setAttribute('w', 1);"
        ),
        "a1|b2"
    );
}

#[test]
fn the_notification_microtask_keeps_its_place_among_promise_jobs() {
    assert_eq!(
        scenario(
            "const a = new MutationObserver(r => log.push('mo' + r.length)); \
             a.observe(t, {attributes: true}); \
             Promise.resolve().then(() => { log.push('p1'); t.setAttribute('m', 1); \
                 Promise.resolve().then(() => log.push('p1b')); }); \
             Promise.resolve().then(() => log.push('p2')); \
             t.setAttribute('n', 1); \
             Promise.resolve().then(() => log.push('p3'));"
        ),
        "p1|p2|mo2|p3|p1b"
    );
    assert_eq!(
        scenario(
            "const a = new MutationObserver(r => log.push('mo' + r.length)); \
             a.observe(t, {attributes: true}); \
             queueMicrotask(() => { log.push('q1'); t.setAttribute('m', 1); }); \
             queueMicrotask(() => log.push('q2'));"
        ),
        "q1|q2|mo1"
    );
}

#[test]
fn a_callback_that_mutates_queues_the_next_notification_before_its_own_promises() {
    assert_eq!(
        scenario(
            "let n = 0; \
             const a = new MutationObserver(r => { log.push('a' + r.length); \
                 if (n++ < 2) { t.setAttribute('k' + n, 1); Promise.resolve().then(() => log.push('pr')); } }); \
             a.observe(t, {attributes: true}); t.setAttribute('s', 1);"
        ),
        "a1|a1|pr|a1|pr"
    );
}

#[test]
fn a_thousand_records_arrive_in_one_call_before_later_promise_jobs() {
    assert_eq!(
        scenario(
            "const a = new MutationObserver(r => log.push('cb' + r.length)); \
             a.observe(t, {attributes: true}); \
             for (let i = 0; i < 1000; i++) t.setAttribute('a', i); \
             Promise.resolve().then(() => log.push('promise'));"
        ),
        "cb1000|promise"
    );
}

#[test]
fn ten_thousand_records_arrive_in_one_call_in_order() {
    assert_eq!(
        scenario(
            "const a = new MutationObserver(r => { \
                 let ordered = true; \
                 for (let i = 0; i < r.length; i++) if (r[i].oldValue !== (i === 0 ? null : String(i - 1))) ordered = false; \
                 log.push('cb' + r.length + ':' + ordered); }); \
             a.observe(t, {attributes: true, attributeOldValue: true}); \
             for (let i = 0; i < 10000; i++) t.setAttribute('a', i); \
             Promise.resolve().then(() => log.push('promise'));"
        ),
        "cb10000:true|promise"
    );
}

#[test]
fn take_records_returns_every_queued_record() {
    assert_eq!(
        scenario(
            "const a = new MutationObserver(() => log.push('cb')); \
             a.observe(t, {attributes: true}); \
             for (let i = 0; i < 10000; i++) t.setAttribute('a', i); \
             log.push('take' + a.takeRecords().length + ':' + a.takeRecords().length);"
        ),
        "take10000:0"
    );
}

#[test]
fn the_number_of_observing_observers_is_bounded_and_the_page_is_told() {
    let mut engine = Engine::with_document(test_document());
    let outcome = engine
        .evaluate(
            "const t = document.getElementById('target'); let made = 0, error = ''; \
             try { for (let i = 0; i < 10001; i++) { \
                 new MutationObserver(() => {}).observe(t, {attributes: true}); made++; } } \
             catch (e) { error = e.name; } \
             made + ':' + error",
        )
        .expect("observe many");
    assert_eq!(outcome, "10000:QuotaExceededError");
    // The observers hold themselves alive: records and values stay bounded by
    // the cap, not by what the page tries.
    assert!(engine.host_record_count() <= 10_001 * 2);
    engine
        .evaluate("document.getElementById('target').remove()")
        .unwrap();
}
