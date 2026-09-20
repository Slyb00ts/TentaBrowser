//! The resolved WebIDL model.
//!
//! Everything the generators need is decided here: typedefs are substituted,
//! partials and mixins are merged, unions are named, overloads are grouped and
//! every member carries its fast-API classification and its extended
//! attributes. The generators are pure functions of this model.

use std::fmt;

use crate::naming;

/// A whole IDL compilation unit: every definition of one or more `.webidl`
/// files, resolved.
#[derive(Debug, Clone, Default)]
pub struct Model {
    /// Regular interfaces, in definition order.
    pub interfaces: Vec<Interface>,
    /// Namespaces, in definition order.
    pub namespaces: Vec<Namespace>,
    /// Dictionaries, in definition order.
    pub dictionaries: Vec<Dictionary>,
    /// Enumerations, in definition order.
    pub enums: Vec<Enum>,
    /// Callback functions, in definition order.
    pub callbacks: Vec<Callback>,
    /// Callback interfaces, in definition order.
    pub callback_interfaces: Vec<CallbackInterface>,
    /// Typedefs with their resolved right-hand side, in definition order.
    pub typedefs: Vec<Typedef>,
    /// Every distinct union type reached from a member, sorted by name.
    pub unions: Vec<Union>,
    /// Interface-typed names referenced but not defined in this unit, sorted.
    pub opaque_interfaces: Vec<String>,
}

impl Model {
    /// Looks an interface up by name.
    pub fn interface(&self, name: &str) -> Option<&Interface> {
        self.interfaces.iter().find(|i| i.name == name)
    }

    /// Looks a namespace up by name.
    pub fn namespace(&self, name: &str) -> Option<&Namespace> {
        self.namespaces.iter().find(|n| n.name == name)
    }

    /// Looks a dictionary up by name.
    pub fn dictionary(&self, name: &str) -> Option<&Dictionary> {
        self.dictionaries.iter().find(|d| d.name == name)
    }

    /// Looks an enumeration up by name.
    pub fn enumeration(&self, name: &str) -> Option<&Enum> {
        self.enums.iter().find(|e| e.name == name)
    }

    /// Looks a callback function up by name.
    pub fn callback(&self, name: &str) -> Option<&Callback> {
        self.callbacks.iter().find(|c| c.name == name)
    }

    /// Looks a callback interface up by name.
    pub fn callback_interface(&self, name: &str) -> Option<&CallbackInterface> {
        self.callback_interfaces.iter().find(|c| c.name == name)
    }

    /// Looks a union up by its generated Rust name.
    pub fn union(&self, name: &str) -> Option<&Union> {
        self.unions.iter().find(|u| u.name == name)
    }

    /// The JavaScript prototype chain of `name`, ending at `Object`.
    ///
    /// `Node` yields `["Node", "EventTarget", "Object"]`. A parent that is not
    /// defined in this unit terminates the chain after its own name.
    pub fn prototype_chain(&self, name: &str) -> Vec<String> {
        let mut chain = vec![name.to_string()];
        let mut seen = vec![name.to_string()];
        let mut current = self.interface(name).and_then(|i| i.parent.clone());
        while let Some(parent) = current {
            if seen.contains(&parent) {
                break;
            }
            seen.push(parent.clone());
            chain.push(parent.clone());
            current = self.interface(&parent).and_then(|i| i.parent.clone());
        }
        chain.push("Object".to_string());
        chain
    }

    /// Dictionary members of `name` and of its ancestors, inherited first.
    pub fn dictionary_members(&self, name: &str) -> Vec<&DictionaryMember> {
        let mut chain = Vec::new();
        let mut current = Some(name.to_string());
        while let Some(next) = current {
            let Some(dict) = self.dictionary(&next) else {
                break;
            };
            if chain.iter().any(|d: &&Dictionary| d.name == dict.name) {
                break;
            }
            chain.push(dict);
            current = dict.parent.clone();
        }
        chain.iter().rev().flat_map(|d| d.members.iter()).collect()
    }
}

/// Where an interface (or its members) may be reached from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exposure {
    /// `[Exposed=*]`: every global.
    All,
    /// `[Exposed=Window]` or `[Exposed=(Window,Worker)]`.
    Globals(Vec<String>),
}

