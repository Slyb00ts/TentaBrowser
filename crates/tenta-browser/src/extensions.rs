//! Typed state that an assembly crate above this one attaches to an engine
//! or a context.
//!
//! `tenta-search` sits above `tenta-browser` in the assembly order
//! (`docs/spec/02-architecture.md` §2), so the search subsystem cannot be a
//! field of [`crate::Engine`]. It still has to live and die with the engine:
//! the internal search context, its warm connections and its response cache
//! are worth exactly as much as they are long-lived. A slot keyed by the
//! caller's own type gives `tentabrowser::EngineSearch` that lifetime
//! without an upward dependency.

use std::any::{Any, TypeId};
use std::sync::Arc;

use parking_lot::Mutex;
use tenta_core::Result;

/// A small set of type-keyed values.
///
/// The map is tiny (one entry per subsystem that asks for one), so a vector
/// with a linear scan beats a hash map and keeps the type free of iteration
/// order surprises.
#[derive(Default)]
pub(crate) struct Extensions {
    slots: Mutex<Vec<(TypeId, Arc<dyn Any + Send + Sync>)>>,
}

impl std::fmt::Debug for Extensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extensions")
            .field("len", &self.slots.lock().len())
            .finish()
    }
}

impl Extensions {
    /// Returns the value stored for `T`, running `init` once if there is
    /// none. A failing `init` stores nothing, so the next call retries.
    ///
    /// `init` runs while the slot map is locked, so it must not ask the same
    /// engine or context for an extension again.
    pub(crate) fn get_or_try_init<T, F>(&self, init: F) -> Result<Arc<T>>
    where
        T: Any + Send + Sync,
        F: FnOnce() -> Result<T>,
    {
        let key = TypeId::of::<T>();
        let mut slots = self.slots.lock();
        if let Some((_, value)) = slots.iter().find(|(candidate, _)| *candidate == key) {
            return Ok(Arc::clone(value)
                .downcast::<T>()
                .expect("a slot holds the type it is keyed by"));
        }
        let value: Arc<T> = Arc::new(init()?);
        slots.push((key, Arc::clone(&value) as Arc<dyn Any + Send + Sync>));
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug, PartialEq, Eq)]
    struct First(u32);
    #[derive(Debug, PartialEq, Eq)]
    struct Second(&'static str);

    #[test]
    fn a_slot_is_initialised_once_per_type() {
        let extensions = Extensions::default();
        let calls = AtomicUsize::new(0);
        let init = || {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(First(7))
        };

        let first = extensions.get_or_try_init(init).expect("init");
        let again = extensions.get_or_try_init(init).expect("cached");
        assert_eq!(*first, First(7));
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        let second = extensions
            .get_or_try_init(|| Ok(Second("other")))
            .expect("a different type gets a different slot");
        assert_eq!(*second, Second("other"));
    }

    #[test]
    fn a_failed_init_stores_nothing() {
        let extensions = Extensions::default();
        let error = extensions
            .get_or_try_init::<First, _>(|| Err(tenta_core::Error::Closed))
            .expect_err("init failed");
        assert!(matches!(error, tenta_core::Error::Closed), "{error:?}");
        let value = extensions
            .get_or_try_init(|| Ok(First(1)))
            .expect("the next call retries");
        assert_eq!(*value, First(1));
    }
}
