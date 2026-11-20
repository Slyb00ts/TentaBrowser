//! `ResizeObserver` and `IntersectionObserver`.
//!
//! Both are built the first time a page names them: each is a lazy data
//! property of the global, so a realm that never asks pays for neither.
//!
//! What they report follows from what this engine is. Nothing scrolls and
//! there is no viewport to leave, so the whole document counts as in view:
//!
//! - **`IntersectionObserver`** tells its callback once, in a task after
//!   `observe()`, that each target is intersecting (`isIntersecting` true,
//!   `intersectionRatio` 1, the target's own box as both rectangles). That is
//!   the notification a page that loads content "when it scrolls into view"
//!   is waiting for. No later notification is made.
//! - **`ResizeObserver`** reports each target once, in a task after
//!   `observe()`, with the box `getBoundingClientRect()` gives — and, as the
//!   standard has it, only if that box is not empty, which it is on a page
//!   with no layout. Later size changes are not observed.
//!
//! Entries are plain objects with the standard's members rather than
//! instances of `ResizeObserverEntry` and `IntersectionObserverEntry`, which
//! are not defined.

use tenta_js::bindings::{
    throw_type_error, v8_begin_stored_value_lease, v8_end_stored_value_lease,
    v8_registered_slow_callback_boundary,
};

use super::queue_task;

guarded_callback!(guarded_resize_constructor => resize_constructor);
guarded_callback!(guarded_intersection_constructor => intersection_constructor);
guarded_callback!(guarded_observe => observe);
guarded_callback!(guarded_unobserve => unobserve);
guarded_callback!(guarded_disconnect => disconnect);
guarded_callback!(guarded_take_records => take_records);
guarded_callback!(guarded_deliver => deliver);

/// The private property that holds a `ResizeObserver`'s callback; its
/// presence is the brand of one.
const RESIZE: &str = "tenta:ResizeObserver#callback";
/// The same for an `IntersectionObserver`.
const INTERSECTION: &str = "tenta:IntersectionObserver#callback";
/// The targets being observed, as an array.
const OBSERVED: &str = "tenta:Observer#observed";
/// The observed targets that have not been reported yet, as an array.
const PENDING: &str = "tenta:Observer#pending";
/// Whether a delivery task is queued.
const SCHEDULED: &str = "tenta:Observer#scheduled";

pub(super) fn key<'s>(scope: &mut v8::PinScope<'s, '_>, name: &str) -> v8::Local<'s, v8::String> {
    v8::String::new(scope, name).expect("a member name fits V8")
}

fn private<'s>(scope: &mut v8::PinScope<'s, '_>, name: &str) -> v8::Local<'s, v8::Private> {
    let name = key(scope, name);
    v8::Private::for_api(scope, Some(name))
}

pub(super) fn get<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<v8::Object>,
    name: &str,
) -> Option<v8::Local<'s, v8::Value>> {
    let name = key(scope, name);
    object.get(scope, name.into())
}

pub(super) fn set(
    scope: &mut v8::PinScope,
    object: v8::Local<v8::Object>,
    name: &str,
    value: v8::Local<v8::Value>,
) {
    let name = key(scope, name);
    object.set(scope, name.into(), value);
}

pub(super) fn get_private<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<v8::Object>,
    name: &str,
) -> Option<v8::Local<'s, v8::Value>> {
    let name = private(scope, name);
    object
        .get_private(scope, name)
        .filter(|value| !value.is_undefined())
}

pub(super) fn set_private(
    scope: &mut v8::PinScope,
    object: v8::Local<v8::Object>,
    name: &str,
    value: v8::Local<v8::Value>,
) {
    let name = private(scope, name);
    object.set_private(scope, name, value);
}

/// Defines `ResizeObserver` and `IntersectionObserver` on `global`, each
/// built on first use.
pub(super) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    let undefined = v8::undefined(scope);
    let resize = key(scope, "ResizeObserver");
    global.set_lazy_data_property_with_data(
        scope,
        resize.into(),
        lazy_resize_observer,
        undefined.into(),
        v8::PropertyAttribute::DONT_ENUM,
        v8::SideEffectType::HasNoSideEffect,
        v8::SideEffectType::HasSideEffect,
    );
    let intersection = key(scope, "IntersectionObserver");
    global.set_lazy_data_property_with_data(
        scope,
        intersection.into(),
        lazy_intersection_observer,
        undefined.into(),
        v8::PropertyAttribute::DONT_ENUM,
        v8::SideEffectType::HasNoSideEffect,
        v8::SideEffectType::HasSideEffect,
    );
}

