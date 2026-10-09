//! Native marked-object property order and lifetime controls.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};
use v8::MapFnTo;

fn initialize() {
  static INIT: Once = Once::new();
  INIT.call_once(|| {
    v8::V8::initialize_platform(
      v8::new_default_platform(0, false).make_shared(),
    );
    v8::V8::initialize();
  });
}

macro_rules! boundary {
  ($scope:ident, $fallback:expr, $body:block) => {
    match catch_unwind(AssertUnwindSafe(|| $body)) {
      Ok(value) => value,
      Err(payload) => {
        // Test callback assertions may panic. Never unwind through V8,
        // or run a second arbitrary payload destructor on that boundary.
        std::mem::forget(payload);
        $scope.terminate_execution();
        $fallback
      }
    }
  };
}

fn named_names<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  args: v8::PropertyCallbackArguments<'s>,
  mut rv: v8::ReturnValue<v8::Array>,
) {
  boundary!(scope, (), {
    let names =
      v8::Local::<v8::Array>::try_from(args.data()).expect("native name array");
    rv.set(names);
  });
}

fn named_query<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  name: v8::Local<'s, v8::Name>,
  args: v8::PropertyCallbackArguments<'s>,
  mut rv: v8::ReturnValue<v8::Integer>,
) -> v8::Intercepted {
  boundary!(scope, v8::Intercepted::kYes, {
    let names =
      v8::Local::<v8::Array>::try_from(args.data()).expect("native name array");
    for index in 0..names.length() {
      let candidate =
        names.get_index(scope, index).expect("native array entry");
      if candidate.strict_equals(name.into()) {
        let hidden = name.to_rust_string_lossy(scope) == "hidden";
        rv.set(v8::Integer::new(scope, if hidden { 2 } else { 0 }));
        return v8::Intercepted::kYes;
      }
    }
    v8::Intercepted::kNo
  })
}

fn indexed_names<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  _args: v8::PropertyCallbackArguments<'s>,
  mut rv: v8::ReturnValue<v8::Array>,
) {
  boundary!(scope, (), {
    // Intentionally unsorted: the marked path establishes numeric order.
    let two = v8::Integer::new_from_unsigned(scope, 2);
    let zero = v8::Integer::new_from_unsigned(scope, 0);
    rv.set(v8::Array::new_with_elements(
      scope,
      &[two.into(), zero.into()],
    ));
  });
}

fn indexed_query<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  index: u32,
  _args: v8::PropertyCallbackArguments<'s>,
  mut rv: v8::ReturnValue<v8::Integer>,
) -> v8::Intercepted {
  boundary!(scope, v8::Intercepted::kYes, {
    if index == 0 || index == 2 {
      rv.set(v8::Integer::new(scope, 0));
      v8::Intercepted::kYes
    } else {
      v8::Intercepted::kNo
    }
  })
}

fn template<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  marked: bool,
  names: &[&str],
  indexed: bool,
) -> v8::Local<'s, v8::ObjectTemplate> {
  let template = v8::ObjectTemplate::new(scope);
  let values: Vec<v8::Local<v8::Value>> = names
    .iter()
    .map(|name| v8::String::new(scope, name).expect("native name").into())
    .collect();
  let data = v8::Array::new_with_elements(scope, &values);
  let flags = || {
    if marked {
      v8::PropertyHandlerFlags::WEB_IDL_LEGACY
    } else {
      v8::PropertyHandlerFlags::NONE
    }
  };
  template.set_named_property_handler(
    v8::NamedPropertyHandlerConfiguration::new()
      .enumerator(named_names)
      .query(named_query)
      .data(data.into())
      .flags(flags() | v8::PropertyHandlerFlags::ONLY_INTERCEPT_STRINGS),
  );
  if indexed {
    template.set_indexed_property_handler(
      v8::IndexedPropertyHandlerConfiguration::new()
        .enumerator(indexed_names)
        .query(indexed_query)
        .flags(flags()),
    );
  }
  template
}

fn publish(
  scope: &mut v8::PinScope<'_, '_>,
  name: &str,
  object: v8::Local<v8::Object>,
) {
  let key = v8::String::new(scope, name).expect("global key");
  let global = scope.get_current_context().global(scope);
  assert_eq!(global.set(scope, key.into(), object.into()), Some(true));
}

fn evaluate(scope: &mut v8::PinScope<'_, '_>, source: &str) -> String {
  let source = v8::String::new(scope, source).expect("source");
  let script = v8::Script::compile(scope, source, None).expect("compile");
  script.run(scope).expect("run").to_rust_string_lossy(scope)
}

fn with_objects(test: impl FnOnce(&mut v8::PinScope<'_, '_>)) {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let mut entered = v8::ContextScope::new(scope, context);
  v8::scope!(let scope, &mut entered);
  let marked = template(scope, true, &["virtual", "hidden"], false);
  let ordinary = template(scope, false, &["virtual", "hidden"], false);
  let object = marked.new_instance(scope).expect("marked instance");
  publish(scope, "marked", object);
  let object = ordinary.new_instance(scope).expect("ordinary instance");
  publish(scope, "ordinary", object);
  test(scope);
}