impl Exposure {
    /// The global names, or `["*"]` for `[Exposed=*]`.
    pub fn names(&self) -> Vec<&str> {
        match self {
            Exposure::All => vec!["*"],
            Exposure::Globals(globals) => globals.iter().map(String::as_str).collect(),
        }
    }
}

/// A parsed extended attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtendedAttribute {
    /// The attribute name, e.g. `CEReactions`.
    pub name: String,
    /// The attribute's right-hand side.
    pub value: ExtendedAttributeValue,
}

/// The right-hand side of an extended attribute.
#[derive(Debug, Clone, PartialEq)]
pub enum ExtendedAttributeValue {
    /// `[Replaceable]`
    None,
    /// `[PutForwards=value]`
    Ident(String),
    /// `[LegacyNamespace="foo"]`
    String(String),
    /// `[Exposed=(Window,Worker)]`
    IdentList(Vec<String>),
    /// `[Exposed=*]`
    Wildcard,
    /// `[LegacyFactoryFunction=Name(DOMString a)]` or `[Foo(DOMString a)]`
    Arguments {
        /// The right-hand identifier, if the form is `Name=Rhs(args)`.
        rhs: Option<String>,
        /// The argument list.
        arguments: Vec<Argument>,
    },
}

/// A list of extended attributes with lookup helpers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtendedAttributes(pub Vec<ExtendedAttribute>);

impl ExtendedAttributes {
    /// True when an attribute with this name is present.
    pub fn has(&self, name: &str) -> bool {
        self.0.iter().any(|a| a.name == name)
    }

    /// The value of `[Name=Ident]`, if present.
    pub fn ident(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|a| a.name == name)
            .and_then(|a| match &a.value {
                ExtendedAttributeValue::Ident(value) => Some(value.as_str()),
                ExtendedAttributeValue::String(value) => Some(value.as_str()),
                _ => None,
            })
    }

    /// The attribute names, in source order.
    pub fn names(&self) -> Vec<&str> {
        self.0.iter().map(|a| a.name.as_str()).collect()
    }
}

/// Which declaration contributed a member to an interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberOrigin {
    /// Declared in the interface body.
    Interface,
    /// Declared in a `partial interface` block.
    Partial,
    /// Declared in an `interface mixin` that the interface includes.
    Mixin(String),
}

/// Why a member cannot use the V8 fast-API path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotFastBecause {
    /// Static members are called without a receiver.
    Static,
    /// The return type is not a primitive or `undefined`.
    ReturnType,
    /// An argument is neither a primitive nor a one-byte string.
    ArgumentType,
    /// More than one one-byte-string argument.
    MultipleStrings,
    /// The member accepts more than one argument count.
    VariableArity,
    /// An overload set whose members have different arity classes.
    Overloaded,
    /// A special operation (`getter`, `setter`, `deleter`).
    Special,
    /// An `[EnforceRange]` argument narrower than 32 bits: V8's fast-call
    /// range check works on the C type, so the extra WebIDL narrowing could
    /// not report a `TypeError`.
    NarrowEnforceRange,
    /// A `[Throws]` operation: it must be able to raise a JS exception, which
    /// the fast-call path cannot do, so it takes the fallible regular path.
    Throws,
}

impl fmt::Display for NotFastBecause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            NotFastBecause::Static => "static members have no receiver",
            NotFastBecause::ReturnType => "the return type is not a primitive",
            NotFastBecause::ArgumentType => "an argument is not a primitive or one-byte string",
            NotFastBecause::MultipleStrings => "more than one one-byte-string argument",
            NotFastBecause::VariableArity => "the member accepts more than one argument count",
            NotFastBecause::Overloaded => "an overload set with different arity classes",
            NotFastBecause::Special => "a special operation",
            NotFastBecause::NarrowEnforceRange => {
                "an `[EnforceRange]` argument narrower than 32 bits"
            }
            NotFastBecause::Throws => "a `[Throws]` operation raises exceptions",
        };
        f.write_str(text)
    }
}

/// The fast-API classification of a member (`docs/spec/05-javascript.md` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastApi {
    /// A V8 fast-API callback can be emitted for this member.
    Eligible,
    /// Only the regular `FunctionCallback` path applies.
    Ineligible(NotFastBecause),
}

impl FastApi {
    /// True for [`FastApi::Eligible`].
    pub fn is_eligible(self) -> bool {
        matches!(self, FastApi::Eligible)
    }

