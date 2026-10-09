// Included by both engine adapters so Web Storage behavior is checked against
// the same scripts and the same native context/page ownership model.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js::storage::{ContextStorage, PageStorage};
use url::Url;

fn document_at(address: &str) -> Rc<RefCell<Document>> {
    Rc::new(RefCell::new(Document::with_url(
        Url::parse(address).expect("test URL"),
    )))
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine.evaluate(source).expect("Web Storage script")
}

fn collect_detached_storage_nodes(engine: &mut Engine, document: &Rc<RefCell<Document>>) -> usize {
    let mut freed = 0;
    for _ in 0..3 {
        tenta_js::engine::PageEngine::run_microtasks(engine);
        assert!(engine.native_failure().is_none(), "microtask native failure");
        tenta_js::engine::PageEngine::trim_memory(engine);
        assert!(engine.native_failure().is_none(), "GC native failure");
        freed += document.borrow_mut().sweep_detached().nodes_freed;
        tenta_js::engine::PageEngine::trim_memory(engine);
        assert!(engine.native_failure().is_none(), "drain native failure");
    }
    freed
}

#[test]
fn window_storage_methods_and_named_properties_are_live() {
    let mut engine = new_engine_with_storage(
        document_at("https://example.test/page"),
        ContextStorage::new(1024 * 1024).new_page(),
    );
    assert_eq!(
        eval(
            &mut engine,
            "localStorage.setItem('beta','two');localStorage.alpha='one'; \
             const keys=Object.keys(localStorage); \
             [localStorage===window.localStorage,localStorage!==sessionStorage, \
              localStorage instanceof Storage,localStorage.length,localStorage.key(0), \
              localStorage.getItem('alpha'),localStorage.alpha, \
              keys.includes('alpha'),keys.includes('beta'), \
              Object.getOwnPropertyDescriptor(localStorage,'alpha').value, \
              localStorage.getItem('missing')===null].join('|')",
        ),
        "true|true|true|2|alpha|one|one|true|true|one|true"
    );
    assert_eq!(
        eval(
            &mut engine,
            "delete localStorage.alpha;localStorage.removeItem('beta'); \
             [localStorage.length,localStorage.alpha===undefined, \
              localStorage.key(0)===null].join('|')",
        ),
        "0|true|true"
    );
}

#[test]
fn window_storage_getters_require_a_window_receiver() {
    let mut engine = new_engine_with_storage(
        document_at("https://example.test/page"),
        ContextStorage::new(1024 * 1024).new_page(),
    );
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const localGetter = Object.getOwnPropertyDescriptor(window, 'localStorage').get;
            const sessionGetter = Object.getOwnPropertyDescriptor(window, 'sessionStorage').get;
            const throwsTypeError = (getter, receiver) => {
                try { getter.call(receiver); return false; }
                catch (error) { return error instanceof TypeError; }
            };
            [localGetter.call(window) === localStorage,
             sessionGetter.call(window) === sessionStorage,
             localGetter.call(null) === localStorage,
             sessionGetter.call(undefined) === sessionStorage,
             throwsTypeError(localGetter, {}),
             throwsTypeError(sessionGetter, Object.create(window)),
             throwsTypeError(localGetter, Storage.prototype),
             throwsTypeError(localGetter, 0)].join('|')
            "#,
        ),
        "true|true|true|true|true|true|true|true"
    );
}

#[test]
fn window_owns_storage_wrapper_state_without_an_external_script_reference() {
    let mut engine = new_engine_with_storage(
        document_at("https://example.test/page"),
        ContextStorage::new(1024 * 1024).new_page(),
    );
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const marker = Symbol('marker'), getter = Symbol('getter');
            const prototype = Object.create(Storage.prototype);
            localStorage[marker] = 41;
            sessionStorage[marker] = 42;
            Object.defineProperty(localStorage, getter, {get(){return 43}});
            Object.setPrototypeOf(localStorage, prototype);
            [localStorage[marker], sessionStorage[marker], localStorage[getter],
             Object.getPrototypeOf(localStorage) === prototype].join('|')
            "#,
        ),
        "41|42|43|true"
    );
    for _ in 0..3 {
        tenta_js::engine::PageEngine::trim_memory(&mut engine);
    }
    assert_eq!(
        eval(
            &mut engine,
            "[localStorage[marker], sessionStorage[marker], localStorage[getter], \
             Object.getPrototypeOf(localStorage) === prototype, \
             !Reflect.deleteProperty(localStorage, getter)].join('|')",
        ),
        "41|42|43|true|true"
    );
}