#[test]
fn marked_string_chronology_precedes_symbols_and_filters_in_the_same_order() {
  with_objects(|scope| {
    assert_eq!(
      evaluate(
        scope,
        r#"
            const s = Symbol('s');
            for (const object of [marked, ordinary]) {
                object[s] = 1;
                object['10'] = 10;
                object.middle = 1;
                object['2'] = 2;
                object['4294967295'] = 3;
            }
            [Reflect.ownKeys(marked).map(String).join(','), Object.keys(marked).join(','),
             Reflect.ownKeys(ordinary).map(String).join(',')].join('|')
        "#
      ),
      "virtual,hidden,10,middle,2,4294967295,Symbol(s)|virtual,10,middle,2,4294967295|2,10,middle,4294967295,Symbol(s),virtual,hidden"
    );
  });
}

#[test]
fn descriptor_replacement_preserves_order_and_delete_recreation_moves_it() {
  with_objects(|scope| {
    assert_eq!(
      evaluate(
        scope,
        r#"
            Object.defineProperty(marked, '10', {value: 1, configurable: true, enumerable: true, writable: true});
            Object.defineProperty(marked, 'middle', {get() {return this === marked ? 7 : 0}, configurable: true, enumerable: true});
            Object.defineProperty(marked, '2', {get() {return 2}, configurable: true, enumerable: true});
            Object.defineProperty(marked, '10', {get() {return 10}});
            Object.defineProperty(marked, '2', {value: 2, writable: true});
            const before = Object.keys(marked).join(',');
            delete marked['10'];
            marked['10'] = 8;
            const after = Object.keys(marked).join(',');
            Object.defineProperty(marked, '2', {configurable: false});
            const refused = !Reflect.deleteProperty(marked, '2') &&
                !Reflect.defineProperty(marked, '2', {get() {return 9}});
            [before, after, refused, marked.middle, marked['2']].join('|')
        "#
      ),
      "virtual,10,middle,2|virtual,middle,2,10|true|7|2"
    );
  });
}

#[test]
fn marked_indexed_and_named_keys_are_normalized_once_in_spec_order() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let mut entered = v8::ContextScope::new(scope, context);
  v8::scope!(let scope, &mut entered);
  let template = template(scope, true, &["2", "10", "alpha", "0"], true);
  let object = template.new_instance(scope).expect("instance");
  publish(scope, "object", object);
  assert_eq!(
    evaluate(
      scope,
      "object[Symbol('s')]=1; object.tail=4; Reflect.ownKeys(object).map(String).join(',')"
    ),
    "0,2,10,alpha,tail,Symbol(s)"
  );
}

#[test]
fn template_clones_have_independent_creation_records() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let mut entered = v8::ContextScope::new(scope, context);
  v8::scope!(let scope, &mut entered);
  let template = template(scope, true, &[], false);
  for name in ["10", "middle", "2"] {
    let key = v8::String::new(scope, name).expect("template key");
    let value = v8::Integer::new(scope, 1);
    template.set(key.into(), value.into());
  }
  for name in ["first", "second", "third"] {
    let object = template.new_instance(scope).expect("cached instance");
    publish(scope, name, object);
  }
  assert_eq!(
    evaluate(
      scope,
      r#"
        delete first['10']; first['10']=2; second.other=3;
        [Object.keys(first).join(','), Object.keys(second).join(','), Object.keys(third).join(',')].join('|')
    "#
    ),
    "middle,2,10|10,middle,2,other|10,middle,2"
  );
}

#[test]
fn hot_named_and_numeric_stores_keep_chronology_across_dictionary_transition() {
  with_objects(|scope| {
    assert_eq!(
      evaluate(
        scope,
        r#"
            function write(object, key, value) { object[key] = value; }
            for (let i=0;i<512;i++) {
                write(marked, String(4096-i), i);
                write(marked, 'p'+i, i);
            }
            const expected = ['virtual'];
            for (let i=0;i<512;i++) expected.push(String(4096-i), 'p'+i);
            const equal = JSON.stringify(Object.keys(marked)) === JSON.stringify(expected);
            for (let i=0;i<512;i++) { delete marked[String(4096-i)]; delete marked['p'+i]; }
            for (let i=0;i<512;i++) { write(marked, 'q'+i, i); write(marked, String(8192-i), i); }
            const again = ['virtual'];
            for (let i=0;i<512;i++) again.push('q'+i, String(8192-i));
            equal && JSON.stringify(Object.keys(marked)) === JSON.stringify(again)
        "#
      ),
      "true"
    );
  });
}

