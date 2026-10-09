//! Real QuickJS values exercise the generated callback union converter.

#![cfg(feature = "quickjs")]

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rquickjs::{Ctx, Object, Persistent, Value};

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
    include!("golden/callback_union/prelude.rs");
    include!("golden/callback_union/types.rs");
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

fn choose<'js>(
    ctx: &Ctx<'js>,
    value: Value<'js>,
    host_calls: &Cell<usize>,
) -> rquickjs::Result<&'static str> {
    quickjs_callback_boundary(ctx, "choose", || {
        let converted = <ListenerOrHandler as rquickjs::FromJs>::from_js(ctx, value)?;
        host_calls.set(host_calls.get() + 1);
        Ok(match converted {
            ListenerOrHandler::Listener(_) => "listener",
            ListenerOrHandler::Handler(_) => "handler",
        })
    })
}

#[test]
fn callback_union_uses_the_object_arm_only_after_callable_precedence() {
    let runtime = rquickjs::Runtime::new().expect("runtime");
    let context = rquickjs::Context::full(&runtime).expect("context");
    context.with(|ctx| {
        let world = Rc::new(World::default());
        ctx.store_userdata(QuickJsWorldSlot(Box::new(Bridge(world.clone()))))
            .unwrap_or_else(|_| panic!("install world"));
        let host_calls = Cell::new(0);

        for (source, expected) in [
            ("({handleEvent() {}})", "listener"),
            ("({get handleEvent() {throw Error('must not read at conversion');}})", "listener"),
            ("new Proxy({}, {})", "listener"),
            ("(function() {})", "handler"),
            ("new Proxy(function() {}, {})", "handler"),
        ] {
            let value: Value = ctx.eval(source).expect("page value");
            assert_eq!(choose(&ctx, value, &host_calls).expect("union conversion"), expected);
            assert_eq!(world.live(), 0, "callback frame releases its temporary root");
        }
        assert_eq!(host_calls.get(), 5);

        for source in ["17", "null", "undefined"] {
            let value: Value = ctx.eval(source).expect("primitive value");
            let error = choose(&ctx, value, &host_calls).expect_err("no union arm accepts the primitive");
            assert!(error.is_exception(), "WebIDL throws a TypeError");
            let thrown = ctx.catch();
            ctx.globals().set("caught", thrown).expect("observe error");
            assert!(ctx.eval::<bool, _>("caught instanceof TypeError").expect("typed error"));
            assert_eq!(host_calls.get(), 5, "conversion failure prevents host effect");
            assert_eq!(world.live(), 0);
        }

        let iterable: Value = ctx.eval("({handleEvent() {}, *[Symbol.iterator]() { yield 'a'; }})").expect("iterable listener-shaped object");
        let selected = quickjs_callback_boundary(&ctx, "chooseIterable", || {
            <ListenerOrSequenceOfDomString as rquickjs::FromJs>::from_js(&ctx, iterable)
        }).expect("iterable union conversion");
        assert!(matches!(selected, ListenerOrSequenceOfDomString::SequenceOfDomString(ref values) if values.len() == 1 && values[0] == "a"));
        assert_eq!(world.live(), 0);
        assert_eq!(world.failure.get(), None);

        world.prototypes.borrow_mut().clear();
        world.intrinsics.borrow_mut().clear();
        drop(ctx.remove_userdata::<QuickJsWorldSlot>().unwrap_or_else(|_| panic!("remove world userdata")));
    });
}
