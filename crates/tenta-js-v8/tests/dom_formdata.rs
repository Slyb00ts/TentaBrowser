//! Tests for `FormData` (P2 slice `d-4b`'s `b-8`), the first host record whose
//! lifetime runs in **both** directions and the first interface that *mints*
//! another interface's record.
//!
//! Three things make this file different from `dom_blob.rs`:
//!
//! 1. A `FormData` entry holds a `File` **record key**, not a copy of its bytes,
//!    because the standard's value type is `(File or USVString)` and a browser
//!    keeps the `File` object the page passed. Two consequences are pinned here:
//!    the identity of a value read back twice (`wpt/checkout/xhr/formdata/
//!    set-blob.any.js`'s `assert_file_identity`), and the record surviving the
//!    page's own last reference to it, which is what the `File` record's owner
//!    list is for.
//! 2. `append` and `set` **create** a `File` from a `Blob` (HTML's *create an
//!    entry*), so the glue has to be able to wrap a record the page never
//!    constructed — which is why `File.prototype` has to be in the minted
//!    object's chain, and why `install_form_data` follows `install_file`.
//! 3. Iteration is a **snapshot**, because every landed pair `iterable` is one:
//!    the generated `@@iterator` views one `iterable_entries()` call. WebIDL's
//!    default iterator is live, and `wpt/checkout/xhr/formdata/iteration.any.js`
//!    requires that, so the last section of this file pins the snapshot — the
//!    test a later live-iterator slice has to come back and flip.
//!
//! The declaration this implements is `idl/xhr/FormData.webidl`, whose header
//! records its deviations; `docs/dev/idl.md` carries the consumer-facing
//! copy.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// A document with `<html><body><p></p></body></html>`.
///
/// The `<body>` provides a node wrapper for the constructor and union brand
/// checks.
fn document() -> Rc<RefCell<Document>> {
    let mut d = Document::new();
    let html = d.create_html_element(atoms::HTML);
    let body = d.create_html_element(atoms::BODY);
    let p = d.create_html_element(atoms::P);
    for (parent, child) in [(d.document_node(), html), (html, body), (body, p)] {
        d.mutate(Mutation::Append { parent, child })
            .expect("append");
    }
    Rc::new(RefCell::new(d))
}

/// Runs `source` and returns its completion value.
fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(document());
    engine
        .evaluate(source)
        .expect("script should evaluate without throwing")
}

/// Runs each of the `run`s in `source` and collects what it returned or threw.
///
/// The error half reads `e.name` rather than `e.constructor.name`, because a
/// `DOMException` reports its own name there and its class would hide the
/// specific refusal.
fn caught(source: &str) -> String {
    eval(&format!(
        r#"
        const attempt = run => {{
            try {{ return 'ok:' + run(); }}
            catch (e) {{ return 'threw:' + e.name; }}
        }};
        const results = [];
        {source}
        results.join('|');
        "#
    ))
}

/// Sweeps until no wrapper's record is left unreclaimed: V8 collects a wrapper
/// on a low-memory notification, and the world applies its finalizers on the
/// next sweep, so one pass of each is not enough. The same five-round loop
/// `dom_response.rs` uses.
fn sweep(engine: &mut Engine) {
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
}

// ---------------------------------------------------------------------------
// Installation and construction
// ---------------------------------------------------------------------------

#[test]
fn form_data_is_installed_and_its_constructor_rejects_wrong_types() {
    // `FormData.length` is 0 because the standard's two parameters are both
    // optional, which is also what this engine's constructors report for every
    // interface — the two agree here, and `dom_blob.rs` records the case where
    // they do not.
    assert_eq!(
        eval(
            r#"
            [
                globalThis.FormData === FormData,
                typeof FormData,
                FormData.name,
                FormData.length,
                Object.getPrototypeOf(FormData.prototype) === Object.prototype,
                new FormData().constructor.name,
                new FormData().has('x'),
            ].join('|');
            "#
        ),
        "true|function|FormData|0|true|FormData|false"
    );
}

#[test]
fn constructing_from_a_form_is_refused_by_name() {
    // `wpt/checkout/xhr/formdata/constructor.any.js`'s "Constructors should
    // throw a type error": only an actual `HTMLFormElement` may select the
    // interface argument. This slice does not install that interface, so null,
    // primitives, ordinary objects and unrelated node wrappers all fail during
    // argument conversion.
    assert_eq!(
        caught(
            r#"
            results.push(attempt(() => new FormData().has('x')));
            results.push(attempt(() => new FormData(null).has('x')));
            results.push(attempt(() => new FormData("string").has('x')));
            results.push(attempt(() => new FormData({}).has('x')));
            results.push(attempt(() => new FormData(document.body).has('x')));
            "#
        ),
        "ok:false|threw:TypeError|threw:TypeError|threw:TypeError|threw:TypeError"
    );
}

