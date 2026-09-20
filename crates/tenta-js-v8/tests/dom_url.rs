//! Tests for the `URL` and `URLSearchParams` bindings (P2 package (c) slice 2a).
//!
//! The two are the first JS-owned host objects the page exposes: they have no
//! document row, their wrapper is their only owner, and `URL.searchParams` is a
//! *live view* of its URL rather than a copy. The tests below cover the parsers
//! and accessors, the four `URLSearchParams` constructor shapes, the pair
//! iterator, form encoding, both directions of the live view, and the
//! `TypeError` a wrongly typed node argument now raises.
//!
//! They live here rather than under `crates/tenta-page/tests/` because they need
//! the V8 glue, and no V8 test may live there.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// A document with `<html><body></body></html>`; tests build their own markup.
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
// URL
// ---------------------------------------------------------------------------

#[test]
fn every_component_of_a_url_reads_back() {
    assert_eq!(
        eval(
            r#"
            const u = new URL('https://user:pass@example.com:8080/a/b?c=d#e');
            [
                u.href, u.origin, u.protocol, u.username, u.password,
                u.host, u.hostname, u.port, u.pathname, u.search, u.hash,
            ].join('|');
        "#
        ),
        concat!(
            "https://user:pass@example.com:8080/a/b?c=d#e",
            "|https://example.com:8080|https:|user|pass",
            "|example.com:8080|example.com|8080|/a/b|?c=d|#e",
        )
    );
}

#[test]
fn a_url_without_the_optional_parts_reads_them_empty() {
    assert_eq!(
        eval(
            r#"
            const u = new URL('http://example.com/');
            [
                u.origin, u.protocol, u.username, u.password, u.host,
                u.hostname, u.port, u.pathname, u.search, u.hash,
            ].join('|');
        "#
        ),
        // An empty port is at the scheme's default, so it serializes as "".
        "http://example.com|http:|||example.com|example.com||/||"
    );
}

#[test]
fn a_relative_url_resolves_against_its_base() {
    assert_eq!(
        eval(
            r#"
            [
                new URL('b/c', 'https://example.com/a/').href,
                new URL('../b', 'https://example.com/a/c/').href,
                new URL('/root', 'https://example.com/a/b').href,
                new URL('?q=1', 'https://example.com/a').href,
                new URL('#f', 'https://example.com/a').href,
                // An absolute URL ignores the base.
                new URL('https://other.test/x', 'https://example.com/a').href,
                // A `data:` URL has no base to resolve against either.
                new URL('data:,hi', 'https://example.com/').href,
            ].join('|');
        "#
        ),
        concat!(
            "https://example.com/a/b/c",
            // `..` pops the `c` segment of the base path, leaving `/a/` for `b`.
            "|https://example.com/a/b",
            "|https://example.com/root",
            "|https://example.com/a?q=1",
            "|https://example.com/a#f",
            "|https://other.test/x",
            "|data:,hi",
        )
    );
}

#[test]
fn an_unparseable_url_throws_a_type_error() {
    assert_eq!(
        eval("try { new URL('not a url'); } catch (e) { e instanceof TypeError; }"),
        "true"
    );
    assert_eq!(
        eval("try { new URL(''); } catch (e) { e instanceof TypeError; }"),
        "true"
    );
    // A relative URL with no base is the same failure: there is nothing to
    // resolve it against. The spec's base is the document's URL, which is not
    // modelled yet.
    assert_eq!(
        eval("try { new URL('/a/b'); } catch (e) { e instanceof TypeError; }"),
        "true"
    );
    // A base that does not itself parse fails the whole construction.
    assert_eq!(
        eval("try { new URL('a', 'nonsense'); } catch (e) { e instanceof TypeError; }"),
        "true"
    );
    // The failure is complete: no object is built, and a valid value still is.
    assert_eq!(
        eval(
            r#"
            const outcomes = [];
            for (const value of ['not a url', '', '/a', 'https://example.com/']) {
                try { outcomes.push(new URL(value).href); } catch (e) { outcomes.push('threw'); }
            }
            outcomes.join('|');
        "#
        ),
        "threw|threw|threw|https://example.com/"
    );
}

