//! `customElements` and the HTML element constructor on the V8 host.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::Atom;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

fn engine() -> (Engine, Rc<RefCell<Document>>) {
    let mut document = Document::new();
    document.set_url(Some("https://example.com/".parse().expect("test URL")));
    let document = Rc::new(RefCell::new(document));
    let mut engine = Engine::with_document(document.clone());
    engine
        .evaluate(
            "var html = document.createElement('html'); document.appendChild(html); \
             var body = document.createElement('body'); html.appendChild(body); \
             globalThis.log = []; \
             var attempt = f => { try { return String(f()); } catch (e) { return e.name; } }; \
             var take = () => log.splice(0).join(' '); \
             'ready'",
        )
        .expect("the test tree");
    (engine, document)
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

/// A class that logs its constructor and every reaction, under `name`.
fn logging_class(name: &str, observed: &str) -> String {
    format!(
        "class {name} extends HTMLElement {{
           static get observedAttributes() {{ return {observed}; }}
           constructor() {{ super(); log.push('{name}:new'); }}
           connectedCallback() {{ log.push('{name}:connected'); }}
           disconnectedCallback() {{ log.push('{name}:disconnected'); }}
           attributeChangedCallback(n, o, v, ns) {{
             log.push('{name}:' + n + '=' + o + '>' + v + '/' + ns);
           }}
         }}"
    )
}

/// The registry is one object of an interface that cannot be constructed,
/// `define()` checks its arguments in HTML's order, and `get()` / `getName()`
/// read a definition back.
#[test]
fn the_registry_validates_and_reports_definitions() {
    let (mut engine, _) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "class A extends HTMLElement {}
             class B extends HTMLElement {}
             customElements.define('x-a', A);
             [typeof customElements, customElements === customElements,
              customElements instanceof CustomElementRegistry,
              Object.prototype.toString.call(customElements),
              Object.keys(globalThis).includes('customElements'),
              attempt(() => new CustomElementRegistry()),
              customElements.define.length,
              customElements.get('x-a') === A, String(customElements.get('x-b')),
              customElements.getName(A), String(customElements.getName(B)),
              attempt(() => customElements.define('x-a', B)),
              attempt(() => customElements.define('x-b', A)),
              attempt(() => customElements.define('plain', B)),
              attempt(() => customElements.define('X-B', B)),
              attempt(() => customElements.define('font-face', B)),
              attempt(() => customElements.define('x-b', {})),
              attempt(() => customElements.define('x-b', () => 1)),
              attempt(() => customElements.define('x-b')),
              attempt(() => customElements.define('x-b', B, {extends: 'div'})),
              attempt(() => customElements.define.call({}, 'x-b', B)),
              String(customElements.get('x-b')),
              attempt(() => new HTMLElement()), attempt(() => HTMLElement()),
              attempt(() => new HTMLDivElement()),
              attempt(() => new B())].join('|')",
        ),
        "object|true|true|[object CustomElementRegistry]|false|TypeError|2|\
         true|undefined|x-a|null|\
         NotSupportedError|NotSupportedError|SyntaxError|SyntaxError|SyntaxError|\
         TypeError|TypeError|TypeError|NotSupportedError|TypeError|undefined|\
         TypeError|TypeError|TypeError|TypeError"
    );
}

/// A class's reaction members must be functions, and `observedAttributes` a
/// sequence, read only when there is an `attributeChangedCallback`.
#[test]
fn define_reads_the_reactions_off_the_prototype() {
    let (mut engine, _) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "class Bad extends HTMLElement {}
             Bad.prototype.connectedCallback = 1;
             class NoList extends HTMLElement { attributeChangedCallback() {} }
             NoList.observedAttributes = 5;
             class Unread extends HTMLElement {
               static get observedAttributes() { throw new Error('not read'); }
             }
             class FromSet extends HTMLElement {
               static get observedAttributes() { return new Set(['a']); }
               attributeChangedCallback(n) { log.push(n); }
             }
             var out = [attempt(() => customElements.define('x-bad', Bad)),
                        attempt(() => customElements.define('x-nolist', NoList)),
                        attempt(() => customElements.define('x-unread', Unread)),
                        attempt(() => customElements.define('x-set', FromSet)),
                        String(customElements.get('x-bad')), typeof customElements.get('x-unread')];
             var el = document.createElement('x-set');
             el.setAttribute('a', '1'); el.setAttribute('b', '2');
             out.concat(take()).join('|')",
        ),
        "TypeError|TypeError|undefined|undefined|undefined|function|a"
    );
}