    /// The reason a member is not eligible, if it is not.
    pub fn reason(self) -> Option<NotFastBecause> {
        match self {
            FastApi::Eligible => None,
            FastApi::Ineligible(reason) => Some(reason),
        }
    }
}

/// A regular WebIDL interface after partial and mixin merging.
#[derive(Debug, Clone)]
pub struct Interface {
    /// The interface name.
    pub name: String,
    /// The inherited interface, if any.
    pub parent: Option<String>,
    /// The `[Exposed]` set.
    pub exposure: Exposure,
    /// Extended attributes on the interface itself.
    pub attrs: ExtendedAttributes,
    /// Constructor overloads, in definition order.
    pub constructors: Vec<Constructor>,
    /// Constants, in definition order (own, then partials, then mixins).
    pub constants: Vec<Constant>,
    /// Attributes and operations, in the same order.
    pub members: Vec<Member>,
    /// The mixins merged into this interface, in `includes` order.
    pub included_mixins: Vec<String>,
    /// How many `partial interface` blocks were merged.
    pub partial_blocks: usize,
    /// A value or pair iterator declaration.
    pub iterable: Option<Iterable>,
    /// The operation or attribute that serves as the stringifier.
    pub stringifier: Option<Stringifier>,
    /// The operation that acts as the indexed property getter.
    pub indexed_getter: Option<String>,
    /// The operation that acts as the named property getter.
    pub named_getter: Option<String>,
    /// Special operations declared without a name. They are not properties, so
    /// they take no part in the enumeration order.
    pub special_operations: Vec<Operation>,
}

impl Interface {
    /// The non-static attributes and operations, in definition order.
    pub fn instance_members(&self) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(|m| !m.is_static())
    }

    /// The static attributes and operations, in definition order.
    pub fn static_members(&self) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(|m| m.is_static())
    }

    /// Finds a member by its WebIDL name.
    pub fn member(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.name() == name)
    }

    /// The WebIDL property enumeration order of this interface's prototype:
    /// constants first, then attributes and operations in definition order,
    /// then `Symbol.toStringTag`.
    ///
    /// Static members live on the interface object, not the prototype, so they
    /// are not part of this order.
    pub fn property_enumeration_order(&self) -> Vec<PropertyKey> {
        let mut keys: Vec<PropertyKey> = self
            .constants
            .iter()
            .map(|c| PropertyKey::Constant(c.name.clone()))
            .collect();
        for member in self.instance_members() {
            keys.push(match member {
                Member::Attribute(a) => PropertyKey::Attribute(a.name.clone()),
                Member::Operation(o) => PropertyKey::Operation(o.name.clone()),
            });
        }
        keys.push(PropertyKey::ToStringTag);
        keys
    }
}

/// One entry of an interface's property enumeration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyKey {
    /// A constant, installed on both the interface object and the prototype.
    Constant(String),
    /// An accessor property.
    Attribute(String),
    /// A method.
    Operation(String),
    /// The `Symbol.toStringTag` data property.
    ToStringTag,
}

impl PropertyKey {
    /// The property name as JavaScript sees it.
    pub fn js_name(&self) -> &str {
        match self {
            PropertyKey::Constant(name)
            | PropertyKey::Attribute(name)
            | PropertyKey::Operation(name) => name,
            PropertyKey::ToStringTag => "Symbol.toStringTag",
        }
    }
}

/// An attribute or an operation group.
#[derive(Debug, Clone)]
pub enum Member {
    /// An IDL attribute.
    Attribute(Attribute),
    /// One or more IDL operations that share a name.
    Operation(OperationGroup),
}

impl Member {
    /// The WebIDL name of the member.
    pub fn name(&self) -> &str {
        match self {
            Member::Attribute(a) => &a.name,
            Member::Operation(o) => &o.name,
        }
    }

    /// True for `static` members.
    pub fn is_static(&self) -> bool {
        match self {
            Member::Attribute(a) => a.is_static,
            Member::Operation(o) => o.is_static,
        }
    }

    /// Where the member came from.
    pub fn origin(&self) -> &MemberOrigin {
        match self {
            Member::Attribute(a) => &a.origin,
            Member::Operation(o) => &o.origin,
        }
    }

    /// The member's extended attributes. For an operation group these are the
    /// attributes of the first overload.
    pub fn attrs(&self) -> &ExtendedAttributes {
        match self {
            Member::Attribute(a) => &a.attrs,
            Member::Operation(o) => &o.overloads[0].attrs,
        }
    }
}