#[test]
fn setting_href_to_an_unparseable_url_throws_and_leaves_the_url_alone() {
    assert_eq!(
        eval(
            r#"
            const u = new URL('https://example.com/a');
            let threw = false;
            try { u.href = 'not a url'; } catch (e) { threw = e instanceof TypeError; }
            [threw, u.href].join('|');
        "#
        ),
        "true|https://example.com/a"
    );
}

#[test]
fn the_stringifier_returns_href() {
    assert_eq!(
        eval(
            r#"
            const u = new URL('https://example.com/a?b=c#d');
            [String(u), `${u}`, u.toString(), u.href].join('|');
        "#
        ),
        concat!(
            "https://example.com/a?b=c#d|https://example.com/a?b=c#d",
            "|https://example.com/a?b=c#d|https://example.com/a?b=c#d",
        )
    );
}

#[test]
fn the_setters_reparse_the_url() {
    assert_eq!(
        eval(
            r#"
            const u = new URL('https://user:pass@example.com:8080/a/b?c=d#e');
            u.protocol = 'http';
            u.username = 'other';
            u.password = 'secret';
            u.hostname = 'other.test';
            u.port = '9090';
            u.pathname = '/x/y';
            u.search = '?z=1';
            u.hash = '#f';
            u.href;
        "#
        ),
        "http://other:secret@other.test:9090/x/y?z=1#f"
    );
}

#[test]
fn the_search_and_hash_setters_remove_one_leading_delimiter() {
    assert_eq!(
        eval(
            r#"
            const u = new URL('https://example.com/a');
            u.search = 'a=1';  const unmarked = u.search;
            u.search = '?a=1'; const marked = u.search;
            // The empty string clears the component outright, and the URL loses
            // its trailing delimiter with it.
            u.search = '';     const cleared = [u.search, u.href].join(',');
            u.hash = 'f';      const bare = u.hash;
            u.hash = '#f';     const hashed = u.hash;
            u.hash = '';       const cleared_hash = [u.hash, u.href].join(',');
            // A lone delimiter is not empty, so it leaves an empty component —
            // a bare `?` or `#` — and reads back as "".
            u.search = '?';    const lone = [u.search, u.href].join(',');
            u.hash = '#';      const lone_hash = [u.hash, u.href].join(',');
            [unmarked, marked, cleared, bare, hashed, cleared_hash, lone, lone_hash].join('|');
        "#
        ),
        concat!(
            "?a=1|?a=1|,https://example.com/a|#f|#f|,https://example.com/a",
            "|,https://example.com/a?|,https://example.com/a?#",
        )
    );
}

#[test]
fn the_port_setter_reads_leading_digits_and_ignores_the_rest() {
    assert_eq!(
        eval(
            r#"
            const u = new URL('https://example.com/');
            u.port = '8080junk';
            const parsed = u.port;
            // A value that starts with a non-digit fails the parser's port
            // state, which leaves the URL untouched.
            u.port = 'junk';
            const untouched = u.port;
            // The scheme's default port is not serialized.
            u.port = '443';
            const defaulted = [u.port, u.href].join(',');
            // The empty string clears it.
            u.port = '8080';
            u.port = '';
            [parsed, untouched, defaulted, u.port, u.href].join('|');
        "#
        ),
        "8080|8080|,https://example.com/||https://example.com/"
    );
}

