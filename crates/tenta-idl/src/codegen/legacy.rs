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

/// Whether WebIDL gives this interface an `@@iterator` it does not declare.
///
/// An interface with an indexed property getter and an integer-typed `length`
/// attribute iterates the way an array does: its `@@iterator` is
/// `%Array.prototype.values%` (<https://webidl.spec.whatwg.org/#js-iterator>),
/// and unlike `iterable<>` nothing else comes with it. That is what makes
/// `for (const rule of sheet.cssRules)` work. The property belongs to the
/// interface that declares the getter; one that inherits it reaches the
/// property through its prototype chain. A `[Global]` interface is left as it
/// was: no page has been seen to iterate `window`, and its members are not
/// installed the way a prototype's are.
pub(super) fn has_indexed_iterator(model: &Model, interface: &Interface) -> bool {
    if interface.iterable.is_some() || interface.attrs.has("Global") {
        return false;
    }
    let declares_getter = declared_specials(interface).any(|operation| {
        operation.special == Some(Special::Getter)
            && matches!(
                operation.arguments.first().map(|argument| &argument.ty),
                Some(Type::UnsignedLong)
            )
    });
    if !declares_getter {
        return false;
    }
    let mut current = Some(interface);
    let mut seen = Vec::new();
    while let Some(interface) = current {
        if seen.contains(&interface.name.as_str()) {
            break;
        }
        seen.push(interface.name.as_str());
        let integer_length = interface.members.iter().any(|member| {
            matches!(member, Member::Attribute(attribute)
                if attribute.name == "length" && !attribute.is_static && attribute.ty.is_integer())
        });
        if integer_length {
            return true;
        }
        current = interface
            .parent
            .as_deref()
            .and_then(|name| model.interface(name));
    }
    false
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