fn lazy_resize_observer<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _name: v8::Local<'s, v8::Name>,
    _args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'_, v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        if let Some(interface) = interface(scope, "ResizeObserver", false) {
            rv.set(interface.into());
        }
    };
    // SAFETY: a registered V8 property callback, where allocation is allowed.
    unsafe { v8_registered_slow_callback_boundary(scope, "lazy_resize_observer", (), call) };
}

fn lazy_intersection_observer<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _name: v8::Local<'s, v8::Name>,
    _args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'_, v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        if let Some(interface) = interface(scope, "IntersectionObserver", true) {
            rv.set(interface.into());
        }
    };
    // SAFETY: a registered V8 property callback, where allocation is allowed.
    unsafe { v8_registered_slow_callback_boundary(scope, "lazy_intersection_observer", (), call) };
}

/// The interface object `name`: a constructor whose prototype has the
/// observer methods.
fn interface<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: &str,
    intersection: bool,
) -> Option<v8::Local<'s, v8::Function>> {
    let constructor: v8::FunctionCallback = if intersection {
        v8::MapFnTo::map_fn_to(guarded_intersection_constructor)
    } else {
        v8::MapFnTo::map_fn_to(guarded_resize_constructor)
    };
    let template = v8::FunctionTemplate::builder_raw(constructor)
        .length(1)
        .build(scope);
    let class_name = key(scope, name);
    template.set_class_name(class_name);
    let prototype = template.prototype_template(scope);
    let mut methods: Vec<(&str, v8::FunctionCallback, i32)> = vec![
        ("observe", v8::MapFnTo::map_fn_to(guarded_observe), 1),
        ("unobserve", v8::MapFnTo::map_fn_to(guarded_unobserve), 1),
        ("disconnect", v8::MapFnTo::map_fn_to(guarded_disconnect), 0),
    ];
    if intersection {
        methods.push((
            "takeRecords",
            v8::MapFnTo::map_fn_to(guarded_take_records),
            0,
        ));
    }
    for (member, callback, length) in methods {
        let method = v8::FunctionTemplate::builder_raw(callback)
            .length(length)
            .build(scope);
        let member = key(scope, member);
        prototype.set(member.into(), method.into());
    }
    let tag = v8::Symbol::get_to_string_tag(scope);
    prototype.set_with_attr(
        tag.into(),
        class_name.into(),
        v8::PropertyAttribute::READ_ONLY | v8::PropertyAttribute::DONT_ENUM,
    );
    template.get_function(scope)
}

/// The steps both constructors share: `new`, a callable first argument, and
/// the observer's empty state. `None` when an exception was thrown.
fn construct<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
    interface: &str,
    brand: &str,
) -> Option<v8::Local<'s, v8::Object>> {
    if args.new_target().is_undefined() {
        throw_type_error(
            scope,
            &format!(
                "Failed to construct '{interface}': Please use the 'new' operator, this DOM object constructor cannot be called as a function."
            ),
        );
        return None;
    }
    let callback = args.get(0);
    if !callback.is_function() {
        throw_type_error(
            scope,
            &format!("Failed to construct '{interface}': parameter 1 is not of type 'Function'."),
        );
        return None;
    }
    let this = args.this();
    set_private(scope, this, brand, callback);
    let observed = v8::Array::new(scope, 0);
    set_private(scope, this, OBSERVED, observed.into());
    let pending = v8::Array::new(scope, 0);
    set_private(scope, this, PENDING, pending.into());
    Some(this)
}

fn resize_constructor<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    construct(scope, &args, "ResizeObserver", RESIZE);
}

fn intersection_constructor<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(this) = construct(scope, &args, "IntersectionObserver", INTERSECTION) else {
        return;
    };
    // The options are kept as the observer reports them back. None of them
    // changes what is reported: there is one root, and everything is in it.
    let options = args.get(1);
    let options = v8::Local::<v8::Object>::try_from(options).ok();
    let root = options
        .and_then(|options| get(scope, options, "root"))
        .filter(|root| !root.is_undefined())
        .unwrap_or_else(|| v8::null(scope).into());
    let margin = options
        .and_then(|options| get(scope, options, "rootMargin"))
        .filter(|margin| margin.is_string())
        .unwrap_or_else(|| key(scope, "0px 0px 0px 0px").into());
    let threshold = options
        .and_then(|options| get(scope, options, "threshold"))
        .filter(|threshold| !threshold.is_undefined());
    let thresholds = match threshold {
        Some(list) if list.is_array() => list,
        Some(single) => v8::Array::new_with_elements(scope, &[single]).into(),
        None => {
            let zero = v8::Number::new(scope, 0.0);
            v8::Array::new_with_elements(scope, &[zero.into()]).into()
        }
    };
    for (name, value) in [
        ("root", root),
        ("rootMargin", margin),
        ("thresholds", thresholds),
    ] {
        let name = key(scope, name);
        this.define_own_property(scope, name.into(), value, v8::PropertyAttribute::READ_ONLY);
    }
}

