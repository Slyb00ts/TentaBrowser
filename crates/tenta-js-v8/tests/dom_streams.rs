//! `ReadableStream`, its default reader and controller, the queuing
//! strategies, and the two places the engine hands a page a stream:
//! `Response.prototype.body` and `Blob.prototype.stream()`
//! (`crates/tenta-js/src/streams.rs`).
//!
//! A read settles in a microtask, so each test runs its script, drains the
//! microtasks with `Engine::run_microtasks` and only then reads what the
//! script recorded.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

/// An engine with the preludes a page gets.
fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some("https://example.com/".parse().expect("test URL")));
    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    engine.install_preludes();
    engine
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .unwrap_or_else(|error| panic!("`{source}` should evaluate: {error}"))
}

/// Runs `source`, drains the microtasks it queued, then reads `after`.
fn settled(source: &str, after: &str) -> String {
    let mut engine = engine();
    eval(&mut engine, source);
    engine.run_microtasks();
    eval(&mut engine, after)
}

/// `source` with the helpers the tests share: `seen`, a log; `drain(stream)`,
/// which reads a stream to its end into `seen`; and `attempt`, which names what
/// a call threw.
fn script(source: &str) -> String {
    format!(
        r#"
        globalThis.seen = [];
        globalThis.attempt = f => {{
            try {{ return String(f()); }} catch (e) {{ return e.constructor.name; }}
        }};
        globalThis.show = chunk => chunk instanceof Uint8Array
            ? 'bytes(' + Array.from(chunk).join(' ') + ')' : String(chunk);
        globalThis.drain = async (stream, label = '') => {{
            const reader = stream.getReader();
            for (;;) {{
                const {{ value, done }} = await reader.read();
                if (done) {{ seen.push(label + 'done'); return; }}
                seen.push(label + show(value));
            }}
        }};
        {source}
        "#
    )
}

fn run(source: &str) -> String {
    settled(&script(source), "seen.join('|')")
}

// ---------------------------------------------------------------------------
// The interfaces
// ---------------------------------------------------------------------------

#[test]
fn the_interfaces_exist_with_the_members_the_standard_names() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const stream = new ReadableStream();
            [
                String(stream), stream instanceof ReadableStream, stream.locked,
                Object.keys(ReadableStream.prototype).join(','),
                typeof ReadableStream.prototype[Symbol.asyncIterator],
                ReadableStream.prototype[Symbol.asyncIterator] === ReadableStream.prototype.values,
                typeof ReadableStream.from,
                Object.keys(ReadableStreamDefaultReader.prototype).join(','),
                Object.keys(ReadableStreamDefaultController.prototype).join(','),
                String(stream.getReader()), stream.locked,
                'pipeTo' in stream, 'pipeThrough' in stream,
                typeof WritableStream, typeof TransformStream,
            ].join('|');
            "#
        ),
        "[object ReadableStream]|true|false|locked,cancel,getReader,tee,values|function|true|\
         function|closed,cancel,read,releaseLock|desiredSize,close,enqueue,error|\
         [object ReadableStreamDefaultReader]|true|false|false|undefined|undefined",
        "what is not implemented is absent, not present and failing"
    );
}

/// The script behind the names runs at a page's first use of one
/// (`crates/tenta-js-v8/src/host/streams.rs`), whichever name that is.
#[test]
fn each_name_works_as_the_first_one_a_page_uses() {
    let first_uses = [
        ("'ReadableStream' in globalThis", "true"),
        ("typeof ReadableStreamDefaultReader", "function"),
        (
            "new CountQueuingStrategy({ highWaterMark: 2 }).highWaterMark",
            "2",
        ),
        ("String(new Response('x').body)", "[object ReadableStream]"),
        (
            "String(new Blob(['x']).stream())",
            "[object ReadableStream]",
        ),
        ("String(new Response().body)", "null"),
    ];
    for (source, expected) in first_uses {
        let mut engine = engine();
        assert_eq!(eval(&mut engine, source), expected, "`{source}` first");
        assert_eq!(
            eval(
                &mut engine,
                "new Response('x').body instanceof ReadableStream                  && new ReadableStream().getReader() instanceof ReadableStreamDefaultReader"
            ),
            "true",
            "after `{source}`"
        );
    }
}

