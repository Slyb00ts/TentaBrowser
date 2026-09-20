//! Tests for the `Headers` bindings (P2 package (c) slice 2c).
//!
//! `Headers` is the third JS-owned host object: like `URL` and
//! `URLSearchParams` it has no document row and its wrapper is its only owner,
//! but it adds three of the Fetch Standard's own rules on top of the generated
//! glue — a name is byte-lowercased and must be an HTTP token, a value has its
//! leading and trailing HTTP whitespace stripped and may not hold a NUL, CR or
//! LF, and the pair iterator walks the *sorted and combined* list while the
//! list itself stays in insertion order. The tests below cover each constructor
//! shape, the members, both `TypeError` rules, the cookie accessor, the
//! iteration view and the finalizer path that frees the record.
//!
//! They live here rather than under `crates/tenta-page/tests/` because they need
//! the V8 glue, and no V8 test may live there.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// A document with `<html><body></body></html>`; the header tests need a
/// browsing context but no markup.
fn document() -> Rc<RefCell<Document>> {
    let mut d = Document::new();
    let html = d.create_html_element(atoms::HTML);
    let body = d.create_html_element(atoms::BODY);
    d.mutate(Mutation::Append {
        parent: d.document_node(),
        child: html,
    })
    .unwrap();
    d.mutate(Mutation::Append {
        parent: html,
        child: body,
    })
    .unwrap();
    Rc::new(RefCell::new(d))
}

fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(document());
    engine
        .evaluate(source)
        .expect("script should evaluate without throwing")
}

// ---------------------------------------------------------------------------
// The constructor
// ---------------------------------------------------------------------------

#[test]
fn a_headers_constructor_takes_a_sequence_of_pairs() {
    assert_eq!(
        eval(
            r#"
            const h = new Headers([['Content-Type', 'text/html'], ['X-Custom', ' 1 ']]);
            [
                h.get('content-type'), h.get('CONTENT-TYPE'), h.has('X-Custom'),
                [...h.keys()].join(','),
                [...h].map(pair => pair.join(':')).join(','),
                h instanceof Headers,
            ].join('|');
        "#
        ),
        concat!(
            "text/html|text/html|true",
            "|content-type,x-custom|content-type:text/html,x-custom:1|true",
        )
    );
}

#[test]
fn a_headers_constructor_takes_a_record_and_lowercases_its_keys() {
    assert_eq!(
        eval(
            r#"
            const h = new Headers({ 'Content-Type': 'a', 'X-B': 'b', 'A': '1', 'a': '2' });
            [
                h.get('content-type'), h.get('x-b'),
                [...h.keys()].join(','),
                h.get('a'),
                [...h.values()].join('|'),
            ].join('|');
        "#
        ),
        // `A` and `a` are two own keys of the object, so they are two headers
        // after lowercasing and `get` combines them in list order.
        "a|b|a,content-type,x-b|1, 2|1, 2|a|b"
    );
}

#[test]
fn an_empty_headers_constructs_empty() {
    assert_eq!(
        eval(
            r#"
            const a = new Headers();
            const b = new Headers(undefined);
            const deleted = (() => {
                try { a.delete('x'); return 'ok'; } catch (e) { return e.constructor.name; }
            })();
            [
                [...a].length, [...b].length,
                a.get('x') === null, a.has('x'), a.getSetCookie().length,
                deleted,
                typeof a.append, typeof a.getSetCookie,
            ].join('|');
        "#
        ),
        "0|0|true|false|0|ok|function|function"
    );
}

#[test]
fn headers_is_installed_as_a_global_interface() {
    // The installer is this slice's own wiring: the interface has to be on the
    // global under its WebIDL name, brand its instances, and root its prototype
    // at `Object` — `Headers` declares no parent, unlike the `URL` pair's
    // shared tag, which is the one thing the record and not the tag decides.
    assert_eq!(
        eval(
            r#"
            const h = new Headers();
            [
                globalThis.Headers === Headers,
                typeof Headers,
                Headers.name,
                h instanceof Headers,
                h.constructor === Headers,
                Object.getPrototypeOf(Headers.prototype) === Object.prototype,
                Object.prototype.toString.call(h),
            ].join('|');
        "#
        ),
        "true|function|Headers|true|true|true|[object Headers]"
    );
}