#[test]
fn changing_origin_releases_old_window_storage_roots_but_respects_script_roots() {
    let document = document_at("https://example.test/first");
    let mut engine = new_engine_with_storage(
        document.clone(),
        ContextStorage::new(1024 * 1024).new_page(),
    );
    eval(
        &mut engine,
        "(()=>{localStorage[Symbol('node')]=document.createElement('div')})()",
    );
    assert_eq!(document.borrow().len(), 2);
    document.borrow_mut().set_url(Some(
        Url::parse("https://other.test/second").expect("second origin"),
    ));
    eval(&mut engine, "void localStorage");
    assert_eq!(collect_detached_storage_nodes(&mut engine, &document), 1);
    assert_eq!(document.borrow().len(), 1);

    eval(
        &mut engine,
        "globalThis.oldStorage=localStorage;oldStorage[Symbol('node')]=document.createElement('div')",
    );
    document.borrow_mut().set_url(Some(
        Url::parse("https://third.test/third").expect("third origin"),
    ));
    eval(&mut engine, "void localStorage");
    assert_eq!(collect_detached_storage_nodes(&mut engine, &document), 0);
    eval(&mut engine, "globalThis.oldStorage=undefined");
    assert_eq!(collect_detached_storage_nodes(&mut engine, &document), 1);
    assert_eq!(document.borrow().len(), 1);
}

#[test]
fn storage_named_properties_follow_webidl_descriptors_and_receiver_rules() {
    let mut engine = new_engine_with_storage(
        document_at("https://example.test/page"),
        ContextStorage::new(1024 * 1024).new_page(),
    );
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const area = localStorage;
            const get = Storage.prototype.getItem;
            area.getItem = 'stored collision';
            const collision = typeof area.getItem === 'function' &&
                get.call(area, 'getItem') === 'stored collision' &&
                !Object.hasOwn(area, 'getItem') &&
                !Object.keys(area).includes('getItem') &&
                Reflect.deleteProperty(area, 'getItem') &&
                get.call(area, 'getItem') === 'stored collision';

            area.native = 'native value';
            const derived = Object.create(area);
            const receiver = Reflect.set(area, 'native', 'derived value', derived) &&
                Object.hasOwn(derived, 'native') && derived.native === 'derived value' &&
                area.native === 'native value';

            const defined = Reflect.defineProperty(area, 'defined', {value: 17});
            const descriptor = Object.getOwnPropertyDescriptor(area, 'defined');
            const virtual = defined && area.defined === '17' && descriptor.value === '17' &&
                descriptor.configurable && descriptor.enumerable && descriptor.writable &&
                Reflect.defineProperty(area, 'undefinedValue', {writable: false}) &&
                area.undefinedValue === 'undefined';
            const rejected = !Reflect.defineProperty(area, 'accessor', {get(){return 1}}) &&
                !Reflect.defineProperty(area, 'generic', {configurable: true}) &&
                get.call(area, 'accessor') === null && get.call(area, 'generic') === null;
            const deleted = Reflect.deleteProperty(area, 'defined') &&
                !Object.hasOwn(area, 'defined') && get.call(area, 'defined') === null;

            const symbol = Symbol('ordinary');
            Object.defineProperty(area, symbol, {value: 19});
            const ordinary = area[symbol] === 19 && !Reflect.set(area, symbol, 20) &&
                !Reflect.deleteProperty(area, symbol) &&
                !Object.getOwnPropertyDescriptor(area, symbol).configurable;
            let frozen = false;
            try { Object.freeze(area); } catch(error) { frozen = error instanceof TypeError; }
            [collision, receiver, virtual, rejected, deleted, ordinary,
             !Reflect.preventExtensions(area), Object.isExtensible(area), frozen].join('|')
            "#,
        ),
        "true|true|true|true|true|true|true|true|true"
    );
}

