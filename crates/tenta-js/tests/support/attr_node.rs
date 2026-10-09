// Shared real-engine Attr-as-Node behavior, included by V8 and QuickJS tests.

use tenta_js::engine::{PageEngine, ScriptKind};

#[test]
fn plain_attribute_names_follow_element_namespace_and_document_case_rules() {
    assert_eq!(
        evaluate(
            r#"
            const xml = document.implementation.createDocument(null, 'root');
            const elements = [
              document.createElement('div'),
              document.createElementNS('http://www.w3.org/2000/svg', 'svg'),
              xml.documentElement,
              xml.createElementNS('http://www.w3.org/1999/xhtml', 'div')
            ];
            const checks = [];
            for (let index = 0; index < elements.length; index++) {
              const element = elements[index];
              const lowercases = index === 0;
              const storedName = lowercases ? 'mixed' : 'MiXeD';
              element.setAttribute('MiXeD', 'before');
              const attr = element.getAttributeNode(storedName);
              checks.push(attr !== null, attr.name === storedName,
                element.getAttribute('MiXeD') === 'before',
                element.hasAttribute('mixed') === lowercases);
              element.setAttribute('MiXeD', 'after');
              checks.push(attr.value === 'after', element.attributes.length === 1);
              element.removeAttribute('mixed');
              checks.push(element.hasAttribute('MiXeD') === !lowercases);
              element.removeAttribute('MiXeD');
              checks.push(attr.ownerElement === null, attr.value === 'after');
              checks.push(element.toggleAttribute('MiXeD'),
                element.getAttributeNode(storedName).value === '',
                !element.toggleAttribute('MiXeD'), element.attributes.length === 0);
            }
            checks.every(Boolean);
            "#,
        ),
        "true"
    );
}

#[test]
fn plain_attribute_operations_preserve_the_first_qualified_namespaced_attribute() {
    assert_eq!(
        evaluate(
            r#"
            const element = document.createElementNS('urn:element', 'root');
            element.setAttributeNS('urn:first', 'p:key', 'first');
            element.setAttributeNS('urn:second', 'p:key', 'second');
            const first = element.getAttributeNodeNS('urn:first', 'key');
            const second = element.getAttributeNodeNS('urn:second', 'key');
            const checks = [element.hasAttribute('p:key'),
              element.getAttribute('p:key') === 'first',
              element.getAttributeNode('p:key') === first,
              element.attributes.getNamedItem('p:key') === first];
            element.setAttribute('p:key', 'changed');
            checks.push(element.attributes.length === 2,
              first.value === 'changed', second.value === 'second',
              first.namespaceURI === 'urn:first', first.prefix === 'p',
              element.toggleAttribute('p:key', true), first.value === 'changed');
            element.removeAttribute('p:key');
            checks.push(first.ownerElement === null, first.value === 'changed',
              element.getAttribute('p:key') === 'second',
              !element.toggleAttribute('p:key'), second.ownerElement === null,
              !element.hasAttribute('p:key'));
            element.setAttribute('p:key', 'plain');
            const plain = element.getAttributeNode('p:key');
            checks.push(plain.namespaceURI === null, plain.prefix === null,
              plain.localName === 'p:key', plain.value === 'plain',
              element.attributes.removeNamedItem('p:key') === plain,
              plain.ownerElement === null);
            checks.every(Boolean);
            "#,
        ),
        "true"
    );
}

