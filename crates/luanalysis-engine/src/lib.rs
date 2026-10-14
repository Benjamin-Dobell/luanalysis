//! Parser-independent access to the pinned Bun TypeScript relation engine.
//!
//! This facade owns resolved types. It does not yet supply Lua lazy expression
//! queries, contextual inference, or control-flow analysis to the checker.

#![feature(linkage)]

mod native;

use bun_sema::check::{Checker, Program};
use bun_sema::hir;
use bun_sema::program::Files;
use bun_sema::resolve::{Host, Options, ParseOptions, ScriptKind};
use bun_sema::session::Session;
use bun_sema::types::{
    Intrinsic, Literalness, MapperId, Prop, PropFlags, PropSource, Shape, SigData, SigParam,
    TypeData, TypeId,
};
use std::borrow::Cow;

pub const BUN_REVISION: &str = "408c9374b266b45fe6b8260e09be73aa82a957cd";

/// Resolved type descriptions; every comparison creates real checker-owned IDs.
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Any,
    Unknown,
    Never,
    Nil,
    Undefined,
    Void,
    Boolean,
    Number,
    String,
    BooleanLiteral(bool),
    NumberLiteral(f64),
    StringLiteral(Vec<u8>),
    Union(Vec<Type>),
    Intersection(Vec<Type>),
    Shape(Vec<Property>),
    FreshShape(Vec<Property>),
    Function(FunctionType),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    pub ty: Type,
    pub optional: bool,
    pub readonly: bool,
}