#[test]
fn ordinary_store_receiver_controls_where_creation_is_recorded() {
  with_objects(|scope| {
    assert_eq!(
      evaluate(
        scope,
        r#"
            const derived = Object.create(marked);
            Reflect.set(marked, '10', 1, derived);
            Reflect.set(marked, 'middle', 2, derived);
            Reflect.set(marked, '2', 3, derived);
            Reflect.set({}, '10', 1, marked);
            Reflect.set({}, 'middle', 2, marked);
            Reflect.set({}, '2', 3, marked);
            [Object.keys(derived).join(','), Object.keys(marked).join(',')].join('|')
        "#
      ),
      "2,10,middle|virtual,10,middle,2"
    );
  });
}

#[test]
fn nonenumerable_ordinary_keys_shadow_prototype_keys() {
  with_objects(|scope| {
    assert_eq!(
      evaluate(
        scope,
        r#"
            const proto = {'10': 1, tail: 2, hidden: 3};
            Object.setPrototypeOf(marked, proto);
            Object.defineProperty(marked, '10', {value: 4, enumerable: false});
            marked.middle = 5;
            const keys=[]; for (const key in marked) keys.push(key);
            [Object.keys(marked).join(','), keys.join(','), Reflect.ownKeys(marked).join(',')].join('|')
        "#
      ),
      "virtual,middle|virtual,middle,tail|virtual,hidden,10,middle"
    );
  });
}

#[test]
fn weak_order_records_do_not_retain_dead_instances() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  let (weak, template_root, context_root) = {
    v8::scope!(let scope, isolate);
    let context = v8::Context::new(scope, Default::default());
    let mut entered = v8::ContextScope::new(scope, context);
    v8::scope!(let scope, &mut entered);
    let template = template(scope, true, &[], false);
    let object = template.new_instance(scope).expect("instance");
    let key = v8::String::new(scope, "10").expect("key");
    assert_eq!(object.set(scope, key.into(), object.into()), Some(true));
    (
      v8::Weak::new(scope, object),
      v8::Global::new(scope, template),
      v8::Global::new(scope, context),
    )
  };
  isolate.perform_microtask_checkpoint();
  isolate.low_memory_notification();
  isolate.low_memory_notification();
  assert!(
    weak.is_empty(),
    "template-owned metadata retained its weak instance"
  );
  drop(context_root);
  drop(template_root);
}

static ENUMERATOR_CALLS: AtomicUsize = AtomicUsize::new(0);

fn names_with_gc<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  args: v8::PropertyCallbackArguments<'s>,
  mut rv: v8::ReturnValue<v8::Array>,
) {
  boundary!(scope, (), {
    ENUMERATOR_CALLS.fetch_add(1, Ordering::SeqCst);
    scope.low_memory_notification();
    let names =
      v8::Local::<v8::Array>::try_from(args.data()).expect("native names");
    rv.set(names);
  });
}

fn throwing_names<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  _args: v8::PropertyCallbackArguments<'s>,
  _rv: v8::ReturnValue<v8::Array>,
) {
  boundary!(scope, (), {
    ENUMERATOR_CALLS.fetch_add(1, Ordering::SeqCst);
    let message =
      v8::String::new(scope, "native enumerator failure").expect("message");
    let error = v8::Exception::range_error(scope, message);
    scope.throw_exception(error);
  });
}

#[test]
fn enumerator_gc_and_exceptions_preserve_exactly_once_callback_behavior() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let mut entered = v8::ContextScope::new(scope, context);
  v8::scope!(let scope, &mut entered);
  for (name, throws) in [("withGc", false), ("throwing", true)] {
    let object_template = v8::ObjectTemplate::new(scope);
    let key = v8::String::new(scope, "virtual").expect("key");
    let data = v8::Array::new_with_elements(scope, &[key.into()]);
    let configuration = v8::NamedPropertyHandlerConfiguration::new()
      .data(data.into())
      .query(named_query)
      .flags(
        v8::PropertyHandlerFlags::ONLY_INTERCEPT_STRINGS
          | v8::PropertyHandlerFlags::WEB_IDL_LEGACY,
      );
    let configuration = if throws {
      configuration.enumerator(throwing_names)
    } else {
      configuration.enumerator(names_with_gc)
    };
    object_template.set_named_property_handler(configuration);
    let object = object_template.new_instance(scope).expect("instance");
    publish(scope, name, object);
  }
  ENUMERATOR_CALLS.store(0, Ordering::SeqCst);
  assert_eq!(
    evaluate(
      scope,
      r#"
        withGc[Symbol('s')]=1; withGc['10']=2; withGc.middle=3; withGc['2']=4;
        const results = [Reflect.ownKeys(withGc).map(String).join(','), Object.keys(withGc).join(',')];
        try { Reflect.ownKeys(throwing); } catch (error) { results.push(error.name); }
        try { Object.keys(throwing); } catch (error) { results.push(error.name); }
        results.join('|')
    "#
    ),
    "virtual,10,middle,2,Symbol(s)|virtual,10,middle,2|RangeError|RangeError"
  );
  assert_eq!(ENUMERATOR_CALLS.load(Ordering::SeqCst), 4);
}

