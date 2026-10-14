//! Parser-independent access to the pinned Bun TypeScript relation engine.
//!
//! This facade owns resolved types. It does not yet supply Lua lazy expression
//! queries, contextual inference, or control-flow analysis to the checker.

#![feature(linkage)]

mod native;

use bun_sema::check::{Checker, Program, ResolvedTypeCallError};
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

#[derive(Debug, Clone, PartialEq)]
pub struct CallResult {
    pub return_type: Type,
    pub errors: Vec<CallError>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    NotCallable,
    UnsupportedGeneric,
    UnsupportedOverloads,
    UnsupportedRest,
    UnsupportedThis,
    Arity {
        actual: usize,
        minimum: usize,
        maximum: Option<usize>,
    },
    Argument {
        index: usize,
        source: Type,
        target: Type,
    },
}

/// Queries available during one native checker session. All inputs and outputs
/// are owned descriptions; checker lifetimes and type IDs remain private.
pub trait TypeQueries {
    fn is_assignable(&mut self, source: &Type, target: &Type) -> bool;
    fn compare(&mut self, source: &Type, target: &Type) -> Relation;
    fn widen(&mut self, ty: &Type) -> Type;
    fn property_type(&mut self, ty: &Type, name: &str) -> Option<Type>;
    fn assignment_value(&mut self, declared: &Type, assigned: &Type) -> Type;
    fn call(&mut self, callee: &Type, arguments: &[Type]) -> CallResult;
    fn allows_implicit_return(&mut self, returned: &Type) -> bool;
    fn format(&mut self, ty: &Type) -> String;
}

#[derive(Default)]
pub struct Engine;

impl Engine {
    pub fn new() -> Self {
        Self
    }

    /// Runs a complete check with one worker, arena, program, and checker task.
    /// Returning owned results ends the batch and frees its native state.
    pub fn with_session<R: Send>(&self, work: impl FnOnce(&mut dyn TypeQueries) -> R + Send) -> R {
        with_checker(|checker| work(&mut SessionQueries { checker }))
    }

    pub fn is_assignable(&self, source: &Type, target: &Type) -> bool {
        self.with_session(|queries| queries.is_assignable(source, target))
    }

    pub fn compare(&self, source: &Type, target: &Type) -> Relation {
        self.with_session(|queries| queries.compare(source, target))
    }

    /// Applies TypeScript's literal widening to a mutable initialized location.
    pub fn widen(&self, ty: &Type) -> Type {
        self.with_session(|queries| queries.widen(ty))
    }

    pub fn property_type(&self, ty: &Type, name: &str) -> Option<Type> {
        self.with_session(|queries| queries.property_type(ty, name))
    }

    /// The native checker's assignment transfer for an explicitly declared
    /// location. Its flow caller retains nonunions and reduces union members.
    pub fn assignment_value(&self, declared: &Type, assigned: &Type) -> Type {
        self.with_session(|queries| queries.assignment_value(declared, assigned))
    }

    pub fn call(&self, callee: &Type, arguments: &[Type]) -> CallResult {
        self.with_session(|queries| queries.call(callee, arguments))
    }

    pub fn allows_implicit_return(&self, returned: &Type) -> bool {
        self.with_session(|queries| queries.allows_implicit_return(returned))
    }

    pub fn format(&self, ty: &Type) -> String {
        self.with_session(|queries| queries.format(ty))
    }
}

struct SessionQueries<'a, 'p, 's> {
    checker: &'a mut Checker<'p, 's>,
}

