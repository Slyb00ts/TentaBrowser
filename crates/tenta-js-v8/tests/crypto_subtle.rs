//! `crypto.subtle` and `SubtleCrypto.digest()` on the V8 host
//! (`tenta_js::subtle`).

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

/// What every test reports with: `hex()` of a buffer, and `settle()`, which
/// records how a promise ended in `out[label]`.
const HELPERS: &str = "globalThis.out = {}; \
     function hex(buffer) { \
       return Array.prototype.map.call(new Uint8Array(buffer), function (byte) { \
         return (byte < 16 ? '0' : '') + byte.toString(16); \
       }).join(''); \
     } \
     function settle(label, promise) { \
       promise.then(function (value) { out[label] = hex(value); }, \
                    function (error) { out[label] = error.name; }); \
     } \
     function bytes(text) { return new TextEncoder().encode(text); } \
     'ready'";

fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some("https://example.com/".parse().expect("test URL")));
    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    engine.install_preludes();
    assert_eq!(eval(&mut engine, HELPERS), "ready");
    engine
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

/// Runs `source`, then the tasks it queued, and answers `out` as JSON.
fn settled(engine: &mut Engine, source: &str) -> String {
    eval(engine, source);
    engine.run_event_loop(16);
    eval(engine, "JSON.stringify(out)")
}

#[test]
fn crypto_has_one_subtle() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var d = Object.getOwnPropertyDescriptor(Crypto.prototype, 'subtle'); \
             function thrown(f) { try { f(); return 'none'; } catch (e) { return e.name; } } \
             [crypto.subtle === crypto.subtle, crypto.subtle instanceof SubtleCrypto, \
              Object.prototype.toString.call(crypto.subtle), typeof SubtleCrypto, \
              typeof d.get, d.set, d.enumerable, d.configurable, \
              thrown(function () { new SubtleCrypto(); }), \
              thrown(function () { d.get.call({}); }), \
              thrown(function () { d.get.call(Object.create(Crypto.prototype)); }), \
              typeof crypto.subtle.digest, crypto.subtle.digest.length, \
              Object.keys(SubtleCrypto.prototype).join(), \
              'importKey' in crypto.subtle, typeof CryptoKey].join('|')",
        ),
        "true|true|[object SubtleCrypto]|function|function||true|true|TypeError|TypeError|\
         TypeError|function|2|digest|false|undefined"
    );
}

/// FIPS 180-4's example message under each algorithm.
#[test]
fn digest_hashes_with_each_algorithm() {
    let mut engine = engine();
    assert_eq!(
        settled(
            &mut engine,
            "['SHA-1', 'SHA-256', 'SHA-384', 'SHA-512'].forEach(function (name) { \
               settle(name, crypto.subtle.digest(name, bytes('abc'))); \
             });",
        ),
        "{\"SHA-1\":\"a9993e364706816aba3e25717850c26c9cd0d89d\",\
          \"SHA-256\":\"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\",\
          \"SHA-384\":\"cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed\
                        8086072ba1e7cc2358baeca134c825a7\",\
          \"SHA-512\":\"ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
                        2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f\"}"
            .replace(char::is_whitespace, "")
    );
}

#[test]
fn the_algorithm_is_a_name_or_an_object_with_one_in_any_case() {
    let mut engine = engine();
    assert_eq!(
        settled(
            &mut engine,
            "settle('object', crypto.subtle.digest({ name: 'SHA-1' }, bytes('abc'))); \
             settle('lower', crypto.subtle.digest('sha-1', bytes('abc'))); \
             settle('mixed', crypto.subtle.digest({ name: 'Sha-1', length: 7 }, bytes('abc'))); \
             settle('stringified', crypto.subtle.digest( \
               { name: { toString: function () { return 'SHA-1'; } } }, bytes('abc')));",
        ),
        "{\"object\":\"a9993e364706816aba3e25717850c26c9cd0d89d\",\
          \"lower\":\"a9993e364706816aba3e25717850c26c9cd0d89d\",\
          \"mixed\":\"a9993e364706816aba3e25717850c26c9cd0d89d\",\
          \"stringified\":\"a9993e364706816aba3e25717850c26c9cd0d89d\"}"
            .replace(char::is_whitespace, "")
    );
}