#[test]
fn the_host_setter_keeps_a_port_and_clears_one_that_was_dropped() {
    assert_eq!(
        eval(
            r#"
            const u = new URL('https://example.com/');
            u.host = 'other.test:8080';
            const with_port = [u.hostname, u.port, u.host].join(',');
            // The spec's `host` setter sets the host *and* the port, so a value
            // with no port clears the one already there.
            u.host = 'third.test';
            const without_port = [u.hostname, u.port, u.host].join(',');
            // Brackets protect the colons of an IPv6 literal.
            u.host = '[::1]:9090';
            const v6 = [u.hostname, u.port, u.host].join(',');
            [with_port, without_port, v6].join('|');
        "#
        ),
        "other.test,8080,other.test:8080|third.test,,third.test|[::1],9090,[::1]:9090"
    );
}

#[test]
fn an_opaque_origin_serializes_as_null() {
    assert_eq!(
        eval(
            r#"
            [
                new URL('data:,hi').origin,
                new URL('blob:https://example.com/x').origin,
                new URL('https://example.com:8443/').origin,
            ].join('|');
        "#
        ),
        "null|https://example.com|https://example.com:8443"
    );
}

// ---------------------------------------------------------------------------
// URLSearchParams
// ---------------------------------------------------------------------------

#[test]
fn url_search_params_constructs_from_a_string() {
    assert_eq!(
        eval(
            r#"
            [
                new URLSearchParams('a=1&b=2').toString(),
                new URLSearchParams('a=1&a=2').getAll('a').join(','),
                new URLSearchParams('').toString(),
                new URLSearchParams().toString(),
                // A pair with no `=` reads as an empty value.
                new URLSearchParams('a&b=').toString(),
                // The string is parsed, not treated as a URL: `?` is data and is
                // escaped on the way back out.
                new URLSearchParams('?a=1').toString(),
            ].join('|');
        "#
        ),
        "a=1&b=2|1,2|||a=&b=|%3Fa=1"
    );
}

#[test]
fn url_search_params_constructs_from_a_pair_sequence() {
    assert_eq!(
        eval(
            r#"
            [
                new URLSearchParams([['a', '1']]).toString(),
                new URLSearchParams([['a', '1'], ['a', '2']]).getAll('a').join(','),
                new URLSearchParams([]).toString(),
                // An inner sequence that is not a pair is a `TypeError`, as the
                // URL Standard requires.
                (() => {
                    try { new URLSearchParams([['a']]); return 'no throw'; }
                    catch (e) { return e.constructor.name; }
                })(),
                (() => {
                    try { new URLSearchParams([['a', '1', '2']]); return 'no throw'; }
                    catch (e) { return e.constructor.name; }
                })(),
            ].join('|');
        "#
        ),
        "a=1|1,2||TypeError|TypeError"
    );
}

#[test]
fn url_search_params_constructs_from_a_record() {
    assert_eq!(
        eval(
            r#"
            [
                new URLSearchParams({a: '1'}).toString(),
                new URLSearchParams({a: '1', b: '2'}).toString(),
                new URLSearchParams({}).toString(),
                // The record arm is reached by a plain object, which the
                // sequence arm has to decline first.
                new URLSearchParams({a: '1'}) instanceof URLSearchParams,
            ].join('|');
        "#
        ),
        "a=1|a=1&b=2||true"
    );
}

#[test]
fn url_search_params_constructs_from_a_map() {
    assert_eq!(
        eval(
            r#"
            const pairs = (function* () { yield ['a', '1']; yield ['b', '2']; })();
            [
                new URLSearchParams(new Map([['a', '1']])).toString(),
                new URLSearchParams(new Map([['a', '1'], ['b', '2']])).toString(),
                new URLSearchParams(new Set()).toString(),
                // Any iterable of pairs works, and a generator is the clearest
                // proof that the `@@iterator` is what selects the arm.
                new URLSearchParams(pairs).toString(),
            ].join('|');
        "#
        ),
        "a=1|a=1&b=2||a=1&b=2"
    );
}

