//! Parser-independent access to the pinned Bun TypeScript relation engine.
//!
//! This facade owns resolved types. It does not yet supply Lua lazy expression
//! queries, contextual inference, or control-flow analysis to the checker.

#![feature(linkage)]

mod native;

use bun_sema::check::{Checker, NativeFunctionProvider, Program, ResolvedTypeCallError};
use bun_sema::hir;
use bun_sema::program::Files;
use bun_sema::resolve::{Host, Options, ParseOptions, ScriptKind};
use bun_sema::session::Session;
use bun_sema::types::{
    Intrinsic, Literalness, MapperId, NativeFunctionId, NativeTypeParamId, Prop, PropFlags,
    PropSource, Shape, SigData, SigParam, TypeData, TypeId,
};
use std::borrow::Cow;

pub const BUN_REVISION: &str = "408c9374b266b45fe6b8260e09be73aa82a957cd";

/// An original-source function identity, independent of backend type numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NativeFunctionKey {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidNativeFunctionKey(pub NativeFunctionKey);

/// A source type parameter, in a namespace separate from function declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NativeTypeParameterKey {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidNativeTypeParameterKey(pub NativeTypeParameterKey);

/// A capability for an exact type in one checker session. Its ordinal is not a
/// backend type ID, and callers cannot construct or change its owner epoch.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionTypeRef {
    epoch: u64,
    slot: u32,
}

impl std::fmt::Debug for SessionTypeRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SessionTypeRef")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidSessionType(pub SessionTypeRef);

struct TypeHandles {
    epoch: u64,
    state: std::cell::RefCell<TypeHandleState>,
}

#[derive(Default)]
struct TypeHandleState {
    types: Vec<TypeId>,
    slots: std::collections::HashMap<TypeId, u32>,
}

impl TypeHandles {
    fn new() -> Self {
        static NEXT_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            epoch: NEXT_EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            state: Default::default(),
        }
    }
    fn retain(&self, ty: TypeId) -> Type {
        let mut state = self.state.borrow_mut();
        let slot = match state.slots.get(&ty).copied() {
            Some(slot) => slot,
            None => {
                let slot = u32::try_from(state.types.len()).expect("session type handle capacity");
                state.types.push(ty);
                state.slots.insert(ty, slot);
                slot
            }
        };
        Type::SessionType(SessionTypeRef {
            epoch: self.epoch,
            slot,
        })
    }
    fn resolve(&self, reference: SessionTypeRef) -> TypeId {
        if reference.epoch != self.epoch {
            std::panic::panic_any(InvalidSessionType(reference));
        }
        let ty = self
            .state
            .borrow()
            .types
            .get(reference.slot as usize)
            .copied();
        ty.unwrap_or_else(|| std::panic::panic_any(InvalidSessionType(reference)))
    }
}

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
    NativeFunction(NativeFunctionKey),
    NativeTypeParameter(NativeTypeParameterKey),
    SessionType(SessionTypeRef),
}