#[test]
fn the_names_are_ordinary_properties_a_page_can_replace() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const shape = (object, name) => {
                const d = Object.getOwnPropertyDescriptor(object, name);
                return [typeof d.value, typeof d.get, d.writable, d.enumerable, d.configurable].join(',');
            };
            const body = Object.getOwnPropertyDescriptor(Response.prototype, 'body');
            const out = [
                shape(globalThis, 'ReadableStream'), shape(Blob.prototype, 'stream'),
                body.get.name, body.get.length, Blob.prototype.stream.name, Blob.prototype.stream.length,
                Object.keys(globalThis).includes('ReadableStream'),
            ];
            globalThis.ByteLengthQueuingStrategy = 'mine';
            out.push(ByteLengthQueuingStrategy, typeof CountQueuingStrategy);
            out.join('|');
            "#
        ),
        "function,undefined,true,false,true|function,undefined,true,true,true|get body|0|stream|0|false|mine|function"
    );
}

#[test]
fn the_constructors_reject_what_they_cannot_take() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const attempt = f => { try { return String(f()); } catch (e) { return e.constructor.name + ':' + e.message; } };
            const name = f => { try { return String(f()); } catch (e) { return e.constructor.name; } };
            [
                name(() => ReadableStream()),
                name(() => new ReadableStream(5)),
                name(() => new ReadableStream({ start: 5 })),
                name(() => new ReadableStream({ pull: 'x' })),
                name(() => new ReadableStream({ type: 'other' })),
                attempt(() => new ReadableStream({ type: 'bytes' })),
                name(() => new ReadableStream({}, { highWaterMark: -1 })),
                name(() => new ReadableStream({}, { highWaterMark: NaN })),
                name(() => new ReadableStream({}, { size: 5 })),
                name(() => new ReadableStream({ start() { throw new SyntaxError('mine'); } })),
                name(() => new ReadableStreamDefaultController()),
                name(() => new ReadableStreamDefaultReader()),
                name(() => new ReadableStreamDefaultReader({})),
                attempt(() => new ReadableStream().getReader({ mode: 'byob' })),
                name(() => new ReadableStream().getReader({ mode: 'other' })),
                name(() => new ReadableStream(null) instanceof ReadableStream),
                name(() => Object.getOwnPropertyDescriptor(ReadableStream.prototype, 'locked').get.call({})),
                name(() => ReadableStream.prototype.getReader.call({})),
            ].join('|');
            "#
        ),
        "TypeError|TypeError|TypeError|TypeError|TypeError|\
         TypeError:Failed to construct 'ReadableStream': byte streams (type: 'bytes') are not supported.|\
         RangeError|RangeError|TypeError|SyntaxError|TypeError|TypeError|TypeError|\
         TypeError:Failed to execute 'getReader' on 'ReadableStream': BYOB readers are not supported.|\
         TypeError|true|TypeError|TypeError"
    );
}

#[test]
fn a_second_reader_needs_the_first_to_let_go() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const name = f => { try { return String(f()); } catch (e) { return e.constructor.name; } };
            const stream = new ReadableStream();
            const first = stream.getReader();
            const out = [stream.locked, name(() => stream.getReader()),
                         name(() => new ReadableStreamDefaultReader(stream)), name(() => stream.tee())];
            first.releaseLock();
            first.releaseLock();
            out.push(stream.locked, new ReadableStreamDefaultReader(stream) instanceof ReadableStreamDefaultReader,
                     stream.locked);
            out.join('|');
            "#
        ),
        "true|TypeError|TypeError|TypeError|false|true|true"
    );
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

