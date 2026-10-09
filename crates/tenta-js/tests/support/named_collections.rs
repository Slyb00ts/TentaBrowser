// Shared real-engine checks for the DOM's named legacy collection properties.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;

fn eval(source: &str) -> String {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = new_engine(document);
    engine.evaluate(source).expect("named collection script")
}

#[test]
fn html_collection_named_properties_follow_tree_order_and_live_changes() {
    assert_eq!(
        eval(
            r#"
            const root = document.createElement('div');
            const first = document.createElement('a');
            first.setAttribute('id', 'first');
            first.setAttribute('name', 'alias');
            root.appendChild(first);
            const second = document.createElement('b');
            second.setAttribute('id', 'alias');
            second.setAttribute('name', 'second');
            root.appendChild(second);
            const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
            svg.setAttribute('id', 'svgId');
            svg.setAttribute('name', 'svgName');
            root.appendChild(svg);
            const collection = root.children;
            const snapshot = root.getElementsByTagName('a');
            const checks = [
              collection[0] === first,
              collection.first === first,
              collection.alias === first,
              collection.namedItem('alias') === first,
              snapshot.alias === first,
              snapshot.namedItem('alias') === first,
              collection.svgId === svg,
              collection.svgName === undefined,
              collection.namedItem('svgName') === null,
              collection.namedItem('') === null,
              Object.getOwnPropertyDescriptor(collection, 'alias').enumerable === false,
              Object.keys(collection).join(',') === '0,1,2',
              Reflect.ownKeys(collection).join(',') ===
                '0,1,2,first,alias,second,svgId'
            ];
            first.setAttribute('name', 'changed');
            checks.push(collection.alias === second, collection.changed === first,
              snapshot.changed === first);
            first.removeAttribute('id');
            checks.push(collection.first === undefined);
            second.setAttribute('id', 'item');
            checks.push(typeof collection.item === 'function',
              collection.namedItem('item') === second);
            checks.every(Boolean);
            "#,
        ),
        "true"
    );
}

#[test]
fn named_node_map_filters_html_uppercase_and_preserves_xml_names() {
    assert_eq!(
        eval(
            r#"
            const element = document.createElement('div');
            element.setAttribute('alpha', 'a');
            element.setAttributeNS('urn:one', 'p:beta', 'b');
            element.setAttributeNS('urn:two', 'P:GAMMA', 'c');
            element.setAttribute('length', 'd');
            const map = element.attributes;
            const alpha = map.getNamedItem('alpha');
            const beta = map.getNamedItemNS('urn:one', 'beta');
            const checks = [
              map.alpha === alpha,
              map['p:beta'] === beta,
              map['P:GAMMA'] === undefined,
              map.length === 4,
              map.getNamedItem('length').value === 'd',
              Object.getOwnPropertyDescriptor(map, 'alpha').enumerable === false,
              Object.keys(map).join(',') === '0,1,2,3',
              Reflect.ownKeys(map).join(',') === '0,1,2,3,alpha,p:beta'
            ];
            element.removeAttribute('alpha');
            checks.push(map.alpha === undefined);
            element.setAttribute('new', 'e');
            checks.push(map.new === map.getNamedItem('new'));

            const xml = document.implementation.createDocument(null, 'root');
            const xmlElement = xml.documentElement;
            xmlElement.setAttribute('MiXeD', 'v');
            xmlElement.setAttributeNS('urn:three', 'P:Tag', 'w');
            const xmlMap = xmlElement.attributes;
            checks.push(xmlMap.MiXeD === xmlMap.getNamedItem('MiXeD'),
              xmlMap['P:Tag'] === xmlMap.getNamedItem('P:Tag'));
            checks.every(Boolean);
            "#,
        ),
        "true"
    );
}

#[test]
fn ordinary_collection_properties_keep_own_and_prototype_precedence() {
    assert_eq!(
        eval(
            r#"
            const root = document.createElement('div');
            const child = document.createElement('a');
            root.appendChild(child);
            const collection = root.children;
            Object.defineProperty(collection, 'later', {
              value: 1, writable: true, configurable: true, enumerable: true
            });
            child.id = 'later';
            const redefined = Reflect.defineProperty(collection, 'later', {value: 2});
            const ownWins = collection.later === 2 &&
              collection.namedItem('later') === child;
            const proto = Object.create(Object.getPrototypeOf(collection));
            Object.defineProperty(proto, 'shadow', {value: 99});
            Object.setPrototypeOf(collection, proto);
            child.id = 'shadow';
            const protoWins = collection.shadow === 99 &&
              collection.namedItem('shadow') === child;
            [redefined, ownWins, protoWins].join('|');
            "#,
        ),
        "true|true|true"
    );
}

#[test]
fn canonical_numeric_name_does_not_override_indexed_collection_semantics() {
    assert_eq!(
        eval(
            r#"
            const parent=document.createElement('div');
            const child=document.createElement('span');
            child.id='10';
            parent.appendChild(child);
            const collection=parent.children;
            [collection.namedItem('10')===child,
             collection[0]===child,
             collection[10]===undefined,
             Object.getOwnPropertyDescriptor(collection,'10')===undefined,
             !Reflect.defineProperty(collection,'10',{value:7}),
             !Object.hasOwn(collection,'10')].join('|')
            "#,
        ),
        "true|true|true|true|true|true"
    );
}
