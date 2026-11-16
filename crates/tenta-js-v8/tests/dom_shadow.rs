//! Shadow trees on the V8 host: `attachShadow()`, `ShadowRoot`, slots, and
//! custom elements that render into a shadow tree.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::{Document, extract};
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

/// `attachShadow()` checks its host and its mode, gives a `ShadowRoot` that is
/// a `DocumentFragment`, and only an open root is reachable from its host.
#[test]
fn attach_shadow_gives_a_shadow_root_and_validates() {
    let (mut engine, _) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var host = body.appendChild(document.createElement('div'));
             var root = host.attachShadow({mode: 'open'});
             var sealed = document.createElement('x-sealed');
             var hidden = sealed.attachShadow({mode: 'closed'});
             [root instanceof ShadowRoot, root instanceof DocumentFragment, root instanceof Node,
              Object.prototype.toString.call(root), root.nodeType, root.nodeName,
              root.mode, root.host === host, host.shadowRoot === root, root.delegatesFocus,
              root.slotAssignment, String(root.parentNode), root.childNodes.length,
              hidden.mode, String(sealed.shadowRoot), hidden.host === sealed,
              attempt(() => host.attachShadow({mode: 'open'})),
              attempt(() => document.createElement('a').attachShadow({mode: 'open'})),
              attempt(() => document.createElement('img').attachShadow({mode: 'open'})),
              attempt(() => document.createElement('section').attachShadow({})),
              attempt(() => document.createElement('section').attachShadow({mode: 'ajar'})),
              attempt(() => document.createElement('section').attachShadow()),
              attempt(() => new ShadowRoot()),
              String(document.createElement('p').shadowRoot)].join('|')",
        ),
        "true|true|true|[object ShadowRoot]|11|#document-fragment|\
         open|true|true|false|named|null|0|\
         closed|null|true|\
         NotSupportedError|NotSupportedError|NotSupportedError|TypeError|TypeError|TypeError|\
         TypeError|null"
    );
}

/// A shadow tree is a tree of its own: queries on the root find its nodes,
/// queries on the document and on the host do not, and the nodes are
/// connected when the host is.
#[test]
fn a_shadow_tree_is_separate_from_the_document_tree() {
    let (mut engine, _) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var host = body.appendChild(document.createElement('div'));
             host.innerHTML = '<span id=light>light</span>';
             var root = host.attachShadow({mode: 'open'});
             root.innerHTML = '<p id=inner class=c>shadow <b>text</b></p><td>cell</td>';
             var inner = root.querySelector('#inner');
             var out = [root.innerHTML, root.children.length, root.childElementCount,
                        root.firstElementChild === inner, root.getElementById('inner') === inner,
                        root.querySelectorAll('b').length, inner.parentNode === root,
                        String(document.getElementById('inner')), String(document.querySelector('.c')),
                        String(host.querySelector('b')), document.getElementsByTagName('b').length,
                        host.textContent, host.innerHTML, host.childNodes.length,
                        inner.isConnected, inner.getRootNode() === root,
                        inner.getRootNode({composed: true}) === document,
                        host.getRootNode() === document, inner.ownerDocument === document,
                        root.contains(inner), host.contains(inner), root.isConnected];
             host.remove();
             out.push(inner.isConnected, root.isConnected, inner.getRootNode({composed: true}) === host);
             var extra = root.appendChild(document.createElement('i'));
             out.push(root.lastChild === extra, root.childNodes.length);
             root.textContent = 'replaced';
             out.push(root.innerHTML);
             out.join('|')",
        ),
        "<p id=\"inner\" class=\"c\">shadow <b>text</b></p>cell|1|1|true|true|1|true|\
         null|null|null|0|light|<span id=\"light\">light</span>|1|\
         true|true|true|true|true|true|false|true|\
         false|false|true|true|3|replaced"
    );
}

/// Slots are assigned by name from the host's children, and the assignment is
/// readable from both sides.
#[test]
fn slots_report_their_assigned_nodes() {
    let (mut engine, _) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var host = body.appendChild(document.createElement('div'));
             host.innerHTML = 'lead<b slot=title>T</b><i>one</i><u slot=none>lost</u>';
             var root = host.attachShadow({mode: 'open'});
             root.innerHTML = '<slot name=title></slot><slot id=rest>fallback</slot>';
             var title = root.firstChild, rest = root.getElementById('rest');
             var b = host.querySelector('b'), i = host.querySelector('i'), u = host.querySelector('u');
             var out = [title instanceof HTMLSlotElement, title.name, rest.name,
                        title.assignedNodes().length, title.assignedNodes()[0] === b,
                        rest.assignedNodes().map(n => n.nodeName).join(','),
                        rest.assignedElements().map(n => n.nodeName).join(','),
                        b.assignedSlot === title, i.assignedSlot === rest, String(u.assignedSlot),
                        b.slot, i.slot, String(host.assignedSlot)];
             u.slot = 'title';
             out.push(u.getAttribute('slot'), title.assignedElements().length, u.assignedSlot === title);
             title.name = 'other';
             out.push(title.assignedNodes().length, String(b.assignedSlot));
             out.join('|')",
        ),
        "true|title||1|true|#text,I|I|true|true|null|title||null|title|2|true|0|null"
    );
}