#[test]
fn snapshot_round_trip_preserves_order_and_accepts_new_records() {
  initialize();
  let references = [
    v8::ExternalReference {
      enumerator: named_names.map_fn_to(),
    },
    v8::ExternalReference {
      named_query: named_query.map_fn_to(),
    },
  ];
  let data = {
    let mut creator =
      v8::Isolate::snapshot_creator(Some(references.to_vec().into()), None);
    {
      v8::scope!(let scope, &mut creator);
      let context = v8::Context::new(scope, Default::default());
      let mut entered = v8::ContextScope::new(scope, context);
      v8::scope!(let scope, &mut entered);
      let template = template(scope, true, &["virtual"], false);
      let object = template.new_instance(scope).expect("instance");
      publish(scope, "object", object);
      assert_eq!(
        evaluate(
          scope,
          "object['10']=1; object.middle=2; object['2']=3; Object.keys(object).join(',')"
        ),
        "virtual,10,middle,2"
      );
      scope.set_default_context(context);
    }
    creator
      .create_blob(v8::FunctionCodeHandling::Clear)
      .expect("snapshot")
  };
  let params = v8::Isolate::create_params()
    .snapshot_blob(data)
    .external_references(references.to_vec().into());
  let isolate = &mut v8::Isolate::new(params);
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let mut entered = v8::ContextScope::new(scope, context);
  v8::scope!(let scope, &mut entered);
  let original_state = "[Object.getOwnPropertyDescriptor(object,'middle').value, Object.getOwnPropertyDescriptor(object,'2').value, Reflect.ownKeys(object).join(',')].join('|')";
  assert_eq!(
    evaluate(scope, original_state),
    "2|3|virtual,10,middle,2",
    "immediately after restore"
  );
  scope.low_memory_notification();
  assert_eq!(
    evaluate(scope, original_state),
    "2|3|virtual,10,middle,2",
    "after restored-isolate GC"
  );
  assert_eq!(
    evaluate(
      scope,
      "delete object['10']; object['10']=4; object.after=5; Object.keys(object).join(',')"
    ),
    "virtual,middle,2,10,after"
  );
}

#[test]
fn ordinary_derived_receiver_enumerates_marked_prototype_without_cached_reordering()
 {
  with_objects(|scope| {
    assert_eq!(
      evaluate(
        scope,
        r#"
            marked['10']=1; marked.middle=2; marked['2']=3;
            const derived = Object.create(marked);
            derived.own=1;
            const first=[]; for (const key in derived) first.push(key);
            delete marked['10']; marked['10']=4;
            const second=[]; for (const key in derived) second.push(key);
            first.join(',')+'|'+second.join(',')
        "#
      ),
      "own,virtual,10,middle,2|own,virtual,middle,2,10"
    );
  });
}

#[test]
fn failed_definitions_do_not_publish_phantom_creation_records() {
  with_objects(|scope| {
    assert_eq!(
      evaluate(
        scope,
        r#"
            marked['10']=1; marked.middle=2; marked['2']=3;
            let threw = false;
            try {
                Object.defineProperty(marked, 'failed', {get value(){throw new RangeError('descriptor')}});
            } catch (error) { threw = error instanceof RangeError; }
            Object.defineProperty(marked, 'middle', {writable:false, configurable:false});
            const refused = !Reflect.defineProperty(marked, 'middle', {value:9}) &&
                !Reflect.set(marked, 'middle', 4) && !Reflect.preventExtensions(marked);
            [threw, refused, Object.keys(marked).join(','), Object.hasOwn(marked,'failed'), marked.middle].join('|')
        "#
      ),
      "true|true|virtual,10,middle,2|false|2"
    );
  });
}

#[test]
fn growing_weak_instance_table_preserves_live_order_and_releases_dead_keys() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  let (weak, survivors, template_root, context_root) = {
    v8::scope!(let scope, isolate);
    let context = v8::Context::new(scope, Default::default());
    let mut entered = v8::ContextScope::new(scope, context);
    v8::scope!(let scope, &mut entered);
    let template = template(scope, true, &[], false);
    let mut weak = Vec::new();
    let mut survivors = Vec::new();
    for index in 0..1024 {
      let object = template.new_instance(scope).expect("instance");
      for name in ["10", "middle", "2"] {
        let key = v8::String::new(scope, name).expect("key");
        assert_eq!(
          object.create_data_property(scope, key.into(), object.into()),
          Some(true)
        );
      }
      if index % 128 == 0 {
        survivors.push(v8::Global::new(scope, object));
      } else {
        weak.push(v8::Weak::new(scope, object));
      }
    }
    (
      weak,
      survivors,
      v8::Global::new(scope, template),
      v8::Global::new(scope, context),
    )
  };
  isolate.low_memory_notification();
  isolate.low_memory_notification();
  assert!(
    weak.iter().all(v8::Weak::is_empty),
    "weak metadata retained instances after growth"
  );
  {
    v8::scope!(let scope, isolate);
    let context = v8::Local::new(scope, &context_root);
    let mut entered = v8::ContextScope::new(scope, context);
    v8::scope!(let scope, &mut entered);
    for object in &survivors {
      let object = v8::Local::new(scope, object);
      publish(scope, "live", object);
      assert_eq!(
        evaluate(scope, "Object.keys(live).join(',')"),
        "10,middle,2"
      );
    }
  }
  drop(survivors);
  drop(template_root);
  drop(context_root);
}