/// Defining a name upgrades the elements already in the document, in tree
/// order: the constructor runs with the element as `this`, the attributes it
/// has are reported, then that it is connected.
#[test]
fn elements_in_the_document_are_upgraded_when_their_name_is_defined() {
    let (mut engine, _) = engine();
    let class = logging_class("Card", "['tone', 'size']");
    assert_eq!(
        eval(
            &mut engine,
            &format!(
                "body.innerHTML = '<x-card id=one tone=warm other=1><x-card id=two></x-card></x-card>';
                 var one = document.getElementById('one');
                 one.kept = 'expando';
                 var before = [one instanceof HTMLElement, one.constructor === HTMLElement].join();
                 {class}
                 customElements.define('x-card', Card);
                 [before, take(), one instanceof Card, one.constructor === Card, one.kept,
                  Object.getPrototypeOf(one) === Card.prototype,
                  document.getElementById('two') instanceof Card, one.localName].join('|')"
            ),
        ),
        "true,true|Card:new Card:tone=null>warm/null Card:connected Card:new Card:connected|\
         true|true|expando|true|true|x-card"
    );
}

/// After the definition, every way of making an element of that name gives a
/// custom element before the script that made it sees it; only elements in
/// the document are told they are connected.
#[test]
fn created_cloned_and_parsed_elements_are_upgraded() {
    let (mut engine, _) = engine();
    let class = logging_class("Chip", "['label']");
    assert_eq!(
        eval(
            &mut engine,
            &format!(
                "{class}
                 customElements.define('x-chip', Chip);
                 var out = [];
                 var made = document.createElement('x-chip');
                 out.push(made instanceof Chip, take());
                 var direct = new Chip();
                 out.push(direct.localName, direct instanceof Chip, direct.isConnected, take());
                 var holder = document.createElement('div');
                 holder.innerHTML = '<x-chip label=a></x-chip>';
                 out.push(holder.firstChild instanceof Chip, take());
                 var copy = holder.firstChild.cloneNode(true);
                 out.push(copy instanceof Chip, copy !== holder.firstChild, take());
                 body.innerHTML = '<p><x-chip label=b></x-chip></p>';
                 out.push(take());
                 out.join('|')"
            ),
        ),
        "true|Chip:new|x-chip|true|false|Chip:new|\
         true|Chip:new Chip:label=null>a/null|\
         true|true|Chip:new Chip:label=null>a/null|\
         Chip:new Chip:label=null>b/null Chip:connected"
    );
}

/// Entering and leaving the document is reported once each, to the whole
/// subtree in tree order, and a move is a leave followed by an entry.
#[test]
fn connection_and_disconnection_are_reported() {
    let (mut engine, _) = engine();
    let outer = logging_class("Outer", "[]");
    let inner = logging_class("Inner", "[]");
    assert_eq!(
        eval(
            &mut engine,
            &format!(
                "{outer} {inner}
                 customElements.define('x-outer', Outer);
                 customElements.define('x-inner', Inner);
                 var out = [];
                 var tree = document.createElement('x-outer');
                 tree.appendChild(document.createElement('section'))
                     .appendChild(document.createElement('x-inner'));
                 take();
                 var other = document.createElement('div');
                 other.appendChild(tree);
                 out.push('detached:' + take());
                 body.appendChild(other);
                 out.push(take());
                 var second = body.appendChild(document.createElement('div'));
                 second.appendChild(tree);
                 out.push(take());
                 other.remove();
                 out.push('empty:' + take());
                 second.remove();
                 out.push(take());
                 out.join('|')"
            ),
        ),
        "detached:|Outer:connected Inner:connected|\
         Outer:disconnected Inner:disconnected Outer:connected Inner:connected|\
         empty:|Outer:disconnected Inner:disconnected"
    );
}

