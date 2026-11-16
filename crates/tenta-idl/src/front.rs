//! The WebIDL front end: `weedle` syntax tree in, resolved [`Model`] out.
//!
//! The lowering runs in three passes over the parsed definitions:
//!
//! 1. every definition name is bucketed so that a bare identifier in a type
//!    position can be told apart (interface, dictionary, enumeration, callback,
//!    callback interface, typedef);
//! 2. typedefs are resolved on demand, with a cycle guard, so that later passes
//!    never see a typedef name in a type;
//! 3. definitions are lowered. Partials are appended to their main definition,
//!    `includes` statements append the mixin's members, operations that share a
//!    name are grouped into overload sets, two members that would collide in
//!    the generated trait are rejected, unions are named and collected, and
//!    every member is classified for the V8 fast-API path.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use weedle::attribute::{ExtendedAttribute as WExtAttr, ExtendedAttributeList, IdentifierOrString};
use weedle::common::Identifier;
use weedle::interface::{
    ConstMember, InterfaceMember, Special as WSpecial, StringifierOrInheritOrStatic,
    StringifierOrStatic,
};
use weedle::literal::{ConstValue as WConstValue, DefaultValue as WDefaultValue, IntegerLit};
use weedle::mixin::MixinMember;
use weedle::namespace::NamespaceMember;
use weedle::types::{
    ConstType, NonAnyType, ReturnType as WReturnType, SingleType, Type as WType, UnionMemberType,
};
use weedle::{Definition, Parse};

use crate::model::*;
use crate::naming;
use crate::{Error, Result};

/// What kind of definition a bare identifier refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Interface,
    Mixin,
    Dictionary,
    Enum,
    Callback,
    CallbackInterface,
    Namespace,
    Typedef,
}

/// Parses one WebIDL source and resolves it into a [`Model`].
pub fn parse_webidl(source: &str) -> Result<Model> {
    let definitions = parse_definitions(source)?;
    lower(&definitions)
}

/// Parses several WebIDL sources as one compilation unit.
///
/// Sources are concatenated in the order given, so a partial or an `includes`
/// statement may live in a different file from the interface it extends.
pub fn parse_webidl_units<'a, I>(sources: I) -> Result<Model>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut definitions = Vec::new();
    for source in sources {
        definitions.extend(parse_definitions(source)?);
    }
    lower(&definitions)
}

fn parse_definitions(source: &str) -> Result<Vec<Definition<'_>>> {
    // `weedle::parse` asserts on trailing input; a build-time tool must report
    // it instead.
    let (rest, definitions) =
        weedle::Definitions::parse(source).map_err(|e| Error::Syntax(e.to_string()))?;
    if !rest.trim().is_empty() {
        let head: String = rest.trim_start().chars().take(60).collect();
        return Err(Error::Syntax(format!(
            "unparsed input starting at `{head}`"
        )));
    }
    Ok(definitions)
}

struct Lowerer<'a> {
    kinds: HashMap<String, Kind>,
    typedef_asts: HashMap<String, &'a weedle::TypedefDefinition<'a>>,
    typedefs: HashMap<String, Type>,
    resolving: Vec<String>,
    unions: BTreeMap<String, Union>,
    opaque: BTreeSet<String>,
}