#[test]
fn namespace_attribute_local_names_use_dom_string_conversion() {
    assert_eq!(
        evaluate(
            r#"
            const element = document.createElement('div');
            element.setAttribute('null', 'null value');
            element.setAttribute('undefined', 'undefined value');
            element.setAttributeNS('urn:example', 'p:null', 'namespaced value');
            const checks = [
              element.getAttributeNS(null, null) === 'null value',
              element.getAttributeNS('', null) === 'null value',
              element.getAttributeNS(undefined, undefined) === 'undefined value',
              element.getAttributeNS('urn:example', null) === 'namespaced value',
              element.hasAttributeNS(null, null),
              element.hasAttributeNS('', undefined),
              element.hasAttributeNS('urn:example', null)
            ];
            element.removeAttributeNS(null, null);
            checks.push(!element.hasAttribute('null'),
              element.hasAttributeNS('urn:example', null));
            element.removeAttributeNS('', undefined);
            checks.push(!element.hasAttribute('undefined'));
            element.removeAttributeNS('urn:example', null);
            checks.push(!element.hasAttributeNS('urn:example', 'null'));
            checks.every(Boolean);
            "#,
        ),
        "true"
    );
}

#[test]
fn namespace_attribute_node_names_use_dom_string_conversion() {
    assert_eq!(
        evaluate(
            r#"
            const element = document.createElement('div');
            element.setAttribute('null', 'null value');
            element.setAttribute('undefined', 'undefined value');
            element.setAttributeNS('urn:example', 'p:null', 'namespaced value');
            const plain = element.getAttributeNode('null');
            const undef = element.getAttributeNode('undefined');
            const namespaced = element.getAttributeNodeNS('urn:example', 'null');
            const map = element.attributes;
            const checks = [
              element.getAttributeNodeNS(null, null) === plain,
              element.getAttributeNodeNS('', undefined) === undef,
              element.getAttributeNodeNS('urn:example', null) === namespaced,
              map.getNamedItemNS(null, null) === plain,
              map.getNamedItemNS('', undefined) === undef,
              map.getNamedItemNS('urn:example', null) === namespaced,
              map.removeNamedItemNS(null, null) === plain,
              plain.ownerElement === null,
              map.removeNamedItemNS('', undefined) === undef,
              undef.ownerElement === null,
              map.removeNamedItemNS('urn:example', null) === namespaced,
              namespaced.ownerElement === null,
              map.length === 0
            ];
            checks.every(Boolean);
            "#,
        ),
        "true"
    );
}

#[test]
fn namespace_tag_queries_convert_local_names_and_preserve_namespace_matching() {
    assert_eq!(
        evaluate(
            r#"
            const doc = document.implementation.createDocument(null, 'root');
            const root = doc.documentElement;
            const plain = doc.createElementNS(null, 'null');
            const undef = doc.createElementNS(null, 'undefined');
            const namespaced = doc.createElementNS('urn:example', 'p:null');
            root.appendChild(plain);
            root.appendChild(undef);
            root.appendChild(namespaced);
            const checks = [];
            for (const target of [doc, root]) {
              checks.push(
                target.getElementsByTagNameNS(null, null).item(0) === plain,
                target.getElementsByTagNameNS('', undefined).item(0) === undef,
                target.getElementsByTagNameNS('urn:example', null).item(0) === namespaced,
                target.getElementsByTagNameNS('*', null).length === 2,
                target.getElementsByTagNameNS('urn:example', '*').length === 1,
                target.getElementsByTagNameNS(null, '').length === 0
              );
            }
            checks.every(Boolean);
            "#,
        ),
        "true"
    );
}

#[test]
fn attr_inherits_node_and_serializes_as_empty_xml() {
    assert_eq!(
        evaluate(
            r#"
            const attr = document.createAttributeNS('urn:example', 'p:key');
            attr.value = 'value';
            const list = attr.childNodes;
            let events = 0;
            attr.addEventListener('probe', event => {
              if (event.target === attr) events++;
            });
            attr.dispatchEvent(new Event('probe', {bubbles: true}));
            [
              attr instanceof Attr, attr instanceof Node,
              attr instanceof EventTarget,
              Object.getPrototypeOf(Attr.prototype) === Node.prototype,
              attr.nodeType, attr.nodeName, attr.nodeValue, attr.textContent,
              attr.ownerDocument === document, attr.ownerElement === null,
              attr.isConnected, attr.parentNode === null,
              attr.parentElement === null, attr.firstChild === null,
              attr.lastChild === null, attr.previousSibling === null,
              attr.nextSibling === null, !attr.hasChildNodes(),
              list === attr.childNodes, list.length, list.item(0) === null,
              attr.contains(attr), document.contains(attr), events,
              new XMLSerializer().serializeToString(attr) === ''
            ].join('|');
            "#,
        ),
        "true|true|true|true|2|p:key|value|value|true|true|false|true|true|true|true|true|true|true|true|0|true|true|false|1|true"
    );
}