// ---------------------------------------------------------------------------
// The entry list
// ---------------------------------------------------------------------------

#[test]
fn append_get_get_all_has_delete_and_set_operate_on_the_entry_list() {
    // One assertion per operation, in the order the corpus states them:
    // `append.any.js`'s "testFormDataAppend1"/"testFormDataAppend2" (an append
    // never replaces — `get` answers with the *first* entry named), `get.any.js`
    // ("testFormDataGetNull1"/"testFormDataGetAll"), `has.any.js`,
    // `delete.any.js`'s "testFormDataDelete", and `set.any.js`'s
    // "testFormDataSet2", where `set` replaces the first entry in place and
    // removes the rest.
    assert_eq!(
        eval(
            r#"
            const fd = new FormData();
            fd.append('key', 'value1');
            fd.append('key', 'value2');
            const out = [
                fd.get('key'),
                fd.getAll('key').join(','),
                fd.has('key'),
                fd.has('nil'),
                fd.get('nil') === null,
                fd.getAll('nil').length,
            ];
            fd.delete('key');
            out.push(fd.get('key') === null, fd.has('key'));
            fd.set('key', 'value1');
            fd.append('key', 'value2');
            fd.set('key', 'value3');
            out.push(fd.getAll('key').join(','), fd.get('key'));
            out.join('|');
            "#
        ),
        "value1|value1,value2|true|false|true|0|true|false|value3|value3"
    );
}

#[test]
fn a_string_value_is_stringified_from_undefined_and_null() {
    // `append.any.js`'s "testFormDataAppendUndefined1"/"testFormDataAppendNull1"
    // and `set.any.js`'s "testFormDataSetUndefined1"/"testFormDataSetNull1": both
    // reach the union's `USVString` fallback, whose conversion is
    // `string_from_v8` — `ToAString` on a non-object — so `undefined` and `null`
    // are the *strings* `"undefined"` and `"null"` rather than an empty value or
    // a refusal.
    //
    // The two `*2` subtests in the same files pin that an earlier entry with the
    // same name is still not replaced by an append.
    assert_eq!(
        eval(
            r#"
            const fd = new FormData();
            fd.append('key', undefined);
            const out = [fd.get('key'), fd.get('key') === 'undefined'];
            fd.append('key', 'value1');
            out.push(fd.get('key'));
            fd.set('key', null);
            out.push(fd.get('key'), fd.getAll('key').length);
            fd.set('key', 'value1');
            fd.set('key', undefined);
            out.push(fd.get('key'));
            out.join('|');
            "#
        ),
        "undefined|true|undefined|null|1|undefined"
    );
}

// ---------------------------------------------------------------------------
// Values: identity, minting and union brands
// ---------------------------------------------------------------------------

#[test]
fn a_file_argument_becomes_the_entry_itself() {
    // HTML's *create an entry* replaces the value only when a `filename` is
    // given or the value is not a `File`, so `append('k', file)` and
    // `set('k', file)` store *that* object. `set-blob.any.js`'s
    // `assert_file_identity` is the contract: `get`, `getAll()[0]` and the pair
    // iterator must all answer with the same object — which they do because the
    // entry holds the record's key and the wrapper world memoises one wrapper
    // per key.
    assert_eq!(
        eval(
            r#"
            const file = new File(['hello'], 'hello.txt', { type: 'text/plain' });
            const fd = new FormData();
            fd.append('k', file);
            fd.append('k2', file);
            fd.set('k3', file);
            const entry = Array.from(fd).find(([name]) => name === 'k2');
            [
                fd.get('k') === file,
                fd.getAll('k')[0] === file,
                entry[1] === file,
                fd.get('k3') === file,
                String([fd.get('k').name, fd.get('k').size, fd.get('k').type]),
                fd.get('k') instanceof File,
            ].join('|');
            "#
        ),
        "true|true|true|true|hello.txt,5,text/plain|true"
    );
}