/// An IDL attribute.
#[derive(Debug, Clone)]
pub struct Attribute {
    /// The WebIDL name.
    pub name: String,
    /// The Rust getter name.
    pub getter_name: String,
    /// The Rust setter name, absent for `readonly` attributes.
    pub setter_name: Option<String>,
    /// The attribute type.
    pub ty: Type,
    /// True for `readonly attribute`.
    pub readonly: bool,
    /// True for `static attribute`.
    pub is_static: bool,
    /// True for `inherit attribute`.
    pub inherit: bool,
    /// True for `stringifier attribute`.
    pub stringifier: bool,
    /// Extended attributes on the member.
    pub attrs: ExtendedAttributes,
    /// Fast-API classification of the getter.
    pub fast_getter: FastApi,
    /// Fast-API classification of the setter, `None` when readonly.
    pub fast_setter: Option<FastApi>,
    /// Which declaration contributed this attribute.
    pub origin: MemberOrigin,
}

impl Attribute {
    /// True for `[SameObject]`.
    pub fn same_object(&self) -> bool {
        self.attrs.has("SameObject")
    }

    /// True for `[Replaceable]`.
    pub fn replaceable(&self) -> bool {
        self.attrs.has("Replaceable")
    }

    /// True for `[LegacyUnforgeable]`.
    pub fn legacy_unforgeable(&self) -> bool {
        self.attrs.has("LegacyUnforgeable")
    }

    /// The target of `[PutForwards=x]`.
    pub fn put_forwards(&self) -> Option<&str> {
        self.attrs.ident("PutForwards")
    }

    /// True for `[Unscopable]`.
    pub fn unscopable(&self) -> bool {
        self.attrs.has("Unscopable")
    }
}

/// One or more operations sharing a name.
#[derive(Debug, Clone)]
pub struct OperationGroup {
    /// The WebIDL name.
    pub name: String,
    /// True when the operations are `static`.
    pub is_static: bool,
    /// The overloads, in definition order.
    pub overloads: Vec<Operation>,
    /// Fast-API classification of the group as a whole.
    pub fast: FastApi,
    /// Which declaration contributed this group.
    pub origin: MemberOrigin,
}

impl OperationGroup {
    /// The set of argument counts the group accepts, sorted.
    ///
    /// A variadic overload contributes [`usize::MAX`] as its open upper bound.
    pub fn arity_classes(&self) -> Vec<usize> {
        let mut classes: Vec<usize> = self
            .overloads
            .iter()
            .flat_map(|o| o.arity_classes())
            .collect();
        classes.sort_unstable();
        classes.dedup();
        classes
    }

    /// True for `[Unscopable]` on the first overload.
    pub fn unscopable(&self) -> bool {
        self.overloads[0].attrs.has("Unscopable")
    }
}

/// A single operation.
#[derive(Debug, Clone)]
pub struct Operation {
    /// The WebIDL name; empty for an anonymous special operation.
    pub name: String,
    /// The Rust method name; overloads get an arity suffix.
    pub rust_name: String,
    /// The arguments, in order.
    pub arguments: Vec<Argument>,
    /// The return type. `undefined` is [`Type::Undefined`].
    pub return_type: Type,
    /// Extended attributes on the operation.
    pub attrs: ExtendedAttributes,
    /// `getter`, `setter` or `deleter`.
    pub special: Option<Special>,
    /// True for `stringifier <type> name()`.
    pub stringifier: bool,
    /// Fast-API classification of this overload alone.
    pub fast: FastApi,
}

impl Operation {
    /// The number of arguments that must be supplied.
    pub fn required_argument_count(&self) -> usize {
        self.arguments
            .iter()
            .filter(|a| !a.optional && !a.variadic)
            .count()
    }

    /// The argument counts this overload accepts, in ascending order.
    pub fn arity_classes(&self) -> Vec<usize> {
        let required = self.required_argument_count();
        if self.arguments.iter().any(|a| a.variadic) {
            return vec![required, usize::MAX];
        }
        (required..=self.arguments.len()).collect()
    }
}

/// A WebIDL special operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Special {
    /// `getter`
    Getter,
    /// `setter`
    Setter,
    /// `deleter`
    Deleter,
    /// `legacycaller`
    LegacyCaller,
}