/// A custom element that renders into a shadow tree: its constructor attaches
/// the root, nested custom elements inside the shadow tree are upgraded and
/// told they are connected with their host, and extraction reads the result
/// with the slotted children in place.
#[test]
fn custom_elements_render_into_shadow_trees() {
    let (mut engine, document) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "class Badge extends HTMLElement {
               connectedCallback() { log.push('badge:' + this.isConnected); this.textContent = 'new'; }
               disconnectedCallback() { log.push('badge:gone'); }
             }
             class Card extends HTMLElement {
               constructor() {
                 super();
                 this.attachShadow({mode: 'open'}).innerHTML =
                   '<h2><slot name=title>Untitled</slot> <x-badge></x-badge></h2><slot></slot>';
               }
               connectedCallback() { log.push('card:connected'); }
             }
             customElements.define('x-badge', Badge);
             customElements.define('x-card', Card);
             var card = document.createElement('x-card');
             card.innerHTML = '<span slot=title>Shadow</span><p>Body text</p>';
             var badge = card.shadowRoot.querySelector('x-badge');
             var out = [badge instanceof Badge, take()];
             body.appendChild(card);
             out.push(take());
             card.remove();
             out.push(take());
             body.appendChild(card);
             take();
             out.join('|')",
        ),
        "true||card:connected badge:true|badge:gone"
    );
    let doc = document.borrow();
    let body = doc.body().expect("body");
    let text = extract::text(&doc, body);
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    assert_eq!(lines, ["Shadow new", "Body text"]);
}

/// An event leaves a shadow tree through the host when it is composed, and
/// listeners outside the tree see the host as its target; one that is not
/// composed stops at the shadow root.
#[test]
fn events_cross_the_shadow_boundary_retargeted() {
    let (mut engine, _) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var host = body.appendChild(document.createElement('div'));
             host.id = 'host';
             var root = host.attachShadow({mode: 'open'});
             root.innerHTML = '<section><button id=inner>go</button></section>';
             var inner = root.getElementById('inner');
             var name = n => n === window ? 'window' : n === document ? 'document'
                           : n === root ? 'root' : (n.id || n.nodeName.toLowerCase());
             var seen = where => e => log.push(where + '<' + name(e.target) + ':' + e.eventPhase);
             inner.addEventListener('ping', seen('inner'));
             root.addEventListener('ping', seen('root'));
             host.addEventListener('ping', seen('host'));
             host.addEventListener('ping', seen('host-capture'), true);
             body.addEventListener('ping', seen('body'));
             document.addEventListener('ping', seen('document'));
             window.addEventListener('ping', seen('window'));
             var out = [];
             var paths = [];
             host.addEventListener('ping', e => paths.push(e.composedPath().map(name).join('>')));
             inner.dispatchEvent(new Event('ping', {bubbles: true, composed: true}));
             out.push(take());
             inner.dispatchEvent(new Event('ping', {bubbles: true}));
             out.push(take());
             inner.dispatchEvent(new Event('ping', {composed: true}));
             out.push(take());
             inner.click();
             out.push(paths.join(' '));
             out.join('|')",
        ),
        "host-capture<host:2 inner<inner:2 root<inner:3 host<host:2 body<host:3 document<host:3 \
         window<host:3|\
         inner<inner:2 root<inner:3|\
         host-capture<host:2 inner<inner:2 host<host:2|\
         inner>section>root>host>body>html>document>window \
         inner>section>root>host>body>html>document>window"
    );
}

/// An event at a slotted node passes through its slot and the shadow tree on
/// the way to the host, and keeps the slotted node as its target throughout.
#[test]
fn events_at_slotted_nodes_pass_through_the_slot() {
    let (mut engine, _) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var host = body.appendChild(document.createElement('div'));
             host.id = 'host';
             host.innerHTML = '<b id=light>x</b>';
             var root = host.attachShadow({mode: 'open'});
             root.innerHTML = '<p id=frame><slot id=slot></slot></p>';
             var light = document.getElementById('light');
             var where = n => n === root ? 'root' : (n.id || n.nodeName.toLowerCase());
             var seen = e => log.push(where(e.currentTarget) + '<' + where(e.target));
             [light, root.getElementById('slot'), root.getElementById('frame'), root, host, body]
               .forEach(n => n.addEventListener('click', seen));
             light.click();
             take()",
        ),
        "light<light slot<light frame<light root<light host<light body<light"
    );
}
