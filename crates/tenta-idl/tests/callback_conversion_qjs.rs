//! Real generated QuickJS callback conversions preserve WebIDL category rules.

#![cfg(feature = "quickjs")]

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rquickjs::{Class, Ctx, Function, Object, Persistent, Value};

#[allow(
    dead_code,
    unused_variables,
    clippy::wrong_self_convention,
    clippy::unnecessary_cast,
    clippy::single_match,
    clippy::unnecessary_map_on_constructor,
    clippy::clone_on_copy
)]
mod bindings {
    use rquickjs::FromJs;
    include!("golden/dom/prelude.rs");
    include!("golden/dom/types.rs");
    include!("golden/dom/manager_ops.rs");
    include!("golden/dom/manager_register.rs");
    include!("golden/dom/manager_qjs.rs");
    include!("golden/dom/event_target_ops.rs");
    include!("golden/dom/event_target_register.rs");
    include!("golden/dom/event_target_qjs.rs");
}
use bindings::*;

struct Frame {
    id: u64,
    lease_id: Option<u64>,
    bits: Vec<u64>,
}

#[derive(Default)]
struct World {
    values: RefCell<Vec<Option<Persistent<Value<'static>>>>>,
    free: RefCell<Vec<u64>>,
    frames: RefCell<Vec<Frame>>,
    next_frame: Cell<u64>,
    prototypes: RefCell<HashMap<&'static str, Persistent<Object<'static>>>>,
    intrinsics: RefCell<HashMap<&'static str, Persistent<Value<'static>>>>,
    failure: Cell<Option<&'static str>>,
}
impl World {
    fn live(&self) -> usize {
        self.values
            .borrow()
            .iter()
            .filter(|value| value.is_some())
            .count()
    }
}

struct Bridge(Rc<World>);
impl QuickJsWrapperWorld for Bridge {
    fn wrapper<'js>(&self, _: &Ctx<'js>, _: &'static str, _: u64) -> Option<Object<'js>> {
        None
    }
    fn handle(&self, _: &'static str, _: &Object<'_>) -> Option<u64> {
        None
    }
    fn begin_callback_roots(&self, _: &Ctx<'_>) -> rquickjs::Result<u64> {
        let id = self.0.next_frame.get() + 1;
        self.0.next_frame.set(id);
        self.0.frames.borrow_mut().push(Frame {
            id,
            lease_id: None,
            bits: Vec::new(),
        });
        Ok(id)
    }
    fn begin_stored_value_lease(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        if self.0.frames.borrow().is_empty() {
            return Err(rquickjs::Exception::throw_type(ctx, "no callback frame"));
        }
        Ok(())
    }
    fn begin_buffer_reference_lease(&self, ctx: &Ctx<'_>, lease_id: u64) -> rquickjs::Result<()> {
        let mut frames = self.0.frames.borrow_mut();
        let Some(frame) = frames.last_mut() else {
            return Err(rquickjs::Exception::throw_type(ctx, "no callback frame"));
        };
        if frame.lease_id.is_some() {
            return Err(rquickjs::Exception::throw_type(
                ctx,
                "duplicate buffer lease",
            ));
        }
        frame.lease_id = Some(lease_id);
        Ok(())
    }
    fn current_buffer_reference_lease(&self) -> Option<u64> {
        self.0
            .frames
            .borrow()
            .last()
            .and_then(|frame| frame.lease_id)
    }
    fn buffer_reference_lease_active(&self, lease_id: u64) -> bool {
        self.0
            .frames
            .borrow()
            .iter()
            .any(|frame| frame.lease_id == Some(lease_id))
    }
    fn track_stored_value(&self, _: &Ctx<'_>, bits: u64) -> rquickjs::Result<()> {
        self.0
            .frames
            .borrow_mut()
            .last_mut()
            .expect("stored value has a callback frame")
            .bits
            .push(bits);
        Ok(())
    }
    fn transfer_stored_value_lease(&self, _: &Ctx<'_>) -> rquickjs::Result<()> {
        let bits = {
            let mut frames = self.0.frames.borrow_mut();
            std::mem::take(&mut frames.last_mut().expect("active callback").bits)
        };
        for bits in bits {
            let root = self.0.values.borrow_mut()[bits as usize].take();
            assert!(
                root.is_some(),
                "fixture host did not keep a transferred callback"
            );
            self.0.free.borrow_mut().push(bits);
            drop(root);
        }
        Ok(())
    }
    fn before_constructor_host(&self, _: &Ctx<'_>) -> rquickjs::Result<()> {
        Ok(())
    }
    fn retain_callback_argument<'js>(&self, _: &Ctx<'js>, _: Value<'js>) -> rquickjs::Result<()> {
        Ok(())
    }
    fn end_callback_roots(&self, id: u64) {
        let frame = self.0.frames.borrow_mut().pop().expect("frame ends once");
        assert_eq!(frame.id, id);
        for bits in frame.bits {
            let root = self.0.values.borrow_mut()[bits as usize].take();
            assert!(root.is_some(), "one temporary root is released once");
            self.0.free.borrow_mut().push(bits);
            drop(root);
        }
    }
    fn store_value(&self, _ctx: &Ctx<'_>, value: Value<'_>) -> u64 {
        let value_ctx = value.ctx().clone();
        let root = Persistent::save(&value_ctx, value);
        if let Some(bits) = self.0.free.borrow_mut().pop() {
            self.0.values.borrow_mut()[bits as usize] = Some(root);
            bits
        } else {
            let mut values = self.0.values.borrow_mut();
            let bits = values.len() as u64;
            values.push(Some(root));
            bits
        }
    }
    fn load_value<'js>(&self, ctx: &Ctx<'js>, bits: u64) -> Option<Value<'js>> {
        self.0
            .values
            .borrow()
            .get(bits as usize)?
            .as_ref()?
            .clone()
            .restore(ctx)
            .ok()
    }
    fn realm(&self) -> Realm {
        Rc::new(())
    }
    fn adopt<'js>(&self, _: &Ctx<'js>, _: &'static str, _: Box<dyn Any>) -> Option<Object<'js>> {
        None
    }
    fn prototype<'js>(&self, ctx: &Ctx<'js>, interface: &'static str) -> Option<Object<'js>> {
        self.0
            .prototypes
            .borrow()
            .get(interface)?
            .clone()
            .restore(ctx)
            .ok()
    }
    fn cache_prototype<'js>(
        &self,
        ctx: &Ctx<'js>,
        interface: &'static str,
        prototype: Object<'js>,
        _: &'static [&'static str],
    ) {
        self.0
            .prototypes
            .borrow_mut()
            .insert(interface, Persistent::save(ctx, prototype));
    }
    fn intrinsic<'js>(&self, ctx: &Ctx<'js>, name: &str) -> Option<Value<'js>> {
        self.0
            .intrinsics
            .borrow()
            .get(name)?
            .clone()
            .restore(ctx)
            .ok()
    }
    fn cache_legacy(&self, _: &'static str, _: QuickJsLegacyMethods) {}
    fn native_failure(&self) -> Option<&'static str> {
        self.0.failure.get()
    }
    fn script_interrupted(&self) -> bool {
        false
    }
    fn record_native_failure(&self, name: &'static str) {
        self.0.failure.set(Some(name));
    }
    fn native_host(&self, _: &'static str, _: &Value<'_>) -> Option<Rc<dyn Any>> {
        None
    }
}