/// A constructor overload.
#[derive(Debug, Clone)]
pub struct Constructor {
    /// The arguments, in order.
    pub arguments: Vec<Argument>,
    /// Extended attributes on the `constructor` member.
    pub attrs: ExtendedAttributes,
    /// The Rust constructor name; overloads get an arity suffix.
    pub rust_name: String,
}

/// An operation or constructor argument.
#[derive(Debug, Clone, PartialEq)]
pub struct Argument {
    /// The WebIDL name.
    pub name: String,
    /// The Rust binding name.
    pub rust_name: String,
    /// The argument type.
    pub ty: Type,
    /// True for `optional`.
    pub optional: bool,
    /// True for `T... name`.
    pub variadic: bool,
    /// The declared default, if any.
    pub default: Option<DefaultValue>,
    /// Extended attributes on the argument.
    pub attrs: ExtendedAttributes,
}

impl Argument {
    /// True for `[Clamp]`.
    pub fn clamp(&self) -> bool {
        self.attrs.has("Clamp")
    }

    /// True for `[EnforceRange]`.
    pub fn enforce_range(&self) -> bool {
        self.attrs.has("EnforceRange")
    }
}

/// A constant member.
#[derive(Debug, Clone)]
pub struct Constant {
    /// The WebIDL name.
    pub name: String,
    /// The Rust associated-constant name.
    pub rust_name: String,
    /// The constant's type.
    pub ty: Type,
    /// The constant's value.
    pub value: ConstValue,
    /// Extended attributes on the constant.
    pub attrs: ExtendedAttributes,
    /// Which declaration contributed this constant.
    pub origin: MemberOrigin,
}

/// The value of a constant.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    /// `true` or `false`.
    Boolean(bool),
    /// An integer literal, decimal, hexadecimal or octal.
    Integer(i64),
    /// A floating-point literal.
    Float(f64),
    /// `null`.
    Null,
}

/// The default value of an optional argument or dictionary member.
#[derive(Debug, Clone, PartialEq)]
pub enum DefaultValue {
    /// `true` or `false`.
    Boolean(bool),
    /// An integer literal.
    Integer(i64),
    /// A floating-point literal.
    Float(f64),
    /// A string literal, also used for enumeration defaults.
    String(String),
    /// `null`.
    Null,
    /// `[]`.
    EmptySequence,
    /// `{}`.
    EmptyDictionary,
}

/// A value or pair iterator declaration.
#[derive(Debug, Clone)]
pub struct Iterable {
    /// The key type of `iterable<K, V>`, absent for `iterable<V>`.
    pub key: Option<Type>,
    /// The value type.
    pub value: Type,
    /// True for `async iterable`.
    pub is_async: bool,
}

/// How an interface stringifies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stringifier {
    /// `stringifier <type> name();`
    Operation(String),
    /// `stringifier attribute <type> name;`
    Attribute(String),
    /// A bare `stringifier;`
    Bare,
}

/// A `namespace` definition after partial merging.
#[derive(Debug, Clone)]
pub struct Namespace {
    /// The namespace name.
    pub name: String,
    /// The `[Exposed]` set.
    pub exposure: Exposure,
    /// Extended attributes on the namespace.
    pub attrs: ExtendedAttributes,
    /// Constants, in definition order.
    pub constants: Vec<Constant>,
    /// Readonly attributes and operations, in definition order.
    pub members: Vec<Member>,
    /// How many `partial namespace` blocks were merged.
    pub partial_blocks: usize,
}

impl Namespace {
    /// Constants, then attributes and operations, then `Symbol.toStringTag`.
    pub fn property_enumeration_order(&self) -> Vec<PropertyKey> {
        let mut keys: Vec<PropertyKey> = self
            .constants
            .iter()
            .map(|c| PropertyKey::Constant(c.name.clone()))
            .collect();
        for member in &self.members {
            keys.push(match member {
                Member::Attribute(a) => PropertyKey::Attribute(a.name.clone()),
                Member::Operation(o) => PropertyKey::Operation(o.name.clone()),
            });
        }
        keys.push(PropertyKey::ToStringTag);
        keys
    }
}

/// A dictionary definition after partial merging.
#[derive(Debug, Clone)]
pub struct Dictionary {
    /// The dictionary name.
    pub name: String,
    /// The inherited dictionary, if any.
    pub parent: Option<String>,
    /// Members, in definition order.
    pub members: Vec<DictionaryMember>,
    /// Extended attributes on the dictionary.
    pub attrs: ExtendedAttributes,
    /// How many `partial dictionary` blocks were merged.
    pub partial_blocks: usize,
}