#[test]
fn a_blob_argument_mints_a_file_named_blob() {
    // `set-blob.any.js`'s "blob without type" and "blob with type": a `Blob`
    // value is never a `File`, so *create an entry* mints one named `"blob"`,
    // copying the bytes and the type and taking the clock for `lastModified`.
    // The minted object must be a `File` *and* a `Blob` — `File.prototype`'s
    // chain reaches `Blob.prototype` — because `set.any.js`'s
    // "testFormDataSetEmptyBlob" asserts `file instanceof File` on exactly this
    // object, and the two reads of the entry must be one object.
    assert_eq!(
        eval(
            r#"
            const fd = new FormData();
            fd.set('k', new Blob([]));
            fd.append('typed', new Blob(['abc'], { type: 'Text/Plain' }));
            const minted = fd.get('k');
            [
                minted instanceof File,
                minted instanceof Blob,
                minted.constructor.name,
                minted.name,
                minted.size,
                minted.type,
                fd.get('typed').name,
                fd.get('typed').size,
                fd.get('typed').type,
                fd.get('k') === minted,
                Math.abs(minted.lastModified - Date.now()) < 200,
            ].join('|');
            "#
        ),
        "true|true|File|blob|0||blob|3|text/plain|true|true"
    );
}

#[test]
fn a_filename_mints_a_file_copying_the_sources_last_modified() {
    // Two rows of the case table, both from `set-blob.any.js`:
    //
    //  - "blob with custom name": a `Blob` and a `filename` mint a `File` whose
    //    name is the `filename` and whose `lastModified` is the clock;
    //  - "file with lastModified and custom name": a `File` and a `filename`
    //    mint a *different* object — `assert_not_equals(file2, value)` — that
    //    keeps the source's `lastModified` (`assert_equals(file2.lastModified,
    //    123)`) and takes the `filename` as its name.
    //
    // `append.any.js`'s "testFormDataAppendEmptyBlob" is the third row and the
    // only one with a two-second window instead of 200 ms; the same clock
    // satisfies both.
    assert_eq!(
        eval(
            r#"
            const fd = new FormData();
            fd.append('blob', new Blob(['bytes'], { type: 'text/plain' }), 'blank.txt');
            fd.set('file', new File([], 'name', { lastModified: 123 }), 'custom name');
            const blob = fd.get('blob');
            const file = fd.get('file');
            [
                blob.name, blob.size, blob.type,
                blob.lastModified >= Date.now() - 2000 && blob.lastModified <= Date.now(),
                file.name, file.lastModified, file.size,
                fd.getAll('file').length,
            ].join('|');
            "#
        ),
        "blank.txt|5|text/plain|true|custom name|123|0|1"
    );
}

#[test]
fn objects_without_the_blob_brand_use_the_string_arm() {
    // The `(Blob or USVString)` conversion selects its interface arm only for
    // actual Blob wrappers. Other platform wrappers and ordinary objects are
    // converted to strings before `append` or `set` stores the entry.
    assert_eq!(
        eval(
            r#"
            const values = [
                new URL('https://example.com/'),
                new Headers(),
                document.body,
                {},
                new Date(0),
            ];
            const expected = values.map(value => String(value));
            const fd = new FormData();
            values.forEach((value, index) => fd.append(String(index), value));
            fd.set('set', values[0]);
            const actual = values.map((_, index) => fd.get(String(index)));
            [
                actual.every((value, index) => value === expected[index]),
                actual[0],
                actual[1],
                actual[2],
                actual[3],
                fd.get('set'),
            ].join('|');
            "#
        ),
        "true|https://example.com/|[object Headers]|[object HTMLElement]|\
         [object Object]|https://example.com/"
    );
}

// ---------------------------------------------------------------------------
// Iteration
// ---------------------------------------------------------------------------