#[test]
fn attached_attr_identity_and_detachment_follow_mutations() {
    assert_eq!(
        evaluate(
            r#"
            const element = document.createElement('div');
            element.setAttribute('id', 'before');
            const attr = element.getAttributeNode('id');
            const identity = [
              attr === element.getAttributeNode('id'),
              attr === element.getAttributeNodeNS(null, 'id'),
              attr === element.attributes.item(0),
              attr === element.attributes.getNamedItem('id'),
              element.setAttributeNode(attr) === attr
            ];
            element.setAttribute('id', 'after');
            identity.push(attr.value === 'after');
            element.removeAttribute('id');
            identity.push(attr.ownerElement === null, attr.value === 'after');
            element.setAttribute('id', 'new');
            identity.push(element.getAttributeNode('id') !== attr);
            identity.join('|');
            "#,
        ),
        "true|true|true|true|true|true|true|true|true"
    );
}

#[test]
fn replacing_and_removing_attribute_nodes_keep_original_objects() {
    assert_eq!(
        evaluate(
            r#"
            const element = document.createElement('div');
            element.setAttribute('data-x', 'old');
            const old = element.getAttributeNode('data-x');
            const next = document.createAttribute('data-x');
            next.value = 'new';
            const replaced = element.setAttributeNode(next);
            const checks = [
              replaced === old, old.ownerElement === null, old.value === 'old',
              next.ownerElement === element,
              element.getAttributeNode('data-x') === next
            ];
            const removed = element.attributes.removeNamedItem('data-x');
            checks.push(removed === next, removed.ownerElement === null,
              removed.value === 'new', element.hasAttribute('data-x') === false);
            checks.join('|');
            "#,
        ),
        "true|true|true|true|true|true|true|true|true"
    );
}

#[test]
fn set_attribute_ns_keeps_the_original_prefix_and_attr_identity() {
    assert_eq!(
        evaluate(
            r#"
            const element = document.createElement('div');
            element.setAttributeNS('urn:example', 'p:key', 'before');
            const attr = element.getAttributeNodeNS('urn:example', 'key');
            element.setAttributeNS('urn:example', 'q:key', 'after');
            [element.attributes.length,
             element.attributes.item(0) === attr,
             element.getAttributeNodeNS('urn:example', 'key') === attr,
             attr.name, attr.nodeName, attr.value,
             element.getAttributeNS('urn:example', 'key')].join('|');
            "#,
        ),
        "1|true|true|p:key|p:key|after|after"
    );
}

#[test]
fn replacing_a_namespaced_attr_node_uses_the_new_objects_prefix() {
    assert_eq!(
        evaluate(
            r#"
            const element = document.createElement('div');
            element.setAttributeNS('urn:example', 'p:key', 'before');
            const old = element.getAttributeNodeNS('urn:example', 'key');
            const next = document.createAttributeNS('urn:example', 'q:key');
            next.value = 'after';
            const replaced = element.setAttributeNodeNS(next);
            [replaced === old, old.ownerElement === null, old.value,
             next.ownerElement === element, next.name,
             element.attributes.item(0) === next,
             element.getAttributeNodeNS('urn:example', 'key') === next,
             element.attributes.length].join('|');
            "#,
        ),
        "true|true|before|true|q:key|true|true|1"
    );
}

