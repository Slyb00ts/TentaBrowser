//! Code generators. Every entry point is a pure function of the [`Model`].
//!
//! [`Model`]: crate::model::Model

mod convert;
mod legacy;
mod ops;
mod prelude;
mod quickjs;
mod register;
mod rust_type;
mod types;
mod v8;
mod v8_legacy;

pub use ops::{interface_ops, namespace_ops};
pub use prelude::prelude;
pub use quickjs::{interface_quickjs, namespace_quickjs};
pub use register::{interface_register, namespace_register};
pub use types::types;
pub use v8::{interface_v8, namespace_v8};

/// Maps each accepted argument count of an overload set to the overload that
/// handles it. The first declared overload wins a tie. Counts beyond the
/// longest finite signature select that signature without converting extras.
/// Type-based disambiguation must still be represented by normalized unions.
pub(super) fn overload_dispatch(group: &crate::model::OperationGroup) -> Vec<(String, usize)> {
    use std::fmt::Write as _;

    let mut assignment: Vec<(usize, usize)> = Vec::new();
    let mut catch_all: Option<usize> = None;
    for (index, overload) in group.overloads.iter().enumerate() {
        let classes = overload.arity_classes();
        if classes.contains(&usize::MAX) {
            catch_all.get_or_insert(index);
            continue;
        }
        for arity in classes {
            if !assignment.iter().any(|(existing, _)| *existing == arity) {
                assignment.push((arity, index));
            }
        }
    }
    assignment.sort_unstable();
    let maximum = assignment.last().map(|(arity, _)| *arity);
    let mut arms: Vec<(String, usize)> = Vec::new();
    for (arity, index) in assignment {
        let pattern = if catch_all.is_none() && Some(arity) == maximum {
            format!("{arity}..")
        } else {
            arity.to_string()
        };
        match arms.last_mut() {
            Some((previous, existing)) if *existing == index => {
                let _ = write!(previous, " | {pattern}");
            }
            _ => arms.push((pattern, index)),
        }
    }
    if let Some(index) = catch_all {
        arms.push(("_".to_string(), index));
    }
    arms
}