#[test]
fn iteration_views_a_snapshot_so_a_mutation_during_it_is_not_observed() {
    // **This pins a defect, and the slice that fixes it must flip this test.**
    //
    // WebIDL's default iterator for `iterable<K, V>` is *live*: it walks the
    // entry list as it stands at each `next()`. `wpt/checkout/xhr/formdata/
    // iteration.any.js` requires exactly that, in its three subtests "Iteration
    // skips elements removed while iterating", "Removing elements already
    // iterated over causes an element to be skipped during iteration" and
    // "Appending a value pair during iteration causes it to be reached during
    // iteration".
    //
    // This engine's pair iterables are snapshots — the generated
    // `pair_iterator_callback` calls the host's `iterable_entries()` once and
    // builds an array from it, which `@@iterator`, `entries`, `keys` and
    // `values` all view — so all three subtests fail here, and no `FormData`
    // implementation can change that: the shared generator path would have to
    // grow a live iterator object per pair `iterable`, which is a slice of its
    // own (`docs/dev/idl.md` records the same consequence for the pair form
    // generally).
    //
    // The two assertions below are the *snapshot* answers for the two WPT
    // subtests that mutate during iteration: the deleted entry is still walked,
    // and the appended one is not reached. A live iterator gives
    // `foo,BAR` / `foo,BAR` and `foo,baz,BAR,quux,X-yZ`.
    assert_eq!(
        eval(
            r#"
            const removed = new FormData();
            for (const [name, value] of [['foo', '0'], ['baz', '1'], ['BAR', '2']]) {
                removed.append(name, value);
            }
            const removedKeys = [];
            for (const [name] of removed) {
                removedKeys.push(name);
                removed.delete('baz');
            }

            const appended = new FormData();
            for (const [name, value] of [['foo', '0'], ['baz', '1'], ['BAR', '2'], ['quux', '3']]) {
                appended.append(name, value);
            }
            const appendedKeys = [];
            for (const [name] of appended) {
                appendedKeys.push(name);
                if (name === 'baz') {
                    appended.append('X-yZ', '4');
                }
            }
            [removedKeys.join(','), appendedKeys.join(',')].join('|');
            "#
        ),
        "foo,baz,BAR|foo,baz,BAR,quux"
    );
}

#[test]
fn the_iterators_agree_with_each_other_and_see_duplicate_keys() {
    // `foreach.any.js`'s four subtests: `@@iterator` and `entries()` yield the
    // same pairs (they are the very same function object, which is why the
    // identity holds), `keys()` sees duplicates, `values()` sees the values that
    // were not deleted, and a `File` value read through `values()` is the object
    // the page passed in.
    assert_eq!(
        eval(
            r#"
            const fd = new FormData();
            fd.append('n1', 'v1');
            fd.append('n2', 'v2');
            fd.append('n1', 'v3');
            fd.delete('n2');
            const file = new File(['hello'], 'hello.txt');
            fd.append('f1', file);
            const show = entry => entry[0] + ':' + (entry[1] instanceof File ? entry[1].name : entry[1]);
            const pairs = [...fd].map(show).join(',');
            const entries = [...fd.entries()].map(show).join(',');
            const keys = [...fd.keys()].join(',');
            const values = [...fd.values()];
            [
                pairs,
                entries,
                keys,
                values[0], values[1],
                values[2] === file,
                FormData.prototype[Symbol.iterator] === FormData.prototype.entries,
            ].join('|');
            "#
        ),
        "n1:v1,n1:v3,f1:hello.txt|n1:v1,n1:v3,f1:hello.txt|n1,n1,f1|v1|v3|true|true"
    );
}

// ---------------------------------------------------------------------------
// Ownership: the two-way link between an entry list and the files it names
// ---------------------------------------------------------------------------

#[test]
fn an_entry_list_keeps_the_file_its_entries_name() {
    // The half of the protocol a wrapper-only rule would get wrong: the page
    // drops every reference to the `File` it appended — the wrapper is finalised
    // and its record's `has_wrapper` goes back to false — and the entry list
    // still has to answer with that record, because a browser's entry list holds
    // the *object* and keeps it uncollectable.
    let mut engine = Engine::with_document(document());
    let baseline = engine.host_record_count();
    engine
        .evaluate(
            r#"
            globalThis.fd = new FormData();
            (() => {
                const file = new File(['hello'], 'hello.txt', { type: 'text/plain' });
                fd.append('k', file);
            })();
        "#,
        )
        .expect("evaluate the append");
    sweep(&mut engine);
    assert_eq!(
        engine.host_record_count(),
        baseline + 2,
        "the entry list and the file record it holds, whose wrapper is gone"
    );
    assert_eq!(
        engine
            .evaluate(
                r#"
                (() => {
                    const file = fd.get('k');
                    return [file.name, file.size, file.type, file instanceof File].join('|');
                })();
                "#
            )
            .expect("the entry is still readable"),
        "hello.txt|5|text/plain|true",
        "the record outlived the page's own reference to it"
    );

    // And it is freed once the entry list that held it is gone: the FormData's
    // wrapper is the page's last reference to anything that can reach it.
    engine
        .evaluate("globalThis.fd = null;")
        .expect("evaluate the release");
    sweep(&mut engine);
    assert_eq!(engine.host_record_count(), baseline);
}