#[test]
fn an_inner_sequence_that_is_not_a_pair_throws() {
    assert_eq!(
        eval(
            r#"
            const results = [];
            for (const init of [[[]], [['a']], [['a', 'b', 'c']]]) {
                try { new Headers(init); results.push('no throw'); }
                catch (e) { results.push(e.constructor.name); }
            }
            results.join('|');
        "#
        ),
        "TypeError|TypeError|TypeError"
    );
}

// ---------------------------------------------------------------------------
// The members
// ---------------------------------------------------------------------------

#[test]
fn append_get_has_set_and_delete_match_names_case_insensitively() {
    assert_eq!(
        eval(
            r#"
            const h = new Headers();
            h.append('X-Foo', 'one');
            const appended = [h.get('x-foo'), h.get('X-FOO'), h.has('X-foo'), h.has('x-foo')].join(',');
            h.set('x-FOO', 'two');
            const set = [h.get('X-Foo'), [...h.keys()].join(',')].join(',');
            h.append('x-foo', 'three');
            const combined = h.get('X-FOO');
            h.delete('X-FOO');
            const deleted = [h.has('x-foo'), h.get('x-foo') === null, [...h].length].join(',');
            [appended, set, combined, deleted].join('|');
        "#
        ),
        concat!(
            "one,one,true,true",
            "|two,x-foo",
            "|two, three",
            "|false,true,0",
        )
    );
}

#[test]
fn appending_the_same_name_combines_the_value_that_get_reports() {
    assert_eq!(
        eval(
            r#"
            const h = new Headers();
            h.append('A', '1');
            h.append('a', '2');
            h.append('b', '3');
            [
                h.get('a'), JSON.stringify([...h]), [...h.keys()].join(','),
                h.getSetCookie().length,
            ].join('|');
        "#
        ),
        r#"1, 2|[["a","1, 2"],["b","3"]]|a,b|0"#
    );
}

#[test]
fn set_replaces_the_first_pair_in_place_and_removes_the_rest() {
    assert_eq!(
        eval(
            r#"
            const h = new Headers();
            h.append('set-cookie', 'a=1');
            h.append('set-cookie', 'b=2');
            h.append('set-cookie', 'c=3');
            h.append('x', 'y');
            h.set('set-cookie', 'z');
            const replaced = [JSON.stringify(h.getSetCookie()), JSON.stringify([...h])].join('|');
            h.set('q', 'r');
            const appended = JSON.stringify([...h]);
            h.append('m', '1');
            h.append('m', '2');
            h.set('m', '9');
            const collapsed = [h.get('m'), [...h.keys()].join(',')].join('|');
            [replaced, appended, collapsed].join('~');
        "#
        ),
        concat!(
            r#"["z"]|[["set-cookie","z"],["x","y"]]"#,
            // `set` on a name that matched nothing appends, so the list is in
            // insertion order while the iteration view is sorted by name —
            // which puts `q` before `set-cookie`.
            r#"~[["q","r"],["set-cookie","z"],["x","y"]]"#,
            r#"~9|m,q,set-cookie,x"#,
        )
    );
}

// ---------------------------------------------------------------------------
// Normalization
// ---------------------------------------------------------------------------

#[test]
fn leading_and_trailing_http_whitespace_is_stripped_from_values() {
    assert_eq!(
        eval(
            r#"
            const h = new Headers([['s1', '  spaced  '], ['s2', '\t\f\tx\n']]);
            h.append('s3', '\r\n\r\n');
            h.append('s4', ' nbsp ');
            const record = new Headers({ 's5': '  v  ' }).get('s5');
            [
                '[' + h.get('s1') + ']',
                JSON.stringify(h.get('s2')),
                JSON.stringify(h.get('s3')),
                JSON.stringify(h.get('s4')),
                record,
            ].join('|');
        "#
        ),
        // A form feed and a no-break space are not HTTP whitespace, so they
        // survive where `str::trim` would have removed them; a value of nothing
        // but whitespace normalizes to the empty string.
        "[spaced]|\"\\f\\tx\"|\"\"|\"\u{a0}nbsp\u{a0}\"|v"
    );
}