#[test]
fn numeric_own_key_shadows_an_unmarked_named_interceptor_prototype() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let mut entered = v8::ContextScope::new(scope, context);
  v8::scope!(let scope, &mut entered);
  let marked_template = template(scope, true, &[], false);
  let plain_template = template(scope, false, &["10", "tail"], false);
  let marked = marked_template.new_instance(scope).expect("marked");
  let plain = plain_template.new_instance(scope).expect("plain");
  publish(scope, "marked", marked);
  publish(scope, "plain", plain);
  assert_eq!(
    evaluate(
      scope,
      r#"
        Object.setPrototypeOf(marked, plain);
        Object.defineProperty(marked, '10', {value:1, enumerable:false});
        marked.middle=2;
        const keys=[]; for (const key in marked) keys.push(key);
        keys.join(',')
    "#
    ),
    "middle,tail"
  );
  // Unlike JavaScript for-in, this native enumeration has no later
  // ForInFilter step that could hide an incorrect accumulator result.
  for key_conversion in [
    v8::KeyConversionMode::KeepNumbers,
    v8::KeyConversionMode::ConvertToString,
  ] {
    let keys = marked
      .get_property_names(
        scope,
        v8::GetPropertyNamesArgs {
          mode: v8::KeyCollectionMode::IncludePrototypes,
          property_filter: v8::PropertyFilter::ONLY_ENUMERABLE
            | v8::PropertyFilter::SKIP_SYMBOLS,
          index_filter: v8::IndexFilter::IncludeIndices,
          key_conversion,
        },
      )
      .expect("native filtered enumeration");
    let names: Vec<String> = (0..keys.length())
      .map(|index| {
        keys
          .get_index(scope, index)
          .expect("enumerated key")
          .to_rust_string_lossy(scope)
      })
      .collect();
    assert_eq!(names, ["middle", "tail"]);
  }
}

#[test]
fn named_only_marked_prototype_observes_updated_supported_names() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let mut entered = v8::ContextScope::new(scope, context);
  v8::scope!(let scope, &mut entered);
  let template = v8::ObjectTemplate::new(scope);
  let first = v8::String::new(scope, "first").expect("first supported name");
  let names = v8::Array::new_with_elements(scope, &[first.into()]);
  template.set_named_property_handler(
    v8::NamedPropertyHandlerConfiguration::new()
      .enumerator(named_names)
      .query(named_query)
      .data(names.into())
      .flags(
        v8::PropertyHandlerFlags::ONLY_INTERCEPT_STRINGS
          | v8::PropertyHandlerFlags::WEB_IDL_LEGACY,
      ),
  );
  let marked = template.new_instance(scope).expect("marked instance");
  publish(scope, "marked", marked);
  publish(scope, "supportedNames", names.into());
  assert_eq!(
    evaluate(
      scope,
      r#"
        const derived=Object.create(marked);
        derived.own=1;
        const before=[]; for (const key in derived) before.push(key);
        supportedNames[0]='replacement';
        const after=[]; for (const key in derived) after.push(key);
        before.join(',')+'|'+after.join(',')
    "#
    ),
    "own,first|own,replacement"
  );
}

#[test]
fn cross_realm_access_keeps_the_creation_context_order() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let first = v8::Context::new(scope, Default::default());
  let second = v8::Context::new(scope, Default::default());
  let shared = {
    let mut entered = v8::ContextScope::new(scope, first);
    v8::scope!(let scope, &mut entered);
    let template = template(scope, true, &["virtual"], false);
    let object = template.new_instance(scope).expect("first-realm object");
    publish(scope, "object", object);
    assert_eq!(
      evaluate(
        scope,
        "object['10']=1; object.middle=2; object['2']=3; Object.keys(object).join(',')"
      ),
      "virtual,10,middle,2"
    );
    v8::Global::new(scope, object)
  };
  {
    let mut entered = v8::ContextScope::new(scope, second);
    v8::scope!(let scope, &mut entered);
    let object = v8::Local::new(scope, &shared);
    publish(scope, "foreign", object);
    scope.low_memory_notification();
    assert_eq!(
      evaluate(
        scope,
        "delete foreign['10']; foreign['10']=4; foreign.after=5; Object.keys(foreign).join(',')"
      ),
      "virtual,middle,2,10,after"
    );
  }
  let mut entered = v8::ContextScope::new(scope, first);
  v8::scope!(let scope, &mut entered);
  assert_eq!(
    evaluate(scope, "Object.keys(object).join(',')"),
    "virtual,middle,2,10,after"
  );
}