#[derive(Default)]
struct CallbackState {
    strict_sets: Cell<usize>,
    loose_sets: Cell<usize>,
    strict_some: Cell<bool>,
    loose_some: Cell<bool>,
    listener_calls: Cell<usize>,
}

struct Host(Rc<CallbackState>);
impl ManagerOps for Host {
    fn handler1(&self) -> Result<Option<OccurrenceHandler>, JsError> {
        Ok(None)
    }
    fn set_handler1(&self, value: Option<OccurrenceHandler>) -> Result<(), JsError> {
        self.0.strict_sets.set(self.0.strict_sets.get() + 1);
        self.0.strict_some.set(value.is_some());
        Ok(())
    }
    fn handler2(&self) -> Result<Option<ErrorHandler>, JsError> {
        Ok(None)
    }
    fn set_handler2(&self, value: Option<ErrorHandler>) -> Result<(), JsError> {
        self.0.loose_sets.set(self.0.loose_sets.get() + 1);
        self.0.loose_some.set(value.is_some());
        Ok(())
    }
}
impl EventTargetOps for Host {
    fn add_event_listener(
        &self,
        _: &str,
        _: Option<EventListener>,
        _: Option<AddEventListenerOptionsOrBoolean>,
    ) -> Result<(), JsError> {
        self.0.listener_calls.set(self.0.listener_calls.get() + 1);
        Ok(())
    }
    fn remove_event_listener(
        &self,
        _: &str,
        _: Option<EventListener>,
        _: Option<EventListenerOptionsOrBoolean>,
    ) -> Result<(), JsError> {
        Ok(())
    }
    fn dispatch_event(&self, _: Handle<Event>) -> Result<bool, JsError> {
        Ok(true)
    }
}
impl EventTargetConstructor for Host {
    fn construct(_: &Realm) -> Result<Self, JsError> {
        Ok(Self(Rc::new(CallbackState::default())))
    }
}

struct Registry;
impl InterfaceRegistry for Registry {
    fn register(&mut self, _: InterfaceDescriptor) {}
}

fn publish_host(
    ctx: &Ctx<'_>,
    world: &Rc<World>,
    state: &Rc<CallbackState>,
    interface: &'static str,
    name: &str,
) {
    let object = Class::instance(
        ctx.clone(),
        QuickJsHost {
            host: Some(Rc::new(Host(state.clone())) as Rc<dyn Any>),
            handle: if name == "manager" { 1 } else { 2 },
            generation: 1,
            native_failure: Rc::new(Cell::new(None)),
            interfaces: &["Manager", "EventTarget"],
            dead: Rc::new(Cell::new(Vec::new())),
        },
    )
    .expect("native host")
    .into_inner();
    let prototype = world
        .prototypes
        .borrow()
        .get(interface)
        .expect("generated prototype")
        .clone()
        .restore(ctx)
        .expect("restore prototype");
    object
        .set_prototype(Some(&prototype))
        .expect("link generated prototype");
    ctx.globals().set(name, object).expect("publish host");
}

