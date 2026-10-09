//! Special-operation lookup shared by both engine generators.

use crate::model::{Interface, Member, Model, Operation, Special, Type};

/// A resolved special operation and the trait declaring its host method.
#[derive(Clone, Copy)]
pub(super) struct SpecialOperation<'a> {
    pub interface: &'a str,
    pub operation: &'a Operation,
}

/// Legacy property behavior, including inherited special operations.
#[derive(Default)]
pub(super) struct LegacyProperties<'a> {
    pub indexed_getter: Option<SpecialOperation<'a>>,
    pub indexed_setter: Option<SpecialOperation<'a>>,
    pub named_getter: Option<SpecialOperation<'a>>,
    pub named_setter: Option<SpecialOperation<'a>>,
    pub named_deleter: Option<SpecialOperation<'a>>,
    pub override_builtins: bool,
    pub unenumerable_names: bool,
}

impl<'a> LegacyProperties<'a> {
    pub fn resolve(model: &'a Model, interface: &'a Interface) -> Self {
        let mut result = Self::default();
        let mut current = Some(interface);
        let mut seen = Vec::new();
        while let Some(interface) = current {
            if seen.contains(&interface.name.as_str()) {
                break;
            }
            seen.push(interface.name.as_str());
            result.override_builtins |= interface.attrs.has("LegacyOverrideBuiltIns");
            result.unenumerable_names |= interface.attrs.has("LegacyUnenumerableNamedProperties");
            for operation in declared_specials(interface) {
                let resolved = SpecialOperation {
                    interface: &interface.name,
                    operation,
                };
                let Some(argument) = operation.arguments.first() else {
                    continue;
                };
                let target = match (operation.special, &argument.ty) {
                    (Some(Special::Getter), Type::UnsignedLong) => &mut result.indexed_getter,
                    (Some(Special::Setter), Type::UnsignedLong) => &mut result.indexed_setter,
                    (Some(Special::Getter), Type::DomString) => &mut result.named_getter,
                    (Some(Special::Setter), Type::DomString) => &mut result.named_setter,
                    (Some(Special::Deleter), Type::DomString) => &mut result.named_deleter,
                    _ => continue,
                };
                if target.is_none() {
                    *target = Some(resolved);
                }
            }
            current = interface
                .parent
                .as_deref()
                .and_then(|name| model.interface(name));
        }
        result
    }
}

pub(super) fn declared_specials(interface: &Interface) -> impl Iterator<Item = &Operation> {
    interface
        .members
        .iter()
        .filter_map(|member| match member {
            Member::Operation(group) if !group.is_static => Some(group.overloads.iter()),
            _ => None,
        })
        .flatten()
        .chain(&interface.special_operations)
        .filter(|operation| operation.special.is_some())
}