#[test]
fn qualified_name_lookup_lowercases_only_html_elements_in_html_documents() {
    assert_eq!(
        evaluate(
            r#"
            const html = document.createElement('div');
            html.setAttribute('id', 'one');
            const attr = html.getAttributeNode('id');
            const htmlChecks = [html.getAttributeNode('ID') === attr,
              html.attributes.getNamedItem('ID') === attr,
              html.attributes.removeNamedItem('ID') === attr];

            const xml = new Document();
            const xmlElement = xml.createElement('item');
            xmlElement.setAttributeNS(null, 'Mixed', 'two');
            const xmlChecks = [xmlElement.getAttributeNode('Mixed') !== null,
              xmlElement.getAttributeNode('mixed') === null,
              xmlElement.attributes.getNamedItem('Mixed') !== null,
              xmlElement.attributes.getNamedItem('mixed') === null];

            const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
            svg.setAttributeNS(null, 'Mixed', 'three');
            const svgChecks = [svg.getAttributeNode('Mixed') !== null,
              svg.getAttributeNode('mixed') === null];
            [...htmlChecks, ...xmlChecks, ...svgChecks].join('|');
            "#,
        ),
        "true|true|true|true|true|true|true|true|true"
    );
}

#[test]
fn attr_clone_and_tree_mutation_errors_use_node_semantics() {
    assert_eq!(
        evaluate(
            r#"
            const element = document.createElement('div');
            const attr = document.createAttribute('data-x');
            attr.nodeValue = 'first';
            attr.textContent = 'second';
            const clone = attr.cloneNode(true);
            const error = callback => { try { callback(); return 'none'; }
              catch (exception) { return exception.name; } };
            [clone instanceof Attr, clone !== attr, clone.ownerElement === null,
             clone.name, clone.value, attr.value,
             error(() => attr.appendChild(document.createTextNode('x'))),
             error(() => element.appendChild(attr)),
             error(() => attr.removeChild(document.createTextNode('x'))),
             error(() => element.removeChild(attr))].join('|');
            "#,
        ),
        "true|true|true|data-x|second|second|HierarchyRequestError|HierarchyRequestError|NotFoundError|NotFoundError"
    );
}

#[test]
fn held_attr_pins_detached_owner_and_releases_it_after_gc() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = new_engine(document.clone());
    engine
        .run_script(
            "(()=>{const element=document.createElement('div'); \
             element.setAttribute('data-x','held'); \
             globalThis.heldAttr=element.getAttributeNode('data-x');})();",
            ScriptKind::Classic,
        )
        .expect("create a detached owner held only by its Attr");
    assert_eq!(document.borrow().len(), 2);
    for _ in 0..3 {
        engine.trim_memory();
        assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 0);
    }
    engine
        .run_script(
            "if (heldAttr.ownerElement.getAttribute('data-x') !== 'held') \
             throw Error('Attr lost its owner'); globalThis.heldAttr=null;",
            ScriptKind::Classic,
        )
        .expect("owner remains live while the Attr is reachable");
    let mut freed = 0;
    for _ in 0..3 {
        engine.trim_memory();
        freed += document.borrow_mut().sweep_detached().nodes_freed;
    }
    assert_eq!(freed, 1, "reclaimed Attr releases its owner pin");
}

#[test]
fn child_list_remains_usable_after_its_attr_wrapper_is_collected() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = new_engine(document);
    engine
        .run_script(
            "globalThis.heldList=(()=>{const attr=document.createAttribute('id'); \
             return attr.childNodes})();",
            ScriptKind::Classic,
        )
        .expect("retain a child list without retaining its Attr");
    for _ in 0..3 {
        engine.trim_memory();
    }
    engine
        .run_script(
            "if (heldList.length!==0 || heldList.item(0)!==null) \
             throw Error('Attr child list became stale after owner collection')",
            ScriptKind::Classic,
        )
        .expect("the child list owns its independent empty snapshot");
}