#[test]
fn normalization_follows_the_fetch_standard_not_str_trim() {
    // The table and the three paths are the Fetch Standard's own
    // `headers-normalize.any.js`: a leading CRLF is stripped, an interior form
    // feed survives, and a no-break space is not HTTP whitespace at all. The
    // constructor's record arm normalizes too, which is why a value with a
    // trailing LF is accepted rather than rejected.
    assert_eq!(
        eval(
            r#"
            const expectations = {
                name1: [' space ', 'space'],
                name2: ['\ttab\t', 'tab'],
                name3: [' spaceAndTab\t', 'spaceAndTab'],
                name4: ['\r\n newLine', 'newLine'],
                name5: ['newLine\r\n ', 'newLine'],
                name6: ['\r\n\tnewLine', 'newLine'],
                name7: ['\t\f\tnewLine\n', '\f\tnewLine'],
                name8: ['newLine\xa0', 'newLine\xa0'],
            };
            const appended = new Headers();
            const set = new Headers();
            const results = [];
            for (const name in expectations) {
                const [value, expected] = expectations[name];
                appended.append(name, value);
                set.set(name, value);
                results.push(appended.get(name) === expected, set.get(name) === expected);
            }
            const built = new Headers(Object.fromEntries(
                Object.entries(expectations).map(([name, [value]]) => [name, value])));
            for (const name in expectations) {
                results.push(built.get(name) === expectations[name][1]);
            }
            results.every(Boolean) + ':' + results.length;
        "#
        ),
        "true:24"
    );
}

#[test]
fn an_invalid_name_or_value_throws_a_type_error_and_leaves_the_list_untouched() {
    assert_eq!(
        eval(
            r#"
            const h = new Headers([['a', 'kept']]);
            const results = [];
            for (const [name, value] of [
                ['a b', 'x'], ['a\tb', 'x'], ['invalidĀ', 'x'], [{}, 'x'],
                ['a', 'x\ny'], ['a', 'x\ry'], ['a', 'x\u0000y'],
            ]) {
                try { h.append(name, value); results.push('no throw'); }
                catch (e) { results.push(e.constructor.name); }
            }
            const afterAppend = [h.get('a'), [...h].length].join(',');
            let setResult;
            // An *interior* LF, which normalization cannot strip: a leading or
            // trailing one is HTTP whitespace and comes off instead of throwing.
            try { h.set('a', 'bro\nken'); setResult = 'no throw'; }
            catch (e) { setResult = e.constructor.name; }
            const afterSet = h.get('a');
            const reads = [];
            for (const name of ['a b', 'invalidĀ', {}]) {
                for (const member of ['get', 'has', 'delete']) {
                    try { h[member](name); reads.push('no throw'); }
                    catch (e) { reads.push(e.constructor.name); }
                }
            }
            const ctor = [];
            for (const init of [[['a b', 'x']], [['a', 'x\ny']], [{'a b': 'x'}], [{'a': 'x\ny'}]]) {
                try { new Headers(init); ctor.push('no throw'); }
                catch (e) { ctor.push(e.constructor.name); }
            }
            [
                results.join(','), afterAppend, setResult, afterSet,
                reads.join(','), ctor.join(','),
            ].join('|');
        "#
        ),
        concat!(
            "TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError",
            // A rejected pair mutates nothing: the list still holds exactly the
            // pair the constructor put there, with the value a failed `set` was
            // about to replace.
            "|kept,1|TypeError|kept",
            "|TypeError,TypeError,TypeError,TypeError,TypeError,TypeError",
            ",TypeError,TypeError,TypeError",
            "|TypeError,TypeError,TypeError,TypeError",
        )
    );
}