#[test]
fn marked_global_template_uses_its_hidden_global_creation_context() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let setup = v8::Context::new(scope, Default::default());
  let mut setup_entered = v8::ContextScope::new(scope, setup);
  v8::scope!(let scope, &mut setup_entered);
  let global_template = template(scope, true, &["virtual"], false);
  let context = v8::Context::new(
    scope,
    v8::ContextOptions {
      global_template: Some(global_template),
      ..Default::default()
    },
  );
  let mut entered = v8::ContextScope::new(scope, context);
  v8::scope!(let scope, &mut entered);
  assert_eq!(
    evaluate(
      scope,
      "globalThis['10']=1; globalThis.middle=2; globalThis['2']=3; Reflect.ownKeys(globalThis).filter(k=>['virtual','10','middle','2'].includes(k)).join(',')"
    ),
    "virtual,10,middle,2"
  );
  scope.low_memory_notification();
  assert_eq!(
    evaluate(
      scope,
      "delete globalThis['10']; globalThis['10']=4; Reflect.ownKeys(globalThis).filter(k=>['virtual','10','middle','2'].includes(k)).join(',')"
    ),
    "virtual,middle,2,10"
  );
}

#[test]
fn independently_restored_contexts_keep_distinct_order_tables() {
  initialize();
  let references = [
    v8::ExternalReference {
      enumerator: named_names.map_fn_to(),
    },
    v8::ExternalReference {
      named_query: named_query.map_fn_to(),
    },
  ];
  let data = {
    let mut creator =
      v8::Isolate::snapshot_creator(Some(references.to_vec().into()), None);
    {
      v8::scope!(let scope, &mut creator);
      let context = v8::Context::new(scope, Default::default());
      let mut entered = v8::ContextScope::new(scope, context);
      v8::scope!(let scope, &mut entered);
      let template = template(scope, true, &["virtual"], false);
      let object = template.new_instance(scope).expect("snapshot instance");
      publish(scope, "object", object);
      assert_eq!(
        evaluate(
          scope,
          "object['10']=1; object.middle=2; object['2']=3; Object.keys(object).join(',')"
        ),
        "virtual,10,middle,2"
      );
      scope.set_default_context(context);
    }
    creator
      .create_blob(v8::FunctionCodeHandling::Clear)
      .expect("snapshot")
  };
  let params = v8::Isolate::create_params()
    .snapshot_blob(data)
    .external_references(references.to_vec().into());
  let isolate = &mut v8::Isolate::new(params);
  v8::scope!(let scope, isolate);
  let first = v8::Context::new(scope, Default::default());
  let second = v8::Context::new(scope, Default::default());
  {
    let mut entered = v8::ContextScope::new(scope, first);
    v8::scope!(let scope, &mut entered);
    assert_eq!(
      evaluate(
        scope,
        "delete object['10']; object['10']=4; object.first=5; Object.keys(object).join(',')"
      ),
      "virtual,middle,2,10,first"
    );
  }
  {
    let mut entered = v8::ContextScope::new(scope, second);
    v8::scope!(let scope, &mut entered);
    scope.low_memory_notification();
    assert_eq!(
      evaluate(scope, "object.second=6; Object.keys(object).join(',')"),
      "virtual,10,middle,2,second"
    );
  }
  let mut entered = v8::ContextScope::new(scope, first);
  v8::scope!(let scope, &mut entered);
  assert_eq!(
    evaluate(scope, "Object.keys(object).join(',')"),
    "virtual,middle,2,10,first"
  );
}

fn publish_indexed_only(scope: &mut v8::PinScope<'_, '_>) {
  let template = v8::ObjectTemplate::new(scope);
  template.set_indexed_property_handler(
    v8::IndexedPropertyHandlerConfiguration::new()
      .enumerator(indexed_names)
      .query(indexed_query)
      .flags(v8::PropertyHandlerFlags::WEB_IDL_LEGACY),
  );
  let object = template.new_instance(scope).expect("indexed-only instance");
  publish(scope, "indexedOnly", object);
}

#[test]
fn public_symbol_store_keeps_ordinary_descriptor_and_order() {
  with_objects(|scope| {
    publish_indexed_only(scope);
    assert_eq!(
      evaluate(
        scope,
        r#"
            (() => {
                for (const object of [marked, ordinary, indexedOnly]) {
                    const first = Symbol('first'), last = Symbol.iterator;
                    function cycle(value) { object[first] = value; const got = object[first]; delete object[first]; return got; }
                    let checksum = 0;
                    for (let i = 0; i < 10000; ++i) checksum += cycle(i);
                    if (checksum !== 49995000) throw Error('checksum');
                    object[first] = 17; object[last] = 23;
                    const d = Object.getOwnPropertyDescriptor(object, first);
                    if (!d.writable || !d.enumerable || !d.configurable || d.value !== 17) throw Error('descriptor');
                    let keys = Object.getOwnPropertySymbols(object);
                    if (keys.length !== 2 || keys[0] !== first || keys[1] !== last) throw Error('first order');
                    delete object[first]; object[first] = 29;
                    keys = Object.getOwnPropertySymbols(object);
                    if (keys[0] !== last || keys[1] !== first || object[last] !== 23) throw Error('reinsert order');
                }
                marked['10'] = 1; marked.middle = 2; marked['2'] = 3;
                if (Object.keys(marked).join(',') !== 'virtual,10,middle,2') throw Error('string order');
                if (Object.keys(indexedOnly).join(',') !== '0,2') throw Error('index order');
                return 'ok';
            })()
        "#
      ),
      "ok"
    );
  });
}