/// A dictionary member.
#[derive(Debug, Clone)]
pub struct DictionaryMember {
    /// The WebIDL name.
    pub name: String,
    /// The Rust field name.
    pub rust_name: String,
    /// The member type.
    pub ty: Type,
    /// True for `required`.
    pub required: bool,
    /// The declared default, if any.
    pub default: Option<DefaultValue>,
    /// Extended attributes on the member.
    pub attrs: ExtendedAttributes,
}

/// An enumeration definition.
#[derive(Debug, Clone)]
pub struct Enum {
    /// The enumeration name.
    pub name: String,
    /// The values, in definition order.
    pub values: Vec<EnumValue>,
    /// Extended attributes on the enumeration.
    pub attrs: ExtendedAttributes,
}

/// One enumeration value.
#[derive(Debug, Clone)]
pub struct EnumValue {
    /// The string as it appears in JavaScript.
    pub value: String,
    /// The Rust variant name.
    pub rust_name: String,
}

/// A callback function definition.
#[derive(Debug, Clone)]
pub struct Callback {
    /// The callback name.
    pub name: String,
    /// The return type.
    pub return_type: Type,
    /// The arguments.
    pub arguments: Vec<Argument>,
    /// Extended attributes on the callback.
    pub attrs: ExtendedAttributes,
}

/// A callback interface definition.
#[derive(Debug, Clone)]
pub struct CallbackInterface {
    /// The callback interface name.
    pub name: String,
    /// The single operation the interface declares.
    pub operation: Operation,
    /// The operation's WebIDL name.
    pub operation_name: String,
    /// Constants declared on the callback interface.
    pub constants: Vec<Constant>,
    /// Extended attributes on the callback interface.
    pub attrs: ExtendedAttributes,
}

/// A `typedef` and its resolved right-hand side.
#[derive(Debug, Clone)]
pub struct Typedef {
    /// The typedef name.
    pub name: String,
    /// The type it resolves to, with nested typedefs already substituted.
    pub ty: Type,
}

/// A distinct union type reached from some member.
#[derive(Debug, Clone)]
pub struct Union {
    /// The generated Rust enum name, e.g. `NodeOrDomString`.
    pub name: String,
    /// The flattened member types, in declaration order.
    pub members: Vec<UnionMember>,
}

/// One arm of a union.
#[derive(Debug, Clone)]
pub struct UnionMember {
    /// The Rust variant name.
    pub rust_name: String,
    /// The arm's type.
    pub ty: Type,
}

/// A resolved WebIDL type.
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    /// `undefined`
    Undefined,
    /// `any`
    Any,
    /// `boolean`
    Boolean,
    /// `byte`
    Byte,
    /// `octet`
    Octet,
    /// `short`
    Short,
    /// `unsigned short`
    UnsignedShort,
    /// `long`
    Long,
    /// `unsigned long`
    UnsignedLong,
    /// `long long`
    LongLong,
    /// `unsigned long long`
    UnsignedLongLong,
    /// `float` or `unrestricted float`
    Float {
        /// True for `unrestricted float`.
        unrestricted: bool,
    },
    /// `double` or `unrestricted double`
    Double {
        /// True for `unrestricted double`.
        unrestricted: bool,
    },
    /// `DOMString`
    DomString,
    /// `ByteString`
    ByteString,
    /// `USVString`
    UsvString,
    /// `object`
    Object,
    /// `symbol`
    Symbol,
    /// An interface type, defined here or opaque.
    Interface(String),
    /// A buffer source: `ArrayBuffer`, one of the typed arrays, `DataView`, or
    /// one of the two typedefs that name a set of them.
    Buffer(BufferType),
    /// A dictionary type.
    Dictionary(String),
    /// An enumeration type.
    Enum(String),
    /// A callback function type.
    Callback(String),
    /// A callback interface type.
    CallbackInterface(String),
    /// `sequence<T>`
    Sequence(Box<Type>),
    /// `FrozenArray<T>`
    FrozenArray(Box<Type>),
    /// `ObservableArray<T>`
    ObservableArray(Box<Type>),
    /// `record<K, V>`
    Record(Box<Type>, Box<Type>),
    /// `Promise<T>`
    Promise(Box<Type>),
    /// A union, referenced by its generated name.
    Union(String),
    /// `T?`
    Nullable(Box<Type>),
}