#[test]
fn a_record_key_above_0xff_is_rejected() {
    assert_eq!(
        eval(
            r#"
            const results = [];
            const attempt = run => {
                try { run(); results.push('no throw'); }
                catch (e) { results.push(e.constructor.name + (e instanceof TypeError ? '' : '!')); }
            };
            // The record key goes through the `ByteString` conversion, which
            // rejects a code unit above 0xFF — the 2b key-type path.
            attempt(() => new Headers({ 'invĀlid': 'x' }));
            attempt(() => new Headers({ [String.fromCharCode(0x100)]: 'x' }));
            // The sequence arm converts its leaf items the same way.
            attempt(() => new Headers([['invĀlid', 'x']]));
            attempt(() => new Headers([['a', 'invĀlid']]));
            // A name of the same bytes fails the *token* rule instead, which is
            // why the 0xFF check is the one that has to come first for a key.
            attempt(() => new Headers({ ['ÿ']: 'x' }));
            // The cases above are all satisfied by another rule as well: a code
            // unit of 0x100 truncates to NUL, which the *token* rule rejects and
            // an HTTP *header value* may not hold. These two use U+0161, whose
            // low byte is a legal token character, so a conversion that
            // truncated instead of throwing would leave the perfectly valid
            // name `xay` or value `xay` behind and be caught here alone.
            attempt(() => new Headers({ ['x' + String.fromCharCode(0x161) + 'y']: 'v' }));
            attempt(() => new Headers({ a: 'x' + String.fromCharCode(0x161) + 'y' }));
            // At the top of the range a value is still a value.
            results.push(new Headers({ 'a': 'ÿ' }).get('a'));
            results.join('|');
        "#
        ),
        "TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|\u{ff}"
    );
}

// ---------------------------------------------------------------------------
// The cookie accessor and the iteration view
// ---------------------------------------------------------------------------

#[test]
fn the_cookie_accessor_reports_the_fields_separately_while_get_combines_them() {
    assert_eq!(
        eval(
            r#"
            const h = new Headers([['Set-Cookie', 'a=1'], ['set-cookie', 'b=2'], ['x', 'y']]);
            const built = JSON.stringify(h.getSetCookie());
            h.append('set-cookie', 'c=3');
            const appended = JSON.stringify(h.getSetCookie());
            const combined = h.get('Set-Cookie');
            const empty = JSON.stringify(new Headers().getSetCookie());
            const blank = JSON.stringify(new Headers([['set-cookie', '']]).getSetCookie());
            [built, appended, combined, empty, blank].join('|');
        "#
        ),
        concat!(
            r#"["a=1","b=2"]"#,
            r#"|["a=1","b=2","c=3"]"#,
            "|a=1, b=2, c=3",
            "|[]",
            r#"|[""]"#,
        )
    );
}

#[test]
fn headers_iterate_the_sorted_and_combined_view() {
    assert_eq!(
        eval(
            r#"
            const h = new Headers();
            h.append('2', 'a');
            h.append('1', 'b');
            h.append('2', 'b');
            const for_of = [];
            for (const [k, v] of h) { for_of.push(k + '=' + v); }
            const spread = JSON.stringify([...h]);
            const from = JSON.stringify(Array.from(h));
            const entries = JSON.stringify([...h.entries()]);
            const keys = [...h.keys()].join(',');
            const values = [...h.values()].join('|');
            const each = [];
            h.forEach((value, key, container) => each.push(key + '=' + value + (container === h)));
            [
                for_of.join(','), spread, from, entries, keys, values, each.join(','),
                // `@@iterator` is the very same function object as `entries`, as
                // the generated pair-iterable glue installs it.
                h[Symbol.iterator] === h.entries,
                h[Symbol.iterator] === Headers.prototype.entries,
                Object.prototype.propertyIsEnumerable.call(Headers.prototype, Symbol.iterator),
            ].join('~');
        "#
        ),
        concat!(
            // Insertion order is `2, 1, 2`; the view is sorted by name, with the
            // two `2`s combined in list order.
            "1=b,2=a, b",
            r#"~[["1","b"],["2","a, b"]]"#,
            r#"~[["1","b"],["2","a, b"]]"#,
            r#"~[["1","b"],["2","a, b"]]"#,
            "~1,2~b|a, b~1=btrue,2=a, btrue~true~true~false",
        )
    );
}