/// Source recipes may use thread-local interior state; release its borrows
/// before reentering the query capability supplied to a callback.
pub trait NativeTypeProvider {
    fn contains_function(&self, key: NativeFunctionKey) -> bool;
    fn function_parameters(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter>;
    fn function_declared_return(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type>;
    fn function_inferred_return(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Type;
    fn return_cycle(&self, key: NativeFunctionKey);
    fn return_type_too_deep(&self, key: NativeFunctionKey);
    fn function_type_parameters(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<NativeTypeParameterKey> {
        Vec::new()
    }
    fn function_outer_type_parameters(&self, _: NativeFunctionKey) -> Vec<NativeTypeParameterKey> {
        Vec::new()
    }
    fn contains_type_parameter(&self, _: NativeTypeParameterKey) -> bool {
        false
    }
    fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String {
        String::new()
    }
    fn has_type_parameter_constraint(&self, _: NativeTypeParameterKey) -> bool {
        false
    }
    fn type_parameter_constraint(
        &self,
        _: NativeTypeParameterKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        None
    }
    fn has_type_parameter_default(&self, _: NativeTypeParameterKey) -> bool {
        false
    }
    fn type_parameter_default(
        &self,
        _: NativeTypeParameterKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        None
    }
    fn type_parameter_order(&self, key: NativeTypeParameterKey) -> u32 {
        key.index
    }
    fn type_parameter_siblings(&self, key: NativeTypeParameterKey) -> Vec<NativeTypeParameterKey> {
        vec![key]
    }
    fn circular_type_parameter_constraint(&self, _: NativeTypeParameterKey) {}
    fn circular_type_parameter_default(&self, _: NativeTypeParameterKey) {}
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
    UnsupportedGenericFunctionArgument {
        index: usize,
    },
    UnsupportedOverloads,
    UnsupportedRest,
    UnsupportedThis,
    TypeArgumentArity {
        actual: usize,
        minimum: usize,
        maximum: usize,
    },
    TypeArgument {
        index: usize,
        source: Type,
        target: Type,
    },
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
    fn union_reduced(&mut self, types: &[Type]) -> Type;
    fn property_type(&mut self, ty: &Type, name: &str) -> Option<Type>;
    fn assignment_value(&mut self, declared: &Type, assigned: &Type) -> Type;
    fn call(&mut self, callee: &Type, arguments: &[Type]) -> CallResult;
    fn call_with_type_arguments(
        &mut self,
        callee: &Type,
        arguments: &[Type],
        type_arguments: &[Type],
        expected_return: Option<&Type>,
    ) -> CallResult;
    fn function_signature(&mut self, ty: &Type) -> Option<FunctionType>;
    fn inspect_type(&mut self, ty: &Type) -> Type;
    fn allows_implicit_return(&mut self, returned: &Type) -> bool;
    fn aggregate_returns(
        &mut self,
        returned: &[Type],
        has_fallthrough: bool,
        context: Option<&Type>,
    ) -> Type;
    fn native_function_signature(&mut self, key: NativeFunctionKey) -> FunctionType;
    fn native_function_type_parameters(
        &mut self,
        key: NativeFunctionKey,
    ) -> Vec<NativeTypeParameterKey>;
    fn type_parameter_constraint(&mut self, key: NativeTypeParameterKey) -> Option<Type>;
    fn type_parameter_default(&mut self, key: NativeTypeParameterKey) -> Option<Type>;
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
        with_program(|program, handles| {
            let mut checker = program.checker();
            work(&mut SessionQueries {
                checker: &mut checker,
                handles,
            })
        })
    }

    /// The provider is constructed inside the worker and need not be Send or
    /// Sync. Reentrant callbacks receive the same native checker capability.
    pub fn with_native_provider<P: NativeTypeProvider, R: Send>(
        &self,
        factory: impl FnOnce() -> P + Send,
        work: impl FnOnce(&P, &mut dyn TypeQueries) -> R + Send,
    ) -> R {
        with_program(|program, handles| {
            let provider = factory();
            let adapter = ProviderAdapter {
                provider: &provider,
                handles,
            };
            let mut checker = program.checker_with_native_provider(&adapter);
            work(
                &provider,
                &mut SessionQueries {
                    checker: &mut checker,
                    handles,
                },
            )
        })
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

    pub fn call_with_type_arguments(
        &self,
        callee: &Type,
        arguments: &[Type],
        type_arguments: &[Type],
        expected_return: Option<&Type>,
    ) -> CallResult {
        self.with_session(|queries| {
            queries.call_with_type_arguments(callee, arguments, type_arguments, expected_return)
        })
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
    handles: &'a TypeHandles,
}

impl TypeQueries for SessionQueries<'_, '_, '_> {
    fn inspect_type(&mut self, ty: &Type) -> Type {
        let id = import_type(self.checker, self.handles, ty);
        let signatures = self.checker.signatures(id, false);
        if signatures.len() == 1 {
            Type::Function(export_signature(self.checker, self.handles, signatures[0]))
        } else {
            export_type(self.checker, self.handles, id)
        }
    }
    fn function_signature(&mut self, ty: &Type) -> Option<FunctionType> {
        let ty = import_type(self.checker, self.handles, ty);
        let signatures = self.checker.signatures(ty, false);
        (signatures.len() == 1).then(|| export_signature(self.checker, self.handles, signatures[0]))
    }
    fn native_function_type_parameters(
        &mut self,
        key: NativeFunctionKey,
    ) -> Vec<NativeTypeParameterKey> {
        let ty = import_type(self.checker, self.handles, &Type::NativeFunction(key));
        let signatures = self.checker.signatures(ty, false);
        let params = self.checker.sig_type_params(signatures[0]).into_vec();
        params
            .into_iter()
            .map(|ty| match export_type(self.checker, self.handles, ty) {
                Type::NativeTypeParameter(key) => key,
                _ => panic!("native generic declaration has a nonparameter type"),
            })
            .collect()
    }

    fn type_parameter_constraint(&mut self, key: NativeTypeParameterKey) -> Option<Type> {
        let ty = import_type(self.checker, self.handles, &Type::NativeTypeParameter(key));
        self.checker
            .constraint_of_type_param(ty)
            .map(|ty| export_type(self.checker, self.handles, ty))
    }

    fn type_parameter_default(&mut self, key: NativeTypeParameterKey) -> Option<Type> {
        let ty = import_type(self.checker, self.handles, &Type::NativeTypeParameter(key));
        self.checker
            .default_of_type_param(ty)
            .map(|ty| export_type(self.checker, self.handles, ty))
    }

    fn aggregate_returns(
        &mut self,
        returned: &[Type],
        has_fallthrough: bool,
        context: Option<&Type>,
    ) -> Type {
        let returned: Vec<_> = returned
            .iter()
            .map(|ty| import_type(self.checker, self.handles, ty))
            .collect();
        let context = context.map(|ty| import_type(self.checker, self.handles, ty));
        let ty = self
            .checker
            .aggregate_return_types(&returned, has_fallthrough, context);
        export_type(self.checker, self.handles, ty)
    }

    fn native_function_signature(&mut self, key: NativeFunctionKey) -> FunctionType {
        let ty = import_type(self.checker, self.handles, &Type::NativeFunction(key));
        let signatures = self.checker.signatures(ty, false);
        export_signature(self.checker, self.handles, signatures[0])
    }

    fn allows_implicit_return(&mut self, returned: &Type) -> bool {
        let returned = import_type(self.checker, self.handles, returned);
        self.checker
            .allows_implicit_return_without_explicit_return(returned)
    }
    fn is_assignable(&mut self, source: &Type, target: &Type) -> bool {
        let source = import_type(self.checker, self.handles, source);
        let target = import_type(self.checker, self.handles, target);
        self.checker.is_assignable(source, target)
    }

    fn compare(&mut self, source: &Type, target: &Type) -> Relation {
        let source_input = source;
        let target_input = target;
        let source = import_type(self.checker, self.handles, source_input);
        let target = import_type(self.checker, self.handles, target_input);
        Relation {
            assignable: self.checker.is_assignable(source, target),
            source: relation_text(self.checker, source_input, source),
            target: relation_text(self.checker, target_input, target),
        }
    }

    fn widen(&mut self, ty: &Type) -> Type {
        let ty = import_type(self.checker, self.handles, ty);
        let ty = self.checker.widened(ty);
        export_type(self.checker, self.handles, ty)
    }

    fn union_reduced(&mut self, types: &[Type]) -> Type {
        let types: Vec<_> = types
            .iter()
            .map(|ty| import_type(self.checker, self.handles, ty))
            .collect();
        let ty = self.checker.union_reduced(&types);
        export_type(self.checker, self.handles, ty)
    }

    fn property_type(&mut self, ty: &Type, name: &str) -> Option<Type> {
        let ty = import_type(self.checker, self.handles, ty);
        let name = self.checker.atoms().intern(name.as_bytes());
        self.checker
            .type_of_property(ty, name)
            .map(|ty| export_type(self.checker, self.handles, ty))
    }

    fn assignment_value(&mut self, declared: &Type, assigned: &Type) -> Type {
        let declared = import_type(self.checker, self.handles, declared);
        let assigned = import_type(self.checker, self.handles, assigned);
        let value = if self.checker.is_union(declared) {
            self.checker.assignment_reduced_type(declared, assigned)
        } else {
            declared
        };
        export_type(self.checker, self.handles, value)
    }

    fn call(&mut self, callee: &Type, arguments: &[Type]) -> CallResult {
        self.call_with_type_arguments(callee, arguments, &[], None)
    }

    fn call_with_type_arguments(
        &mut self,
        callee: &Type,
        arguments: &[Type],
        type_arguments: &[Type],
        expected_return: Option<&Type>,
    ) -> CallResult {
        let callee = import_type(self.checker, self.handles, callee);
        let arguments: Vec<_> = arguments
            .iter()
            .map(|argument| import_type(self.checker, self.handles, argument))
            .collect();
        let type_arguments: Vec<_> = type_arguments
            .iter()
            .map(|ty| import_type(self.checker, self.handles, ty))
            .collect();
        let expected_return = expected_return.map(|ty| import_type(self.checker, self.handles, ty));
        let result = self.checker.call_resolved_types_with_type_arguments(
            callee,
            &arguments,
            &type_arguments,
            expected_return,
        );
        let return_type = export_type(self.checker, self.handles, result.ret);
        let errors = result
            .errors
            .into_iter()
            .map(|error| match error {
                ResolvedTypeCallError::NotCallable => CallError::NotCallable,
                ResolvedTypeCallError::UnsupportedGeneric => CallError::UnsupportedGeneric,
                ResolvedTypeCallError::UnsupportedGenericFunctionArgument { index } => {
                    CallError::UnsupportedGenericFunctionArgument { index }
                }
                ResolvedTypeCallError::UnsupportedOverloads => CallError::UnsupportedOverloads,
                ResolvedTypeCallError::UnsupportedRest => CallError::UnsupportedRest,
                ResolvedTypeCallError::UnsupportedThis => CallError::UnsupportedThis,
                ResolvedTypeCallError::TypeArgumentArity { actual, min, max } => {
                    CallError::TypeArgumentArity {
                        actual,
                        minimum: min,
                        maximum: max,
                    }
                }
                ResolvedTypeCallError::TypeArgument {
                    index,
                    source,
                    target,
                } => CallError::TypeArgument {
                    index,
                    source: export_type(self.checker, self.handles, source),
                    target: export_type(self.checker, self.handles, target),
                },
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
                    source: export_type(self.checker, self.handles, source),
                    target: export_type(self.checker, self.handles, target),
                },
            })
            .collect();
        CallResult {
            return_type,
            errors,
        }
    }

    fn format(&mut self, ty: &Type) -> String {
        let ty = import_type(self.checker, self.handles, ty);
        String::from_utf8_lossy(&self.checker.type_to_string(ty)).into_owned()
    }
}

fn contains_native_function(ty: &Type) -> bool {
    match ty {
        Type::NativeFunction(_) | Type::SessionType(_) => true,
        Type::Union(types) | Type::Intersection(types) => {
            types.iter().any(contains_native_function)
        }
        Type::Shape(properties) | Type::FreshShape(properties) => properties
            .iter()
            .any(|property| contains_native_function(&property.ty)),
        Type::Function(function) => {
            contains_native_function(&function.returns)
                || function
                    .params
                    .iter()
                    .any(|parameter| contains_native_function(&parameter.ty))
        }
        _ => false,
    }
}

fn relation_text(checker: &mut Checker<'_, '_>, source: &Type, ty: TypeId) -> String {
    if contains_native_function(source) {
        // Rendering a deferred function resolves its return. Diagnostics must
        // not add a query while an unrelated source body is still being checked.
        "source function value".into()
    } else {
        String::from_utf8_lossy(&checker.type_to_string(ty)).into_owned()
    }
}

// The native checker measures its remaining stack; use the same stack as its
// upstream standalone worker rather than guessing the caller's thread stack.
#[cfg(test)]
fn with_checker<R: Send>(work: impl FnOnce(&mut Checker<'_, '_>) -> R + Send) -> R {
    with_program(|program, _handles| {
        let mut checker = program.checker();
        work(&mut checker)
    })
}

fn with_program<R: Send>(work: impl FnOnce(&mut Program<'_>, &TypeHandles) -> R + Send) -> R {
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
                let handles = TypeHandles::new();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    work(&mut program, &handles)
                }));
                program.release();
                result.unwrap_or_else(|error| std::panic::resume_unwind(error))
            })
            .expect("create native type-core worker")
            .join()
            .unwrap_or_else(|error| std::panic::resume_unwind(error))
    })
}