impl Type {
    /// True for the numeric, boolean and `undefined` types that a V8 fast-API
    /// call can carry.
    pub fn is_fast_primitive(&self) -> bool {
        matches!(
            self,
            Type::Undefined
                | Type::Boolean
                | Type::Byte
                | Type::Octet
                | Type::Short
                | Type::UnsignedShort
                | Type::Long
                | Type::UnsignedLong
                | Type::LongLong
                | Type::UnsignedLongLong
                | Type::Float { .. }
                | Type::Double { .. }
        )
    }

    /// True for the string types V8 can pass as a `FastOneByteString`.
    pub fn is_one_byte_string(&self) -> bool {
        matches!(self, Type::DomString | Type::ByteString | Type::UsvString)
    }

    /// True for the WebIDL integer types.
    pub fn is_integer(&self) -> bool {
        matches!(
            self,
            Type::Byte
                | Type::Octet
                | Type::Short
                | Type::UnsignedShort
                | Type::Long
                | Type::UnsignedLong
                | Type::LongLong
                | Type::UnsignedLongLong
        )
    }

    /// The type inside a `T?`, or the type itself.
    pub fn unwrap_nullable(&self) -> &Type {
        match self {
            Type::Nullable(inner) => inner,
            other => other,
        }
    }

    /// A stable, human-readable spelling used for union naming and diagnostics.
    pub fn idl_name(&self) -> String {
        match self {
            Type::Undefined => "undefined".into(),
            Type::Any => "any".into(),
            Type::Boolean => "boolean".into(),
            Type::Byte => "byte".into(),
            Type::Octet => "octet".into(),
            Type::Short => "short".into(),
            Type::UnsignedShort => "unsigned short".into(),
            Type::Long => "long".into(),
            Type::UnsignedLong => "unsigned long".into(),
            Type::LongLong => "long long".into(),
            Type::UnsignedLongLong => "unsigned long long".into(),
            Type::Float { unrestricted } => {
                if *unrestricted {
                    "unrestricted float".into()
                } else {
                    "float".into()
                }
            }
            Type::Double { unrestricted } => {
                if *unrestricted {
                    "unrestricted double".into()
                } else {
                    "double".into()
                }
            }
            Type::DomString => "DOMString".into(),
            Type::ByteString => "ByteString".into(),
            Type::UsvString => "USVString".into(),
            Type::Object => "object".into(),
            Type::Symbol => "symbol".into(),
            Type::Buffer(kind) => kind.idl_name().to_string(),
            Type::Interface(name)
            | Type::Dictionary(name)
            | Type::Enum(name)
            | Type::Callback(name)
            | Type::CallbackInterface(name)
            | Type::Union(name) => name.clone(),
            Type::Sequence(inner) => format!("sequence<{}>", inner.idl_name()),
            Type::FrozenArray(inner) => format!("FrozenArray<{}>", inner.idl_name()),
            Type::ObservableArray(inner) => format!("ObservableArray<{}>", inner.idl_name()),
            Type::Record(key, value) => {
                format!("record<{}, {}>", key.idl_name(), value.idl_name())
            }
            Type::Promise(inner) => format!("Promise<{}>", inner.idl_name()),
            Type::Nullable(inner) => format!("{}?", inner.idl_name()),
        }
    }

    /// The Rust identifier fragment used when naming a union arm or a union.
    pub fn variant_name(&self) -> String {
        match self {
            Type::Interface(name)
            | Type::Dictionary(name)
            | Type::Enum(name)
            | Type::Callback(name)
            | Type::CallbackInterface(name)
            | Type::Union(name) => name.clone(),
            Type::Sequence(inner) => format!("SequenceOf{}", inner.variant_name()),
            Type::FrozenArray(inner) => format!("FrozenArrayOf{}", inner.variant_name()),
            Type::ObservableArray(inner) => format!("ObservableArrayOf{}", inner.variant_name()),
            Type::Record(key, value) => {
                format!("RecordOf{}To{}", key.variant_name(), value.variant_name())
            }
            Type::Promise(inner) => format!("PromiseOf{}", inner.variant_name()),
            Type::Nullable(inner) => format!("Nullable{}", inner.variant_name()),
            other => naming::pascal_case(&other.idl_name()),
        }
    }
}