#[test]
fn storage_own_keys_preserve_supported_name_order_before_symbols() {
    let mut engine = new_engine_with_storage(
        document_at("https://example.test/page"),
        ContextStorage::new(1024 * 1024).new_page(),
    );
    let observed = eval(
        &mut engine,
        r#"
            const storage=localStorage, symbol=Symbol('ordinary');
            for (const name of ['10','2','alpha']) storage.setItem(name,name);
            storage[symbol]=41;
            const supported=Array.from({length:storage.length},(_,index)=>storage.key(index));
            const keys=Reflect.ownKeys(storage);
            [keys.length===supported.length+1,
             supported.every((name,index)=>keys[index]===name),
             keys.at(-1)===symbol].join('|')+';'+
             JSON.stringify({supported,keys:keys.map(key=>
               typeof key==='symbol'?'symbol:'+key.description:'string:'+key)})
            "#,
    );
    assert!(
        observed.starts_with("true|true|true;"),
        "Storage ownKeys order: {observed}"
    );
}

#[test]
fn canonical_numeric_storage_names_use_named_property_operations() {
    let mut engine = new_engine_with_storage(
        document_at("https://example.test/page"),
        ContextStorage::new(1024 * 1024).new_page(),
    );
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const area=localStorage;
            area['10']='x';
            const assigned=area.getItem('10')==='x' && area[10]==='x';
            const defined=Reflect.defineProperty(area,'2',{value:'y'}) &&
              area.getItem('2')==='y' && area[2]==='y';
            const descriptor=Object.getOwnPropertyDescriptor(area,'2');
            const described=descriptor && descriptor.value==='y' &&
              descriptor.configurable && descriptor.enumerable && descriptor.writable;
            const deleted=Reflect.deleteProperty(area,'10') &&
              area.getItem('10')===null && area[10]===undefined;
            const listed=Object.keys(area).includes('2') &&
              Reflect.ownKeys(area).includes('2');
            [assigned,defined,described,deleted,listed].join('|')
            "#,
        ),
        "true|true|true|true|true"
    );
}

#[test]
fn local_storage_is_context_shared_but_sessions_and_origins_are_isolated() {
    let context = ContextStorage::new(1024 * 1024);
    let mut first = new_engine_with_storage(
        document_at("https://example.test/first"),
        context.new_page(),
    );
    let mut same_origin = new_engine_with_storage(
        document_at("https://example.test/second"),
        context.new_page(),
    );
    let mut other_origin =
        new_engine_with_storage(document_at("https://other.test/page"), context.new_page());
    eval(
        &mut first,
        "localStorage.setItem('shared','yes');sessionStorage.setItem('private','first')",
    );
    assert_eq!(
        eval(
            &mut same_origin,
            "[localStorage.getItem('shared'), \
              sessionStorage.getItem('private')===null].join('|')",
        ),
        "yes|true"
    );
    assert_eq!(
        eval(
            &mut other_origin,
            "[localStorage.getItem('shared')===null, \
              sessionStorage.getItem('private')===null].join('|')",
        ),
        "true|true"
    );
    eval(&mut same_origin, "localStorage.shared='updated'");
    assert_eq!(eval(&mut first, "localStorage.shared"), "updated");
}

#[test]
fn opaque_and_closed_storage_throw_security_error_and_quota_is_atomic() {
    let mut opaque = new_engine_with_storage(
        Rc::new(RefCell::new(Document::new())),
        PageStorage::isolated(),
    );
    assert_eq!(
        eval(
            &mut opaque,
            "try { localStorage } catch (error) { error.name }",
        ),
        "SecurityError"
    );

    let context = ContextStorage::new(1);
    let page = context.new_page();
    let mut limited = new_engine_with_storage(document_at("https://quota.test/page"), page.clone());
    assert_eq!(
        eval(
            &mut limited,
            "const area=localStorage; \
             let result='none';try { area.setItem('x','y') } \
             catch(error) { result=error.name } \
             [result,area.length,area.getItem('x')===null].join('|')",
        ),
        "QuotaExceededError|0|true"
    );
    page.close_session();
    assert_eq!(
        eval(
            &mut limited,
            "try { area.length } catch (error) { error.name }",
        ),
        "SecurityError"
    );
    assert_eq!(
        eval(
            &mut limited,
            "try { localStorage } catch (error) { error.name }",
        ),
        "SecurityError"
    );
}
