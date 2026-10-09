//! Real-engine controls for generated WebIDL legacy property interceptors.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;
use url::Url;

fn engine() -> Engine {
    let document = Rc::new(RefCell::new(Document::with_url(
        Url::parse("https://example.test/legacy").expect("test URL"),
    )));
    Engine::with_document(document)
}

#[test]
fn readonly_indexed_properties_leave_inherited_setters_usable() {
    let mut engine = engine();
    let result = engine
        .evaluate(
            r#"
            const parent = document.createElement('div');
            const child = document.createElement('span');
            parent.appendChild(child);
            const list = parent.childNodes;
            const reflectSet = Reflect.set;
            Reflect.set = () => { throw new Error('page replaced Reflect.set'); };
            let inherited = 0;
            const prototype = Object.create(NodeList.prototype);
            Object.defineProperty(prototype, '1', {set(value) { inherited = value; }});
            Object.setPrototypeOf(list, prototype);
            const descriptor = Object.getOwnPropertyDescriptor(list, '0');
            const inheritedSet = reflectSet(list, '1', 7) && inherited === 7;
            const supportedRefused = !reflectSet(list, '0', 9) && list[0] === child;
            const absentRefused = !reflectSet(list, '2', 8);
            const absentUncreated = !Object.hasOwn(list, '2');
            const derived = Object.create(list);
            const derivedSet = reflectSet(list, '2', 8, derived) &&
                Object.hasOwn(derived, '2') && derived[2] === 8 &&
                !Object.hasOwn(list, '2');
            let strictRefused = false;
            try { (function () { 'use strict'; list[0] = 9; })(); }
            catch (error) { strictRefused = error instanceof TypeError; }
            const descriptorCorrect = descriptor.value === child &&
                descriptor.enumerable && descriptor.configurable && !descriptor.writable;
            const deleteRules = !Reflect.deleteProperty(list, '0') &&
                Reflect.deleteProperty(list, '2');
            const extensibility = !Reflect.preventExtensions(list) &&
                Object.isExtensible(list);
            let proxyCalls = 0;
            const proxy = new Proxy(Object.create(NodeList.prototype), {
                set(target, key, value, receiver) {
                    proxyCalls++;
                    return key === '3' && value === 11 && receiver === list;
                }
            });
            Object.setPrototypeOf(list, proxy);
            const inheritedProxySet = reflectSet(list, '3', 11) &&
                proxyCalls === 1 && !Object.hasOwn(list, '3');
            [inheritedSet, supportedRefused, absentRefused, absentUncreated, derivedSet, strictRefused,
             descriptorCorrect, deleteRules, extensibility, inheritedProxySet].join('|')
            "#,
        )
        .expect("legacy indexed script");
    assert_eq!(result, "true|true|true|true|true|true|true|true|true|true");
}