#[test]
fn public_symbol_store_obeys_accessors_and_readonly() {
  with_objects(|scope| {
    assert_eq!(
      evaluate(
        scope,
        r#"
            (() => {
                for (const object of [marked, ordinary]) {
                    const key = Symbol('setter');
                    function store(value) { object[key] = value; }
                    function strictStore(value) { 'use strict'; object[key] = value; }
                    for (let i = 0; i < 10000; ++i) store(i);
                    let calls = 0, receiver, seen;
                    const setter = function(value) { ++calls; receiver = this; seen = value; };
                    Object.defineProperty(object, key, {set: setter, configurable: true});
                    store(123);
                    if (calls !== 1 || receiver !== object || seen !== 123 || Object.getOwnPropertyDescriptor(object, key).set !== setter) throw Error('own setter');
                    Object.defineProperty(object, key, {value: 7, writable: false, configurable: true});
                    store(99);
                    if (object[key] !== 7 || Reflect.set(object, key, 88)) throw Error('own readonly');
                    let threw = false;
                    try { strictStore(66); } catch (e) { threw = e instanceof TypeError; }
                    if (!threw || object[key] !== 7) throw Error('strict readonly');
                    delete object[key];
                    const proto = Object.create(Object.getPrototypeOf(object));
                    Object.setPrototypeOf(object, proto);
                    Object.defineProperty(proto, key, {set: setter, configurable: true});
                    store(321);
                    if (calls !== 2 || receiver !== object || seen !== 321 || Object.hasOwn(object, key)) throw Error('inherited setter');
                    Object.defineProperty(proto, key, {value: 11, writable: false, configurable: true});
                    store(55);
                    if (Reflect.set(object, key, 44) || Object.hasOwn(object, key) || object[key] !== 11) throw Error('inherited readonly');
                    threw = false;
                    try { strictStore(33); } catch (e) { threw = e instanceof TypeError; }
                    if (!threw) throw Error('inherited strict readonly');
                }
                return 'ok';
            })()
        "#
      ),
      "ok"
    );
  });
}

#[test]
fn public_symbol_store_obeys_extensibility_and_distinct_receiver() {
  with_objects(|scope| {
    assert_eq!(
      evaluate(
        scope,
        r#"
            (() => {
                for (const target of [marked, ordinary]) {
                    const key = Symbol('distinct');
                    target[key] = 1;
                    const receiver = {};
                    function store(value) { return Reflect.set(target, key, value, receiver); }
                    for (let i = 0; i < 10000; ++i) if (!store(i)) throw Error('warmup');
                    if (receiver[key] !== 9999 || target[key] !== 1) throw Error('distinct receiver');
                    Object.defineProperty(receiver, key, {value: 5, writable: false, configurable: true});
                    if (store(9) || receiver[key] !== 5 || target[key] !== 1) throw Error('receiver readonly');
                    let calls = 0;
                    Object.defineProperty(receiver, key, {set() { ++calls; }, configurable: true});
                    if (store(9) || calls !== 0) throw Error('receiver accessor');
                    delete receiver[key]; Object.preventExtensions(receiver);
                    if (store(9) || Object.hasOwn(receiver, key)) throw Error('nonextensible receiver');
                    const fresh = Symbol('fresh');
                    const ordinaryTarget = {[fresh]: 3};
                    if (!Reflect.set(ordinaryTarget, fresh, 4, target) || target[fresh] !== 4 || ordinaryTarget[fresh] !== 3) throw Error('marked receiver');
                    Object.defineProperty(target, fresh, {value: 8, writable: false, configurable: true});
                    if (Reflect.set(ordinaryTarget, fresh, 4, target) || target[fresh] !== 8) throw Error('marked receiver readonly');
                    Object.defineProperty(target, fresh, {set() { ++calls; }, configurable: true});
                    if (Reflect.set(ordinaryTarget, fresh, 4, target) || calls !== 0) throw Error('marked receiver accessor');
                    // Interceptor objects deliberately refuse PreventExtensions in this V8.
                    if (Reflect.preventExtensions(target) || !Object.isExtensible(target)) throw Error('API extensibility contract');
                }
                const key = Symbol('ordinary sealed');
                const plain = {[key]: 1};
                function write(value) { plain[key] = value; }
                for (let i = 0; i < 10000; ++i) write(i);
                Object.preventExtensions(plain); write(13);
                const fresh = Symbol('new');
                if (plain[key] !== 13 || Reflect.set(plain, fresh, 7)) throw Error('plain nonextensible');
                let threw = false;
                try { (function() { 'use strict'; plain[fresh] = 9; })(); } catch (e) { threw = e instanceof TypeError; }
                if (!threw || Object.hasOwn(plain, fresh)) throw Error('strict nonextensible');
                return 'ok';
            })()
        "#
      ),
      "ok"
    );
  });
}