#[test]
fn url_search_params_encodes_like_a_form() {
    assert_eq!(
        eval(
            r#"
            const p = new URLSearchParams();
            p.append('a b', 'c+d');
            p.append('e', 'f/g');
            p.append('h', 'i~j');
            p.append('k-l.m_n', '*1');
            p.toString();
        "#
        ),
        // The form-urlencoded serializer keeps `A-Za-z0-9*-._`, turns a space
        // into `+`, and percent-encodes everything else — `~` included.
        "a+b=c%2Bd&e=f%2Fg&h=i%7Ej&k-l.m_n=*1"
    );
    assert_eq!(
        eval(
            r#"
            // The parser is the inverse: `+` is a space, and a percent escape is
            // decoded. `%C3%BC` is `ü`, so the decode is a UTF-8 one.
            [
                new URLSearchParams('a+b=c+d').get('a b'),
                new URLSearchParams('a=%C3%BC').get('a'),
                new URLSearchParams('a=%zz').get('a'),
                new URLSearchParams('a=1').toString(),
            ].join('|');
        "#
        ),
        "c d|ü|%zz|a=1"
    );
}

#[test]
fn url_search_params_get_set_has_and_delete() {
    assert_eq!(
        eval(
            r#"
            const p = new URLSearchParams('a=1&b=2&a=3');
            const out = [
                p.get('a'), p.get('zz'), p.getAll('a').join(','), p.getAll('zz').length,
                p.has('a'), p.has('a', '1'), p.has('a', '2'), p.has('zz'),
            ];
            // `set` replaces the first matching pair in place and drops the
            // rest, so the pair keeps its position.
            p.set('a', '9');
            out.push(p.toString());
            // `set` on an absent name appends.
            p.set('z', '0');
            out.push(p.toString());
            // `delete(name)` removes every pair with that name; `delete(name,
            // value)` only the ones matching both halves.
            p.delete('a');
            out.push(p.toString());
            p.delete('b', 'nope');
            out.push(p.toString());
            p.delete('b', '2');
            out.push(p.toString());
            out.join('|');
        "#
        ),
        "1||1,3|0|true|true|false|false|a=9&b=2|a=9&b=2&z=0|b=2&z=0|b=2&z=0|z=0"
    );
}

#[test]
fn url_search_params_sorts_by_code_units_and_keeps_ties_stable() {
    assert_eq!(
        eval(
            r#"
            const p = new URLSearchParams();
            p.append('b', '1');
            p.append('a', '2');
            p.append('B', '3');
            p.append('a', '4');
            p.sort();
            p.toString();
        "#
        ),
        // `B` (U+0042) precedes `a` (U+0061), and the two `a` pairs keep their
        // relative order.
        "B=3&a=2&a=4&b=1"
    );
}

#[test]
fn url_search_params_iterates_its_pairs() {
    assert_eq!(
        eval(
            r#"
            const p = new URLSearchParams('a=1&b=2');
            const for_of = [];
            for (const [k, v] of p) { for_of.push(k + '=' + v); }
            const spread = [...p].map(([k, v]) => k + v).join(',');
            const from = Array.from(p).map(pair => pair.join(':')).join(',');
            const entries = [...p.entries()].map(pair => pair.join(':')).join(',');
            const keys = [...p.keys()].join(',');
            const values = [...p.values()].join(',');
            const each = [];
            p.forEach((value, key, container) => each.push(key + value + (container === p)));
            [
                for_of.join(','), spread, from, entries, keys, values, each.join(','),
                // `@@iterator` is the very same function object as `entries`,
                // as the generated pair-iterable glue installs it.
                p[Symbol.iterator] === p.entries,
                p[Symbol.iterator] === URLSearchParams.prototype.entries,
                Object.prototype.propertyIsEnumerable.call(
                    URLSearchParams.prototype, Symbol.iterator),
            ].join('|');
        "#
        ),
        concat!(
            "a=1,b=2|a1,b2|a:1,b:2|a:1,b:2|a,b|1,2|a1true,b2true",
            "|true|true|false",
        )
    );
}