#[test]
fn set_cookie_is_the_one_name_the_iterator_does_not_combine() {
    assert_eq!(
        eval(
            r#"
            const h = new Headers();
            h.append('set-cookie', 'a=1');
            h.append('set-cookie', 'b=2');
            h.append('set-cookie2', 'c=3');
            h.append('set-cookie2', 'd=4');
            [
                JSON.stringify([...h]),
                JSON.stringify([...h.keys()]),
                JSON.stringify(h.get('set-cookie2')),
            ].join('|');
        "#
        ),
        concat!(
            r#"[["set-cookie","a=1"],["set-cookie","b=2"],["set-cookie2","c=3, d=4"]]"#,
            r#"|["set-cookie","set-cookie","set-cookie2"]"#,
            r#"|"c=3, d=4""#,
        )
    );
}

// ---------------------------------------------------------------------------
// Reclamation
// ---------------------------------------------------------------------------

#[test]
fn a_collected_headers_wrapper_frees_its_record() {
    // A `Headers` record has no second referent, so the wrapper's finalizer is
    // the record's only owner and the sweep must return the slab to its
    // baseline. This is the only test that exercises the finalizer path: the
    // `Headers` wrappers below are unreachable the moment they are built.
    let mut engine = Engine::with_document(document());
    let baseline = engine.host_record_count();
    engine
        .evaluate(
            r#"
            for (let i = 0; i < 50; i += 1) {
                new Headers({ a: String(i) });
            }
        "#,
        )
        .expect("evaluate the allocation loop");

    // Force collection and drain the finalizer-recorded dead wrappers a few
    // times; finalizers may run across more than one GC.
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }

    assert_eq!(
        engine.host_record_count(),
        baseline,
        "a collected Headers wrapper should free its record"
    );
}

#[test]
fn a_live_headers_wrapper_keeps_its_record() {
    let mut engine = Engine::with_document(document());
    let baseline = engine.host_record_count();
    engine
        .evaluate("globalThis.keep = new Headers({ a: '1' });")
        .expect("evaluate the assignment");
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(engine.host_record_count(), baseline + 1);
    // The record is still the one the wrapper was built over.
    assert_eq!(
        engine
            .evaluate("globalThis.keep.get('a') + ':' + globalThis.keep.has('A')")
            .expect("read the kept record"),
        "1:true"
    );

    engine
        .evaluate("globalThis.keep = null;")
        .expect("evaluate the release");
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(engine.host_record_count(), baseline);
}

#[test]
fn headers_survive_many_round_trips() {
    // Interleaves construction, mutation and reads many times over, so a record
    // freed while a live wrapper still needs it — or a slot reused behind one —
    // shows up as a wrong value rather than as a crash. The sum is
    // `Σ(i + 1) + Σ(2i)` for `i` in `0..50`.
    let mut engine = Engine::with_document(document());
    let baseline = engine.host_record_count();
    assert_eq!(
        engine
            .evaluate(
                r#"
                let sum = 0;
                for (let i = 0; i < 50; i += 1) {
                    const h = new Headers([['n', String(i)]]);
                    h.append('n', String(i + 1));
                    h.set('m', String(i * 2));
                    sum += Number(h.get('n').split(', ')[1]) + Number(h.get('m'));
                    if (h.getSetCookie().length !== 0) { sum += 1000; }
                }
                String(sum === 3725);
            "#
            )
            .expect("evaluate the round-trip loop"),
        "true"
    );
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(engine.host_record_count(), baseline);
}
