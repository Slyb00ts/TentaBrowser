//! Web IDL receivers on global EventTarget operations used by WPT testharness.js.

use tenta_js_qjs::Engine;

fn eval(source: &str) -> String {
    let mut engine = Engine::new().expect("QuickJS runtime");
    engine.evaluate(source).expect("global EventTarget script")
}

#[test]
fn nullish_event_target_receivers_use_the_realm_global() {
    assert_eq!(
        eval(
            r#"
            const seen = [];
            function listener(event) { seen.push(event.type); }
            addEventListener('wpt-probe', listener);
            const first = dispatchEvent(new Event('wpt-probe'));
            removeEventListener('wpt-probe', listener);
            EventTarget.prototype.addEventListener.call(undefined, 'wpt-probe', listener);
            const second = EventTarget.prototype.dispatchEvent.call(null, new Event('wpt-probe'));
            EventTarget.prototype.removeEventListener.call(null, 'wpt-probe', listener);
            const third = window.dispatchEvent(new Event('wpt-probe'));
            [first, second, third, seen.join(',')].join('|');
            "#,
        ),
        "true|true|true|wpt-probe,wpt-probe"
    );
}

#[test]
fn receiver_fallback_does_not_accept_foreign_objects_or_interface_arguments() {
    assert_eq!(
        eval(
            r#"
            const bad = [
              () => EventTarget.prototype.addEventListener.call({}, 'x', () => {}),
              () => EventTarget.prototype.addEventListener.call(7, 'x', () => {}),
              () => EventTarget.prototype.addEventListener.call(
                Object.create(EventTarget.prototype), 'x', () => {}),
              () => Object.getOwnPropertyDescriptor(Event.prototype, 'type').get.call(null),
              () => Node.prototype.appendChild.call(document, null)
            ];
            bad.map(call => {
              try { call(); return false; }
              catch (error) { return error instanceof TypeError; }
            }).every(Boolean);
            "#,
        ),
        "true"
    );
}

#[test]
fn event_constructors_require_new_before_reading_a_plain_calls_receiver() {
    assert_eq!(
        eval(
            r#"
        let gets=0;
        const receiver=new Proxy(function Receiver(){},{get(target,key,actual){
            if(key==='prototype'){gets++;throw new Error('unexpected prototype access')}
            return Reflect.get(target,key,actual);
        }});
        const calls=[()=>Event('x'),()=>Event.call(Event,'x'),()=>Event.call(receiver,'x')];
        const rejected=calls.every(call=>{
            try{call();return false}catch(error){return error instanceof TypeError}
        });
        class Derived extends Event {}
        const direct=new Event('direct');
        const derived=Reflect.construct(Event,['derived'],Derived);
        [rejected,gets,direct.type,derived.type,derived instanceof Derived,
            derived instanceof Event].join('|')
    "#
        ),
        "true|0|direct|derived|true|true"
    );
}