#[test]
fn url_search_params_to_string_is_the_form_serialization() {
    assert_eq!(
        eval(
            r#"
            const p = new URLSearchParams('a=1&b=2');
            [String(p), `${p}`, p.toString()].join('|');
        "#
        ),
        "a=1&b=2|a=1&b=2|a=1&b=2"
    );
}

#[test]
fn an_empty_search_params_still_iterates() {
    assert_eq!(
        eval(
            r#"
            const p = new URLSearchParams();
            let count = 0;
            for (const _ of p) { count += 1; }
            [count, [...p].length, Array.from(p).length, String(p)].join('|');
        "#
        ),
        "0|0|0|"
    );
}

// ---------------------------------------------------------------------------
// The live view
// ---------------------------------------------------------------------------

#[test]
fn search_params_is_the_same_object_on_every_access() {
    assert_eq!(
        eval(
            r#"
            const u = new URL('https://example.com/?a=1');
            const first = u.searchParams;
            [
                u.searchParams === first,
                u.searchParams === u.searchParams,
                first instanceof URLSearchParams,
                first.get('a'),
            ].join('|');
        "#
        ),
        "true|true|true|1"
    );
}

#[test]
fn a_mutation_through_search_params_reaches_the_url() {
    assert_eq!(
        eval(
            r#"
            const u = new URL('https://example.com/?a=1');
            const p = u.searchParams;
            p.append('b', '2');
            const appended = [u.href, u.search].join(',');
            p.set('a', '9');
            const set = u.href;
            p.sort();
            const sorted = u.href;
            p.delete('b');
            const deleted = u.href;
            [appended, set, sorted, deleted].join('|');
        "#
        ),
        concat!(
            "https://example.com/?a=1&b=2,?a=1&b=2",
            "|https://example.com/?a=9&b=2",
            "|https://example.com/?a=9&b=2",
            "|https://example.com/?a=9",
        )
    );
}

#[test]
fn a_url_without_a_query_gets_one_when_its_params_are_mutated() {
    assert_eq!(
        eval(
            r#"
            const u = new URL('https://example.com/');
            u.searchParams.append('a', '1');
            [u.href, u.search].join('|');
        "#
        ),
        "https://example.com/?a=1|?a=1"
    );
}

#[test]
fn an_assignment_to_the_url_is_visible_through_an_earlier_search_params() {
    assert_eq!(
        eval(
            r#"
            const u = new URL('https://example.com/?a=1');
            const p = u.searchParams;
            const before = p.get('a');
            u.search = '?b=2&c=3';
            const after = [p.get('b'), p.get('c'), p.get('a'), p.has('a')].join(',');
            // A whole-URL assignment is just as visible, and the object is still
            // the one that was obtained first.
            u.href = 'https://example.com/?d=4';
            const replaced = [p.get('d'), p.get('b'), (u.searchParams === p)].join(',');
            [before, after, replaced, p.toString()].join('|');
        "#
        ),
        "1|2,3,,false|4,,true|d=4"
    );
}

#[test]
fn reading_the_pairs_does_not_rewrite_the_url() {
    // The pair list of `?a` is `[('a', '')]`, whose serialization is `a=`; a read
    // must not perform the spec's update step, so the URL keeps its bare `?`
    // until something actually mutates the list.
    assert_eq!(
        eval(
            r#"
            const u = new URL('https://example.com/?a');
            const p = u.searchParams;
            const read = [p.get('a'), p.has('a'), p.toString(), u.href].join(',');
            p.append('b', '');
            const written = u.href;
            [read, written].join('|');
        "#
        ),
        ",true,a=,https://example.com/?a|https://example.com/?a=&b="
    );
}

#[test]
fn deleting_the_last_pair_leaves_an_empty_query() {
    assert_eq!(
        eval(
            r#"
            const u = new URL('https://example.com/?a=1');
            u.searchParams.delete('a');
            // The update step sets the query to the empty string, which is the
            // bare `?` and not the absence of a query.
            [u.href, u.search, String(u.searchParams)].join('|');
        "#
        ),
        "https://example.com/?||"
    );
}

