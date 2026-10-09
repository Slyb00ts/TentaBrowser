//! Web IDL constructor prototypes in the supported single QuickJS realm.

use tenta_js_qjs::Engine;

fn evaluate(source: &str) -> String {
    let mut engine = Engine::new().expect("QuickJS runtime");
    engine
        .evaluate(source)
        .expect("constructor prototype script")
}

#[test]
fn generated_interface_constructors_follow_the_webidl_inheritance_chain() {
    assert_eq!(
        evaluate(
            r#"
            const links = [
                Object.getPrototypeOf(File) === Blob,
                Object.getPrototypeOf(Blob) === Function.prototype,
                Object.getPrototypeOf(HTMLDivElement) === HTMLElement,
                Object.getPrototypeOf(HTMLElement) === Element,
                Object.getPrototypeOf(Element) === Node,
                Object.getPrototypeOf(Node) === EventTarget,
                Object.getPrototypeOf(EventTarget) === Function.prototype,
                Object.getPrototypeOf(UIEvent) === Event,
                Object.getPrototypeOf(MouseEvent) === UIEvent,
                Object.getPrototypeOf(FocusEvent) === UIEvent,
                Object.getPrototypeOf(WheelEvent) === MouseEvent,
                Object.getPrototypeOf(Event) === Function.prototype,
            ];
            const savedFile = File, savedBlob = Blob;
            const savedWheel = WheelEvent, savedMouse = MouseEvent;
            globalThis.Blob = function ReplacementBlob() {};
            globalThis.MouseEvent = function ReplacementMouseEvent() {};
            links.every(Boolean) &&
                Object.getPrototypeOf(savedFile) === savedBlob &&
                Object.getPrototypeOf(savedWheel) === savedMouse;
            "#,
        ),
        "true"
    );
}

#[test]
fn non_object_new_target_prototypes_use_the_interface_intrinsic() {
    assert_eq!(
        evaluate(
            r#"
            const intrinsic=Event.prototype;
            function Derived() {}
            const values=[undefined,null,false,7,'text'];
            const checks=values.map(value=>{
                Derived.prototype=value;
                const event=Reflect.construct(Event,['probe'],Derived);
                return Object.getPrototypeOf(event)===intrinsic && event.type==='probe';
            });
            checks.every(Boolean) && intrinsic!==Object.prototype;
            "#,
        ),
        "true"
    );
}

#[test]
fn object_new_target_prototype_wins_and_abrupt_get_is_preserved() {
    assert_eq!(
        evaluate(
            r#"
            const custom=Object.create(Event.prototype);custom.marker=37;
            function Derived() {}
            Derived.prototype=custom;
            const event=Reflect.construct(Event,['probe'],Derived);
            const marker={};let reads=0,received=false;
            const throwing=new Proxy(function Target(){},{get(target,key,receiver){
                if(key==='prototype'){reads++;throw marker}
                return Reflect.get(target,key,receiver);
            }});
            try{Reflect.construct(Event,['unseen'],throwing)}
            catch(error){received=error===marker}
            [Object.getPrototypeOf(event)===custom,event.type,received,reads].join('|');
            "#,
        ),
        "true|probe|true|1"
    );
}

#[test]
fn dom_exception_uses_its_intrinsic_and_requires_construction() {
    assert_eq!(
        evaluate(
            r#"
            const intrinsic=DOMException.prototype;
            function Derived() {}
            Derived.prototype=null;
            const error=Reflect.construct(DOMException,['message','AbortError'],Derived);
            const prototype=Object.getPrototypeOf(error)===intrinsic;
            const fields=error.name==='AbortError'&&error.message==='message';
            let ordinary=false;
            try { DOMException('message'); }
            catch (caught) { ordinary=caught instanceof TypeError; }
            [prototype,fields,ordinary].join('|');
            "#,
        ),
        "true|true|true"
    );
}

#[test]
fn constructor_arguments_convert_before_the_derived_prototype_is_read() {
    assert_eq!(
        evaluate(
            r#"
            const reads=[];
            const expected=Object.create(Event.prototype);
            const Derived=new Proxy(function Derived(){},{get(target,key,receiver){
                if(key==='prototype'){reads.push('prototype');return expected}
                return Reflect.get(target,key,receiver);
            }});
            const event=Reflect.construct(Event,['ordered',{
                get bubbles(){reads.push('argument');return true}
            }],Derived);
            [reads.join(','),event.bubbles,Object.getPrototypeOf(event)===expected].join('|');
            "#,
        ),
        "argument,prototype|true|true"
    );
}

#[test]
fn a_throwing_argument_prevents_prototype_access_and_a_plain_call_reads_neither() {
    assert_eq!(
        evaluate(
            r#"
            const marker={},reads=[];
            const Derived=new Proxy(function Derived(){},{get(target,key,receiver){
                if(key==='prototype')reads.push('prototype');
                return Reflect.get(target,key,receiver);
            }});
            let same=false;
            try{Reflect.construct(Event,['bad',{
                get bubbles(){reads.push('argument');throw marker}
            }],Derived)}catch(error){same=error===marker}
            let plain=false;
            try{Event({[Symbol.toPrimitive](){reads.push('plain argument');return 'x'}})}
            catch(error){plain=error instanceof TypeError}
            [same,plain,reads.join(',')].join('|');
            "#,
        ),
        "true|true|argument"
    );
}

#[test]
fn dom_exception_converts_its_message_before_the_derived_prototype() {
    assert_eq!(
        evaluate(
            r#"
            const reads=[];
            const expected=Object.create(DOMException.prototype);
            const Derived=new Proxy(function Derived(){},{get(target,key,receiver){
                if(key==='prototype'){reads.push('prototype');return expected}
                return Reflect.get(target,key,receiver);
            }});
            const error=Reflect.construct(DOMException,[{
                toString(){reads.push('argument');return 'message'}
            }],Derived);
            [reads.join(','),error.message,Object.getPrototypeOf(error)===expected].join('|');
            "#,
        ),
        "argument,prototype|message|true"
    );
}

#[test]
fn dom_exception_defaults_only_missing_or_undefined_arguments() {
    assert_eq!(
        evaluate(
            r#"
            const absent=new DOMException();
            const explicitUndefined=new DOMException(undefined,undefined);
            const explicitNull=new DOMException(null,null);
            const mixed=new DOMException(undefined,'AbortError');
            [absent.message,absent.name,explicitUndefined.message,
             explicitUndefined.name,explicitNull.message,explicitNull.name,
             mixed.message,mixed.name].join('|');
            "#,
        ),
        "|Error||Error|null|null||AbortError"
    );
}

#[test]
fn dom_exception_converts_both_arguments_before_the_derived_prototype() {
    assert_eq!(
        evaluate(
            r#"
            const reads=[];
            const Derived=new Proxy(function Derived(){},{get(target,key,receiver){
                if(key==='prototype'){reads.push('prototype');return DOMException.prototype}
                return Reflect.get(target,key,receiver);
            }});
            const error=Reflect.construct(DOMException,[
                {[Symbol.toPrimitive](){reads.push('message');return 'm'}},
                {[Symbol.toPrimitive](){reads.push('name');return 'n'}}
            ],Derived);
            [reads.join(','),error.message,error.name].join('|');
            "#,
        ),
        "message,name,prototype|m|n"
    );
}