impl Property {
    pub fn new(name: impl Into<String>, ty: Type) -> Self {
        Self {
            name: name.into(),
            ty,
            optional: false,
            readonly: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionType {
    pub params: Vec<Parameter>,
    pub returns: Box<Type>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub ty: Type,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub assignable: bool,
    /// Checker-generated type text, in its native TypeScript vocabulary.
    pub source: String,
    pub target: String,
}

#[derive(Default)]
pub struct Engine;

impl Engine {
    pub fn new() -> Self {
        Self
    }

    pub fn is_assignable(&self, source: &Type, target: &Type) -> bool {
        self.compare(source, target).assignable
    }

    pub fn compare(&self, source: &Type, target: &Type) -> Relation {
        with_checker(|checker| {
            let source = import_type(checker, source);
            let target = import_type(checker, target);
            Relation {
                assignable: checker.is_assignable(source, target),
                source: String::from_utf8_lossy(&checker.type_to_string(source)).into_owned(),
                target: String::from_utf8_lossy(&checker.type_to_string(target)).into_owned(),
            }
        })
    }

    /// Applies TypeScript's literal widening to a mutable initialized location.
    pub fn widen(&self, ty: &Type) -> Type {
        with_checker(|checker| {
            let ty = import_type(checker, ty);
            let ty = checker.widen_literal(ty);
            export_type(checker, ty)
        })
    }

    pub fn property_type(&self, ty: &Type, name: &str) -> Option<Type> {
        with_checker(|checker| {
            let ty = import_type(checker, ty);
            let name = checker.atoms().intern(name.as_bytes());
            checker
                .type_of_property(ty, name)
                .map(|ty| export_type(checker, ty))
        })
    }

    /// The native checker's assignment transfer for an explicitly declared
    /// location. Its flow caller retains nonunions and reduces union members.
    pub fn assignment_value(&self, declared: &Type, assigned: &Type) -> Type {
        with_checker(|checker| {
            let declared = import_type(checker, declared);
            let assigned = import_type(checker, assigned);
            let value = if checker.is_union(declared) {
                checker.assignment_reduced_type(declared, assigned)
            } else {
                declared
            };
            export_type(checker, value)
        })
    }

    pub fn format(&self, ty: &Type) -> String {
        with_checker(|checker| {
            let ty = import_type(checker, ty);
            String::from_utf8_lossy(&checker.type_to_string(ty)).into_owned()
        })
    }
}

// The native checker measures its remaining stack; use the same stack as its
// upstream standalone worker rather than guessing the caller's thread stack.
fn with_checker<R: Send>(work: impl FnOnce(&mut Checker<'_, '_>) -> R + Send) -> R {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("luanalysis-type-core".into())
            .stack_size(bun_threading::thread_pool::DEFAULT_THREAD_STACK_SIZE as usize)
            .spawn_scoped(scope, || {
                native::set_stack_size(
                    bun_threading::thread_pool::DEFAULT_THREAD_STACK_SIZE as usize - (256 << 10),
                );
                let session = Session::new();
                let options = Options {
                    no_lib: true,
                    strict_null_checks: true,
                    strict_function_types: true,
                    strict_bind_call_apply: true,
                    no_implicit_any: true,
                    ..Options::default()
                };
                let files = session.arena().alloc(Files::load(
                    &session,
                    None,
                    &TypeCoreHost,
                    options,
                    &[],
                    &|_| {},
                ));
                let mut program = Program::new(&session, files);
                let result = {
                    let mut checker = program.checker();
                    work(&mut checker)
                };
                program.release();
                result
            })
            .expect("create native type-core worker")
            .join()
            .unwrap_or_else(|error| std::panic::resume_unwind(error))
    })
}

fn import_type(checker: &mut Checker<'_, '_>, ty: &Type) -> TypeId {
    match ty {
        Type::Any => TypeId::ANY,
        Type::Unknown => TypeId::UNKNOWN,
        Type::Never => TypeId::NEVER,
        // Lua's nil uses the checker's missing/optional value domain. There is
        // no separate Lua null value. Full truthiness still needs a Lua policy.
        Type::Nil => TypeId::UNDEFINED,
        Type::Undefined => TypeId::UNDEFINED,
        Type::Void => TypeId::VOID,
        Type::Boolean => TypeId::BOOLEAN,
        Type::Number => TypeId::NUMBER,
        Type::String => TypeId::STRING,
        Type::BooleanLiteral(value) => checker.bool_literal(*value, true),
        Type::NumberLiteral(value) => checker.number_literal(*value, true),
        Type::StringLiteral(value) => checker.string_literal(checker.atoms().intern(value), true),
        Type::Union(types) => {
            let types: Vec<_> = types.iter().map(|ty| import_type(checker, ty)).collect();
            checker.union(&types)
        }
        Type::Intersection(types) => {
            let types: Vec<_> = types.iter().map(|ty| import_type(checker, ty)).collect();
            checker.intersection(&types)
        }
        Type::Shape(properties) | Type::FreshShape(properties) => {
            let mut shape = Shape::new_in(checker.arena);
            if matches!(ty, Type::FreshShape(_)) {
                shape.literal = Literalness::Literal;
            }
            for property in properties {
                let mut flags = PropFlags::empty();
                flags.set(PropFlags::OPTIONAL, property.optional);
                flags.set(PropFlags::READONLY, property.readonly);
                flags.set(PropFlags::WRITTEN, matches!(ty, Type::FreshShape(_)));
                let mut ty = import_type(checker, &property.ty);
                if property.optional {
                    ty = checker.optional(ty);
                }
                shape.props.push(Prop {
                    name: checker.atoms().intern(property.name.as_bytes()),
                    flags,
                    source: PropSource::Type(ty),
                    mapper: MapperId::IDENTITY,
                    name_type: TypeId::UNRESOLVED,
                });
            }
            checker.synth(shape)
        }
        Type::Function(function) => {
            let mut params = Vec::new();
            for param in &function.params {
                params.push(SigParam {
                    name: checker.atoms().intern(param.name.as_bytes()),
                    ty: import_type(checker, &param.ty),
                    optional: param.optional,
                    rest: false,
                    is_required_rest: false,
                    declaration: None,
                });
            }
            let ret = import_type(checker, &function.returns);
            let sig = checker.types().intern_sig(SigData::Synth {
                type_params: checker.list(&[]),
                params: checker.list(&params),
                ret,
                this: None,
                of: checker.list(&[]),
                is_union: false,
            });
            let mut shape = Shape::new_in(checker.arena);
            shape.call.push(sig);
            checker.synth(shape)
        }
    }
}

fn export_type(checker: &mut Checker<'_, '_>, ty: TypeId) -> Type {
    if ty == TypeId::BOOLEAN {
        return Type::Boolean;
    }
    match checker.data(ty) {
        TypeData::Intrinsic(kind) => match kind {
            Intrinsic::Any => Type::Any,
            Intrinsic::Unknown => Type::Unknown,
            Intrinsic::Never => Type::Never,
            Intrinsic::Null | Intrinsic::NullWidening => Type::Nil,
            Intrinsic::Undefined | Intrinsic::UndefinedWidening | Intrinsic::Missing => Type::Nil,
            Intrinsic::Void => Type::Void,
            Intrinsic::String => Type::String,
            Intrinsic::Number => Type::Number,
            other => panic!("unsupported exported intrinsic: {other:?}"),
        },
        TypeData::BoolLit { value, .. } => Type::BooleanLiteral(*value),
        TypeData::NumberLit { bits, .. } => Type::NumberLiteral(f64::from_bits(*bits)),
        TypeData::StringLit { value, .. } => {
            Type::StringLiteral(checker.atoms().bytes(*value).to_vec())
        }
        TypeData::Union(types) => {
            let types = types.to_vec();
            Type::Union(
                types
                    .into_iter()
                    .map(|ty| export_type(checker, ty))
                    .collect(),
            )
        }
        TypeData::Intersection(types) => {
            let types = types.to_vec();
            Type::Intersection(
                types
                    .into_iter()
                    .map(|ty| export_type(checker, ty))
                    .collect(),
            )
        }
        TypeData::Synth(shape) if shape.call.len() == 1 && shape.props.is_empty() => {
            let sig = shape.call[0];
            let params = checker.sig_params(sig).into_vec();
            let returns = checker.sig_return(sig);
            Type::Function(FunctionType {
                params: params
                    .into_iter()
                    .map(|param| Parameter {
                        name: String::from_utf8_lossy(checker.atoms().bytes(param.name))
                            .into_owned(),
                        ty: export_type(checker, param.ty),
                        optional: param.optional,
                    })
                    .collect(),
                returns: Box::new(export_type(checker, returns)),
            })
        }
        TypeData::Synth(shape) => {
            let properties: Vec<_> = shape
                .props
                .iter()
                .map(|property| {
                    let ty = match property.source {
                        PropSource::Type(ty) => ty,
                        _ => panic!("unsupported lazy exported property"),
                    };
                    (
                        String::from_utf8_lossy(checker.atoms().bytes(property.name)).into_owned(),
                        ty,
                        property.flags.contains(PropFlags::OPTIONAL),
                        property.flags.contains(PropFlags::READONLY),
                    )
                })
                .collect();
            Type::Shape(
                properties
                    .into_iter()
                    .map(|(name, ty, optional, readonly)| Property {
                        name,
                        ty: export_type(checker, ty),
                        optional,
                        readonly,
                    })
                    .collect(),
            )
        }
        other => panic!("unsupported exported type: {other:?}"),
    }
}

/// No source files are loaded for resolved type operations. In particular, Lua
/// source is never passed to a TS parser, binder, or TS HIR builder.
struct TypeCoreHost;

impl Host for TypeCoreHost {
    fn read(&self, _: &[u8]) -> Option<Cow<'static, [u8]>> {
        None
    }
    fn is_file(&self, _: &[u8]) -> bool {
        false
    }
    fn is_dir(&self, _: &[u8]) -> bool {
        false
    }
    fn realpath(&self, path: &[u8]) -> Vec<u8> {
        path.to_vec()
    }
    fn list_dir(&self, _: &[u8]) -> Vec<Vec<u8>> {
        Vec::new()
    }
    fn script_kind(&self, _: &[u8]) -> Option<ScriptKind> {
        None
    }
    fn extra_file_extensions(&self) -> &[(Vec<u8>, ScriptKind)] {
        &[]
    }
    fn scripts_of_page(&self, _: &[u8]) -> Vec<Vec<u8>> {
        Vec::new()
    }
    fn parse<'s>(
        &self,
        _: &'s bun_sema::session::Arena,
        _: &[u8],
        _: &[u8],
        _: &bun_sema::atom::Interner<'s>,
        _: ParseOptions,
    ) -> hir::File<'s> {
        unreachable!("resolved-type host does not parse files")
    }
    fn parse_package_json(
        &self,
        _: &bun_sema::session::Arena,
        _: &[u8],
    ) -> Option<bun_sema::json::Json> {
        None
    }
    fn parallel(&self, count: usize, work: &(dyn Fn(usize) + Sync)) {
        self.on_this_thread(count, work);
    }
    fn is_of_one_thread(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_primitive_union_and_literal_relations() {
        let engine = Engine::new();
        assert!(engine.is_assignable(&Type::NumberLiteral(42.0), &Type::Number));
        assert!(!engine.is_assignable(&Type::String, &Type::Number));
        assert!(engine.is_assignable(&Type::Nil, &Type::Union(vec![Type::String, Type::Nil])));
        assert!(!engine.is_assignable(&Type::Nil, &Type::String));
        assert_eq!(
            engine.widen(&Type::StringLiteral(b"hello".to_vec())),
            Type::String
        );
        assert_eq!(engine.widen(&Type::NumberLiteral(42.0)), Type::Number);
        assert_eq!(engine.widen(&Type::BooleanLiteral(false)), Type::Boolean);
    }

    #[test]
    fn native_shape_relations_and_member_lookup() {
        let engine = Engine::new();
        let point = Type::Shape(vec![
            Property::new("x", Type::Number),
            Property::new("y", Type::Number),
        ]);
        let x = Type::Shape(vec![Property::new("x", Type::Number)]);
        assert!(engine.is_assignable(&point, &x));
        assert!(!engine.is_assignable(&x, &point));
        assert!(!engine.is_assignable(&Type::Shape(vec![Property::new("x", Type::String)]), &x));
        assert_eq!(engine.property_type(&point, "x"), Some(Type::Number));
        assert_eq!(engine.property_type(&point, "z"), None);
    }

    #[test]
    fn native_function_parameter_variance() {
        let engine = Engine::new();
        let function = |ty| {
            Type::Function(FunctionType {
                params: vec![Parameter {
                    name: "value".into(),
                    ty,
                    optional: false,
                }],
                returns: Box::new(Type::String),
            })
        };
        assert!(engine.is_assignable(&function(Type::Unknown), &function(Type::Number)));
        assert!(!engine.is_assignable(&function(Type::Number), &function(Type::Unknown)));
    }

    #[test]
    fn native_freshness_and_optional_properties() {
        let engine = Engine::new();
        let target = Type::Shape(vec![Property::new("x", Type::Number)]);
        let properties = vec![
            Property::new("x", Type::Number),
            Property::new("extra", Type::Boolean),
        ];
        assert!(engine.is_assignable(&Type::Shape(properties.clone()), &target));
        assert!(!engine.is_assignable(&Type::FreshShape(properties), &target));
        let mut property = Property::new("name", Type::String);
        property.optional = true;
        let optional = Type::Shape(vec![property]);
        assert!(engine.is_assignable(&Type::Shape(vec![]), &optional));
        let read = engine.property_type(&optional, "name").unwrap();
        assert!(engine.is_assignable(&Type::Nil, &read));
        assert!(!engine.is_assignable(&read, &Type::String));
    }

    #[test]
    fn lua_string_bytes_are_not_decoded_by_the_type_core() {
        let engine = Engine::new();
        let bytes = Type::StringLiteral(vec![0x66, 0xff, 0, 0x80]);
        let record = Type::Shape(vec![Property::new("value", bytes.clone())]);
        assert_eq!(engine.property_type(&record, "value"), Some(bytes.clone()));
        assert_eq!(engine.widen(&bytes), Type::String);
        assert!(!engine.is_assignable(&bytes, &Type::StringLiteral(b"f".to_vec())));
    }

    #[test]
    fn native_assignment_transfer_respects_declared_views() {
        let engine = Engine::new();
        let declared = Type::Shape(vec![Property::new("x", Type::Number)]);
        let assigned = Type::Shape(vec![
            Property::new("x", Type::Number),
            Property::new("extra", Type::String),
        ]);
        let current = engine.assignment_value(&declared, &assigned);
        assert_eq!(engine.property_type(&current, "x"), Some(Type::Number));
        assert_eq!(engine.property_type(&current, "extra"), None);
        assert_eq!(
            engine.assignment_value(&Type::Unknown, &Type::Number),
            Type::Unknown
        );
        assert_eq!(
            engine.assignment_value(
                &Type::Union(vec![Type::Number, Type::String, Type::Nil]),
                &Type::NumberLiteral(42.0)
            ),
            Type::Number
        );
        assert_eq!(
            engine.assignment_value(&Type::Union(vec![Type::Number, Type::String]), &Type::Any),
            Type::Union(vec![Type::String, Type::Number])
        );
        assert_eq!(
            engine.assignment_value(&Type::Boolean, &Type::BooleanLiteral(false)),
            Type::BooleanLiteral(false)
        );
    }
}