#[test]
fn chunks_enqueued_at_start_are_read_in_order_and_then_the_stream_ends() {
    assert_eq!(
        run(r#"
            const stream = new ReadableStream({
                start(controller) {
                    seen.push('size:' + controller.desiredSize);
                    controller.enqueue('a'); controller.enqueue('b');
                    seen.push('size:' + controller.desiredSize);
                    controller.close();
                    seen.push(attempt(() => controller.enqueue('c')), attempt(() => controller.close()));
                },
            });
            drain(stream);
        "#),
        "size:1|size:-1|TypeError|TypeError|a|b|done"
    );
}

#[test]
fn pull_is_called_when_a_read_is_waiting_and_not_again_while_it_runs() {
    assert_eq!(
        run(r#"
            let calls = 0;
            const stream = new ReadableStream({
                pull(controller) {
                    calls += 1;
                    seen.push('pull' + calls);
                    if (calls === 3) { controller.close(); return; }
                    return Promise.resolve().then(() => controller.enqueue('chunk' + calls));
                },
            }, { highWaterMark: 0 });
            seen.push('made:' + calls);
            drain(stream);
        "#),
        "made:0|pull1|chunk1|pull2|chunk2|pull3|done",
        "with a high water mark of 0 nothing is pulled until something reads"
    );
}

#[test]
fn the_default_high_water_mark_pulls_one_chunk_ahead() {
    assert_eq!(
        settled(
            r#"
            globalThis.calls = 0;
            globalThis.stream = new ReadableStream({ pull(c) { calls += 1; c.enqueue(calls); } });
            "#,
            "calls"
        ),
        "1",
        "one chunk fills the queue, so pull is not called a second time"
    );
}

#[test]
fn a_reads_promise_resolves_with_value_and_done() {
    assert_eq!(
        run(r#"
            const stream = new ReadableStream({ start(c) { c.enqueue(7); c.close(); } });
            const reader = stream.getReader();
            reader.read().then(r => seen.push(JSON.stringify(r), Object.keys(r).join(',')));
            reader.read().then(r => seen.push(JSON.stringify(r), 'value' in r));
            reader.closed.then(v => seen.push('closed:' + v));
        "#),
        r#"{"value":7,"done":false}|value,done|{"done":true}|true|closed:undefined"#
    );
}

#[test]
fn an_errored_stream_rejects_its_reads_and_its_closed_promise() {
    assert_eq!(
        run(r#"
            let controller;
            const stream = new ReadableStream({ start(c) { controller = c; c.enqueue('kept?'); } });
            const reader = stream.getReader();
            reader.closed.catch(e => seen.push('closed:' + e.message));
            controller.error(new Error('broken'));
            seen.push('size:' + controller.desiredSize);
            reader.read().catch(e => seen.push('read:' + e.message));
            controller.error(new Error('second'));
            seen.push(attempt(() => controller.enqueue('x')));
        "#),
        "size:null|TypeError|closed:broken|read:broken",
        "the queue is dropped, and a second error changes nothing"
    );
}

#[test]
fn an_error_thrown_by_start_or_pull_errors_the_stream() {
    assert_eq!(
        run(r#"
            new ReadableStream({ start() { return Promise.reject(new Error('start')); } })
                .getReader().read().catch(e => seen.push(e.message));
            new ReadableStream({ pull() { throw new Error('pull'); } })
                .getReader().read().catch(e => seen.push(e.message));
            new ReadableStream({
                start(c) { try { c.enqueue('x'); } catch (e) { seen.push('enqueue threw ' + e.message); } },
            }, { size() { throw new Error('size'); } })
                .getReader().read().catch(e => seen.push(e.message));
        "#),
        "enqueue threw size|size|start|pull",
        "a size function that throws errors the stream and the enqueue that called it"
    );
}

#[test]
fn a_released_reader_rejects_and_the_stream_can_be_read_again() {
    assert_eq!(
        run(r#"
            const stream = new ReadableStream({ start(c) { c.enqueue('a'); c.enqueue('b'); c.close(); } });
            const first = stream.getReader();
            first.read().then(r => {
                seen.push(r.value);
                first.releaseLock();
                first.read().catch(e => seen.push('read:' + e.constructor.name));
                first.cancel().catch(e => seen.push('cancel:' + e.constructor.name));
                first.closed.catch(e => seen.push('closed:' + e.constructor.name));
                return drain(stream);
            });
        "#),
        "a|read:TypeError|cancel:TypeError|closed:TypeError|b|done"
    );
}

#[test]
fn a_read_pending_at_release_is_rejected() {
    assert_eq!(
        run(r#"
            const stream = new ReadableStream();
            const reader = stream.getReader();
            reader.read().catch(e => seen.push('pending:' + e.constructor.name));
            reader.releaseLock();
            seen.push('locked:' + stream.locked);
        "#),
        "locked:false|pending:TypeError"
    );
}

// ---------------------------------------------------------------------------
// Cancelling
// ---------------------------------------------------------------------------

#[test]
fn cancel_calls_the_source_once_drops_the_queue_and_closes() {
    assert_eq!(
        run(r#"
            const stream = new ReadableStream({
                start(c) { c.enqueue('queued'); },
                cancel(reason) { seen.push('source:' + reason); return 'ignored'; },
            });
            const reader = stream.getReader();
            reader.cancel('why').then(v => seen.push('cancelled:' + v));
            reader.read().then(r => seen.push('read:' + r.done + ':' + r.value));
            reader.cancel('again').then(v => seen.push('again:' + v));
            new ReadableStream().getReader().closed.then(() => seen.push('never'));
        "#),
        "source:why|read:true:undefined|again:undefined|cancelled:undefined"
    );
}

#[test]
fn a_locked_stream_cannot_be_cancelled_from_the_stream() {
    assert_eq!(
        run(r#"
            const stream = new ReadableStream({ cancel() { throw new Error('refused'); } });
            const reader = stream.getReader();
            stream.cancel().catch(e => seen.push('locked:' + e.constructor.name));
            reader.releaseLock();
            stream.cancel().catch(e => seen.push('source:' + e.message));
            ReadableStream.prototype.cancel.call({}).catch(e => seen.push('brand:' + e.constructor.name));
        "#),
        "locked:TypeError|brand:TypeError|source:refused",
        "a promise-returning operation rejects where another would throw"
    );
}

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

#[test]
fn the_queuing_strategies_measure_by_count_and_by_byte_length() {
    assert_eq!(
        run(r#"
            const count = new CountQueuingStrategy({ highWaterMark: 3 });
            const bytes = new ByteLengthQueuingStrategy({ highWaterMark: 10 });
            seen.push(count.highWaterMark, count.size('anything'), bytes.highWaterMark,
                      bytes.size(new Uint8Array(4)), String(count), String(bytes),
                      attempt(() => new CountQueuingStrategy()),
                      attempt(() => new CountQueuingStrategy({})),
                      attempt(() => CountQueuingStrategy({ highWaterMark: 1 })));
            new ReadableStream({
                start(c) {
                    c.enqueue(new Uint8Array(4));
                    seen.push('left:' + c.desiredSize);
                    c.enqueue(new Uint8Array(8));
                    seen.push('left:' + c.desiredSize);
                },
            }, bytes);
        "#),
        "3|1|10|4|[object CountQueuingStrategy]|[object ByteLengthQueuingStrategy]|TypeError|\
         TypeError|TypeError|left:6|left:-2"
    );
}

// ---------------------------------------------------------------------------
// tee(), iteration and from()
// ---------------------------------------------------------------------------

#[test]
fn tee_gives_two_streams_that_each_see_every_chunk() {
    assert_eq!(
        run(r#"
            const stream = new ReadableStream({ start(c) { c.enqueue('a'); c.enqueue('b'); c.close(); } });
            const branches = stream.tee();
            seen.push(branches.length, stream.locked, branches[0] instanceof ReadableStream,
                      branches[0] !== branches[1]);
            drain(branches[0], '1:').then(() => drain(branches[1], '2:'));
        "#),
        "2|true|true|true|1:a|1:b|1:done|2:a|2:b|2:done"
    );
}

#[test]
fn tee_cancels_the_source_only_when_both_branches_are_cancelled() {
    assert_eq!(
        run(r#"
            const stream = new ReadableStream({
                cancel(reason) { seen.push('source:' + JSON.stringify(reason)); },
            });
            const [left, right] = stream.tee();
            left.cancel('l');
            seen.push('after one');
            right.cancel('r').then(() => seen.push('both'));
        "#),
        r#"after one|source:["l","r"]|both"#
    );
}

#[test]
fn a_stream_is_async_iterable() {
    assert_eq!(
        run(r#"
            const make = () => new ReadableStream({
                start(c) { c.enqueue(1); c.enqueue(2); c.enqueue(3); c.close(); },
                cancel(reason) { seen.push('cancelled'); },
            });
            (async () => {
                const all = make();
                for await (const chunk of all) seen.push(chunk);
                seen.push('locked:' + all.locked);
                const left = make();
                for await (const chunk of left) { seen.push('first:' + chunk); break; }
                seen.push('locked:' + left.locked);
                const kept = make();
                for await (const chunk of kept.values({ preventCancel: true })) break;
                seen.push('kept:' + kept.locked);
                await drain(kept, 'rest:');
            })();
        "#),
        "1|2|3|locked:false|first:1|cancelled|locked:false|kept:false|rest:2|rest:3|rest:done",
        "leaving the loop early cancels the stream unless the iteration was asked not to"
    );
}

#[test]
fn from_makes_a_stream_of_an_iterable_or_an_async_iterable() {
    assert_eq!(
        run(r#"
            (async () => {
                await drain(ReadableStream.from(['a', Promise.resolve('b')]), 'array:');
                await drain(ReadableStream.from((async function* () { yield 1; yield 2; })()), 'gen:');
                let returned = false;
                const source = { [Symbol.asyncIterator]() { return {
                    next() { return Promise.resolve({ value: 'x', done: false }); },
                    return() { returned = true; return Promise.resolve({}); },
                }; } };
                const reader = ReadableStream.from(source).getReader();
                seen.push((await reader.read()).value);
                await reader.cancel();
                seen.push('returned:' + returned, attempt(() => ReadableStream.from(5)),
                          attempt(() => ReadableStream.from({})));
            })();
        "#),
        "array:a|array:b|array:done|gen:1|gen:2|gen:done|x|returned:true|TypeError|TypeError"
    );
}

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

#[test]
fn a_responses_body_is_one_stream_of_its_bytes() {
    assert_eq!(
        run(r#"
            const response = new Response('hi!');
            const body = response.body;
            seen.push(body instanceof ReadableStream, body === response.body, response.bodyUsed);
            drain(body).then(() => {
                seen.push('used:' + response.bodyUsed);
                return response.text();
            }).catch(e => seen.push('text:' + e.constructor.name));
        "#),
        "true|true|false|bytes(104 105 33)|done|used:true|text:TypeError",
        "reading the stream is reading the body"
    );
}

#[test]
fn a_response_without_a_body_has_no_stream() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const name = f => { try { return String(f()); } catch (e) { return e.constructor.name; } };
            const descriptor = Object.getOwnPropertyDescriptor(Response.prototype, 'body');
            [
                String(new Response().body), String(new Response(null, { status: 204 }).body),
                new Response('').body instanceof ReadableStream,
                typeof descriptor.get, String(descriptor.set), descriptor.enumerable,
                name(() => descriptor.get.call({})),
                name(() => descriptor.get.call(new Blob(['x']))),
            ].join('|');
            "#
        ),
        "null|null|true|function|undefined|true|TypeError|TypeError",
        "an empty body is still a body"
    );
}

#[test]
fn an_empty_body_ends_without_a_chunk_and_a_used_body_errors_its_stream() {
    assert_eq!(
        run(r#"
            (async () => {
                await drain(new Response('').body, 'empty:');
                const response = new Response('taken');
                const body = response.body;
                seen.push(await response.text());
                await body.getReader().read().then(
                    () => seen.push('read'), e => seen.push('used:' + e.constructor.name));
            })();
        "#),
        "empty:done|taken|used:TypeError"
    );
}

#[test]
fn a_cloned_responses_bodies_are_read_apart() {
    assert_eq!(
        run(r#"
            const response = new Response(new Uint8Array([1, 2]));
            const copy = response.clone();
            drain(response.body, 'a:').then(() => drain(copy.body, 'b:'));
        "#),
        "a:bytes(1 2)|a:done|b:bytes(1 2)|b:done"
    );
}

#[test]
fn a_blobs_stream_reads_its_bytes_and_can_be_made_again() {
    assert_eq!(
        run(r#"
            const blob = new Blob(['ab', 'c']);
            const first = blob.stream();
            seen.push(first instanceof ReadableStream, first !== blob.stream(),
                      attempt(() => Blob.prototype.stream.call({})));
            drain(first, '1:').then(() => drain(blob.stream(), '2:'))
                .then(() => drain(new Blob([]).stream(), 'empty:'))
                .then(() => drain(blob.slice(1).stream(), 'slice:'));
        "#),
        "true|true|TypeError|1:bytes(97 98 99)|1:done|2:bytes(97 98 99)|2:done|empty:done|\
         slice:bytes(98 99)|slice:done"
    );
}
