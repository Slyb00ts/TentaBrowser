//! `MessageChannel`, `MessagePort`, `MessageEvent` and `window.postMessage()`
//! on the V8 host, and `EventTarget`'s operations with the window as their
//! receiver.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some("https://example.com/page".parse().expect("test URL")));
    Engine::with_document(Rc::new(RefCell::new(document)))
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

/// A polyfill copies `EventTarget.prototype.addEventListener` to another name
/// and calls it on the window, and a stored reference is called bare: Web IDL
/// gives both the window as receiver.
#[test]
fn event_target_operations_take_the_window_as_receiver() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var seen = []; \
             var proto = EventTarget.prototype; \
             Object.defineProperty(proto, 'copied', \
               Object.getOwnPropertyDescriptor(proto, 'addEventListener')); \
             window.copied('ping', e => seen.push('copied:' + (e.target === window))); \
             proto.addEventListener.call(window, 'ping', () => seen.push('called')); \
             var bare = proto.addEventListener; bare('ping', () => seen.push('bare')); \
             var gone = () => seen.push('removed'); \
             window.addEventListener('ping', gone); \
             proto.removeEventListener.call(window, 'ping', gone); \
             var delivered = proto.dispatchEvent.call(window, new Event('ping')); \
             var other = new EventTarget(); \
             proto.addEventListener.call(other, 'ping', e => seen.push('other:' + (e.target === other))); \
             other.dispatchEvent(new Event('ping')); \
             var refused; try { proto.addEventListener.call({}, 'ping', () => {}); refused = 'accepted'; } \
               catch (error) { refused = error.name; } \
             [seen.join(','), delivered, refused, proto.addEventListener.name, \
              proto.addEventListener.length, proto.dispatchEvent.length].join('|')"
        ),
        "copied:true,called,bare,other:true|true|TypeError|addEventListener|2|1"
    );
}

#[test]
fn a_channel_delivers_clones_in_order_from_tasks_of_their_own() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var seen = []; \
             var channel = new MessageChannel(); \
             var value = {n: 1, list: [1, 2]}; \
             channel.port2.onmessage = e => seen.push( \
               JSON.stringify(e.data) + ':' + (e.data !== value) + ':' + (e.target === channel.port2) \
               + ':' + (e instanceof MessageEvent) + ':' + e.type + ':' + e.origin + ':' + e.ports.length); \
             channel.port1.postMessage(value); value.n = 2; \
             channel.port1.postMessage('second'); \
             [seen.length, channel.port1 instanceof MessagePort, channel.port1 instanceof EventTarget, \
              channel.port1 === channel.port1, channel.port1 !== channel.port2, \
              String(channel.port1), String(channel)].join('|')"
        ),
        "0|true|true|true|true|[object MessagePort]|[object MessageChannel]"
    );
    engine.run_event_loop(16);
    assert_eq!(
        eval(&mut engine, "seen.join(' ')"),
        "{\"n\":1,\"list\":[1,2]}:true:true:true:message::0 \"second\":true:true:true:message::0"
    );
}

#[test]
fn a_port_keeps_messages_until_it_is_started_and_stops_when_closed() {
    let mut engine = engine();
    eval(
        &mut engine,
        "var seen = []; var channel = new MessageChannel(); \
         channel.port2.addEventListener('message', e => seen.push(e.data)); \
         channel.port1.postMessage('held'); 'posted'",
    );
    engine.run_event_loop(16);
    // A listener alone does not start the port.
    assert_eq!(eval(&mut engine, "seen.join(',')"), "");
    eval(&mut engine, "channel.port2.start(); 'started'");
    engine.run_event_loop(16);
    assert_eq!(eval(&mut engine, "seen.join(',')"), "held");

    eval(
        &mut engine,
        "channel.port1.postMessage('in flight'); channel.port2.close(); \
         channel.port1.postMessage('after close'); 'closed'",
    );
    engine.run_event_loop(16);
    assert_eq!(eval(&mut engine, "seen.join(',')"), "held");
}

#[test]
fn what_cannot_be_cloned_is_refused_when_it_is_posted() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var attempt = f => { try { f(); return 'ok'; } catch (e) { return e.name; } }; \
             var channel = new MessageChannel(); \
             [attempt(() => channel.port1.postMessage(() => {})), \
              attempt(() => channel.port1.postMessage()), \
              attempt(() => MessagePort.prototype.postMessage.call({}, 1)), \
              attempt(() => new MessagePort()), \
              attempt(() => MessageChannel()), \
              attempt(() => postMessage(() => {}))].join('|')"
        ),
        "DataCloneError|TypeError|TypeError|TypeError|TypeError|DataCloneError"
    );
}

#[test]
fn a_message_event_is_an_event_with_the_message_members() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var port = new MessageChannel().port1; \
             var event = new MessageEvent('note', {data: {a: 1}, origin: 'https://a.test', \
               lastEventId: 7, source: port, bubbles: true}); \
             var bare = new MessageEvent('x'); \
             [event instanceof Event, event.type, event.bubbles, event.data.a, event.origin, \
              event.lastEventId, event.source === port, event.ports.length, \
              String(bare.data), bare.origin, String(bare.source), String(event), \
              Object.getPrototypeOf(MessageEvent.prototype) === Event.prototype].join('|')"
        ),
        "true|note|true|1|https://a.test|7|true|0|null||null|[object MessageEvent]|true"
    );
}

#[test]
fn window_post_message_reaches_the_window_from_a_task() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var seen = []; \
             window.onmessage = e => seen.push('handler:' + e.data.k); \
             addEventListener('message', e => seen.push( \
               [e.data.k, e.origin, e.source === window, e.target === window].join('/'))); \
             postMessage({k: 'v'}, '*'); seen.length"
        ),
        "0"
    );
    engine.run_event_loop(16);
    assert_eq!(
        eval(&mut engine, "seen.join(' ')"),
        "handler:v v/https://example.com/true/true"
    );
}
