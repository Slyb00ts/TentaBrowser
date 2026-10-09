//! Native indexed wrappers preserve WebIDL descriptors and actual GC edges.

use std::cell::RefCell;
use std::rc::Rc;
use tenta_dom::{Document, SharedDocument};
use tenta_js_qjs::Engine;

fn engine() -> (SharedDocument, Engine) {
    let document = Rc::new(RefCell::new(Document::new()));
    let engine = Engine::with_document(document.clone()).expect("QuickJS engine");
    (document, engine)
}
fn eval(engine: &mut Engine, source: &str) -> String {
    engine.evaluate(source).expect("native legacy properties")
}
fn collect(document: &SharedDocument, engine: &mut Engine) -> usize {
    let mut freed = 0;
    for _ in 0..3 {
        engine.run_microtasks();
        engine.trim_memory();
        freed += document.borrow_mut().sweep_detached().nodes_freed;
        engine.trim_memory();
    }
    freed
}

#[test]
fn indexed_properties_are_live_and_refuse_incompatible_changes() {
    let (_, mut engine) = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
        const parent = document.createElement('div');
        const list = parent.childNodes;
        const first = parent.appendChild(document.createElement('b'));
        const d = Object.getOwnPropertyDescriptor(list, '0');
        let strict = false;
        try { (()=>{'use strict'; list[0] = 1})() } catch (e) { strict = e instanceof TypeError; }
        const before = [list[0]===first, list[1]===undefined, 0 in list, !(1 in list),
          d.value===first, d.configurable, d.enumerable, !d.writable,
          !Reflect.set(list,'0',1), strict, !Reflect.deleteProperty(list,'0'),
          Reflect.deleteProperty(list,'1'),
          !Reflect.defineProperty(list,'0',{value:3}),
          !Reflect.defineProperty(list,'99',{value:3})];
        parent.appendChild(document.createElement('i'));
        before.push(list[1] === list.item(1));
        parent.removeChild(first);
        before.push(list[0].nodeName === 'I', list[1] === undefined);
        before.every(Boolean)
    "#
        ),
        "true"
    );
}

#[test]
fn expandos_preserve_order_descriptors_and_derived_receivers() {
    let (_, mut engine) = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
        const parent = document.createElement('div');
        parent.appendChild(document.createElement('b'));
        const list = parent.childNodes;
        const symbol = Symbol('expando');
        list.alpha = 1; list[symbol] = 2; list['00'] = 3; list['4294967295'] = 4;
        Object.defineProperty(list,'getter',{get(){return this}, configurable:true});
        Object.defineProperty(list,'setter',{set(value){this.received=value}, configurable:true});
        const receiver = {};
        const keys = Reflect.ownKeys(list);
        const before = keys.length===7 && keys[0]==='0' && keys[1]==='alpha' &&
            keys[2]==='00' && keys[3]==='4294967295' && keys[4]==='getter' && keys[5]==='setter' && keys[6]===symbol;
        const get = Reflect.get(list,'getter',receiver)===receiver;
        const set = Reflect.set(list,'setter',42,receiver) && receiver.received===42;
        const derived = Object.create(list);
        const own = Reflect.set(list,'alpha',9,derived) && derived.alpha===9 && list.alpha===1 && Object.hasOwn(derived,'alpha');
        Object.defineProperty(list,'locked',{value:8});
        const locked = !Reflect.set(list,'locked',9) && !Reflect.deleteProperty(list,'locked') && !Reflect.defineProperty(list,'locked',{value:9});
        delete list.alpha; list.alpha = 11;
        const order = Reflect.ownKeys(list).at(-2)==='alpha';
        [before,get,set,own,locked,order].every(Boolean)
    "#
        ),
        "true"
    );
}

#[test]
fn legacy_prevent_extensions_refuses_but_ordinary_wrappers_are_unchanged() {
    let (_, mut engine) = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
        const node = document.createElement('div'), list = node.childNodes;
        let throws=0;
        for (const operation of [Object.preventExtensions,Object.seal,Object.freeze]) {
          try { operation(list); } catch (error) { if(error instanceof TypeError) throws++; }
        }
        [!Reflect.preventExtensions(list),Object.isExtensible(list),throws===3,
         Reflect.preventExtensions(node),!Object.isExtensible(node)].every(Boolean)
    "#
        ),
        "true"
    );
}