#[test]
fn one_file_can_be_owned_by_two_entry_lists() {
    // Why the owner link is a `Vec<HostKey>` and not an `Option<HostKey>`: one
    // `File` object may be appended to any number of `FormData`s, and each has
    // to be able to let go of it independently. The record survives until the
    // last of them is gone, whichever order they are released in.
    let mut engine = Engine::with_document(document());
    let baseline = engine.host_record_count();
    engine
        .evaluate(
            r#"
            globalThis.a = new FormData();
            globalThis.b = new FormData();
            (() => {
                const file = new File(['x'], 'x.txt');
                a.append('k', file);
                b.append('k', file);
            })();
        "#,
        )
        .expect("evaluate the two appends");
    sweep(&mut engine);
    assert_eq!(
        engine.host_record_count(),
        baseline + 3,
        "two entry lists and the one file both of them hold"
    );

    engine
        .evaluate("globalThis.a = null;")
        .expect("evaluate the first release");
    sweep(&mut engine);
    assert_eq!(
        engine.host_record_count(),
        baseline + 2,
        "the survivor still holds the file, so it is not freed with its other owner"
    );
    assert_eq!(
        engine
            .evaluate("b.get('k').name")
            .expect("the surviving entry is still readable"),
        "x.txt"
    );

    engine
        .evaluate("globalThis.b = null;")
        .expect("evaluate the second release");
    sweep(&mut engine);
    assert_eq!(engine.host_record_count(), baseline);
}

#[test]
fn a_collected_entry_list_frees_the_files_it_minted() {
    // The allocation shape a page really has: entry lists built and dropped in a
    // loop, each holding one string, one identity-kept `File`, one minted `File`
    // and one `File` a `set` replacement should have dropped at the moment it
    // replaced it. Nothing may be left behind.
    let mut engine = Engine::with_document(document());
    let baseline = engine.host_record_count();
    engine
        .evaluate(
            r#"
            for (let i = 0; i < 50; i += 1) {
                const fd = new FormData();
                fd.append('s', String(i));
                fd.append('kept', new File(['a'], 'a.txt'));
                fd.append('minted', new Blob(['b'], { type: 'text/plain' }));
                fd.set('minted', new Blob(['c']), 'c.txt');
                fd.get('minted').size;
            }
        "#,
        )
        .expect("evaluate the allocation loop");
    sweep(&mut engine);
    assert_eq!(
        engine.host_record_count(),
        baseline,
        "every entry list and every file it minted should be freed"
    );
}

#[test]
fn deleting_and_setting_release_exactly_the_files_they_drop() {
    // The mutation-time half of the protocol, counted exactly. A record is
    // released when the last entry naming it is gone — not once per entry, and
    // not when the list is merely read — and a record the page still holds is
    // *not* released, which is the property the `File` record's `has_wrapper`
    // flag exists for.
    let mut engine = Engine::with_document(document());
    let baseline = engine.host_record_count();
    engine
        .evaluate(
            r#"
            globalThis.fd = new FormData();
            globalThis.keep = new File(['k'], 'keep.txt');
            fd.append('a', keep);
            fd.append('b', new Blob(['b']));
            fd.append('c', new Blob(['c']));
        "#,
        )
        .expect("evaluate the entries");
    sweep(&mut engine);
    assert_eq!(
        engine.host_record_count(),
        baseline + 4,
        "the entry list, the page's own file, and the two files it minted"
    );

    // `delete` drops the minted file outright, and `set`'s replacement drops the
    // one it replaced — both at the mutation, with no sweep in between, because
    // nothing else holds either record.
    engine
        .evaluate("fd.delete('b'); fd.set('c', 'a plain string');")
        .expect("evaluate the two mutations");
    assert_eq!(
        engine.host_record_count(),
        baseline + 2,
        "the two minted files are freed at the mutation that dropped them"
    );

    // The page's own file is released by the entry list but *kept*: `keep` still
    // holds the wrapper, so the record must not be freed underneath it.
    engine
        .evaluate("fd.delete('a');")
        .expect("evaluate the release of the kept file");
    assert_eq!(
        engine.host_record_count(),
        baseline + 2,
        "a file the page still holds survives its entry being deleted"
    );
    assert_eq!(
        engine
            .evaluate("[keep.name, keep.size].join('|')")
            .expect("the page's file is still readable"),
        "keep.txt|1"
    );

    engine
        .evaluate("globalThis.keep = null;")
        .expect("evaluate the release of the page's file");
    sweep(&mut engine);
    assert_eq!(engine.host_record_count(), baseline + 1);

    engine
        .evaluate("globalThis.fd = null;")
        .expect("evaluate the release of the entry list");
    sweep(&mut engine);
    assert_eq!(engine.host_record_count(), baseline);
}