/// A WebIDL buffer source type.
///
/// The thirteen spellings that weedle lowers through a keyword arm —
/// `ArrayBuffer`, `DataView`, the nine typed arrays, and the `ArrayBufferView`
/// and `BufferSource` typedefs — are one thing to this generator: a byte
/// container whose conversion is the byte sequence it holds. They therefore
/// share the single [`Type::Buffer`] variant parameterised by this enum instead
/// of taking a variant each, which would multiply the conversion table by
/// thirteen without changing a line of its logic.
///
/// `BigInt64Array`, `BigUint64Array` and `Float16Array` have no member here
/// because `weedle4` 0.4.0 — the `weedle` this repository depends on — has no
/// keyword arm for them: they parse as plain identifiers and stay opaque
/// interfaces (`Handle<BigInt64Array>`), a pre-existing gap this slice neither
/// introduces nor closes. The runtime conversions do accept them, since all
/// three are `ArrayBufferView`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferType {
    /// `ArrayBuffer`
    ArrayBuffer,
    /// `DataView`
    DataView,
    /// `Int8Array`
    Int8Array,
    /// `Int16Array`
    Int16Array,
    /// `Int32Array`
    Int32Array,
    /// `Uint8Array`
    Uint8Array,
    /// `Uint16Array`
    Uint16Array,
    /// `Uint32Array`
    Uint32Array,
    /// `Uint8ClampedArray`
    Uint8ClampedArray,
    /// `Float32Array`
    Float32Array,
    /// `Float64Array`
    Float64Array,
    /// `ArrayBufferView`, the typedef naming the ten view types
    ArrayBufferView,
    /// `BufferSource`, the typedef naming an `ArrayBuffer` or any view
    BufferSource,
}

impl BufferType {
    /// Every member, in WebIDL's declaration order: the `ArrayBuffer`, then the
    /// view types, then the two typedefs that name a set of them.
    pub const ALL: [BufferType; 13] = [
        BufferType::ArrayBuffer,
        BufferType::DataView,
        BufferType::Int8Array,
        BufferType::Int16Array,
        BufferType::Int32Array,
        BufferType::Uint8Array,
        BufferType::Uint16Array,
        BufferType::Uint32Array,
        BufferType::Uint8ClampedArray,
        BufferType::Float32Array,
        BufferType::Float64Array,
        BufferType::ArrayBufferView,
        BufferType::BufferSource,
    ];

    /// The WebIDL spelling, which is also the name the type is diagnosed under.
    pub fn idl_name(&self) -> &'static str {
        match self {
            BufferType::ArrayBuffer => "ArrayBuffer",
            BufferType::DataView => "DataView",
            BufferType::Int8Array => "Int8Array",
            BufferType::Int16Array => "Int16Array",
            BufferType::Int32Array => "Int32Array",
            BufferType::Uint8Array => "Uint8Array",
            BufferType::Uint16Array => "Uint16Array",
            BufferType::Uint32Array => "Uint32Array",
            BufferType::Uint8ClampedArray => "Uint8ClampedArray",
            BufferType::Float32Array => "Float32Array",
            BufferType::Float64Array => "Float64Array",
            BufferType::ArrayBufferView => "ArrayBufferView",
            BufferType::BufferSource => "BufferSource",
        }
    }

    /// The name fragment of the conversions emitted for this type, so that
    /// `Uint8Array` reads `uint8_array_from_v8` / `uint8_array_to_v8`.
    ///
    /// The prelude names its helpers from this, and `codegen::convert` builds
    /// the calls from it, so a call and its definition cannot drift apart.
    pub fn conversion_suffix(&self) -> &'static str {
        match self {
            BufferType::ArrayBuffer => "array_buffer",
            BufferType::DataView => "data_view",
            BufferType::Int8Array => "int8_array",
            BufferType::Int16Array => "int16_array",
            BufferType::Int32Array => "int32_array",
            BufferType::Uint8Array => "uint8_array",
            BufferType::Uint16Array => "uint16_array",
            BufferType::Uint32Array => "uint32_array",
            BufferType::Uint8ClampedArray => "uint8_clamped_array",
            BufferType::Float32Array => "float32_array",
            BufferType::Float64Array => "float64_array",
            BufferType::ArrayBufferView => "array_buffer_view",
            BufferType::BufferSource => "buffer_source",
        }
    }
}