fn intercept_symbol_store<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  name: v8::Local<'s, v8::Name>,
  value: v8::Local<'s, v8::Value>,
  args: v8::PropertyCallbackArguments<'s>,
  mut rv: v8::ReturnValue<v8::Boolean>,
) -> v8::Intercepted {
  boundary!(scope, v8::Intercepted::kYes, {
    if !name.is_symbol() {
      return v8::Intercepted::kNo;
    }
    let data =
      v8::Local::<v8::Array>::try_from(args.data()).expect("counter array");
    let count = data
      .get_index(scope, 0)
      .expect("counter")
      .uint32_value(scope)
      .expect("integer");
    let next = v8::Integer::new_from_unsigned(scope, count + 1);
    assert_eq!(data.set_index(scope, 0, next.into()), Some(true));
    assert_eq!(data.set_index(scope, 1, value), Some(true));
    rv.set_bool(true);
    v8::Intercepted::kYes
  })
}

#[test]
fn symbol_intercepting_handler_keeps_its_callbacks() {
  with_objects(|scope| {
    for (name, indexed_mark) in
      [("symbolNamed", false), ("symbolIndexed", true)]
    {
      let template = v8::ObjectTemplate::new(scope);
      let zero = v8::Integer::new(scope, 0);
      let data =
        v8::Array::new_with_elements(scope, &[zero.into(), zero.into()]);
      template.set_named_property_handler(
        v8::NamedPropertyHandlerConfiguration::new()
          .setter(intercept_symbol_store)
          .data(data.into())
          .flags(if indexed_mark {
            v8::PropertyHandlerFlags::NONE
          } else {
            v8::PropertyHandlerFlags::WEB_IDL_LEGACY
          }),
      );
      if indexed_mark {
        template.set_indexed_property_handler(
          v8::IndexedPropertyHandlerConfiguration::new()
            .query(indexed_query)
            .flags(v8::PropertyHandlerFlags::WEB_IDL_LEGACY),
        );
      }
      let object = template.new_instance(scope).expect("symbol interceptor");
      publish(scope, name, object);
      publish(scope, &format!("{name}Counts"), data.into());
    }
    assert_eq!(
      evaluate(
        scope,
        r#"
            (() => {
                for (const [object, counts] of [[symbolNamed, symbolNamedCounts], [symbolIndexed, symbolIndexedCounts]]) {
                    const key = Symbol('intercepted');
                    function store(value) { object[key] = value; }
                    for (let i = 0; i < 10000; ++i) store(i);
                    if (counts[0] !== 10000 || counts[1] !== 9999 || Object.hasOwn(object, key) || Object.getOwnPropertySymbols(object).length) throw Error('interception');
                }
                return 'ok';
            })()
        "#
      ),
      "ok"
    );
  });
}

#[test]
fn private_keys_stay_out_of_public_symbol_exemption() {
  with_objects(|scope| {
    let key = v8::String::new(scope, "marked").expect("key");
    let global = scope.get_current_context().global(scope);
    let value = global.get(scope, key.into()).expect("marked");
    let object = v8::Local::<v8::Object>::try_from(value).expect("object");
    let private = v8::Private::new(scope, None);
    let sentinel = v8::Integer::new(scope, 42);
    assert_eq!(
      object.set_private(scope, private, sentinel.into()),
      Some(true)
    );
    assert_eq!(
      object
        .get_private(scope, private)
        .expect("private")
        .int32_value(scope),
      Some(42)
    );
    assert_eq!(object.delete_private(scope, private), Some(true));
    assert!(
      object
        .get_private(scope, private)
        .expect("deleted private")
        .is_undefined()
    );
    assert_eq!(
      evaluate(
        scope,
        r#"
            (() => {
                class ReturnTarget { constructor(target) { return target; } }
                class Stamp extends ReturnTarget {
                    #value = 1;
                    static read(target) { return target.#value; }
                    static write(target, value) { target.#value = value; }
                    static has(target) { return #value in target; }
                }
                for (const object of [marked, ordinary]) {
                    new Stamp(object);
                    for (let i = 0; i < 10000; ++i) Stamp.write(object, i);
                    if (Stamp.read(object) !== 9999 || !Stamp.has(object) || Object.getOwnPropertySymbols(object).length) throw Error('private brand');
                    let threw = false;
                    try { new Stamp(object); } catch (e) { threw = e instanceof TypeError; }
                    if (!threw) throw Error('duplicate brand');
                }
                if (Stamp.has({})) throw Error('brand leaked');
                let threw = false;
                try { Stamp.write({}, 8); } catch (e) { threw = e instanceof TypeError; }
                if (!threw) throw Error('missing brand');
                return 'ok';
            })()
        "#
      ),
      "ok"
    );
  });
}