/// Every spelling of a `BufferSource` gives the bytes it covers, and no more.
#[test]
fn the_data_is_what_the_buffer_source_covers() {
    let mut engine = engine();
    assert_eq!(
        settled(
            &mut engine,
            "var whole = new Uint8Array([120, 97, 98, 99, 121]); \
             settle('buffer', crypto.subtle.digest('SHA-1', bytes('abc').buffer)); \
             settle('window', crypto.subtle.digest('SHA-1', new Uint8Array(whole.buffer, 1, 3))); \
             settle('view', crypto.subtle.digest('SHA-1', new DataView(whole.buffer, 1, 3))); \
             settle('wide', crypto.subtle.digest('SHA-1', new Uint16Array([0x6261, 0x6463]))); \
             settle('empty', crypto.subtle.digest('SHA-1', new ArrayBuffer(0)));",
        ),
        "{\"buffer\":\"a9993e364706816aba3e25717850c26c9cd0d89d\",\
          \"window\":\"a9993e364706816aba3e25717850c26c9cd0d89d\",\
          \"view\":\"a9993e364706816aba3e25717850c26c9cd0d89d\",\
          \"wide\":\"81fe8bfe87576c3ecb22426f8e57847382917acf\",\
          \"empty\":\"da39a3ee5e6b4b0d3255bfef95601890afd80709\"}"
            .replace(char::is_whitespace, "")
    );
}

/// A megabyte and a bit: more than one piece of the host's hashing loop.
#[test]
fn a_buffer_larger_than_one_piece_hashes_as_a_whole() {
    let mut engine = engine();
    assert_eq!(
        settled(
            &mut engine,
            "var big = new Uint8Array((1 << 20) + 17); \
             for (var i = 0; i < big.length; i++) big[i] = i % 251; \
             settle('big', crypto.subtle.digest('SHA-256', big));",
        ),
        "{\"big\":\"f4bf9b8dec6e3e28b2ec33266145d244fe97a6bce1ec054ace053bab67d2ef9a\"}"
    );
}

/// The promise is resolved from a task, not during the call: a reaction
/// attached at once still runs after a microtask queued later.
#[test]
fn the_promise_resolves_in_a_later_task_with_an_array_buffer() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "globalThis.order = []; \
             var p = crypto.subtle.digest('SHA-256', bytes('abc')); \
             p.then(function (value) { \
               order.push('digest:' + (value instanceof ArrayBuffer) + ':' + value.byteLength); \
             }); \
             Promise.resolve().then(function () { order.push('microtask'); }); \
             order.push('script'); \
             [p instanceof Promise, order.join()].join('|')",
        ),
        "true|script"
    );
    engine.run_event_loop(16);
    assert_eq!(
        eval(&mut engine, "order.join()"),
        "script,microtask,digest:true:32"
    );
}

/// What goes wrong rejects the promise; the call itself never throws.
#[test]
fn every_failure_is_a_rejection() {
    let mut engine = engine();
    assert_eq!(
        settled(
            &mut engine,
            "globalThis.threw = 'nothing'; \
             var digest = crypto.subtle.digest; \
             try { \
               settle('unknown', crypto.subtle.digest('SHA-224', bytes('abc'))); \
               settle('md5', crypto.subtle.digest({ name: 'MD5' }, bytes('abc'))); \
               settle('null', crypto.subtle.digest(null, bytes('abc'))); \
               settle('nameless', crypto.subtle.digest({}, bytes('abc'))); \
               settle('symbol', crypto.subtle.digest(Symbol('SHA-1'), bytes('abc'))); \
               settle('getter', crypto.subtle.digest( \
                 { get name() { throw new RangeError('no'); } }, bytes('abc'))); \
               settle('string', crypto.subtle.digest('SHA-1', 'abc')); \
               settle('array', crypto.subtle.digest('SHA-1', [97, 98, 99])); \
               settle('missing', crypto.subtle.digest('SHA-1')); \
               settle('none', crypto.subtle.digest()); \
               settle('receiver', digest.call({}, 'SHA-1', bytes('abc'))); \
               settle('prototype', digest.call(SubtleCrypto.prototype, 'SHA-1', bytes('abc'))); \
             } catch (error) { threw = error.name; } \
             out.threw = threw;",
        ),
        "{\"threw\":\"nothing\",\"unknown\":\"NotSupportedError\",\"md5\":\"NotSupportedError\",\
          \"null\":\"NotSupportedError\",\"nameless\":\"TypeError\",\"symbol\":\"TypeError\",\
          \"getter\":\"RangeError\",\"string\":\"TypeError\",\"array\":\"TypeError\",\
          \"missing\":\"TypeError\",\"none\":\"TypeError\",\"receiver\":\"TypeError\",\
          \"prototype\":\"TypeError\"}"
            .replace(char::is_whitespace, "")
    );
}