/// An attribute change is reported if the definition observes the name,
/// connected or not, with the old and the new value.
#[test]
fn attribute_changes_are_reported_for_observed_names() {
    let (mut engine, _) = engine();
    let class = logging_class("Tone", "['tone', 'class']");
    assert_eq!(
        eval(
            &mut engine,
            &format!(
                "{class}
                 customElements.define('x-tone', Tone);
                 var el = document.createElement('x-tone');
                 take();
                 el.setAttribute('tone', 'warm');
                 el.setAttribute('tone', 'cool');
                 el.setAttribute('other', '1');
                 el.className = 'a b';
                 el.toggleAttribute('tone');
                 el.id = 'ignored';
                 var plain = document.createElement('x-undefined');
                 plain.setAttribute('tone', 'x');
                 take()"
            ),
        ),
        "Tone:tone=null>warm/null Tone:tone=warm>cool/null Tone:class=null>a b/null \
         Tone:tone=cool>null/null"
    );
}

/// `whenDefined()` gives one promise per name, settled with the constructor
/// by `define()`; an invalid name rejects.
#[test]
fn when_defined_settles_with_the_constructor() {
    let (mut engine, _) = engine();
    eval(
        &mut engine,
        "globalThis.seen = [];
         var pending = customElements.whenDefined('x-late');
         globalThis.same = pending === customElements.whenDefined('x-late');
         pending.then(c => seen.push('late:' + c.name));
         customElements.whenDefined('nohyphen').catch(e => seen.push('bad:' + e.name));
         class Late extends HTMLElement {}
         customElements.define('x-late', Late);
         customElements.whenDefined('x-late').then(c => seen.push('again:' + (c === Late)));
         'queued'",
    );
    tenta_js::engine::PageEngine::run_microtasks(&mut engine);
    assert_eq!(
        eval(&mut engine, "[same].concat(seen.sort()).join('|')"),
        "true|again:true|bad:SyntaxError|late:Late"
    );
}

/// A constructor that throws, or that does not return the element, leaves the
/// element as it was and is not tried again; the other elements are upgraded.
#[test]
fn a_failed_upgrade_leaves_the_element_and_the_rest_continue() {
    let (mut engine, _) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "body.innerHTML = '<x-bad id=b></x-bad><x-odd id=o></x-odd><x-good id=g></x-good>';
             class Bad extends HTMLElement { constructor() { super(); throw new Error('no'); } }
             class Odd extends HTMLElement { constructor() { super(); return document.createElement('p'); } }
             class Good extends HTMLElement { connectedCallback() { log.push('good'); } }
             customElements.define('x-bad', Bad);
             customElements.define('x-odd', Odd);
             customElements.define('x-good', Good);
             var bad = document.getElementById('b');
             customElements.upgrade(bad);
             [bad instanceof Bad, document.getElementById('o') instanceof Odd,
              document.getElementById('g') instanceof Good, take(),
              attempt(() => customElements.upgrade(1))].join('|')",
        ),
        "true|true|true|good|TypeError"
    );
}

/// Elements in `<template>` contents are inert: they are upgraded when they
/// are cloned out, and `upgrade()` upgrades a detached tree in place.
#[test]
fn template_contents_stay_inert_until_cloned() {
    let (mut engine, _) = engine();
    let class = logging_class("Leaf", "[]");
    assert_eq!(
        eval(
            &mut engine,
            &format!(
                "var early = document.createElement('x-leaf');
                 {class}
                 customElements.define('x-leaf', Leaf);
                 var template = document.createElement('template');
                 template.innerHTML = '<x-leaf></x-leaf>';
                 var inert = template.content.firstChild;
                 var out = [inert instanceof Leaf, take()];
                 var copy = template.content.cloneNode(true);
                 out.push(copy.firstChild instanceof Leaf, take());
                 out.push(early instanceof Leaf);
                 customElements.upgrade(early);
                 out.push(early instanceof Leaf, take());
                 out.join('|')"
            ),
        ),
        "false||true|Leaf:new|false|true|Leaf:new"
    );
}