#[test]
fn separate_urls_have_separate_search_params() {
    assert_eq!(
        eval(
            r#"
            const a = new URL('https://example.com/?x=1');
            const b = new URL('https://example.com/?x=1');
            a.searchParams.set('x', '9');
            [a.href, b.href, (a.searchParams === b.searchParams)].join('|');
        "#
        ),
        "https://example.com/?x=9|https://example.com/?x=1|false"
    );
}

#[test]
fn a_standalone_search_params_is_not_a_live_view() {
    // A constructed `URLSearchParams` has no URL behind it, so its list is its
    // own state and nothing writes it anywhere else.
    assert_eq!(
        eval(
            r#"
            const p = new URLSearchParams('a=1');
            p.append('b', '2');
            [p.toString(), typeof p.searchParams].join('|');
        "#
        ),
        "a=1&b=2|undefined"
    );
}

// ---------------------------------------------------------------------------
// The interface-typed argument decode
// ---------------------------------------------------------------------------

#[test]
fn a_url_is_not_a_node() {
    // `contains` used to decode the handle's payload as a node id behind a
    // `debug_assert`, so this aborted in debug and searched an unrelated arena
    // slot in release. It is a `TypeError` now.
    assert_eq!(
        eval(
            r#"
            const u = new URL('https://example.com/');
            try { document.contains(u); 'no throw' }
            catch (e) { [e.constructor.name, e instanceof TypeError].join('|') }
        "#
        ),
        "TypeError|true"
    );
}

#[test]
fn a_url_is_not_a_child() {
    assert_eq!(
        eval(
            r#"
            const u = new URL('https://example.com/');
            const p = new URLSearchParams('a=1');
            const div = document.createElement('div');
            const results = [];
            for (const run of [
                () => document.body.appendChild(u),
                () => document.body.insertBefore(u, null),
                () => document.body.removeChild(u),
                () => document.body.replaceChild(p, div),
            ]) {
                try { run(); results.push('no throw'); }
                catch (e) { results.push(e.constructor.name); }
            }
            results.join('|');
        "#
        ),
        "TypeError|TypeError|TypeError|TypeError"
    );
}

#[test]
fn a_rejected_child_argument_leaves_the_tree_alone() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<p id="a"></p>';
            const u = new URL('https://example.com/');
            try { document.body.appendChild(u); } catch (e) {}
            [document.body.children.length, document.getElementById('a') !== null].join('|');
        "#
        ),
        "1|true"
    );
}

#[test]
fn a_real_node_argument_still_works() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<p id="a"></p>';
            const p = document.getElementById('a');
            const div = document.createElement('div');
            const appended = document.body.appendChild(div) === div;
            const contains = document.body.contains(div) && document.body.contains(p);
            const inserted = document.body.insertBefore(div, p) === div;
            const removed = document.body.removeChild(div) === div;
            [appended, contains, inserted, removed, document.body.children.length].join('|');
        "#
        ),
        "true|true|true|true|1"
    );
}

#[test]
fn url_objects_survive_many_navigations_of_the_live_view() {
    // A `URL` and its `URLSearchParams` are the first hosts whose record is
    // freed when the wrapper is collected. This exercises creation, the live
    // view and their release many times over, so a leaked or prematurely freed
    // record shows up as a wrong value rather than as a crash.
    assert_eq!(
        eval(
            r#"
            let seen = 0;
            for (let i = 0; i < 50; i += 1) {
                const u = new URL('https://example.com/?i=' + i);
                u.searchParams.set('j', String(i * 2));
                seen += Number(u.searchParams.get('i')) + Number(u.searchParams.get('j'));
            }
            String(seen === (49 * 50 / 2) + (2 * 49 * 50 / 2));
        "#
        ),
        "true"
    );
}
