//! What the script preludes built at first use have in common
//! (`super::streams`, `super::selection`, `super::subtle`, `super::matrix`).
//!
//! Each of them defines names when the engine is built - lazy data properties
//! of the global, a native accessor or method here and there - and runs its
//! script when a page first uses one. The script evaluates to a function; what
//! that function returns, the exports, is kept in a private property of the
//! global. A realm where the script fails keeps `false` there: it has the
//! names and nothing behind them, and does not try again.

use super::observers::{get_private, key, set_private};

/// The exports of the script `source`, built on first use and kept in the
/// private property `slot` of the global. `arguments` makes what the
/// script's function is called with, and is asked only when the script is
/// about to run. `None` in a realm where the script failed, now or earlier.
pub(super) fn exports<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    slot: &str,
    source: &str,
    arguments: impl FnOnce(&mut v8::PinScope<'s, '_>) -> Option<Vec<v8::Local<'s, v8::Value>>>,
) -> Option<v8::Local<'s, v8::Object>> {
    let global = scope.get_current_context().global(scope);
    if let Some(known) = get_private(scope, global, slot) {
        return v8::Local::<v8::Object>::try_from(known).ok();
    }
    let arguments = arguments(scope);
    let built = {
        v8::tc_scope!(let tc, scope);
        let built = arguments
            .and_then(|arguments| {
                let source = v8::String::new(tc, source)?;
                let build = v8::Script::compile(tc, source, None)?.run(tc)?;
                let build = v8::Local::<v8::Function>::try_from(build).ok()?;
                let receiver = v8::undefined(tc).into();
                build.call(tc, receiver, &arguments)
            })
            .and_then(|value| v8::Local::<v8::Object>::try_from(value).ok())
            .map(|object| v8::Global::new(tc, object));
        // A termination is the page's to see; anything else the script threw
        // is this realm going without what the script defines.
        if !tc.has_terminated() {
            tc.reset();
        }
        built
    };
    let built = built.map(|object| v8::Local::new(scope, object));
    let stored: v8::Local<v8::Value> = match built {
        Some(object) => object.into(),
        None => v8::Boolean::new(scope, false).into(),
    };
    set_private(scope, global, slot, stored);
    built
}

/// Defines `name` on `global` as a lazy data property that `getter` answers
/// the first time it is read: non-enumerable, as an interface object is.
pub(super) fn define_interface(
    scope: &mut v8::PinScope,
    global: v8::Local<v8::Object>,
    name: &str,
    getter: impl v8::MapFnTo<v8::AccessorNameGetterCallback>,
) {
    let undefined = v8::undefined(scope);
    let name = key(scope, name);
    global.set_lazy_data_property_with_data(
        scope,
        name.into(),
        getter,
        undefined.into(),
        v8::PropertyAttribute::DONT_ENUM,
        v8::SideEffectType::HasSideEffect,
        v8::SideEffectType::HasSideEffect,
    );
}