/// What the parser does between scripts reaches the definitions when script
/// is next entered: the page's own tree changes are acted on before the next
/// script can see the elements.
#[test]
fn elements_added_outside_script_are_upgraded_at_the_next_entry() {
    let (mut engine, document) = engine();
    let class = logging_class("Row", "[]");
    eval(
        &mut engine,
        &format!("{class} customElements.define('x-row', Row); 'defined'"),
    );
    {
        let mut doc = document.borrow_mut();
        let body = doc.body().expect("the body");
        let row = doc.create_html_element(Atom::from("x-row"));
        doc.mutate(Mutation::Append {
            parent: body,
            child: row,
        })
        .expect("append like the parser");
    }
    assert_eq!(
        eval(
            &mut engine,
            "[take(), body.lastChild instanceof Row].join('|')"
        ),
        "Row:new Row:connected|true"
    );
}

/// A custom element keeps its object, and the state on it, while the element
/// is in the document and script holds nothing.
#[test]
fn a_custom_element_keeps_its_state_across_collection() {
    let (mut engine, _) = engine();
    eval(
        &mut engine,
        "class Keeper extends HTMLElement {
           constructor() { super(); this.state = {count: 41}; }
           bump() { return ++this.state.count; }
         }
         customElements.define('x-keeper', Keeper);
         body.innerHTML = '<x-keeper></x-keeper>';
         'made'",
    );
    engine.low_memory_gc();
    engine.reclaim_dead_hosts();
    assert_eq!(
        eval(
            &mut engine,
            "var k = document.querySelector('x-keeper'); [k instanceof Keeper, k.bump()].join('|')"
        ),
        "true|42"
    );
}

/// `define()` upgrades the elements it finds after its own call has returned,
/// so their reactions may build more nodes than one binding call is allowed
/// to materialize: every element is upgraded and renders.
#[test]
fn a_definition_upgrades_many_elements_that_each_build_a_subtree() {
    let (mut engine, _) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var markup = '';
             for (var i = 0; i < 700; i++) markup += '<x-list></x-list>';
             body.innerHTML = markup;
             var failures = 0;
             class List extends HTMLElement {
               connectedCallback() {
                 try {
                   for (var i = 0; i < 10; i++) {
                     this.appendChild(document.createElement('li')).textContent = 'item ' + i;
                   }
                 } catch (e) { failures++; }
               }
             }
             customElements.define('x-list', List);
             var lists = document.querySelectorAll('x-list');
             var upgraded = 0;
             for (var i = 0; i < lists.length; i++) if (lists[i] instanceof List) upgraded++;
             [lists.length, upgraded, failures, document.querySelectorAll('li').length,
              lists[699].lastChild.textContent].join('|')",
        ),
        "700|700|0|7000|item 9"
    );
    assert_eq!(engine.native_failure(), None);
}

/// One markup assignment can make more custom elements than one binding call
/// may materialize wrappers for; the reactions run after it and none is lost.
#[test]
fn one_insertion_of_many_custom_elements_upgrades_them_all() {
    let (mut engine, _) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var made = 0, connected = 0;
             class Dot extends HTMLElement {
               constructor() { super(); made++; }
               connectedCallback() { connected++; }
             }
             customElements.define('x-dot', Dot);
             var markup = '';
             for (var i = 0; i < 6000; i++) markup += '<x-dot></x-dot>';
             body.innerHTML = markup;
             [made, connected, body.firstChild instanceof Dot, body.lastChild instanceof Dot].join('|')",
        ),
        "6000|6000|true|true"
    );
    assert_eq!(engine.native_failure(), None);
}

/// Replacing the children of an element tells the custom elements that left.
#[test]
fn replacing_children_disconnects_custom_elements() {
    let (mut engine, _) = engine();
    let class = logging_class("Gone", "[]");
    assert_eq!(
        eval(
            &mut engine,
            &format!(
                "{class}
                 customElements.define('x-gone', Gone);
                 body.innerHTML = '<div><x-gone></x-gone></div><x-gone></x-gone>';
                 take();
                 body.innerHTML = '<p>next</p>';
                 var first = take();
                 body.appendChild(document.createElement('x-gone'));
                 take();
                 body.textContent = '';
                 [first, take()].join('|')"
            ),
        ),
        "Gone:disconnected Gone:disconnected|Gone:disconnected"
    );
}