impl TypeQueries for SessionQueries<'_, '_, '_> {
    fn allows_implicit_return(&mut self, returned: &Type) -> bool {
        let returned = import_type(self.checker, returned);
        self.checker
            .allows_implicit_return_without_explicit_return(returned)
    }
    fn is_assignable(&mut self, source: &Type, target: &Type) -> bool {
        let source = import_type(self.checker, source);
        let target = import_type(self.checker, target);
        self.checker.is_assignable(source, target)
    }

    fn compare(&mut self, source: &Type, target: &Type) -> Relation {
        let source = import_type(self.checker, source);
        let target = import_type(self.checker, target);
        Relation {
            assignable: self.checker.is_assignable(source, target),
            source: String::from_utf8_lossy(&self.checker.type_to_string(source)).into_owned(),
            target: String::from_utf8_lossy(&self.checker.type_to_string(target)).into_owned(),
        }
    }

    fn widen(&mut self, ty: &Type) -> Type {
        let ty = import_type(self.checker, ty);
        let ty = self.checker.widen_literal(ty);
        export_type(self.checker, ty)
    }

    fn property_type(&mut self, ty: &Type, name: &str) -> Option<Type> {
        let ty = import_type(self.checker, ty);
        let name = self.checker.atoms().intern(name.as_bytes());
        self.checker
            .type_of_property(ty, name)
            .map(|ty| export_type(self.checker, ty))
    }

    fn assignment_value(&mut self, declared: &Type, assigned: &Type) -> Type {
        let declared = import_type(self.checker, declared);
        let assigned = import_type(self.checker, assigned);
        let value = if self.checker.is_union(declared) {
            self.checker.assignment_reduced_type(declared, assigned)
        } else {
            declared
        };
        export_type(self.checker, value)
    }

    fn call(&mut self, callee: &Type, arguments: &[Type]) -> CallResult {
        let callee = import_type(self.checker, callee);
        let arguments: Vec<_> = arguments
            .iter()
            .map(|argument| import_type(self.checker, argument))
            .collect();
        let result = self.checker.call_resolved_types(callee, &arguments);
        let return_type = export_type(self.checker, result.ret);
        let errors = result
            .errors
            .into_iter()
            .map(|error| match error {
                ResolvedTypeCallError::NotCallable => CallError::NotCallable,
                ResolvedTypeCallError::UnsupportedGeneric => CallError::UnsupportedGeneric,
                ResolvedTypeCallError::UnsupportedOverloads => CallError::UnsupportedOverloads,
                ResolvedTypeCallError::UnsupportedRest => CallError::UnsupportedRest,
                ResolvedTypeCallError::UnsupportedThis => CallError::UnsupportedThis,
                ResolvedTypeCallError::Arity { actual, min, max } => CallError::Arity {
                    actual,
                    minimum: min,
                    maximum: max,
                },
                ResolvedTypeCallError::Argument {
                    index,
                    source,
                    target,
                } => CallError::Argument {
                    index,
                    source: export_type(self.checker, source),
                    target: export_type(self.checker, target),
                },
            })
            .collect();
        CallResult {
            return_type,
            errors,
        }
    }

    fn format(&mut self, ty: &Type) -> String {
        let ty = import_type(self.checker, ty);
        String::from_utf8_lossy(&self.checker.type_to_string(ty)).into_owned()
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
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut checker = program.checker();
                    work(&mut checker)
                }));
                program.release();
                result.unwrap_or_else(|error| std::panic::resume_unwind(error))
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
                let mut ty = import_type(checker, &param.ty);
                if param.optional {
                    ty = checker.optional(ty);
                }
                params.push(SigParam {
                    name: checker.atoms().intern(param.name.as_bytes()),
                    ty,
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
            Intrinsic::Any | Intrinsic::Error => Type::Any,
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

    #[test]
    fn batch_assignment_and_member_queries_return_owned_results() {
        let engine = Engine::new();
        let declared = Type::Union(vec![
            Type::Shape(vec![Property::new("value", Type::Number)]),
            Type::Nil,
        ]);
        let (current, property, failure, printed) = engine.with_session(|queries| {
            let inferred = queries.widen(&Type::NumberLiteral(42.0));
            let assigned = Type::Shape(vec![
                Property::new("value", inferred),
                Property::new("extra", Type::String),
            ]);
            assert!(queries.is_assignable(&assigned, &declared));
            let current = queries.assignment_value(&declared, &assigned);
            let property = queries.property_type(&current, "value").unwrap();
            assert_eq!(queries.property_type(&current, "extra"), None);
            let failure = queries.compare(&Type::String, &property);
            let printed = queries.format(&current);
            (current, property, failure, printed)
        });
        assert_eq!(property, Type::Number);
        assert!(!failure.assignable);
        assert_eq!(failure.source, "string");
        assert_eq!(failure.target, "number");
        assert!(printed.contains("value: number"));
        assert_eq!(engine.property_type(&current, "value"), Some(Type::Number));
    }

    #[test]
    fn batch_panic_preserves_payload_and_allows_subsequent_queries() {
        #[derive(Debug, PartialEq, Eq)]
        struct CallbackFailure(String);

        let engine = Engine::new();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine.with_session(|queries| {
                let record = Type::Shape(vec![Property::new("value", Type::Number)]);
                assert_eq!(queries.property_type(&record, "value"), Some(Type::Number));
                std::panic::panic_any(CallbackFailure("Lua check interrupted".into()));
            });
        }))
        .expect_err("callback panic must reach the caller");
        assert_eq!(
            *panic
                .downcast::<CallbackFailure>()
                .expect("original typed payload"),
            CallbackFailure("Lua check interrupted".into())
        );
        let result = engine.with_session(|queries| {
            let inferred = queries.widen(&Type::NumberLiteral(42.0));
            queries.compare(&inferred, &Type::Number)
        });
        assert!(result.assignable);
        assert_eq!(result.source, "number");
        assert_eq!(result.target, "number");
    }

    fn function(parameter: Type, optional: bool, returns: Type) -> Type {
        Type::Function(FunctionType {
            params: vec![Parameter {
                name: "value".into(),
                ty: parameter,
                optional,
            }],
            returns: Box::new(returns),
        })
    }

    #[test]
    fn native_scalar_call_checks_argument_types_and_arity() {
        let engine = Engine::new();
        let callee = function(Type::Number, false, Type::String);
        let success = engine.call(&callee, &[Type::NumberLiteral(42.0)]);
        assert_eq!(success.return_type, Type::String);
        assert!(success.errors.is_empty());
        assert_eq!(
            engine.call(&callee, &[]).errors,
            vec![CallError::Arity {
                actual: 0,
                minimum: 1,
                maximum: Some(1),
            }]
        );
        assert_eq!(
            engine.call(&callee, &[Type::Number, Type::Number]).errors,
            vec![CallError::Arity {
                actual: 2,
                minimum: 1,
                maximum: Some(1),
            }]
        );
        let failure = engine.call(&callee, &[Type::Boolean]);
        assert_eq!(failure.return_type, Type::String);
        assert_eq!(
            failure.errors,
            vec![CallError::Argument {
                index: 0,
                source: Type::Boolean,
                target: Type::Number,
            }]
        );
        let two_parameters = Type::Function(FunctionType {
            params: vec![
                Parameter {
                    name: "first".into(),
                    ty: Type::Number,
                    optional: false,
                },
                Parameter {
                    name: "second".into(),
                    ty: Type::String,
                    optional: false,
                },
            ],
            returns: Box::new(Type::Boolean),
        });
        assert_eq!(
            engine
                .call(&two_parameters, &[Type::String, Type::Number])
                .errors,
            vec![CallError::Argument {
                index: 0,
                source: Type::String,
                target: Type::Number,
            }]
        );
    }

    #[test]
    fn native_call_optional_parameters_accept_omission_and_nil() {
        let engine = Engine::new();
        let callee = function(Type::Number, true, Type::String);
        assert!(engine.call(&callee, &[]).errors.is_empty());
        assert!(engine.call(&callee, &[Type::Nil]).errors.is_empty());
        assert!(engine.call(&callee, &[Type::Number]).errors.is_empty());
        assert!(matches!(
            engine.call(&callee, &[Type::String]).errors.as_slice(),
            [CallError::Argument { index: 0, .. }]
        ));
    }

    #[test]
    fn native_call_uses_function_variance_and_union_signatures() {
        let engine = Engine::new();
        let callback = function(Type::Number, false, Type::String);
        let takes_callback = function(callback, false, Type::Boolean);
        assert!(
            engine
                .call(
                    &takes_callback,
                    &[function(Type::Unknown, false, Type::String)]
                )
                .errors
                .is_empty()
        );
        assert!(matches!(
            engine
                .call(
                    &takes_callback,
                    &[function(Type::String, false, Type::String)]
                )
                .errors
                .as_slice(),
            [CallError::Argument { index: 0, .. }]
        ));
        let callee = Type::Union(vec![
            function(Type::Number, false, Type::String),
            function(Type::Number, false, Type::Number),
        ]);
        let result = engine.call(&callee, &[Type::Number]);
        assert!(result.errors.is_empty());
        assert!(engine.is_assignable(&Type::String, &result.return_type));
        assert!(engine.is_assignable(&Type::Number, &result.return_type));
        let disjoint = Type::Union(vec![
            function(Type::Number, false, Type::String),
            function(Type::String, false, Type::String),
        ]);
        assert!(matches!(
            engine.call(&disjoint, &[Type::Number]).errors.as_slice(),
            [CallError::Argument {
                target: Type::Never,
                ..
            }]
        ));
    }

    #[test]
    fn native_calls_report_noncallable_and_unsupported_overloads() {
        let engine = Engine::new();
        assert_eq!(
            engine.call(&Type::Number, &[]).errors,
            vec![CallError::NotCallable]
        );
        assert_eq!(
            engine.call(&Type::Unknown, &[]).errors,
            vec![CallError::NotCallable]
        );
        let untyped = engine.call(&Type::Any, &[Type::Number, Type::String]);
        assert_eq!(untyped.return_type, Type::Any);
        assert!(untyped.errors.is_empty());
        let overloaded = Type::Intersection(vec![
            function(Type::Number, false, Type::String),
            function(Type::String, false, Type::Number),
        ]);
        assert_eq!(
            engine.call(&overloaded, &[Type::Number]).errors,
            vec![CallError::UnsupportedOverloads]
        );
    }

    #[test]
    fn native_call_boundary_rejects_this_and_correlated_rest_metadata() {
        with_checker(|checker| {
            let signature = |checker: &Checker<'_, '_>, this, params: &[SigParam]| {
                let sig = checker.types().intern_sig(SigData::Synth {
                    type_params: checker.list(&[]),
                    params: checker.list(params),
                    ret: TypeId::STRING,
                    this,
                    of: checker.list(&[]),
                    is_union: false,
                });
                let mut shape = Shape::new_in(checker.arena);
                shape.call.push(sig);
                checker.synth(shape)
            };
            let with_this = signature(checker, Some(TypeId::NUMBER), &[]);
            let result = checker.call_resolved_types(with_this, &[]);
            assert!(matches!(
                result.errors.as_slice(),
                [ResolvedTypeCallError::UnsupportedThis]
            ));
            assert_eq!(result.ret, TypeId::STRING);
            let flags = [bun_sema::types::ElemFlags::REQUIRED; 2];
            let first = checker.tuple(&[TypeId::STRING, TypeId::NUMBER], &flags, false);
            let second = checker.tuple(&[TypeId::NUMBER, TypeId::STRING], &flags, false);
            let correlated = checker.union(&[first, second]);
            let with_rest = signature(
                checker,
                None,
                &[SigParam {
                    name: checker.atoms().intern(b"arguments"),
                    ty: correlated,
                    optional: false,
                    rest: true,
                    is_required_rest: false,
                    declaration: None,
                }],
            );
            let result = checker.call_resolved_types(with_rest, &[TypeId::STRING, TypeId::NUMBER]);
            assert!(matches!(
                result.errors.as_slice(),
                [ResolvedTypeCallError::UnsupportedRest]
            ));
            assert_eq!(result.ret, TypeId::STRING);
        });
    }
}