/// The observer `this` is, if it is one of either kind.
fn observer_of<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
) -> Option<v8::Local<'s, v8::Object>> {
    let this = args.this();
    if get_private(scope, this, RESIZE).is_some()
        || get_private(scope, this, INTERSECTION).is_some()
    {
        return Some(this);
    }
    throw_type_error(scope, "Illegal invocation");
    None
}

fn array_of<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    observer: v8::Local<v8::Object>,
    name: &str,
) -> Option<v8::Local<'s, v8::Array>> {
    let value = get_private(scope, observer, name)?;
    v8::Local::<v8::Array>::try_from(value).ok()
}

fn index_of(
    scope: &mut v8::PinScope,
    list: v8::Local<v8::Array>,
    item: v8::Local<v8::Value>,
) -> Option<u32> {
    (0..list.length()).find(|index| {
        list.get_index(scope, *index)
            .is_some_and(|value| value.strict_equals(item))
    })
}

/// `list` without `item`, as a new array stored under `name`.
fn remove(
    scope: &mut v8::PinScope,
    observer: v8::Local<v8::Object>,
    name: &str,
    item: v8::Local<v8::Value>,
) {
    let Some(list) = array_of(scope, observer, name) else {
        return;
    };
    let mut kept = Vec::with_capacity(list.length() as usize);
    for index in 0..list.length() {
        if let Some(value) = list.get_index(scope, index)
            && !value.strict_equals(item)
        {
            kept.push(value);
        }
    }
    let kept = v8::Array::new_with_elements(scope, &kept);
    set_private(scope, observer, name, kept.into());
}

/// Whether `value` is an element: an object whose `nodeType` is 1.
fn is_element(scope: &mut v8::PinScope, value: v8::Local<v8::Value>) -> bool {
    let Ok(object) = v8::Local::<v8::Object>::try_from(value) else {
        return false;
    };
    get(scope, object, "nodeType")
        .and_then(|kind| kind.number_value(scope))
        .is_some_and(|kind| kind == 1.0)
}

fn observe<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(observer) = observer_of(scope, &args) else {
        return;
    };
    let target = args.get(0);
    if !is_element(scope, target) {
        throw_type_error(
            scope,
            "Failed to execute 'observe': parameter 1 is not of type 'Element'.",
        );
        return;
    }
    let (Some(observed), Some(pending)) = (
        array_of(scope, observer, OBSERVED),
        array_of(scope, observer, PENDING),
    ) else {
        return;
    };
    if index_of(scope, observed, target).is_some() {
        return;
    }
    observed.set_index(scope, observed.length(), target);
    pending.set_index(scope, pending.length(), target);
    schedule(scope, observer);
}

fn unobserve<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(observer) = observer_of(scope, &args) else {
        return;
    };
    let target = args.get(0);
    remove(scope, observer, OBSERVED, target);
    remove(scope, observer, PENDING, target);
}

fn disconnect<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(observer) = observer_of(scope, &args) else {
        return;
    };
    for name in [OBSERVED, PENDING] {
        let empty = v8::Array::new(scope, 0);
        set_private(scope, observer, name, empty.into());
    }
}

/// `IntersectionObserver.takeRecords()`: records are handed over by the
/// delivery task, so none are ever queued in between.
fn take_records<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    if observer_of(scope, &args).is_some() {
        rv.set(v8::Array::new(scope, 0).into());
    }
}

/// Queues the task that reports the pending targets, unless one is queued.
fn schedule(scope: &mut v8::PinScope, observer: v8::Local<v8::Object>) {
    if get_private(scope, observer, SCHEDULED).is_some_and(|flag| flag.is_true()) {
        return;
    }
    let Some(task) = v8::Function::builder(guarded_deliver)
        .data(observer.into())
        .build(scope)
    else {
        return;
    };
    if queue_task(scope, task).is_some() {
        let yes = v8::Boolean::new(scope, true);
        set_private(scope, observer, SCHEDULED, yes.into());
    }
}

/// The target's box, as `getBoundingClientRect()` reports it.
fn rect_of<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    target: v8::Local<'s, v8::Object>,
) -> Option<v8::Local<'s, v8::Object>> {
    let method = get(scope, target, "getBoundingClientRect")?;
    let method = v8::Local::<v8::Function>::try_from(method).ok()?;
    let rect = method.call(scope, target.into(), &[])?;
    v8::Local::<v8::Object>::try_from(rect).ok()
}