#[test]
fn pristine_reflection_and_plain_receiver_brand_survive_page_mutation() {
    let (_, mut engine) = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
        const list=document.createElement('div').childNodes;
        const descriptor=Object.getOwnPropertyDescriptor;
        for(const name of ['get','set','has','getOwnPropertyDescriptor','getPrototypeOf','defineProperty','deleteProperty']) Reflect[name]=()=>{throw Error('tampered')};
        Object.defineProperty(Object.prototype,'get',{get(){throw Error('polluted get')},configurable:true});
        Object.defineProperty(Object.prototype,'value',{get(){throw Error('polluted value')},configurable:true});
        list.extra=41;
        let brand=false;
        try { NodeList.prototype.item.call(Object.create(NodeList.prototype),0); } catch(e) { brand=e instanceof TypeError; }
        const result=list.extra===41 && descriptor(list,'extra').value===41 && brand && list.length===0;
        delete Object.prototype.get; delete Object.prototype.value;
        result
    "#
        ),
        "true"
    );
}

#[test]
fn traced_data_accessor_and_symbol_cycles_are_collectible() {
    let (document, mut engine) = engine();
    eval(
        &mut engine,
        r#"
        (()=>{
          const parent=document.createElement('div');
          parent.appendChild(document.createElement('b'));
          const list=parent.childNodes;
          list.owner=parent; list.self=list;
          list[Symbol('cycle')]=list;
          Object.defineProperty(list,'closure',{get:()=>list});
        })();
    "#,
    );
    assert_eq!(document.borrow().len(), 3);
    assert_eq!(collect(&document, &mut engine), 2);
    assert_eq!(document.borrow().len(), 1);
}

#[test]
fn reachable_expando_edges_keep_wrappers_and_their_nodes_alive() {
    let (document, mut engine) = engine();
    eval(
        &mut engine,
        r#"
        globalThis.kept=(()=>{
          const parent=document.createElement('div');
          const child=parent.appendChild(document.createElement('b'));
          const list=parent.childNodes;
          list.owner=parent; list.self=list;
          Object.defineProperty(list,'closure',{get:()=>child});
          list[Symbol.for('cycle')]=list;
          return list;
        })();
    "#,
    );
    assert_eq!(collect(&document, &mut engine), 0);
    assert_eq!(
        eval(
            &mut engine,
            "[kept.self===kept,kept[Symbol.for('cycle')]===kept,kept.closure===kept[0],kept.owner.childNodes===kept].every(Boolean)"
        ),
        "true"
    );
    eval(&mut engine, "delete globalThis.kept");
    assert_eq!(collect(&document, &mut engine), 2);
}

#[test]
fn hostile_named_visibility_prototype_cycle_terminates() {
    let document = Rc::new(RefCell::new(Document::with_url(
        url::Url::parse("https://example.test/").unwrap(),
    )));
    let mut engine = Engine::with_document(document).unwrap();
    assert_eq!(
        eval(
            &mut engine,
            r#"
        const storage=localStorage;
        storage.setItem('probe','value');
        let cyclic;
        cyclic=new Proxy(Object.create(Storage.prototype),{getPrototypeOf(){return cyclic}});
        Object.setPrototypeOf(storage,cyclic);
        try { storage.probe; 'bad' } catch(error) { error.name }
    "#
        ),
        "TypeError"
    );
    assert_eq!(eval(&mut engine, "21 * 2"), "42");
}

#[test]
fn hostile_prototype_back_to_legacy_receiver_does_not_reenter_descriptor() {
    let document = Rc::new(RefCell::new(Document::with_url(
        url::Url::parse("https://example.test/").unwrap(),
    )));
    let mut engine = Engine::with_document(document).unwrap();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const storage=localStorage;
            storage.setItem('probe','value');
            const cyclic=new Proxy({}, {getPrototypeOf(){return storage}});
            Object.setPrototypeOf(storage,cyclic);
            try { Reflect.getOwnPropertyDescriptor(storage,'probe'); 'bad' }
            catch(error) { error.name }
            "#,
        ),
        "TypeError"
    );
    assert_eq!(eval(&mut engine, "21 * 2"), "42");
}

#[test]
fn mutually_recursive_legacy_prototypes_remain_a_catchable_script_error() {
    let document = Rc::new(RefCell::new(Document::with_url(
        url::Url::parse("https://example.test/").unwrap(),
    )));
    let mut engine = Engine::with_document(document).unwrap();
    engine.set_script_timeout(Some(std::time::Duration::from_secs(1)));
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const local=localStorage, session=sessionStorage;
            local.setItem('probe','local'); session.setItem('probe','session');
            Object.setPrototypeOf(local,new Proxy({}, {getPrototypeOf(){return session}}));
            Object.setPrototypeOf(session,new Proxy({}, {getPrototypeOf(){return local}}));
            try { Reflect.getOwnPropertyDescriptor(local,'probe'); false }
            catch(error) { error instanceof Error }
            "#,
        ),
        "true"
    );
    assert_eq!(eval(&mut engine, "21 * 2"), "42");
}