fn core_key(key: NativeFunctionKey) -> NativeFunctionId {
    NativeFunctionId {
        snapshot: key.snapshot,
        index: key.index,
    }
}

fn source_key(key: NativeFunctionId) -> NativeFunctionKey {
    NativeFunctionKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}

fn core_parameter_key(key: NativeTypeParameterKey) -> NativeTypeParamId {
    NativeTypeParamId {
        snapshot: key.snapshot,
        index: key.index,
    }
}

fn source_parameter_key(key: NativeTypeParamId) -> NativeTypeParameterKey {
    NativeTypeParameterKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}

struct ProviderAdapter<'a, P> {
    provider: &'a P,
    handles: &'a TypeHandles,
}

impl<P: NativeTypeProvider> NativeFunctionProvider for ProviderAdapter<'_, P> {
    fn outer_type_parameters(&self, key: NativeFunctionId) -> Vec<NativeTypeParamId> {
        self.provider
            .function_outer_type_parameters(source_key(key))
            .into_iter()
            .map(core_parameter_key)
            .collect()
    }
    fn type_parameters(&self, checker: &mut Checker<'_, '_>, key: NativeFunctionId) -> Vec<TypeId> {
        let keys = self.provider.function_type_parameters(
            source_key(key),
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        keys.into_iter()
            .map(|key| import_type(checker, self.handles, &Type::NativeTypeParameter(key)))
            .collect()
    }

    fn contains_type_parameter(&self, key: NativeTypeParamId) -> bool {
        self.provider
            .contains_type_parameter(source_parameter_key(key))
    }
    fn type_parameter_name(
        &self,
        checker: &Checker<'_, '_>,
        key: NativeTypeParamId,
    ) -> bun_sema::atom::Atom {
        checker.atoms().intern(
            self.provider
                .type_parameter_name(source_parameter_key(key))
                .as_bytes(),
        )
    }
    fn has_type_parameter_constraint(&self, key: NativeTypeParamId) -> bool {
        self.provider
            .has_type_parameter_constraint(source_parameter_key(key))
    }
    fn type_parameter_constraint(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeTypeParamId,
    ) -> Option<TypeId> {
        let ty = self.provider.type_parameter_constraint(
            source_parameter_key(key),
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        ty.map(|ty| import_type(checker, self.handles, &ty))
    }
    fn has_type_parameter_default(&self, key: NativeTypeParamId) -> bool {
        self.provider
            .has_type_parameter_default(source_parameter_key(key))
    }
    fn type_parameter_default(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeTypeParamId,
    ) -> Option<TypeId> {
        let ty = self.provider.type_parameter_default(
            source_parameter_key(key),
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        ty.map(|ty| import_type(checker, self.handles, &ty))
    }
    fn type_parameter_order(&self, key: NativeTypeParamId) -> u32 {
        self.provider
            .type_parameter_order(source_parameter_key(key))
    }
    fn type_parameter_siblings(&self, key: NativeTypeParamId) -> Vec<NativeTypeParamId> {
        self.provider
            .type_parameter_siblings(source_parameter_key(key))
            .into_iter()
            .map(core_parameter_key)
            .collect()
    }
    fn circular_type_parameter_constraint(&self, key: NativeTypeParamId) {
        self.provider
            .circular_type_parameter_constraint(source_parameter_key(key));
    }
    fn circular_type_parameter_default(&self, key: NativeTypeParamId) {
        self.provider
            .circular_type_parameter_default(source_parameter_key(key));
    }
    fn contains_function(&self, key: NativeFunctionId) -> bool {
        self.provider.contains_function(source_key(key))
    }

    fn parameters(&self, checker: &mut Checker<'_, '_>, key: NativeFunctionId) -> Vec<SigParam> {
        let parameters = self.provider.function_parameters(
            source_key(key),
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        parameters
            .into_iter()
            .map(|parameter| {
                let mut ty = import_type(checker, self.handles, &parameter.ty);
                if parameter.optional {
                    ty = checker.optional(ty);
                }
                SigParam {
                    name: checker.atoms().intern(parameter.name.as_bytes()),
                    ty,
                    optional: parameter.optional,
                    rest: false,
                    is_required_rest: false,
                    declaration: None,
                }
            })
            .collect()
    }

    fn return_type(&self, checker: &mut Checker<'_, '_>, key: NativeFunctionId) -> TypeId {
        let key = source_key(key);
        let declared = self.provider.function_declared_return(
            key,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        let returned = match declared {
            Some(ty) => ty,
            None => self.provider.function_inferred_return(
                key,
                &mut SessionQueries {
                    checker,
                    handles: self.handles,
                },
            ),
        };
        import_type(checker, self.handles, &returned)
    }

    fn circular_return_type(&self, key: NativeFunctionId) {
        self.provider.return_cycle(source_key(key));
    }

    fn return_type_too_deep(&self, key: NativeFunctionId) {
        self.provider.return_type_too_deep(source_key(key));
    }
}

fn import_type(checker: &mut Checker<'_, '_>, handles: &TypeHandles, ty: &Type) -> TypeId {
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
        Type::SessionType(reference) => handles.resolve(*reference),
        Type::BooleanLiteral(value) => checker.bool_literal(*value, true),
        Type::NumberLiteral(value) => checker.number_literal(*value, true),
        Type::StringLiteral(value) => checker.string_literal(checker.atoms().intern(value), true),
        Type::NativeFunction(key) => checker
            .native_function_type(core_key(*key))
            .unwrap_or_else(|| std::panic::panic_any(InvalidNativeFunctionKey(*key))),
        Type::NativeTypeParameter(key) => checker
            .native_type_parameter(core_parameter_key(*key))
            .unwrap_or_else(|| std::panic::panic_any(InvalidNativeTypeParameterKey(*key))),
        Type::Union(types) => {
            let types: Vec<_> = types
                .iter()
                .map(|ty| import_type(checker, handles, ty))
                .collect();
            checker.union(&types)
        }
        Type::Intersection(types) => {
            let types: Vec<_> = types
                .iter()
                .map(|ty| import_type(checker, handles, ty))
                .collect();
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
                let mut ty = import_type(checker, handles, &property.ty);
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
                let mut ty = import_type(checker, handles, &param.ty);
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
            let ret = import_type(checker, handles, &function.returns);
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

fn export_type(checker: &mut Checker<'_, '_>, handles: &TypeHandles, ty: TypeId) -> Type {
    if ty == TypeId::BOOLEAN {
        return Type::Boolean;
    }
    match checker.data(ty) {
        TypeData::NativeTypeParam {
            declaration,
            mapper,
        } => {
            let key = source_parameter_key(*declaration);
            if *mapper != MapperId::IDENTITY {
                return handles.retain(ty);
            }
            Type::NativeTypeParameter(key)
        }
        TypeData::Intrinsic(kind) => match kind {
            Intrinsic::Any | Intrinsic::Error | Intrinsic::Unresolved => Type::Any,
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
                    .map(|ty| export_type(checker, handles, ty))
                    .collect(),
            )
        }
        TypeData::Intersection(types) => {
            let types = types.to_vec();
            Type::Intersection(
                types
                    .into_iter()
                    .map(|ty| export_type(checker, handles, ty))
                    .collect(),
            )
        }
        TypeData::Synth(shape) if shape.call.len() == 1 && shape.props.is_empty() => {
            let sig = shape.call[0];
            if let Some((key, mapper)) = checker.sig_native_decl(sig) {
                let key = source_key(key);
                if checker
                    .types()
                    .mapping(mapper)
                    .iter()
                    .any(|(source, target)| source != target)
                {
                    return handles.retain(ty);
                }
                Type::NativeFunction(key)
            } else {
                Type::Function(export_signature(checker, handles, sig))
            }
        }
        TypeData::Synth(shape) => {
            let fresh = shape.literal.is_of_expression() && !shape.is_regular;
            let properties: Vec<_> = shape
                .props
                .iter()
                .map(|property| {
                    let ty = checker.type_of_prop(property, shape.mapper);
                    (
                        String::from_utf8_lossy(checker.atoms().bytes(property.name)).into_owned(),
                        ty,
                        property.flags.contains(PropFlags::OPTIONAL),
                        property.flags.contains(PropFlags::READONLY),
                    )
                })
                .collect();
            let properties = properties
                .into_iter()
                .map(|(name, ty, optional, readonly)| Property {
                    name,
                    ty: export_type(checker, handles, ty),
                    optional,
                    readonly,
                })
                .collect();
            if fresh {
                Type::FreshShape(properties)
            } else {
                Type::Shape(properties)
            }
        }
        _ => handles.retain(ty),
    }
}

fn export_signature(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    sig: bun_sema::types::SigId,
) -> FunctionType {
    let params = checker.sig_params(sig).into_vec();
    let returns = checker.sig_return(sig);
    FunctionType {
        params: params
            .into_iter()
            .map(|param| Parameter {
                name: String::from_utf8_lossy(checker.atoms().bytes(param.name)).into_owned(),
                ty: export_type(checker, handles, param.ty),
                optional: param.optional,
            })
            .collect(),
        returns: Box::new(export_type(checker, handles, returns)),
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

    struct StaticGenericProvider;
    impl StaticGenericProvider {
        fn function(index: u32) -> NativeFunctionKey {
            NativeFunctionKey {
                snapshot: 91,
                index,
            }
        }
        fn parameter(index: u32) -> NativeTypeParameterKey {
            NativeTypeParameterKey {
                snapshot: 91,
                index,
            }
        }
        fn variable(index: u32) -> Type {
            Type::NativeTypeParameter(Self::parameter(index))
        }
        fn argument(name: &str, ty: Type) -> Parameter {
            Parameter {
                name: name.into(),
                ty,
                optional: false,
            }
        }
        fn callback(parameter: Option<Type>, returned: Type) -> Type {
            Type::Function(FunctionType {
                params: parameter
                    .into_iter()
                    .map(|ty| Self::argument("value", ty))
                    .collect(),
                returns: Box::new(returned),
            })
        }
    }
    impl NativeTypeProvider for StaticGenericProvider {
        fn contains_function(&self, key: NativeFunctionKey) -> bool {
            key.snapshot == 91 && key.index < 7
        }
        fn function_parameters(
            &self,
            key: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<Parameter> {
            match key.index {
                0 | 1 | 6 => vec![Self::argument("value", Self::variable(key.index))],
                3 => vec![Self::argument("value", Self::variable(1))],
                4 => vec![
                    Self::argument("value", Self::variable(3)),
                    Self::argument(
                        "map",
                        Self::callback(Some(Self::variable(3)), Self::variable(4)),
                    ),
                ],
                5 => vec![
                    Self::argument("produce", Self::callback(None, Self::variable(5))),
                    Self::argument(
                        "consume",
                        Self::callback(Some(Self::variable(5)), Type::Void),
                    ),
                ],
                _ => Vec::new(),
            }
        }
        fn function_declared_return(
            &self,
            key: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Option<Type> {
            Some(match key.index {
                1 => Type::NativeFunction(Self::function(3)),
                3 => Self::variable(1),
                4 => Self::variable(4),
                _ => Self::variable(key.index),
            })
        }
        fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
            panic!("static generic fixture returns are annotated");
        }
        fn return_cycle(&self, _: NativeFunctionKey) {
            panic!("static generic fixture cannot cycle");
        }
        fn return_type_too_deep(&self, _: NativeFunctionKey) {
            panic!("static generic fixture exceeds depth");
        }
        fn function_type_parameters(
            &self,
            key: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<NativeTypeParameterKey> {
            match key.index {
                3 => vec![],
                4 => vec![Self::parameter(3), Self::parameter(4)],
                _ => vec![Self::parameter(key.index)],
            }
        }
        fn function_outer_type_parameters(
            &self,
            key: NativeFunctionKey,
        ) -> Vec<NativeTypeParameterKey> {
            if key.index == 3 {
                vec![Self::parameter(1)]
            } else {
                Vec::new()
            }
        }
        fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
            key.snapshot == 91 && key.index < 7
        }
        fn type_parameter_name(&self, key: NativeTypeParameterKey) -> String {
            format!("T{}", key.index)
        }
        fn has_type_parameter_constraint(&self, key: NativeTypeParameterKey) -> bool {
            key.index == 6
        }
        fn type_parameter_constraint(
            &self,
            _: NativeTypeParameterKey,
            _: &mut dyn TypeQueries,
        ) -> Option<Type> {
            Some(Type::Number)
        }
        fn has_type_parameter_default(&self, key: NativeTypeParameterKey) -> bool {
            key.index == 2
        }
        fn type_parameter_default(
            &self,
            _: NativeTypeParameterKey,
            _: &mut dyn TypeQueries,
        ) -> Option<Type> {
            Some(Type::String)
        }
        fn type_parameter_siblings(
            &self,
            key: NativeTypeParameterKey,
        ) -> Vec<NativeTypeParameterKey> {
            if matches!(key.index, 3 | 4) {
                vec![Self::parameter(3), Self::parameter(4)]
            } else {
                vec![key]
            }
        }
    }

    #[test]
    fn native_static_generic_identity_defaults_constraints_and_explicit_arguments() {
        Engine::new().with_native_provider(
            || StaticGenericProvider,
            |_, queries| {
                let id = Type::NativeFunction(StaticGenericProvider::function(0));
                for value in [Type::Number, Type::String] {
                    let result = queries.call(&id, std::slice::from_ref(&value));
                    assert!(result.errors.is_empty(), "{result:?}");
                    assert_eq!(result.return_type, value);
                }
                let explicit =
                    queries.call_with_type_arguments(&id, &[Type::Number], &[Type::Number], None);
                assert!(explicit.errors.is_empty());
                assert_eq!(explicit.return_type, Type::Number);
                assert!(matches!(
                    queries
                        .call_with_type_arguments(
                            &id,
                            &[Type::Number],
                            &[Type::Number, Type::String],
                            None
                        )
                        .errors
                        .as_slice(),
                    [CallError::TypeArgumentArity { .. }]
                ));
                let default = Type::NativeFunction(StaticGenericProvider::function(2));
                assert_eq!(queries.call(&default, &[]).return_type, Type::String);
                let contextual =
                    queries.call_with_type_arguments(&default, &[], &[], Some(&Type::Number));
                assert!(contextual.errors.is_empty());
                assert_eq!(contextual.return_type, Type::Number);
                let bounded = Type::NativeFunction(StaticGenericProvider::function(6));
                assert!(queries.call(&bounded, &[Type::Number]).errors.is_empty());
                assert!(matches!(
                    queries.call(&bounded, &[Type::String]).errors.as_slice(),
                    [CallError::Argument { .. }]
                ));
                assert!(matches!(
                    queries
                        .call_with_type_arguments(&bounded, &[Type::String], &[Type::String], None)
                        .errors
                        .as_slice(),
                    [CallError::TypeArgument { .. }]
                ));
            },
        );
    }

    #[test]
    fn native_static_generic_callbacks_use_the_native_signature_inference_loop() {
        Engine::new().with_native_provider(
            || StaticGenericProvider,
            |_, queries| {
                let map = Type::NativeFunction(StaticGenericProvider::function(4));
                let result = queries.call(
                    &map,
                    &[
                        Type::Number,
                        StaticGenericProvider::callback(Some(Type::Number), Type::String),
                    ],
                );
                assert!(result.errors.is_empty(), "{result:?}");
                assert_eq!(result.return_type, Type::String);
                let pipeline = Type::NativeFunction(StaticGenericProvider::function(5));
                let result = queries.call(
                    &pipeline,
                    &[
                        StaticGenericProvider::callback(None, Type::Number),
                        StaticGenericProvider::callback(Some(Type::Number), Type::Void),
                    ],
                );
                assert!(result.errors.is_empty(), "{result:?}");
                assert_eq!(result.return_type, Type::Number);
            },
        );
    }

    #[test]
    fn native_static_generic_returned_function_handles_preserve_context_and_reject_other_epochs() {
        let retained = Engine::new().with_native_provider(
            || StaticGenericProvider,
            |_, queries| {
                let maker = Type::NativeFunction(StaticGenericProvider::function(1));
                let result = queries.call(&maker, &[Type::Number]);
                assert!(result.errors.is_empty(), "{result:?}");
                assert!(matches!(result.return_type, Type::SessionType(_)));
                let signature = queries.function_signature(&result.return_type).unwrap();
                assert_eq!(signature.params[0].ty, Type::Number);
                assert_eq!(signature.returns.as_ref(), &Type::Number);
                assert!(matches!(
                    queries.inspect_type(&result.return_type),
                    Type::Function(_)
                ));
                assert_eq!(
                    queries
                        .call(&result.return_type, &[Type::Number])
                        .return_type,
                    Type::Number
                );
                assert!(matches!(
                    queries
                        .call(&result.return_type, &[Type::String])
                        .errors
                        .as_slice(),
                    [CallError::Argument { .. }]
                ));
                assert!(queries.format(&result.return_type).contains("number"));
                result.return_type
            },
        );
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Engine::new().with_session(|queries| {
                queries.format(&retained);
            })
        }))
        .expect_err("a native context handle cannot cross checker epochs");
        assert!(failure.downcast::<InvalidSessionType>().is_ok());
        assert!(Engine::new().is_assignable(&Type::Number, &Type::Number));
    }

    struct SourceProvider {
        calls: [std::cell::Cell<usize>; 5],
        parameters: [std::cell::Cell<usize>; 5],
        cycles: std::cell::Cell<usize>,
        depths: std::cell::Cell<usize>,
        thread: std::thread::ThreadId,
        _local: std::rc::Rc<()>,
    }

    struct GenericProvider {
        constraint_cycles: std::cell::Cell<usize>,
        default_cycles: std::cell::Cell<usize>,
        with_outer: bool,
    }

    impl GenericProvider {
        fn new() -> Self {
            Self {
                constraint_cycles: std::cell::Cell::new(0),
                default_cycles: std::cell::Cell::new(0),
                with_outer: false,
            }
        }
        fn parameter(index: u32) -> NativeTypeParameterKey {
            NativeTypeParameterKey {
                snapshot: 81,
                index,
            }
        }
        fn function() -> NativeFunctionKey {
            NativeFunctionKey {
                snapshot: 81,
                index: 0,
            }
        }
    }

    impl NativeTypeProvider for GenericProvider {
        fn contains_function(&self, key: NativeFunctionKey) -> bool {
            key == Self::function()
        }
        fn function_parameters(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<Parameter> {
            vec![Parameter {
                name: "value".into(),
                ty: Type::NativeTypeParameter(Self::parameter(0)),
                optional: false,
            }]
        }
        fn function_declared_return(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Option<Type> {
            Some(Type::NativeTypeParameter(Self::parameter(0)))
        }
        fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
            panic!("generic fixture return is declared");
        }
        fn return_cycle(&self, _: NativeFunctionKey) {
            panic!("declared generic return must not infer a cycle");
        }
        fn return_type_too_deep(&self, _: NativeFunctionKey) {
            panic!("small generic fixture exceeds query depth");
        }
        fn function_type_parameters(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<NativeTypeParameterKey> {
            vec![Self::parameter(0), Self::parameter(1)]
        }
        fn function_outer_type_parameters(
            &self,
            _: NativeFunctionKey,
        ) -> Vec<NativeTypeParameterKey> {
            if self.with_outer {
                vec![Self::parameter(4)]
            } else {
                Vec::new()
            }
        }
        fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
            key.snapshot == 81 && key.index < if self.with_outer { 5 } else { 4 }
        }
        fn type_parameter_name(&self, key: NativeTypeParameterKey) -> String {
            ["T", "U", "C", "D", "X"][key.index as usize].into()
        }
        fn has_type_parameter_constraint(&self, key: NativeTypeParameterKey) -> bool {
            key.index < 3
        }
        fn type_parameter_constraint(
            &self,
            key: NativeTypeParameterKey,
            _: &mut dyn TypeQueries,
        ) -> Option<Type> {
            match key.index {
                0 if self.with_outer => Some(Type::NativeTypeParameter(Self::parameter(4))),
                0 => Some(Type::Number),
                1 => Some(Type::NativeTypeParameter(Self::parameter(0))),
                2 => Some(Type::NativeTypeParameter(key)),
                _ => None,
            }
        }
        fn has_type_parameter_default(&self, key: NativeTypeParameterKey) -> bool {
            matches!(key.index, 0 | 1 | 3)
        }
        fn type_parameter_default(
            &self,
            key: NativeTypeParameterKey,
            queries: &mut dyn TypeQueries,
        ) -> Option<Type> {
            match key.index {
                0 if self.with_outer => Some(Type::NativeTypeParameter(Self::parameter(4))),
                0 => Some(Type::Number),
                1 => Some(Type::NativeTypeParameter(Self::parameter(0))),
                3 => queries.type_parameter_default(key),
                _ => None,
            }
        }
        fn type_parameter_siblings(
            &self,
            key: NativeTypeParameterKey,
        ) -> Vec<NativeTypeParameterKey> {
            if key.index < 2 {
                vec![Self::parameter(0), Self::parameter(1)]
            } else {
                vec![key]
            }
        }
        fn circular_type_parameter_constraint(&self, _: NativeTypeParameterKey) {
            self.constraint_cycles.set(self.constraint_cycles.get() + 1);
        }
        fn circular_type_parameter_default(&self, _: NativeTypeParameterKey) {
            self.default_cycles.set(self.default_cycles.get() + 1);
        }
    }

    #[test]
    fn native_generic_parameters_use_native_constraints_defaults_and_relations() {
        Engine::new().with_native_provider(GenericProvider::new, |_, queries| {
            let t = Type::NativeTypeParameter(GenericProvider::parameter(0));
            let u = Type::NativeTypeParameter(GenericProvider::parameter(1));
            assert_eq!(
                queries.type_parameter_constraint(GenericProvider::parameter(0)),
                Some(Type::Number)
            );
            assert_eq!(
                queries.type_parameter_default(GenericProvider::parameter(0)),
                Some(Type::Number)
            );
            assert_eq!(
                queries.type_parameter_constraint(GenericProvider::parameter(1)),
                Some(t.clone())
            );
            assert_eq!(
                queries.type_parameter_default(GenericProvider::parameter(1)),
                Some(t.clone())
            );
            assert!(queries.is_assignable(&t, &Type::Number));
            assert!(!queries.is_assignable(&Type::Number, &t));
            assert!(queries.is_assignable(&u, &t));
            assert!(queries.is_assignable(&u, &Type::Number));
            assert_eq!(queries.format(&t), "T");
            assert_eq!(
                queries.native_function_type_parameters(GenericProvider::function()),
                vec![GenericProvider::parameter(0), GenericProvider::parameter(1)]
            );
            let signature = queries.native_function_signature(GenericProvider::function());
            assert_eq!(signature.params[0].ty, t);
            assert_eq!(signature.returns.as_ref(), &t);
            assert_eq!(
                queries
                    .call(
                        &Type::NativeFunction(GenericProvider::function()),
                        &[Type::Number]
                    )
                    .errors,
                vec![]
            );
        });
    }

    #[test]
    fn native_generic_parameter_cycles_invoke_source_callbacks() {
        Engine::new().with_native_provider(GenericProvider::new, |provider, queries| {
            assert_eq!(
                queries.type_parameter_constraint(GenericProvider::parameter(2)),
                None
            );
            assert!(provider.constraint_cycles.get() > 0);
            assert_eq!(
                queries.type_parameter_default(GenericProvider::parameter(3)),
                None
            );
            assert!(provider.default_cycles.get() > 0);
        });
    }

    #[test]
    fn native_parameter_clone_keeps_sibling_mapper_in_session_handle() {
        with_program(|program, handles| {
            let provider = GenericProvider {
                with_outer: true,
                ..GenericProvider::new()
            };
            let adapter = ProviderAdapter {
                provider: &provider,
                handles,
            };
            let mut checker = program.checker_with_native_provider(&adapter);
            let t = checker
                .native_type_parameter(core_parameter_key(GenericProvider::parameter(0)))
                .unwrap();
            let outer = checker
                .native_type_parameter(core_parameter_key(GenericProvider::parameter(4)))
                .unwrap();
            let around = checker.mapper_from(&[outer], &[TypeId::STRING]);
            let cloned = checker.cloned_native_type_param(
                core_parameter_key(GenericProvider::parameter(1)),
                around,
            );
            for referenced in [
                checker.constraint_of_type_param(cloned).unwrap(),
                checker.default_of_type_param(cloned).unwrap(),
            ] {
                assert_eq!(
                    checker.native_type_parameter_decl(referenced),
                    Some((core_parameter_key(GenericProvider::parameter(0)), around))
                );
                assert_ne!(referenced, t);
                assert_eq!(
                    checker.constraint_of_type_param(referenced),
                    Some(TypeId::STRING)
                );
                assert_eq!(
                    checker.default_of_type_param(referenced),
                    Some(TypeId::STRING)
                );
            }
            let retained = export_type(&mut checker, handles, cloned);
            assert!(matches!(retained, Type::SessionType(_)));
            assert_eq!(import_type(&mut checker, handles, &retained), cloned);
        });
    }

    #[test]
    fn native_type_parameters_reject_foreign_source_membership() {
        for key in [
            NativeTypeParameterKey {
                snapshot: 82,
                index: 0,
            },
            GenericProvider::parameter(4),
        ] {
            let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Engine::new().with_native_provider(GenericProvider::new, |_, queries| {
                    queries.widen(&Type::NativeTypeParameter(key));
                });
            }))
            .expect_err("unregistered source parameter must be rejected");
            assert_eq!(
                *error.downcast::<InvalidNativeTypeParameterKey>().unwrap(),
                InvalidNativeTypeParameterKey(key)
            );
        }
    }

    #[test]
    fn native_reduced_unions_remove_redundant_structural_members() {
        Engine::new().with_session(|queries| {
            assert_eq!(
                queries.union_reduced(&[Type::NumberLiteral(42.0), Type::Number]),
                Type::Number
            );
            let base = Type::Shape(vec![Property::new("value", Type::Number)]);
            let derived = Type::Shape(vec![
                Property::new("value", Type::Number),
                Property::new("extra", Type::String),
            ]);
            let reduced = queries.union_reduced(&[base.clone(), derived]);
            assert!(queries.is_assignable(&base, &reduced));
            assert_eq!(queries.property_type(&reduced, "extra"), None);
        });
    }

    #[test]
    fn native_shape_freshness_survives_union_queries_and_widens_at_storage() {
        Engine::new().with_session(|queries| {
            let target = Type::Shape(vec![Property::new("x", Type::Number)]);
            let literal = Type::FreshShape(vec![
                Property::new("x", Type::Number),
                Property::new("extra", Type::Boolean),
            ]);
            let propagated = queries.union_reduced(std::slice::from_ref(&literal));
            assert_eq!(propagated, literal);
            assert!(!queries.is_assignable(&propagated, &target));
            let stored = queries.widen(&propagated);
            assert!(matches!(stored, Type::Shape(_)));
            assert!(queries.is_assignable(&stored, &target));
            let returned = queries.aggregate_returns(&[literal.clone()], false, None);
            assert!(matches!(returned, Type::Shape(_)));
            assert!(queries.is_assignable(&returned, &target));
            let nested = Type::FreshShape(vec![Property::new("inner", literal)]);
            let stored_nested = queries.widen(&nested);
            let inner = queries.property_type(&stored_nested, "inner").unwrap();
            assert!(matches!(inner, Type::Shape(_)));
            assert!(queries.is_assignable(&inner, &target));
        });
    }

    #[test]
    fn native_function_identity_pairs_preserve_refs_and_substitution_preserves_session_context() {
        Engine::new().with_native_provider(
            || GenericProvider {
                with_outer: true,
                ..GenericProvider::new()
            },
            |_, queries| {
                let function = Type::NativeFunction(GenericProvider::function());
                assert_eq!(queries.widen(&function), function);
            },
        );
        with_program(|program, handles| {
            let provider = GenericProvider {
                with_outer: true,
                ..GenericProvider::new()
            };
            let adapter = ProviderAdapter {
                provider: &provider,
                handles,
            };
            let mut checker = program.checker_with_native_provider(&adapter);
            let function = import_type(
                &mut checker,
                handles,
                &Type::NativeFunction(GenericProvider::function()),
            );
            let outer = checker
                .native_type_parameter(core_parameter_key(GenericProvider::parameter(4)))
                .unwrap();
            let mapper = checker.mapper_from(&[outer], &[TypeId::STRING]);
            let instantiated = checker.instantiate(function, mapper);
            assert_ne!(instantiated, function);
            let retained = export_type(&mut checker, handles, instantiated);
            assert!(matches!(retained, Type::SessionType(_)));
            assert_eq!(import_type(&mut checker, handles, &retained), instantiated);
        });
    }

    impl SourceProvider {
        fn new() -> Self {
            Self {
                calls: std::array::from_fn(|_| std::cell::Cell::new(0)),
                parameters: std::array::from_fn(|_| std::cell::Cell::new(0)),
                cycles: std::cell::Cell::new(0),
                depths: std::cell::Cell::new(0),
                thread: std::thread::current().id(),
                _local: std::rc::Rc::new(()),
            }
        }
        fn key(index: u32) -> NativeFunctionKey {
            NativeFunctionKey {
                snapshot: 71,
                index,
            }
        }
    }

    impl NativeTypeProvider for SourceProvider {
        fn contains_function(&self, key: NativeFunctionKey) -> bool {
            key.snapshot == 71 && key.index < 5
        }
        fn function_parameters(
            &self,
            key: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<Parameter> {
            let count = &self.parameters[key.index as usize];
            count.set(count.get() + 1);
            Vec::new()
        }
        fn function_declared_return(
            &self,
            key: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Option<Type> {
            (key.index == 3).then_some(Type::Number)
        }
        fn function_inferred_return(
            &self,
            key: NativeFunctionKey,
            queries: &mut dyn TypeQueries,
        ) -> Type {
            assert_eq!(self.thread, std::thread::current().id());
            let count = &self.calls[key.index as usize];
            count.set(count.get() + 1);
            match key.index {
                0 => queries.aggregate_returns(&[Type::NumberLiteral(42.0)], false, None),
                1 => Type::NativeFunction(Self::key(0)),
                2 => queries.call(&Type::NativeFunction(key), &[]).return_type,
                4 => Type::NativeFunction(key),
                _ => panic!("an explicit annotation must avoid return-body inference"),
            }
        }
        fn return_cycle(&self, _: NativeFunctionKey) {
            self.cycles.set(self.cycles.get() + 1);
        }
        fn return_type_too_deep(&self, _: NativeFunctionKey) {
            self.depths.set(self.depths.get() + 1);
        }
    }

    #[test]
    fn native_provider_preserves_lazy_identity_and_cached_returns() {
        Engine::new().with_native_provider(SourceProvider::new, |provider, queries| {
            let outer = queries.call(&Type::NativeFunction(SourceProvider::key(1)), &[]);
            assert!(outer.errors.is_empty());
            assert_eq!(
                outer.return_type,
                Type::NativeFunction(SourceProvider::key(0))
            );
            assert_eq!(provider.calls[0].get(), 0);
            let record = Type::Shape(vec![Property::new("callback", outer.return_type.clone())]);
            assert_eq!(
                queries.property_type(&record, "callback"),
                Some(outer.return_type.clone())
            );
            assert_eq!(provider.calls[0].get(), 0);
            for _ in 0..2 {
                let result = queries.call(&outer.return_type, &[]);
                assert!(result.errors.is_empty());
                assert_eq!(result.return_type, Type::Number);
            }
            assert_eq!(provider.calls[0].get(), 1);
            assert_eq!(provider.parameters[0].get(), 1);
            assert_eq!(provider.calls[1].get(), 1);
            assert_eq!(
                queries
                    .native_function_signature(SourceProvider::key(0))
                    .returns
                    .as_ref(),
                &Type::Number
            );
            assert_eq!(
                queries
                    .call(&Type::NativeFunction(SourceProvider::key(3)), &[])
                    .return_type,
                Type::Number
            );
            assert_eq!(provider.calls[3].get(), 0);
        });
    }

    #[test]
    fn native_provider_cycles_reenter_the_same_checker() {
        Engine::new().with_native_provider(SourceProvider::new, |provider, queries| {
            let recursive = Type::NativeFunction(SourceProvider::key(2));
            let result = queries.call(&recursive, &[]);
            assert_eq!(result.return_type, Type::Any);
            assert!(provider.cycles.get() > 0);
            let before = provider.calls[2].get();
            assert_eq!(queries.call(&recursive, &[]).return_type, Type::Any);
            assert_eq!(provider.calls[2].get(), before);
            assert_eq!(
                queries
                    .call(&Type::NativeFunction(SourceProvider::key(0)), &[])
                    .return_type,
                Type::Number
            );
        });
    }

    #[test]
    fn native_provider_rejects_foreign_snapshot_and_missing_function_keys() {
        for key in [
            NativeFunctionKey {
                snapshot: 72,
                index: 0,
            },
            SourceProvider::key(5),
        ] {
            let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Engine::new().with_native_provider(SourceProvider::new, |_, queries| {
                    queries.call(&Type::NativeFunction(key), &[]);
                });
            }))
            .expect_err("foreign source identity must be rejected");
            assert_eq!(
                *failure.downcast::<InvalidNativeFunctionKey>().unwrap(),
                InvalidNativeFunctionKey(key)
            );
        }
    }

    #[test]
    fn native_self_reference_is_not_expanded_by_decorative_queries() {
        Engine::new().with_native_provider(SourceProvider::new, |provider, queries| {
            let function = Type::NativeFunction(SourceProvider::key(4));
            assert!(queries.compare(&function, &function).assignable);
            assert_eq!(queries.widen(&function), function);
            assert_eq!(queries.assignment_value(&function, &function), function);
            assert_eq!(provider.calls[4].get(), 0);
            assert_eq!(queries.call(&function, &[]).return_type, function);
            assert_eq!(provider.calls[4].get(), 1);
            assert_eq!(provider.cycles.get(), 0);
        });
    }

    #[test]
    fn native_return_depth_refusal_reports_before_recovery() {
        with_program(|program, handles| {
            let provider = SourceProvider::new();
            let adapter = ProviderAdapter {
                provider: &provider,
                handles,
            };
            let mut checker = program.checker_with_native_provider(&adapter);
            let ty = checker
                .native_function_type(core_key(SourceProvider::key(0)))
                .unwrap();
            let sig = checker.signatures(ty, false)[0];
            checker.sig_params(sig);
            checker.set_stack_limit(1);
            let returned = checker.sig_return(sig);
            checker.set_stack_limit(6 << 20);
            assert_eq!(provider.depths.get(), 1);
            assert_eq!(provider.calls[0].get(), 0);
            assert_eq!(export_type(&mut checker, handles, returned), Type::Any);
        });
    }

    #[test]
    fn native_source_snapshots_do_not_alias_equal_function_indices() {
        struct TwoSnapshots;
        impl NativeTypeProvider for TwoSnapshots {
            fn contains_function(&self, key: NativeFunctionKey) -> bool {
                key.index == 0 && matches!(key.snapshot, 71 | 72)
            }
            fn function_parameters(
                &self,
                _: NativeFunctionKey,
                _: &mut dyn TypeQueries,
            ) -> Vec<Parameter> {
                Vec::new()
            }
            fn function_declared_return(
                &self,
                key: NativeFunctionKey,
                _: &mut dyn TypeQueries,
            ) -> Option<Type> {
                Some(if key.snapshot == 71 {
                    Type::Number
                } else {
                    Type::String
                })
            }
            fn function_inferred_return(
                &self,
                _: NativeFunctionKey,
                _: &mut dyn TypeQueries,
            ) -> Type {
                panic!("annotated returns must remain authoritative");
            }
            fn return_cycle(&self, _: NativeFunctionKey) {
                panic!("nonrecursive declarations must not cycle");
            }
            fn return_type_too_deep(&self, _: NativeFunctionKey) {
                panic!("small declarations exceed query depth");
            }
        }
        Engine::new().with_native_provider(
            || TwoSnapshots,
            |_, queries| {
                for _ in 0..2 {
                    assert_eq!(
                        queries
                            .call(
                                &Type::NativeFunction(NativeFunctionKey {
                                    snapshot: 71,
                                    index: 0
                                }),
                                &[]
                            )
                            .return_type,
                        Type::Number
                    );
                    assert_eq!(
                        queries
                            .call(
                                &Type::NativeFunction(NativeFunctionKey {
                                    snapshot: 72,
                                    index: 0
                                }),
                                &[]
                            )
                            .return_type,
                        Type::String
                    );
                }
            },
        );
    }

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