#[test]
fn generated_quickjs_callbacks_obey_function_and_object_categories() {
    let runtime = rquickjs::Runtime::new().expect("runtime");
    let context = rquickjs::Context::full(&runtime).expect("context");
    let state = Rc::new(CallbackState::default());
    context.with(|ctx| {
        let world = Rc::new(World::default());
        ctx.store_userdata(QuickJsWorldSlot(Box::new(Bridge(world.clone()))))
            .unwrap_or_else(|_| panic!("install world"));
        install_manager::<Host>(&ctx, &ctx.globals(), &mut Registry)
            .expect("install generated Manager");
        install_event_target::<Host>(&ctx, &ctx.globals(), &mut Registry)
            .expect("install generated EventTarget");
        publish_host(&ctx, &world, &state, "Manager", "manager");
        publish_host(&ctx, &world, &state, "EventTarget", "target");
        let weak_world = Rc::downgrade(&world);
        ctx.globals()
            .set(
                "leaseDepth",
                Function::new(ctx.clone(), move || {
                    weak_world.upgrade().map_or(0_i32, |world| {
                        i32::try_from(world.live()).expect("fixture root count fits")
                    })
                })
                .expect("native lease observer"),
            )
            .expect("publish lease observer");

        assert!(ctx.eval::<bool, _>("(() => { for (const value of [17, 'text', true, {}, []]) { try { manager.handler1 = value; return false; } catch (error) { if (!(error instanceof TypeError)) return false; } } return true; })()").expect("strict callback gate"));
        assert_eq!(state.strict_sets.get(), 0, "invalid values never reach host");
        assert_eq!(world.live(), 0, "failed conversions keep no reference");

        assert!(ctx.eval::<bool, _>("(() => { manager.handler1 = function() {}; manager.handler1 = new Proxy(function() {}, {}); return true; })()").expect("callable function and Proxy"));
        assert_eq!(state.strict_sets.get(), 2);
        assert!(state.strict_some.get());
        assert_eq!(world.live(), 0, "setter consumes temporary references");
        assert!(ctx.eval::<bool, _>("(() => { manager.handler1 = null; manager.handler1 = undefined; return true; })()").expect("nullable strict callback"));
        assert!(!state.strict_some.get());

        assert!(ctx.eval::<bool, _>("(() => { manager.handler2 = {}; return true; })()").expect("legacy callback accepts non-callable object"));
        assert!(state.loose_some.get());
        assert!(ctx.eval::<bool, _>("(() => { manager.handler2 = 17; manager.handler2 = false; manager.handler2 = null; return true; })()").expect("legacy nonobjects become null"));
        assert!(!state.loose_some.get());
        assert_eq!(world.live(), 0);

        assert!(ctx.eval::<bool, _>("(() => { for (const value of [17, 'text', true, Symbol('x')]) { try { target.addEventListener('x', value); return false; } catch (error) { if (!(error instanceof TypeError)) return false; } } return true; })()").expect("callback interface object gate"));
        assert_eq!(state.listener_calls.get(), 0);
        assert_eq!(world.live(), 0);
        assert!(ctx.eval::<bool, _>("(() => { target.addEventListener('x', {handleEvent() {}}); target.addEventListener('x', function() {}); return true; })()").expect("object listener and callable listener"));
        assert_eq!(state.listener_calls.get(), 2);
        assert_eq!(world.live(), 0, "fixture host releases transferred references it does not retain");

        assert!(ctx.eval::<bool, _>("(() => { const marker = {}; target.addEventListener('x', {get handleEvent() {throw marker;}}); return true; })()").expect("callback-interface conversion does not read handleEvent"));
        assert_eq!(state.listener_calls.get(), 3);
        assert!(ctx.eval::<bool, _>("(() => { const marker = {}; const callback = {handleEvent() {}}; let leaseCount = -1; try { target.addEventListener('x', callback, {get capture() {leaseCount = leaseDepth(); throw marker;}}); return false; } catch (error) { return error === marker && leaseCount === 1; } })()").expect("page getter exception identity and held lease"));
        assert_eq!(state.listener_calls.get(), 3, "later exception prevents host entry");
        assert_eq!(world.live(), 0, "later exception releases callback lease");
        assert!(ctx.eval::<bool, _>("(() => { const marker = {}; try { target.addEventListener({toString() {throw marker;}}, {handleEvent() {}}); return false; } catch (error) { return error === marker; } })()").expect("earlier conversion exception identity"));
        assert_eq!(world.live(), 0);
        assert_eq!(world.failure.get(), None);

        world.prototypes.borrow_mut().clear();
        world.intrinsics.borrow_mut().clear();
        drop(ctx.remove_userdata::<QuickJsWorldSlot>().unwrap_or_else(|_| panic!("remove world userdata")));
    });
}
