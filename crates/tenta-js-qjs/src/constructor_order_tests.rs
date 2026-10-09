use super::Engine;

#[test]
fn abrupt_constructor_conversion_and_prototype_get_do_not_publish_a_host() {
    let mut engine = Engine::new().expect("QuickJS runtime");
    let initial = engine.state.world.host.events.borrow().len();

    let converted = engine
        .evaluate(
            r#"
            const conversionMarker={},conversionReads=[];
            const ConversionTarget=new Proxy(function(){},{get(target,key,receiver){
                if(key==='prototype')conversionReads.push('prototype');
                return Reflect.get(target,key,receiver);
            }});
            let conversionSame=false;
            try{Reflect.construct(Event,['failed',{
                get bubbles(){conversionReads.push('argument');throw conversionMarker}
            }],ConversionTarget)}catch(error){conversionSame=error===conversionMarker}
            [conversionSame,conversionReads.join(',')].join('|');
            "#,
        )
        .expect("conversion failure remains catchable");
    assert_eq!(converted, "true|argument");
    assert_eq!(engine.state.world.host.events.borrow().len(), initial);

    let prototype = engine
        .evaluate(
            r#"
            const prototypeMarker={},prototypeReads=[];
            const PrototypeTarget=new Proxy(function(){},{get(target,key,receiver){
                if(key==='prototype'){prototypeReads.push('prototype');throw prototypeMarker}
                return Reflect.get(target,key,receiver);
            }});
            let prototypeSame=false;
            try{Reflect.construct(Event,['failed',{
                get bubbles(){prototypeReads.push('argument');return true}
            }],PrototypeTarget)}catch(error){prototypeSame=error===prototypeMarker}
            [prototypeSame,prototypeReads.join(',')].join('|');
            "#,
        )
        .expect("prototype failure remains catchable");
    assert_eq!(prototype, "true|argument,prototype");
    assert_eq!(engine.state.world.host.events.borrow().len(), initial);

    assert_eq!(
        engine
            .evaluate("globalThis.survivor=new Event('alive');survivor.type")
            .expect("the next constructor remains healthy"),
        "alive"
    );
    assert_eq!(engine.state.world.host.events.borrow().len(), initial + 1);
    assert_eq!(engine.state.world.native_failure.get(), None);
}