/// The data is copied before the algorithm's `name` is read and before the
/// call returns, so nothing the page does afterwards changes the digest.
#[test]
fn the_data_is_copied_before_script_can_change_it() {
    let mut engine = engine();
    assert_eq!(
        settled(
            &mut engine,
            "var during = bytes('abc'), after = bytes('abc'); \
             settle('during', crypto.subtle.digest( \
               { get name() { during[0] = 0; return 'SHA-1'; } }, during)); \
             settle('after', crypto.subtle.digest('SHA-1', after)); \
             after.fill(0); \
             out.changed = during[0] === 0;",
        ),
        "{\"changed\":true,\"during\":\"a9993e364706816aba3e25717850c26c9cd0d89d\",\
          \"after\":\"a9993e364706816aba3e25717850c26c9cd0d89d\"}"
            .replace(char::is_whitespace, "")
    );
}

/// The rejection is a real `DOMException` of the page's realm.
#[test]
fn an_unknown_algorithm_rejects_with_a_dom_exception() {
    let mut engine = engine();
    eval(
        &mut engine,
        "crypto.subtle.digest('whirlpool', bytes('abc')).catch(function (error) { \
           out.error = [error instanceof DOMException, error.name, error.code].join(':'); \
         });",
    );
    engine.run_event_loop(16);
    assert_eq!(eval(&mut engine, "out.error"), "true:NotSupportedError:9");
}

/// Data the page's native memory budget cannot hold a copy of is that call's
/// failure: the page goes on, and nothing stays charged.
#[test]
fn data_beyond_the_memory_budget_is_refused_without_stopping_the_page() {
    use tenta_core::{NativeMemoryBudget, NativeMemoryComponent};
    use tenta_js::event_loop::{Clock, MonoTime};
    use tenta_js::storage::PageStorage;
    use tenta_net::CookieJar;

    let budget = NativeMemoryBudget::with_limits(1 << 20, 1 << 20, 1 << 20, 64);
    let mut engine = Engine::with_document_on_clock_storage_and_memory(
        Rc::new(RefCell::new(Document::new())),
        Clock::virtual_at(MonoTime::ZERO),
        None,
        CookieJar::new(),
        PageStorage::isolated(),
        budget.clone(),
    );
    engine.install_preludes();
    assert_eq!(eval(&mut engine, HELPERS), "ready");
    assert_eq!(
        settled(
            &mut engine,
            "settle('fits', crypto.subtle.digest('SHA-1', new Uint8Array(64))); \
             settle('over', crypto.subtle.digest('SHA-1', new Uint8Array(65))); \
             settle('after', crypto.subtle.digest('SHA-1', bytes('abc')));",
        ),
        "{\"over\":\"OperationError\",\
          \"fits\":\"c8d7d0ef0eedfa82d2ea1aa592845b9a6d4b02b7\",\
          \"after\":\"a9993e364706816aba3e25717850c26c9cd0d89d\"}"
            .replace(char::is_whitespace, "")
    );
    assert_eq!(budget.first_failure(), None);
    assert_eq!(budget.used(NativeMemoryComponent::WebCrypto), 0);
    assert_eq!(budget.total_used(), 0);
}