fn number(scope: &mut v8::PinScope, object: v8::Local<v8::Object>, name: &str) -> f64 {
    get(scope, object, name)
        .and_then(|value| value.number_value(scope))
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
}

/// One `ResizeObserverEntry`, or `None` for a target with an empty box, which
/// the standard does not report.
fn resize_entry<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    target: v8::Local<'s, v8::Object>,
) -> Option<v8::Local<'s, v8::Object>> {
    let rect = rect_of(scope, target)?;
    let (width, height) = (number(scope, rect, "width"), number(scope, rect, "height"));
    if width == 0.0 && height == 0.0 {
        return None;
    }
    let entry = v8::Object::new(scope);
    set(scope, entry, "target", target.into());
    set(scope, entry, "contentRect", rect.into());
    for name in [
        "borderBoxSize",
        "contentBoxSize",
        "devicePixelContentBoxSize",
    ] {
        let size = v8::Object::new(scope);
        let inline = v8::Number::new(scope, width);
        let block = v8::Number::new(scope, height);
        set(scope, size, "inlineSize", inline.into());
        set(scope, size, "blockSize", block.into());
        let sizes = v8::Array::new_with_elements(scope, &[size.into()]);
        set(scope, entry, name, sizes.into());
    }
    Some(entry)
}

/// One `IntersectionObserverEntry`: the target is in view.
fn intersection_entry<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    target: v8::Local<'s, v8::Object>,
    time: f64,
) -> v8::Local<'s, v8::Object> {
    let entry = v8::Object::new(scope);
    set(scope, entry, "target", target.into());
    let time = v8::Number::new(scope, time);
    set(scope, entry, "time", time.into());
    let rect: v8::Local<v8::Value> = match rect_of(scope, target) {
        Some(rect) => rect.into(),
        None => v8::null(scope).into(),
    };
    set(scope, entry, "boundingClientRect", rect);
    set(scope, entry, "intersectionRect", rect);
    let null = v8::null(scope);
    set(scope, entry, "rootBounds", null.into());
    let yes = v8::Boolean::new(scope, true);
    set(scope, entry, "isIntersecting", yes.into());
    let no = v8::Boolean::new(scope, false);
    set(scope, entry, "isVisible", no.into());
    let one = v8::Number::new(scope, 1.0);
    set(scope, entry, "intersectionRatio", one.into());
    entry
}

/// `performance.now()` of the realm, for an entry's `time`.
fn now(scope: &mut v8::PinScope) -> f64 {
    let global = scope.get_current_context().global(scope);
    (|| {
        let performance = get(scope, global, "performance")?;
        let performance = v8::Local::<v8::Object>::try_from(performance).ok()?;
        let method = get(scope, performance, "now")?;
        let method = v8::Local::<v8::Function>::try_from(method).ok()?;
        method
            .call(scope, performance.into(), &[])?
            .number_value(scope)
    })()
    .unwrap_or(0.0)
}

/// The delivery task: hands the observer's callback one entry per target
/// that has not been reported yet.
fn deliver<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Ok(observer) = v8::Local::<v8::Object>::try_from(args.data()) else {
        return;
    };
    let no = v8::Boolean::new(scope, false);
    set_private(scope, observer, SCHEDULED, no.into());
    let Some(pending) = array_of(scope, observer, PENDING) else {
        return;
    };
    let empty = v8::Array::new(scope, 0);
    set_private(scope, observer, PENDING, empty.into());
    let intersection = get_private(scope, observer, INTERSECTION);
    let callback = intersection.or_else(|| get_private(scope, observer, RESIZE));
    let Some(callback) = callback.and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
    else {
        return;
    };
    let time = if intersection.is_some() {
        now(scope)
    } else {
        0.0
    };
    let mut entries: Vec<v8::Local<v8::Value>> = Vec::new();
    for index in 0..pending.length() {
        let Some(target) = pending
            .get_index(scope, index)
            .and_then(|target| v8::Local::<v8::Object>::try_from(target).ok())
        else {
            continue;
        };
        if intersection.is_some() {
            entries.push(intersection_entry(scope, target, time).into());
        } else if let Some(entry) = resize_entry(scope, target) {
            entries.push(entry.into());
        }
    }
    if entries.is_empty() {
        return;
    }
    let entries = v8::Array::new_with_elements(scope, &entries);
    // An exception the callback throws is reported by the task that runs it.
    let _frame = tenta_js::bindings::v8_page_callback_frame(scope);
    let _ = callback.call(scope, observer.into(), &[entries.into(), observer.into()]);
}