fn lower(definitions: &[Definition<'_>]) -> Result<Model> {
    let mut kinds = HashMap::new();
    let mut typedef_asts = HashMap::new();
    for definition in definitions {
        let (name, kind) = match definition {
            Definition::Interface(d) => (d.identifier.0, Kind::Interface),
            Definition::InterfaceMixin(d) => (d.identifier.0, Kind::Mixin),
            Definition::Dictionary(d) => (d.identifier.0, Kind::Dictionary),
            Definition::Enum(d) => (d.identifier.0, Kind::Enum),
            Definition::Callback(d) => (d.identifier.0, Kind::Callback),
            Definition::CallbackInterface(d) => (d.identifier.0, Kind::CallbackInterface),
            Definition::Namespace(d) => (d.identifier.0, Kind::Namespace),
            Definition::Typedef(d) => {
                typedef_asts.insert(d.identifier.0.to_string(), d);
                (d.identifier.0, Kind::Typedef)
            }
            _ => continue,
        };
        if let Some(previous) = kinds.insert(name.to_string(), kind) {
            if previous != kind {
                return Err(Error::Model(format!(
                    "`{name}` is declared both as {previous:?} and as {kind:?}"
                )));
            }
            return Err(Error::Model(format!(
                "`{name}` is declared twice as {kind:?}; use a `partial` block to extend it"
            )));
        }
    }

    let mut lowerer = Lowerer {
        kinds,
        typedef_asts,
        typedefs: HashMap::new(),
        resolving: Vec::new(),
        unions: BTreeMap::new(),
        opaque: BTreeSet::new(),
    };

    let typedef_names: Vec<String> = lowerer.typedef_asts.keys().cloned().collect();
    for name in typedef_names {
        lowerer.typedef(&name)?;
    }

    let mut model = Model::default();
    lowerer.lower_definitions(definitions, &mut model)?;
    reject_misplaced_buffer_references(&model)?;

    model.unions = lowerer.unions.into_values().collect();
    model.opaque_interfaces = lowerer.opaque.into_iter().collect();
    model.typedefs = {
        let mut typedefs: Vec<Typedef> = definitions
            .iter()
            .filter_map(|d| match d {
                Definition::Typedef(t) => Some(t.identifier.0.to_string()),
                _ => None,
            })
            .map(|name| {
                let ty = lowerer.typedefs.get(&name).cloned().unwrap_or(Type::Any);
                Typedef { name, ty }
            })
            .collect();
        typedefs.dedup_by(|a, b| a.name == b.name);
        typedefs
    };
    Ok(model)
}

impl<'a> Lowerer<'a> {
    fn lower_definitions(
        &mut self,
        definitions: &[Definition<'a>],
        model: &mut Model,
    ) -> Result<()> {
        for definition in definitions {
            match definition {
                Definition::Enum(d) => model.enums.push(self.lower_enum(d)),
                Definition::Callback(d) => {
                    let callback = self.lower_callback(d)?;
                    model.callbacks.push(callback);
                }
                Definition::CallbackInterface(d) => {
                    let iface = self.lower_callback_interface(d)?;
                    model.callback_interfaces.push(iface);
                }
                Definition::Dictionary(d) => {
                    let dictionary = self.lower_dictionary(d)?;
                    model.dictionaries.push(dictionary);
                }
                Definition::Interface(d) => {
                    let interface = self.lower_interface(d)?;
                    model.interfaces.push(interface);
                }
                Definition::Namespace(d) => {
                    let namespace = self.lower_namespace(d)?;
                    model.namespaces.push(namespace);
                }
                _ => {}
            }
        }

        for definition in definitions {
            match definition {
                Definition::PartialDictionary(d) => {
                    let members = self.lower_dictionary_members(&d.members.body)?;
                    let name = d.identifier.0;
                    let target = model
                        .dictionaries
                        .iter_mut()
                        .find(|x| x.name == name)
                        .ok_or_else(|| {
                            Error::Model(format!(
                                "partial dictionary `{name}` has no main declaration"
                            ))
                        })?;
                    target.members.extend(members);
                    target.partial_blocks += 1;
                }
                Definition::PartialInterface(d) => {
                    let name = d.identifier.0;
                    let mut scratch = InterfaceParts::default();
                    self.lower_interface_members(
                        &d.members.body,
                        MemberOrigin::Partial,
                        &mut scratch,
                    )?;
                    let target = model
                        .interfaces
                        .iter_mut()
                        .find(|x| x.name == name)
                        .ok_or_else(|| {
                            Error::Model(format!(
                                "partial interface `{name}` has no main declaration"
                            ))
                        })?;
                    scratch.merge_into(target);
                    target.partial_blocks += 1;
                }
                Definition::PartialNamespace(d) => {
                    let name = d.identifier.0;
                    let mut constants = Vec::new();
                    let mut members = Vec::new();
                    self.lower_namespace_members(&d.members.body, &mut constants, &mut members)?;
                    let target = model
                        .namespaces
                        .iter_mut()
                        .find(|x| x.name == name)
                        .ok_or_else(|| {
                            Error::Model(format!(
                                "partial namespace `{name}` has no main declaration"
                            ))
                        })?;
                    target.constants.extend(constants);
                    for member in members {
                        push_member(&mut target.members, member);
                    }
                    target.partial_blocks += 1;
                }
                _ => {}
            }
        }

        // Mixins are lowered once per `includes` statement so that each copy
        // records the mixin it came from.
        let mixins: HashMap<&str, &weedle::InterfaceMixinDefinition<'a>> = definitions
            .iter()
            .filter_map(|d| match d {
                Definition::InterfaceMixin(m) => Some((m.identifier.0, m)),
                _ => None,
            })
            .collect();
        let partial_mixins: Vec<&weedle::PartialInterfaceMixinDefinition<'a>> = definitions
            .iter()
            .filter_map(|d| match d {
                Definition::PartialInterfaceMixin(m) => Some(m),
                _ => None,
            })
            .collect();

        for definition in definitions {
            let Definition::IncludesStatement(statement) = definition else {
                continue;
            };
            let host = statement.lhs_identifier.0;
            let mixin_name = statement.rhs_identifier.0;
            let mixin = mixins.get(mixin_name).ok_or_else(|| {
                Error::Model(format!(
                    "`{host} includes {mixin_name}` names an unknown mixin"
                ))
            })?;
            let origin = MemberOrigin::Mixin(mixin_name.to_string());
            let mut scratch = InterfaceParts::default();
            self.lower_mixin_members(&mixin.members.body, &origin, &mut scratch)?;
            for partial in &partial_mixins {
                if partial.identifier.0 == mixin_name {
                    self.lower_mixin_members(&partial.members.body, &origin, &mut scratch)?;
                }
            }
            let target = model
                .interfaces
                .iter_mut()
                .find(|x| x.name == host)
                .ok_or_else(|| {
                    Error::Model(format!(
                        "`{host} includes {mixin_name}` names an unknown interface"
                    ))
                })?;
            scratch.merge_into(target);
            target.included_mixins.push(mixin_name.to_string());
        }

        for interface in &mut model.interfaces {
            check_member_collisions(interface)?;
            finish_interface(interface);
        }
        for namespace in &mut model.namespaces {
            finish_namespace(namespace);
        }
        Ok(())
    }

    // -- types ------------------------------------------------------------

    fn typedef(&mut self, name: &str) -> Result<Type> {
        if let Some(ty) = self.typedefs.get(name) {
            return Ok(ty.clone());
        }
        if self.resolving.iter().any(|n| n == name) {
            return Err(Error::Model(format!("typedef `{name}` is cyclic")));
        }
        let Some(ast) = self.typedef_asts.get(name).copied() else {
            return Err(Error::Model(format!("unknown typedef `{name}`")));
        };
        self.resolving.push(name.to_string());
        let ty = self.lower_type(&ast.type_.type_);
        self.resolving.pop();
        let ty = ty?;
        self.typedefs.insert(name.to_string(), ty.clone());
        Ok(ty)
    }

    fn resolve_identifier(&mut self, name: &str) -> Result<Type> {
        match self.kinds.get(name).copied() {
            Some(Kind::Typedef) => self.typedef(name),
            Some(Kind::Dictionary) => Ok(Type::Dictionary(name.to_string())),
            Some(Kind::Enum) => Ok(Type::Enum(name.to_string())),
            Some(Kind::Callback) => Ok(Type::Callback(name.to_string())),
            Some(Kind::CallbackInterface) => Ok(Type::CallbackInterface(name.to_string())),
            Some(Kind::Interface) | Some(Kind::Mixin) | Some(Kind::Namespace) => {
                Ok(Type::Interface(name.to_string()))
            }
            None => {
                self.opaque.insert(name.to_string());
                Ok(Type::Interface(name.to_string()))
            }
        }
    }

    fn lower_type(&mut self, ty: &WType<'a>) -> Result<Type> {
        match ty {
            WType::Single(SingleType::Any(_)) => Ok(Type::Any),
            WType::Single(SingleType::NonAny(inner)) => self.lower_non_any(inner),
            WType::Union(union) => {
                let mut members = Vec::new();
                self.flatten_union(&union.type_.body.list, &mut members)?;
                let name = self.intern_union(members);
                Ok(nullable_if(union.q_mark.is_some(), Type::Union(name)))
            }
        }
    }

    fn flatten_union(
        &mut self,
        members: &[UnionMemberType<'a>],
        out: &mut Vec<Type>,
    ) -> Result<()> {
        for member in members {
            match member {
                UnionMemberType::Single(single) => {
                    let ty = self.lower_non_any(&single.type_)?;
                    if !out.contains(&ty) {
                        out.push(ty);
                    }
                }
                UnionMemberType::Union(nested) => {
                    // WebIDL flattens nested union member types.
                    self.flatten_union(&nested.type_.body.list, out)?;
                }
            }
        }
        Ok(())
    }

    fn intern_union(&mut self, members: Vec<Type>) -> String {
        let name = members
            .iter()
            .map(|m| m.variant_name())
            .collect::<Vec<_>>()
            .join("Or");
        let mut used: Vec<String> = Vec::new();
        let arms = members
            .into_iter()
            .map(|ty| {
                let mut rust_name = ty.variant_name();
                if used.contains(&rust_name) {
                    rust_name = format!("{rust_name}{}", used.len());
                }
                used.push(rust_name.clone());
                UnionMember { rust_name, ty }
            })
            .collect();
        self.unions.entry(name.clone()).or_insert_with(|| Union {
            name: name.clone(),
            members: arms,
        });
        name
    }

    fn lower_non_any(&mut self, ty: &NonAnyType<'a>) -> Result<Type> {
        use weedle::types::{FloatingPointType, IntegerType};

        let resolved = match ty {
            NonAnyType::Promise(promise) => {
                Type::Promise(Box::new(self.lower_return_type(&promise.generics.body)?))
            }
            NonAnyType::Integer(int) => {
                let base = match int.type_ {
                    IntegerType::LongLong(t) => {
                        if t.unsigned.is_some() {
                            Type::UnsignedLongLong
                        } else {
                            Type::LongLong
                        }
                    }
                    IntegerType::Long(t) => {
                        if t.unsigned.is_some() {
                            Type::UnsignedLong
                        } else {
                            Type::Long
                        }
                    }
                    IntegerType::Short(t) => {
                        if t.unsigned.is_some() {
                            Type::UnsignedShort
                        } else {
                            Type::Short
                        }
                    }
                };
                nullable_if(int.q_mark.is_some(), base)
            }
            NonAnyType::FloatingPoint(float) => {
                let base = match float.type_ {
                    FloatingPointType::Float(t) => Type::Float {
                        unrestricted: t.unrestricted.is_some(),
                    },
                    FloatingPointType::Double(t) => Type::Double {
                        unrestricted: t.unrestricted.is_some(),
                    },
                };
                nullable_if(float.q_mark.is_some(), base)
            }
            NonAnyType::Boolean(t) => nullable_if(t.q_mark.is_some(), Type::Boolean),
            NonAnyType::Byte(t) => nullable_if(t.q_mark.is_some(), Type::Byte),
            NonAnyType::Octet(t) => nullable_if(t.q_mark.is_some(), Type::Octet),
            NonAnyType::ByteString(t) => nullable_if(t.q_mark.is_some(), Type::ByteString),
            NonAnyType::DOMString(t) => nullable_if(t.q_mark.is_some(), Type::DomString),
            NonAnyType::USVString(t) => nullable_if(t.q_mark.is_some(), Type::UsvString),
            NonAnyType::Object(t) => nullable_if(t.q_mark.is_some(), Type::Object),
            NonAnyType::Symbol(t) => nullable_if(t.q_mark.is_some(), Type::Symbol),
            NonAnyType::Sequence(t) => {
                let inner = self.lower_type(&t.type_.generics.body)?;
                nullable_if(t.q_mark.is_some(), Type::Sequence(Box::new(inner)))
            }
            NonAnyType::FrozenArrayType(t) => {
                let inner = self.lower_type(&t.type_.generics.body)?;
                nullable_if(t.q_mark.is_some(), Type::FrozenArray(Box::new(inner)))
            }
            NonAnyType::ObservableArrayType(t) => {
                let inner = self.lower_type(&t.type_.generics.body)?;
                nullable_if(t.q_mark.is_some(), Type::ObservableArray(Box::new(inner)))
            }
            NonAnyType::RecordType(t) => {
                let (key, _, value) = &t.type_.generics.body;
                let key = self.lower_record_key(key)?;
                let value = self.lower_type(value)?;
                nullable_if(
                    t.q_mark.is_some(),
                    Type::Record(Box::new(key), Box::new(value)),
                )
            }
            NonAnyType::Identifier(t) => {
                let resolved = self.resolve_identifier(t.type_.0)?;
                nullable_if(t.q_mark.is_some(), resolved)
            }
            NonAnyType::Error(t) => {
                nullable_if(t.q_mark.is_some(), Type::Interface("Error".to_string()))
            }
            // The buffer source types reach here rather than through
            // `resolve_identifier`: weedle has a keyword arm for each of them
            // and tries those before the `Identifier` arm, so a unit's own
            // `typedef (ArrayBufferView or ArrayBuffer) BufferSource;` is
            // shadowed by the arm below — harmlessly, since the two agree on
            // what the name means.
            NonAnyType::ArrayBuffer(t) => {
                nullable_if(t.q_mark.is_some(), Type::Buffer(BufferType::ArrayBuffer))
            }
            NonAnyType::DataView(t) => {
                nullable_if(t.q_mark.is_some(), Type::Buffer(BufferType::DataView))
            }
            NonAnyType::Int8Array(t) => {
                nullable_if(t.q_mark.is_some(), Type::Buffer(BufferType::Int8Array))
            }
            NonAnyType::Int16Array(t) => {
                nullable_if(t.q_mark.is_some(), Type::Buffer(BufferType::Int16Array))
            }
            NonAnyType::Int32Array(t) => {
                nullable_if(t.q_mark.is_some(), Type::Buffer(BufferType::Int32Array))
            }
            NonAnyType::Uint8Array(t) => {
                nullable_if(t.q_mark.is_some(), Type::Buffer(BufferType::Uint8Array))
            }
            NonAnyType::Uint16Array(t) => {
                nullable_if(t.q_mark.is_some(), Type::Buffer(BufferType::Uint16Array))
            }
            NonAnyType::Uint32Array(t) => {
                nullable_if(t.q_mark.is_some(), Type::Buffer(BufferType::Uint32Array))
            }
            NonAnyType::Uint8ClampedArray(t) => nullable_if(
                t.q_mark.is_some(),
                Type::Buffer(BufferType::Uint8ClampedArray),
            ),
            NonAnyType::Float32Array(t) => {
                nullable_if(t.q_mark.is_some(), Type::Buffer(BufferType::Float32Array))
            }
            NonAnyType::Float64Array(t) => {
                nullable_if(t.q_mark.is_some(), Type::Buffer(BufferType::Float64Array))
            }
            NonAnyType::ArrayBufferView(t) => nullable_if(
                t.q_mark.is_some(),
                Type::Buffer(BufferType::ArrayBufferView),
            ),
            NonAnyType::BufferSource(t) => {
                nullable_if(t.q_mark.is_some(), Type::Buffer(BufferType::BufferSource))
            }
        };
        if let Type::Interface(name) = resolved.unwrap_nullable()
            && !self.kinds.contains_key(name)
        {
            self.opaque.insert(name.clone());
        }
        Ok(resolved)
    }

    fn lower_record_key(&mut self, key: &weedle::types::RecordKeyType<'a>) -> Result<Type> {
        use weedle::types::RecordKeyType;
        Ok(match key {
            RecordKeyType::Byte(_) => Type::ByteString,
            RecordKeyType::DOM(_) => Type::DomString,
            RecordKeyType::USV(_) => Type::UsvString,
            RecordKeyType::NonAny(inner) => self.lower_non_any(inner)?,
        })
    }

    fn lower_return_type(&mut self, ty: &WReturnType<'a>) -> Result<Type> {
        match ty {
            WReturnType::Undefined(_) => Ok(Type::Undefined),
            WReturnType::Type(inner) => self.lower_type(inner),
        }
    }

    fn lower_const_type(&mut self, ty: &ConstType<'a>) -> Result<Type> {
        use weedle::types::{FloatingPointType, IntegerType};
        Ok(match ty {
            ConstType::Integer(int) => match int.type_ {
                IntegerType::LongLong(t) => {
                    if t.unsigned.is_some() {
                        Type::UnsignedLongLong
                    } else {
                        Type::LongLong
                    }
                }
                IntegerType::Long(t) => {
                    if t.unsigned.is_some() {
                        Type::UnsignedLong
                    } else {
                        Type::Long
                    }
                }
                IntegerType::Short(t) => {
                    if t.unsigned.is_some() {
                        Type::UnsignedShort
                    } else {
                        Type::Short
                    }
                }
            },
            ConstType::FloatingPoint(float) => match float.type_ {
                FloatingPointType::Float(t) => Type::Float {
                    unrestricted: t.unrestricted.is_some(),
                },
                FloatingPointType::Double(t) => Type::Double {
                    unrestricted: t.unrestricted.is_some(),
                },
            },
            ConstType::Boolean(_) => Type::Boolean,
            ConstType::Byte(_) => Type::Byte,
            ConstType::Octet(_) => Type::Octet,
            ConstType::Identifier(ident) => self.resolve_identifier(ident.type_.0)?,
        })
    }

    // -- definitions ------------------------------------------------------

    fn lower_enum(&mut self, definition: &weedle::EnumDefinition<'a>) -> Enum {
        let mut used: Vec<String> = Vec::new();
        let values = definition
            .values
            .body
            .list
            .iter()
            .map(|value| {
                let mut rust_name = naming::pascal_case(value.0);
                while used.contains(&rust_name) {
                    rust_name.push('_');
                }
                used.push(rust_name.clone());
                EnumValue {
                    value: value.0.to_string(),
                    rust_name,
                }
            })
            .collect();
        Enum {
            name: definition.identifier.0.to_string(),
            values,
            attrs: self.lower_ext_attrs(definition.attributes.as_ref()),
        }
    }

    fn lower_callback(&mut self, definition: &weedle::CallbackDefinition<'a>) -> Result<Callback> {
        Ok(Callback {
            name: definition.identifier.0.to_string(),
            return_type: self.lower_return_type(&definition.return_type)?,
            arguments: self.lower_arguments(&definition.arguments.body.list)?,
            attrs: self.lower_ext_attrs(definition.attributes.as_ref()),
        })
    }

    fn lower_callback_interface(
        &mut self,
        definition: &weedle::CallbackInterfaceDefinition<'a>,
    ) -> Result<CallbackInterface> {
        let name = definition.identifier.0.to_string();
        let mut constants = Vec::new();
        let mut operation = None;
        let mut operation_name = String::new();
        for member in &definition.members.body {
            match member {
                InterfaceMember::Const(constant) => {
                    constants.push(self.lower_constant(constant, MemberOrigin::Interface)?);
                }
                InterfaceMember::Operation(op) => {
                    if operation.is_some() {
                        return Err(Error::Model(format!(
                            "callback interface `{name}` declares more than one operation"
                        )));
                    }
                    let idl_name = op.identifier.map(|i| i.0).unwrap_or_default().to_string();
                    operation_name = idl_name.clone();
                    operation = Some(Operation {
                        name: idl_name.clone(),
                        rust_name: naming::snake_case(&idl_name),
                        arguments: self.lower_arguments(&op.args.body.list)?,
                        return_type: self.lower_return_type(&op.return_type)?,
                        attrs: self.lower_ext_attrs(op.attributes.as_ref()),
                        special: None,
                        stringifier: false,
                        fast: FastApi::Ineligible(NotFastBecause::ReturnType),
                    });
                }
                _ => {
                    return Err(Error::Model(format!(
                        "callback interface `{name}` may only declare constants and one operation"
                    )));
                }
            }
        }
        let operation = operation.ok_or_else(|| {
            Error::Model(format!("callback interface `{name}` declares no operation"))
        })?;
        Ok(CallbackInterface {
            name,
            operation,
            operation_name,
            constants,
            attrs: self.lower_ext_attrs(definition.attributes.as_ref()),
        })
    }

    fn lower_dictionary(
        &mut self,
        definition: &weedle::DictionaryDefinition<'a>,
    ) -> Result<Dictionary> {
        Ok(Dictionary {
            name: definition.identifier.0.to_string(),
            parent: definition.inheritance.map(|i| i.identifier.0.to_string()),
            members: self.lower_dictionary_members(&definition.members.body)?,
            attrs: self.lower_ext_attrs(definition.attributes.as_ref()),
            partial_blocks: 0,
        })
    }

    fn lower_dictionary_members(
        &mut self,
        members: &[weedle::dictionary::DictionaryMember<'a>],
    ) -> Result<Vec<DictionaryMember>> {
        members
            .iter()
            .map(|member| {
                let ty = self.lower_type(&member.type_.type_)?;
                let mut attrs = self.lower_ext_attrs(member.attributes.as_ref());
                attrs
                    .0
                    .extend(self.lower_ext_attrs(member.type_.attributes.as_ref()).0);
                validate_string_annotation(&ty, &attrs)?;
                Ok(DictionaryMember {
                    name: member.identifier.0.to_string(),
                    rust_name: naming::snake_case(member.identifier.0),
                    ty,
                    required: member.required.is_some(),
                    default: member.default.map(|d| lower_default(&d.value)),
                    attrs,
                })
            })
            .collect()
    }

    fn lower_interface(
        &mut self,
        definition: &weedle::InterfaceDefinition<'a>,
    ) -> Result<Interface> {
        let attrs = self.lower_ext_attrs(definition.attributes.as_ref());
        let mut parts = InterfaceParts::default();
        self.lower_interface_members(
            &definition.members.body,
            MemberOrigin::Interface,
            &mut parts,
        )?;
        let mut interface = Interface {
            name: definition.identifier.0.to_string(),
            parent: definition.inheritance.map(|i| i.identifier.0.to_string()),
            exposure: exposure_of(&attrs),
            attrs,
            constructors: Vec::new(),
            constants: Vec::new(),
            members: Vec::new(),
            included_mixins: Vec::new(),
            partial_blocks: 0,
            iterable: None,
            stringifier: None,
            indexed_getter: None,
            named_getter: None,
            special_operations: Vec::new(),
        };
        parts.merge_into(&mut interface);
        Ok(interface)
    }

    fn lower_interface_members(
        &mut self,
        members: &[InterfaceMember<'a>],
        origin: MemberOrigin,
        out: &mut InterfaceParts,
    ) -> Result<()> {
        for member in members {
            match member {
                InterfaceMember::Const(constant) => {
                    out.constants
                        .push(self.lower_constant(constant, origin.clone())?);
                }
                InterfaceMember::Constructor(ctor) => {
                    let arguments = self.lower_arguments(&ctor.args.body.list)?;
                    out.constructors.push(Constructor {
                        rust_name: String::new(),
                        arguments,
                        attrs: self.lower_ext_attrs(ctor.attributes.as_ref()),
                    });
                }
                InterfaceMember::Attribute(attribute) => {
                    let (is_static, inherit, stringifier) = match attribute.modifier {
                        Some(StringifierOrInheritOrStatic::Static(_)) => (true, false, false),
                        Some(StringifierOrInheritOrStatic::Inherit(_)) => (false, true, false),
                        Some(StringifierOrInheritOrStatic::Stringifier(_)) => (false, false, true),
                        None => (false, false, false),
                    };
                    let lowered = self.build_attribute(
                        attribute.identifier.0,
                        &attribute.type_,
                        attribute.readonly.is_some(),
                        is_static,
                        inherit,
                        stringifier,
                        attribute.attributes.as_ref(),
                        origin.clone(),
                    )?;
                    if stringifier {
                        out.stringifier = Some(Stringifier::Attribute(lowered.name.clone()));
                    }
                    out.members.push(Member::Attribute(lowered));
                }
                InterfaceMember::Operation(operation) => {
                    let (is_static, stringifier) = match operation.modifier {
                        Some(StringifierOrStatic::Static(_)) => (true, false),
                        Some(StringifierOrStatic::Stringifier(_)) => (false, true),
                        None => (false, false),
                    };
                    let special = operation.special.map(lower_special);
                    let name = operation.identifier.map(|i| i.0).unwrap_or_default();
                    let arguments = self.lower_arguments(&operation.args.body.list)?;
                    let lowered = Operation {
                        name: name.to_string(),
                        rust_name: if name.is_empty() {
                            anonymous_special_name(special, &arguments).to_string()
                        } else {
                            naming::snake_case(name)
                        },
                        arguments,
                        return_type: self.lower_return_type(&operation.return_type)?,
                        attrs: self.lower_ext_attrs(operation.attributes.as_ref()),
                        special,
                        stringifier,
                        fast: FastApi::Eligible,
                    };
                    validate_buffer_references(&lowered)?;
                    if stringifier {
                        out.stringifier = Some(Stringifier::Operation(name.to_string()));
                    }
                    if let Some(Special::Getter) = special {
                        let indexed = lowered
                            .arguments
                            .first()
                            .is_some_and(|a| a.ty.unwrap_nullable().is_integer());
                        let label = if indexed {
                            "IndexedGetter"
                        } else {
                            "NamedGetter"
                        };
                        let slot = if indexed {
                            &mut out.indexed_getter
                        } else {
                            &mut out.named_getter
                        };
                        *slot = Some(if name.is_empty() {
                            format!("[[{label}]]")
                        } else {
                            name.to_string()
                        });
                    }
                    if name.is_empty() {
                        out.special_operations.push(lowered);
                    } else {
                        push_operation(&mut out.members, is_static, origin.clone(), lowered);
                    }
                }
                InterfaceMember::Iterable(iterable) => {
                    out.iterable = Some(self.lower_iterable(iterable)?);
                }
                InterfaceMember::AsyncIterable(iterable) => {
                    out.iterable = Some(self.lower_async_iterable(iterable)?);
                }
                InterfaceMember::Stringifier(_) => {
                    out.stringifier.get_or_insert(Stringifier::Bare);
                }
                InterfaceMember::Maplike(_) | InterfaceMember::Setlike(_) => {
                    return Err(Error::Unsupported(
                        "`maplike` and `setlike` declarations".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn lower_mixin_members(
        &mut self,
        members: &[MixinMember<'a>],
        origin: &MemberOrigin,
        out: &mut InterfaceParts,
    ) -> Result<()> {
        for member in members {
            match member {
                MixinMember::Const(constant) => {
                    out.constants
                        .push(self.lower_constant(constant, origin.clone())?);
                }
                MixinMember::Attribute(attribute) => {
                    let stringifier = attribute.stringifier.is_some();
                    let lowered = self.build_attribute(
                        attribute.identifier.0,
                        &attribute.type_,
                        attribute.readonly.is_some(),
                        false,
                        false,
                        stringifier,
                        attribute.attributes.as_ref(),
                        origin.clone(),
                    )?;
                    if stringifier {
                        out.stringifier = Some(Stringifier::Attribute(lowered.name.clone()));
                    }
                    out.members.push(Member::Attribute(lowered));
                }
                MixinMember::Operation(operation) => {
                    let name = operation.identifier.map(|i| i.0).unwrap_or_default();
                    let stringifier = operation.stringifier.is_some();
                    let lowered = Operation {
                        name: name.to_string(),
                        rust_name: naming::snake_case(name),
                        arguments: self.lower_arguments(&operation.args.body.list)?,
                        return_type: self.lower_return_type(&operation.return_type)?,
                        attrs: self.lower_ext_attrs(operation.attributes.as_ref()),
                        special: None,
                        stringifier,
                        fast: FastApi::Eligible,
                    };
                    if stringifier {
                        out.stringifier = Some(Stringifier::Operation(name.to_string()));
                    }
                    validate_buffer_references(&lowered)?;
                    if name.is_empty() {
                        out.special_operations.push(lowered);
                    } else {
                        push_operation(&mut out.members, false, origin.clone(), lowered);
                    }
                }
                MixinMember::Stringifier(_) => {
                    out.stringifier.get_or_insert(Stringifier::Bare);
                }
            }
        }
        Ok(())
    }

    fn lower_namespace(
        &mut self,
        definition: &weedle::NamespaceDefinition<'a>,
    ) -> Result<Namespace> {
        let attrs = self.lower_ext_attrs(definition.attributes.as_ref());
        let mut constants = Vec::new();
        let mut members = Vec::new();
        self.lower_namespace_members(&definition.members.body, &mut constants, &mut members)?;
        let mut namespace = Namespace {
            name: definition.identifier.0.to_string(),
            exposure: exposure_of(&attrs),
            attrs,
            constants,
            members: Vec::new(),
            partial_blocks: 0,
        };
        for member in members {
            push_member(&mut namespace.members, member);
        }
        Ok(namespace)
    }

    fn lower_namespace_members(
        &mut self,
        members: &[NamespaceMember<'a>],
        constants: &mut Vec<Constant>,
        out: &mut Vec<Member>,
    ) -> Result<()> {
        for member in members {
            match member {
                NamespaceMember::Const(constant) => {
                    let ty = self.lower_const_type(&constant.const_type)?;
                    constants.push(Constant {
                        name: constant.identifier.0.to_string(),
                        rust_name: naming::screaming_snake_case(constant.identifier.0),
                        ty,
                        value: lower_const_value(&constant.const_value),
                        attrs: self.lower_ext_attrs(constant.attributes.as_ref()),
                        origin: MemberOrigin::Interface,
                    });
                }
                NamespaceMember::Attribute(attribute) => {
                    let lowered = self.build_attribute(
                        attribute.identifier.0,
                        &attribute.type_,
                        true,
                        true,
                        false,
                        false,
                        attribute.attributes.as_ref(),
                        MemberOrigin::Interface,
                    )?;
                    out.push(Member::Attribute(lowered));
                }
                NamespaceMember::Operation(operation) => {
                    let name = operation.identifier.map(|i| i.0).unwrap_or_default();
                    let lowered = Operation {
                        name: name.to_string(),
                        rust_name: naming::snake_case(name),
                        arguments: self.lower_arguments(&operation.args.body.list)?,
                        return_type: self.lower_return_type(&operation.return_type)?,
                        attrs: self.lower_ext_attrs(operation.attributes.as_ref()),
                        special: None,
                        stringifier: false,
                        fast: FastApi::Eligible,
                    };
                    validate_buffer_references(&lowered)?;
                    out.push(Member::Operation(OperationGroup {
                        name: name.to_string(),
                        is_static: true,
                        overloads: vec![lowered],
                        fast: FastApi::Ineligible(NotFastBecause::Static),
                        origin: MemberOrigin::Interface,
                    }));
                }
            }
        }
        Ok(())
    }

    fn lower_iterable(
        &mut self,
        iterable: &weedle::interface::IterableInterfaceMember<'a>,
    ) -> Result<Iterable> {
        use weedle::interface::IterableInterfaceMember as It;
        Ok(match iterable {
            It::Single(single) => Iterable {
                key: None,
                value: self.lower_type(&single.generics.body.type_)?,
                is_async: false,
            },
            It::Double(double) => {
                let (key, _, value) = &double.generics.body;
                Iterable {
                    key: Some(self.lower_type(&key.type_)?),
                    value: self.lower_type(&value.type_)?,
                    is_async: false,
                }
            }
        })
    }

    fn lower_async_iterable(
        &mut self,
        iterable: &weedle::interface::AsyncIterableInterfaceMember<'a>,
    ) -> Result<Iterable> {
        use weedle::interface::AsyncIterableInterfaceMember as It;
        Ok(match iterable {
            It::Single(single) => Iterable {
                key: None,
                value: self.lower_type(&single.generics.body.type_)?,
                is_async: true,
            },
            It::Double(double) => {
                let (key, _, value) = &double.generics.body;
                Iterable {
                    key: Some(self.lower_type(&key.type_)?),
                    value: self.lower_type(&value.type_)?,
                    is_async: true,
                }
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_attribute(
        &mut self,
        name: &str,
        ty: &weedle::types::AttributedType<'a>,
        readonly: bool,
        is_static: bool,
        inherit: bool,
        stringifier: bool,
        attributes: Option<&ExtendedAttributeList<'a>>,
        origin: MemberOrigin,
    ) -> Result<Attribute> {
        let lowered_type = self.lower_type(&ty.type_)?;
        let mut attrs = self.lower_ext_attrs(attributes);
        attrs
            .0
            .extend(self.lower_ext_attrs(ty.attributes.as_ref()).0);
        if attrs.has("PutForwards")
            && (is_static
                || !readonly
                || !matches!(lowered_type, Type::Interface(_))
                || attrs.ident("PutForwards").is_none())
        {
            return Err(Error::Model(format!(
                "attribute `{name}` uses [PutForwards] without an identifier-valued target on a readonly, non-static interface type"
            )));
        }
        validate_string_annotation(&lowered_type, &attrs)?;
        let getter_name = naming::snake_case(name);
        let setter_name = if readonly {
            None
        } else {
            Some(naming::snake_case(&format!("set_{name}")))
        };
        let throws = attrs.has("Throws");
        let fast_getter = classify_getter(&lowered_type, is_static, throws);
        let fast_setter = if readonly {
            None
        } else {
            Some(classify_setter(&lowered_type, is_static, throws))
        };
        Ok(Attribute {
            name: name.to_string(),
            getter_name,
            setter_name,
            ty: lowered_type,
            readonly,
            is_static,
            inherit,
            stringifier,
            attrs,
            fast_getter,
            fast_setter,
            origin,
        })
    }

    fn lower_constant(
        &mut self,
        constant: &ConstMember<'a>,
        origin: MemberOrigin,
    ) -> Result<Constant> {
        Ok(Constant {
            name: constant.identifier.0.to_string(),
            rust_name: naming::screaming_snake_case(constant.identifier.0),
            ty: self.lower_const_type(&constant.const_type)?,
            value: lower_const_value(&constant.const_value),
            attrs: self.lower_ext_attrs(constant.attributes.as_ref()),
            origin,
        })
    }

    fn lower_arguments(
        &mut self,
        arguments: &[weedle::argument::Argument<'a>],
    ) -> Result<Vec<Argument>> {
        use weedle::argument::Argument as Arg;
        arguments
            .iter()
            .map(|argument| match argument {
                Arg::Single(single) => {
                    let mut attrs = self.lower_ext_attrs(single.attributes.as_ref());
                    attrs
                        .0
                        .extend(self.lower_ext_attrs(single.type_.attributes.as_ref()).0);
                    let ty = self.lower_type(&single.type_.type_)?;
                    validate_string_annotation(&ty, &attrs)?;
                    Ok(Argument {
                        name: single.identifier.0.to_string(),
                        rust_name: naming::snake_case(single.identifier.0),
                        ty,
                        optional: single.optional.is_some(),
                        variadic: false,
                        default: single.default.map(|d| lower_default(&d.value)),
                        attrs,
                    })
                }
                Arg::Variadic(variadic) => {
                    let ty = self.lower_type(&variadic.type_)?;
                    let attrs = self.lower_ext_attrs(variadic.attributes.as_ref());
                    validate_string_annotation(&ty, &attrs)?;
                    Ok(Argument {
                        name: variadic.identifier.0.to_string(),
                        rust_name: naming::snake_case(variadic.identifier.0),
                        ty,
                        optional: false,
                        variadic: true,
                        default: None,
                        attrs,
                    })
                }
            })
            .collect()
    }

    fn lower_ext_attrs(&mut self, list: Option<&ExtendedAttributeList<'a>>) -> ExtendedAttributes {
        let Some(list) = list else {
            return ExtendedAttributes::default();
        };
        let attributes = list
            .body
            .list
            .iter()
            .map(|attribute| match attribute {
                WExtAttr::NoArgs(no_args) => ExtendedAttribute {
                    name: (no_args.0).0.to_string(),
                    value: ExtendedAttributeValue::None,
                },
                WExtAttr::Ident(ident) => ExtendedAttribute {
                    name: ident.lhs_identifier.0.to_string(),
                    value: match ident.rhs {
                        IdentifierOrString::Identifier(Identifier(value)) => {
                            ExtendedAttributeValue::Ident(value.to_string())
                        }
                        IdentifierOrString::String(value) => {
                            ExtendedAttributeValue::String(value.0.to_string())
                        }
                    },
                },
                WExtAttr::IdentList(list) => ExtendedAttribute {
                    name: list.identifier.0.to_string(),
                    value: ExtendedAttributeValue::IdentList(
                        list.list
                            .body
                            .list
                            .iter()
                            .map(|i| i.0.to_string())
                            .collect(),
                    ),
                },
                WExtAttr::WildCard(wildcard) => ExtendedAttribute {
                    name: wildcard.identifier.0.to_string(),
                    value: ExtendedAttributeValue::Wildcard,
                },
                WExtAttr::ArgList(args) => ExtendedAttribute {
                    name: args.identifier.0.to_string(),
                    value: ExtendedAttributeValue::Arguments {
                        rhs: None,
                        arguments: self
                            .lower_arguments(&args.args.body.list)
                            .unwrap_or_default(),
                    },
                },
                WExtAttr::NamedArgList(args) => ExtendedAttribute {
                    name: args.lhs_identifier.0.to_string(),
                    value: ExtendedAttributeValue::Arguments {
                        rhs: Some(args.rhs_identifier.0.to_string()),
                        arguments: self
                            .lower_arguments(&args.args.body.list)
                            .unwrap_or_default(),
                    },
                },
            })
            .collect();
        ExtendedAttributes(attributes)
    }
}

/// Members collected from one interface body, partial block or mixin.
#[derive(Default)]
struct InterfaceParts {
    constants: Vec<Constant>,
    constructors: Vec<Constructor>,
    members: Vec<Member>,
    special_operations: Vec<Operation>,
    iterable: Option<Iterable>,
    stringifier: Option<Stringifier>,
    indexed_getter: Option<String>,
    named_getter: Option<String>,
}

impl InterfaceParts {
    fn merge_into(self, interface: &mut Interface) {
        interface.constants.extend(self.constants);
        interface.constructors.extend(self.constructors);
        for member in self.members {
            push_member(&mut interface.members, member);
        }
        interface.special_operations.extend(self.special_operations);
        if self.iterable.is_some() {
            interface.iterable = self.iterable;
        }
        if self.stringifier.is_some() {
            interface.stringifier = self.stringifier;
        }
        if self.indexed_getter.is_some() {
            interface.indexed_getter = self.indexed_getter;
        }
        if self.named_getter.is_some() {
            interface.named_getter = self.named_getter;
        }
    }
}

fn push_member(members: &mut Vec<Member>, member: Member) {
    match member {
        Member::Attribute(attribute) => members.push(Member::Attribute(attribute)),
        Member::Operation(group) => {
            for overload in group.overloads {
                push_operation(members, group.is_static, group.origin.clone(), overload);
            }
        }
    }
}

fn push_operation(
    members: &mut Vec<Member>,
    is_static: bool,
    origin: MemberOrigin,
    operation: Operation,
) {
    if let Some(Member::Operation(group)) = members.iter_mut().find(|m| match m {
        Member::Operation(g) => g.name == operation.name && g.is_static == is_static,
        Member::Attribute(_) => false,
    }) {
        group.overloads.push(operation);
        return;
    }
    members.push(Member::Operation(OperationGroup {
        name: operation.name.clone(),
        is_static,
        overloads: vec![operation],
        fast: FastApi::Eligible,
        origin,
    }));
}

/// Rejects two members that would collide in the generated Rust trait.
///
/// Partials and mixins are merged before this runs, so a duplicate that only
/// appears once an `includes` statement has copied a mixin's members is caught
/// here instead of surfacing as a trait with two identically named methods.
fn check_member_collisions(interface: &Interface) -> Result<()> {
    check_duplicate_attributes(interface)?;
    check_duplicate_operations(interface)
}

/// Rejects two attributes of the same name in one interface. WebIDL names in an
/// interface are unique, so a mixin attribute that an interface also declares
/// is an authoring error, not a second property.
fn check_duplicate_attributes(interface: &Interface) -> Result<()> {
    let mut seen: Vec<&Attribute> = Vec::new();
    for member in &interface.members {
        let Member::Attribute(attribute) = member else {
            continue;
        };
        let Some(earlier) = seen.iter().find(|seen| seen.name == attribute.name) else {
            seen.push(attribute);
            continue;
        };
        return Err(Error::Model(format!(
            "interface `{}` declares the attribute `{}` more than once: {}",
            interface.name,
            attribute.name,
            describe_contributors(&interface.name, &earlier.origin, &attribute.origin),
        )));
    }
    Ok(())
}

/// Rejects two overloads that no caller could tell apart: the same name and
/// staticness with argument lists that agree in type, optionality and
/// variadicness. The generator names overloads by arity alone, so such a pair
/// would become a dead Rust method beside a live twin.
///
/// Overloads that differ in argument type, count or optionality are legal
/// WebIDL and stay accepted.
fn check_duplicate_operations(interface: &Interface) -> Result<()> {
    for member in &interface.members {
        let Member::Operation(group) = member else {
            continue;
        };
        if group.overloads.iter().any(Operation::buffer_references)
            && !group.overloads.iter().all(Operation::buffer_references)
        {
            return Err(Error::Unsupported(format!(
                "[TentaBufferReferences] must be present on every overload of `{}`",
                group.name
            )));
        }
        for (index, overload) in group.overloads.iter().enumerate() {
            let duplicated = group.overloads[..index]
                .iter()
                .any(|earlier| same_signature(earlier, overload));
            if duplicated {
                return Err(Error::Model(format!(
                    "interface `{}` declares the operation `{}` more than once with the \
                     argument list `{}`; overloads must differ in argument type or count",
                    interface.name,
                    group.name,
                    operation_signature(overload),
                )));
            }
        }
    }
    Ok(())
}

/// True when two overloads of one group carry the same argument list in every
/// way WebIDL overload resolution can see. The argument names are deliberately
/// ignored: they take no part in dispatch.
fn same_signature(left: &Operation, right: &Operation) -> bool {
    left.arguments.len() == right.arguments.len()
        && left
            .arguments
            .iter()
            .zip(&right.arguments)
            .all(|(a, b)| a.ty == b.ty && a.optional == b.optional && a.variadic == b.variadic)
}

/// One overload's argument list, spelled for a diagnostic.
fn operation_signature(operation: &Operation) -> String {
    let arguments = operation
        .arguments
        .iter()
        .map(|argument| {
            let optional = if argument.optional { "optional " } else { "" };
            let variadic = if argument.variadic { "..." } else { "" };
            format!("{optional}{}{variadic}", argument.ty.idl_name())
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("({arguments})")
}

/// Names the two declarations that contributed a duplicate member, as
/// precisely as the position-less syntax tree allows.
fn describe_contributors(interface: &str, first: &MemberOrigin, second: &MemberOrigin) -> String {
    let first = origin_description(interface, first);
    let second = origin_description(interface, second);
    if first == second {
        format!("declared twice in {first}")
    } else {
        format!("declared in {first} and in {second}")
    }
}

fn origin_description(interface: &str, origin: &MemberOrigin) -> String {
    match origin {
        MemberOrigin::Interface => format!("`interface {interface}`"),
        MemberOrigin::Partial => format!("`partial interface {interface}`"),
        MemberOrigin::Mixin(mixin) => format!("`mixin {mixin}` included by `{interface}`"),
    }
}

/// Assigns overload-aware Rust names, records origins and classifies members
/// once every partial and mixin has been merged.
fn finish_interface(interface: &mut Interface) {
    for (index, constructor) in interface.constructors.iter_mut().enumerate() {
        constructor.rust_name = if index == 0 {
            "construct".to_string()
        } else {
            format!("construct_{}", constructor.arguments.len())
        };
    }
    add_stringifier_member(interface);
    finish_members(&mut interface.members);
    for operation in &mut interface.special_operations {
        operation.fast = FastApi::Ineligible(NotFastBecause::Special);
    }
}

/// Adds the `toString` member a `stringifier` implies.
///
/// WebIDL's stringifier *is* the interface's `toString`: `stringifier attribute
/// USVString href;` means `toString` returns `this.href`, `stringifier DOMString
/// serialize();` means it is that operation, and a bare `stringifier;` leaves the
/// result to the interface's own definition. The front end lowers all three
/// spellings into [`Interface::stringifier`] but every generator works from
/// *members*, so the derived operation is materialised here — once, ahead of
/// every generator and both engines, rather than three times over. The body is
/// the host's: only the interface knows whether `toString` forwards to an
/// attribute's getter or serializes its own state.
///
/// A member the interface declares itself wins, as WebIDL requires, so an
/// explicit `toString` suppresses the derived one.
fn add_stringifier_member(interface: &mut Interface) {
    if interface.stringifier.is_none() {
        return;
    }
    let declared = interface
        .members
        .iter()
        .any(|member| matches!(member, Member::Operation(group) if group.name == "toString"));
    if declared {
        return;
    }
    let group = OperationGroup {
        name: "toString".to_string(),
        is_static: false,
        overloads: vec![Operation {
            name: "toString".to_string(),
            rust_name: String::new(),
            arguments: Vec::new(),
            return_type: Type::DomString,
            attrs: ExtendedAttributes::default(),
            special: None,
            stringifier: true,
            fast: FastApi::Eligible,
        }],
        fast: FastApi::Eligible,
        origin: MemberOrigin::Interface,
    };
    push_member(&mut interface.members, Member::Operation(group));
}

fn finish_namespace(namespace: &mut Namespace) {
    finish_members(&mut namespace.members);
}

fn finish_members(members: &mut [Member]) {
    for member in members.iter_mut() {
        let Member::Operation(group) = member else {
            continue;
        };
        let base = naming::snake_case(&group.name);
        let multiple = group.overloads.len() > 1;
        let mut used: Vec<String> = Vec::new();
        for overload in &mut group.overloads {
            overload.fast = classify_operation(overload, group.is_static);
            let mut name = if multiple {
                format!("{base}_{}", overload.arguments.len())
            } else {
                base.clone()
            };
            while used.contains(&name) {
                name.push('_');
            }
            used.push(name.clone());
            overload.rust_name = name;
        }
        group.fast = classify_group(group);
    }
}

fn classify_group(group: &OperationGroup) -> FastApi {
    if group.is_static {
        return FastApi::Ineligible(NotFastBecause::Static);
    }
    // The arity rule comes first: an overload set that V8 could not dispatch
    // on argument count is reported as such even when a single overload would
    // also fail the type rule.
    if group.arity_classes().len() != 1 {
        let reason = if group.overloads.len() > 1 {
            NotFastBecause::Overloaded
        } else {
            NotFastBecause::VariableArity
        };
        return FastApi::Ineligible(reason);
    }
    for overload in &group.overloads {
        if let FastApi::Ineligible(reason) = overload.fast {
            return FastApi::Ineligible(reason);
        }
    }
    FastApi::Eligible
}

fn contains_buffer(ty: &Type) -> bool {
    match ty {
        Type::Buffer(_) => true,
        Type::Sequence(inner)
        | Type::FrozenArray(inner)
        | Type::ObservableArray(inner)
        | Type::Promise(inner)
        | Type::Nullable(inner) => contains_buffer(inner),
        Type::Record(key, value) => contains_buffer(key) || contains_buffer(value),
        _ => false,
    }
}

fn reject_misplaced_buffer_references(model: &Model) -> Result<()> {
    let reject = |attrs: &ExtendedAttributes| -> Result<()> {
        if attrs.has("TentaBufferReferences") {
            Err(Error::Unsupported(
                "[TentaBufferReferences] is valid only on a regular operation".into(),
            ))
        } else {
            Ok(())
        }
    };
    let arguments = |args: &[Argument]| -> Result<()> {
        for argument in args {
            reject(&argument.attrs)?;
        }
        Ok(())
    };
    let members = |members: &[Member]| -> Result<()> {
        for member in members {
            match member {
                Member::Attribute(attribute) => reject(&attribute.attrs)?,
                Member::Operation(group) => {
                    for operation in &group.overloads {
                        arguments(&operation.arguments)?;
                    }
                }
            }
        }
        Ok(())
    };
    for interface in &model.interfaces {
        reject(&interface.attrs)?;
        for constructor in &interface.constructors {
            reject(&constructor.attrs)?;
            arguments(&constructor.arguments)?;
        }
        for constant in &interface.constants {
            reject(&constant.attrs)?;
        }
        members(&interface.members)?;
        for operation in &interface.special_operations {
            arguments(&operation.arguments)?;
        }
    }
    for namespace in &model.namespaces {
        reject(&namespace.attrs)?;
        for constant in &namespace.constants {
            reject(&constant.attrs)?;
        }
        members(&namespace.members)?;
        for member in &namespace.members {
            if let Member::Operation(group) = member {
                for operation in &group.overloads {
                    if operation.buffer_references() {
                        return Err(Error::Unsupported(
                            "[TentaBufferReferences] supports only interface operations in this slice"
                                .into(),
                        ));
                    }
                }
            }
        }
    }
    for dictionary in &model.dictionaries {
        reject(&dictionary.attrs)?;
        for member in &dictionary.members {
            reject(&member.attrs)?;
        }
    }
    for enumeration in &model.enums {
        reject(&enumeration.attrs)?;
    }
    for callback in &model.callbacks {
        reject(&callback.attrs)?;
        arguments(&callback.arguments)?;
    }
    for callback in &model.callback_interfaces {
        reject(&callback.attrs)?;
        reject(&callback.operation.attrs)?;
        arguments(&callback.operation.arguments)?;
        for constant in &callback.constants {
            reject(&constant.attrs)?;
        }
    }
    Ok(())
}

fn validate_buffer_references(operation: &Operation) -> Result<()> {
    if !operation.buffer_references() {
        return Ok(());
    }
    if operation.special.is_some() || operation.stringifier || operation.name.is_empty() {
        return Err(Error::Unsupported(
            "[TentaBufferReferences] requires a regular named operation".into(),
        ));
    }
    let direct = |ty: &Type| {
        matches!(
            ty,
            Type::Buffer(BufferType::BufferSource | BufferType::ArrayBufferView)
        )
    };
    let mut seen = direct(&operation.return_type);
    // A Promise<ArrayBuffer> return remains an opaque Promise token in this
    // slice. No other nested buffer-bearing return is silently lowered.
    if contains_buffer(&operation.return_type) && !seen {
        let supported_promise = matches!(
            &operation.return_type,
            Type::Promise(inner) if matches!(inner.as_ref(), Type::Buffer(BufferType::ArrayBuffer))
        );
        if !supported_promise {
            return Err(Error::Unsupported(
                "[TentaBufferReferences] supports only direct buffer references or Promise<ArrayBuffer> returns".into(),
            ));
        }
    }
    for argument in &operation.arguments {
        if direct(&argument.ty) {
            seen = true;
            if argument.optional || argument.variadic {
                return Err(Error::Unsupported(
                    "[TentaBufferReferences] does not support optional or variadic buffer references".into(),
                ));
            }
        } else if contains_buffer(&argument.ty) {
            return Err(Error::Unsupported(
                "[TentaBufferReferences] supports only direct BufferSource and ArrayBufferView buffer arguments".into(),
            ));
        }
    }
    if !seen {
        return Err(Error::Unsupported(
            "[TentaBufferReferences] requires a direct BufferSource or ArrayBufferView".into(),
        ));
    }
    Ok(())
}

fn classify_operation(operation: &Operation, is_static: bool) -> FastApi {
    if is_static {
        return FastApi::Ineligible(NotFastBecause::Static);
    }
    if operation.special.is_some() {
        return FastApi::Ineligible(NotFastBecause::Special);
    }
    if operation.attrs.has("Throws") {
        return FastApi::Ineligible(NotFastBecause::Throws);
    }
    if !operation.return_type.is_fast_primitive() {
        return FastApi::Ineligible(NotFastBecause::ReturnType);
    }
    let mut strings = 0;
    for argument in &operation.arguments {
        if argument.enforce_range() && is_narrow_integer(&argument.ty) {
            return FastApi::Ineligible(NotFastBecause::NarrowEnforceRange);
        }
        if argument.ty.is_fast_primitive() {
            continue;
        }
        if argument.ty.is_one_byte_string() {
            strings += 1;
            if strings > 1 {
                return FastApi::Ineligible(NotFastBecause::MultipleStrings);
            }
            continue;
        }
        return FastApi::Ineligible(NotFastBecause::ArgumentType);
    }
    if operation.arity_classes().len() != 1 {
        return FastApi::Ineligible(NotFastBecause::VariableArity);
    }
    FastApi::Eligible
}

/// True for the WebIDL integer types narrower than the 32-bit C types V8's
/// fast API can carry.
fn is_narrow_integer(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Byte | Type::Octet | Type::Short | Type::UnsignedShort
    )
}

fn classify_getter(ty: &Type, is_static: bool, throws: bool) -> FastApi {
    if is_static {
        return FastApi::Ineligible(NotFastBecause::Static);
    }
    if throws {
        return FastApi::Ineligible(NotFastBecause::Throws);
    }
    if ty.is_fast_primitive() {
        FastApi::Eligible
    } else {
        FastApi::Ineligible(NotFastBecause::ReturnType)
    }
}

fn classify_setter(ty: &Type, is_static: bool, throws: bool) -> FastApi {
    if is_static {
        return FastApi::Ineligible(NotFastBecause::Static);
    }
    if throws {
        return FastApi::Ineligible(NotFastBecause::Throws);
    }
    if ty.is_fast_primitive() || ty.is_one_byte_string() {
        FastApi::Eligible
    } else {
        FastApi::Ineligible(NotFastBecause::ArgumentType)
    }
}

fn exposure_of(attrs: &ExtendedAttributes) -> Exposure {
    for attribute in &attrs.0 {
        if attribute.name != "Exposed" {
            continue;
        }
        return match &attribute.value {
            ExtendedAttributeValue::Wildcard => Exposure::All,
            ExtendedAttributeValue::Ident(name) => Exposure::Globals(vec![name.clone()]),
            ExtendedAttributeValue::IdentList(names) => Exposure::Globals(names.clone()),
            _ => Exposure::Globals(vec!["Window".to_string()]),
        };
    }
    Exposure::Globals(vec!["Window".to_string()])
}

/// The host method an anonymous special operation becomes. WebIDL lets
/// `getter`, `setter` and `deleter` go unnamed (`getter DOMString (DOMString
/// name);`, as `DOMStringMap` declares them), so there is no identifier to
/// derive a Rust name from: the name says what the operation is instead, and
/// whether it is keyed by index or by name, which its first argument decides.
fn anonymous_special_name(special: Option<Special>, arguments: &[Argument]) -> &'static str {
    let indexed = arguments
        .first()
        .is_some_and(|argument| argument.ty.unwrap_nullable().is_integer());
    match (special, indexed) {
        (Some(Special::Getter), true) => "indexed_getter",
        (Some(Special::Getter), false) => "named_getter",
        (Some(Special::Setter), true) => "indexed_setter",
        (Some(Special::Setter), false) => "named_setter",
        (Some(Special::Deleter), true) => "indexed_deleter",
        (Some(Special::Deleter), false) => "named_deleter",
        (Some(Special::LegacyCaller), _) => "legacy_caller",
        (None, _) => "anonymous_operation",
    }
}

fn lower_special(special: WSpecial) -> Special {
    match special {
        WSpecial::Getter(_) => Special::Getter,
        WSpecial::Setter(_) => Special::Setter,
        WSpecial::Deleter(_) => Special::Deleter,
        WSpecial::LegacyCaller(_) => Special::LegacyCaller,
    }
}

fn lower_integer(literal: &IntegerLit<'_>) -> i64 {
    let parse = |text: &str, radix: u32, skip: usize| -> i64 {
        let (sign, digits) = match text.strip_prefix('-') {
            Some(rest) => (-1i64, rest),
            None => (1i64, text),
        };
        let digits = &digits[skip.min(digits.len())..];
        let value = i64::from_str_radix(digits, radix).unwrap_or(0);
        sign * value
    };
    match literal {
        IntegerLit::Dec(lit) => lit.0.parse().unwrap_or(0),
        IntegerLit::Hex(lit) => parse(lit.0, 16, 2),
        IntegerLit::Oct(lit) => parse(lit.0, 8, 1),
    }
}

fn lower_float(literal: &weedle::literal::FloatLit<'_>) -> f64 {
    use weedle::literal::FloatLit;
    match literal {
        FloatLit::Value(value) => value.0.parse().unwrap_or(f64::NAN),
        FloatLit::NegInfinity(_) => f64::NEG_INFINITY,
        FloatLit::Infinity(_) => f64::INFINITY,
        FloatLit::NaN(_) => f64::NAN,
    }
}

fn lower_const_value(value: &WConstValue<'_>) -> ConstValue {
    match value {
        WConstValue::Boolean(boolean) => ConstValue::Boolean(boolean.0),
        WConstValue::Integer(int) => ConstValue::Integer(lower_integer(int)),
        WConstValue::Float(float) => ConstValue::Float(lower_float(float)),
        WConstValue::Null(_) => ConstValue::Null,
    }
}

fn lower_default(value: &WDefaultValue<'_>) -> DefaultValue {
    match value {
        WDefaultValue::Boolean(boolean) => DefaultValue::Boolean(boolean.0),
        WDefaultValue::Integer(int) => DefaultValue::Integer(lower_integer(int)),
        WDefaultValue::Float(float) => DefaultValue::Float(lower_float(float)),
        WDefaultValue::String(text) => DefaultValue::String(text.0.to_string()),
        WDefaultValue::Null(_) => DefaultValue::Null,
        WDefaultValue::EmptyArray(_) => DefaultValue::EmptySequence,
        WDefaultValue::EmptyDictionary(_) => DefaultValue::EmptyDictionary,
    }
}

fn nullable_if(nullable: bool, ty: Type) -> Type {
    if nullable {
        Type::Nullable(Box::new(ty))
    } else {
        ty
    }
}

fn validate_string_annotation(ty: &Type, attrs: &ExtendedAttributes) -> Result<()> {
    if let Some(attribute) = attrs.0.iter().find(|a| a.name == "LegacyNullToEmptyString")
        && (!matches!(ty, Type::DomString | Type::UsvString)
            || !matches!(attribute.value, ExtendedAttributeValue::None))
    {
        return Err(Error::Model(
            "[LegacyNullToEmptyString] requires a non-nullable DOMString or USVString and takes no arguments".to_owned(),
        ));
    }
    Ok(())
}
