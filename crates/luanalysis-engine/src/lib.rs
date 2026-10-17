//! Parser-independent access to the pinned Bun TypeScript relation engine.
//!
//! Owned type descriptions and original-source providers retain the checker's
//! lazy signatures, contextual inference, and flow-query ownership.

#![feature(linkage)]

mod native;

/// The checker's recursion budget. It is the same on every platform, so that results do not
/// depend on the platform; Windows threads reserve more stack, which only adds headroom.
pub(crate) const CHECKER_STACK_BUDGET: usize = (4 << 20) - (256 << 10);
mod parallel;
pub use parallel::{NativeTaskRun, NativeTaskRunStats};
mod native_plan;
pub use native_plan::{NativeCheckAhead, NativeCheckPlan, NativeCheckTask, NativePlanOptions};
#[cfg(test)]
mod native_plan_tests;
#[cfg(test)]
mod parallel_tests;
#[cfg(test)]
mod native_overload_failure_tests;
mod native_assertions;
mod native_language_service;
#[cfg(test)]
mod native_runtime_primitive_tests;
#[cfg(test)]
mod native_opaque_runtime_tests;
#[cfg(test)]
mod native_array_form_tests;
#[cfg(test)]
mod native_pack_inspection_tests;
#[cfg(test)]
mod native_inheritance_tests;
mod native_flow;
mod native_project_flow_router;
pub use native_flow::*;
#[cfg(test)]
mod native_auto_tests;
mod native_project;
pub use native_project::*;
#[cfg(test)]
mod native_project_tests;
mod native_named;
#[cfg(test)]
mod native_runtime_table_tests;
#[cfg(test)]
mod native_argument_reuse_tests;
#[cfg(test)]
mod native_alias_tests;
#[cfg(test)]
mod native_call_certificate_tests;
#[cfg(test)]
mod native_standard_call_certificate_tests;
#[cfg(test)]
mod native_query_context_tests;
#[cfg(test)]
mod native_contextual_lifetime_tests;
#[cfg(test)]
mod native_global_query_context_tests;
#[cfg(test)]
mod native_standard_loop_write_tests;
#[cfg(test)]
mod native_fresh_namespace_tests;
#[cfg(test)]
mod native_alias_written_contract_tests;
#[cfg(test)]
mod native_alias_environment_tests;
#[cfg(test)]
mod native_written_member_demand_tests;

pub use native_named::*;
#[cfg(test)]
mod native_member_owner_application_tests;
mod native_globals;
pub use native_globals::*;
#[cfg(test)]
mod native_primitive_members_tests;
#[cfg(test)]
mod native_intrinsic_assert_tests;
#[cfg(test)]
mod native_assertion_flow_tests;
#[cfg(test)]
mod native_pack_spread_inference_tests;
mod native_iterators;
mod native_packs;
pub use native_packs::*;
mod native_signatures;
pub use native_signatures::*;
#[cfg(test)]
mod native_pack_signature_tests;
#[cfg(test)]
mod native_scalar_contract_tests;
mod native_tables;
pub use native_tables::*;
#[cfg(test)]
mod regular_literal_tests;
#[cfg(test)]
mod native_lua_print_tests;

pub use bun_sema::check::CheckMode as NativeCheckMode;
/// Deterministic, fast hashing for host tables keyed by original source ids.
pub use bun_sema::util::{FxBuild as FxBuildHasher, FxHashMap, FxHashSet};
pub use bun_sema::check::NativeFunctionCheckState;
pub use bun_sema::check::native_named::NativeOperator;
use bun_sema::check::{
    Checker, NativeFunctionProvider, NativePackResult, NativeTypeResult, Program,
    ResolvedTypeCallError,
};
use bun_sema::hir;
use bun_sema::program::Files;
use bun_sema::resolve::{Host, Options, ParseOptions, ScriptKind};
use bun_sema::session::Session;
use bun_sema::types::{
    Intrinsic, Literalness, MapperId, NativeBindingId, NativeCapturedBinding,
    NativeCapturedFunctionContext, NativeExpressionId, NativeFunctionContext, NativeFunctionId,
    NativeResultPack, NativeTypeParamId, Prop, PropFlags, PropSource, Shape, SigData, SigParam,
    TypeData, TypeId,
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

/// An expression in the original source snapshot, never a synthetic TS node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NativeExpressionKey {
    pub snapshot: u64,
    pub node: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidNativeExpressionKey(pub NativeExpressionKey);

/// A lexical binding identity in the original source snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NativeBindingKey {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidNativeBindingKey(pub NativeBindingKey);

/// An immutable lexical type environment retained in one native session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CapturedFunctionContext {
    epoch: u64,
    slot: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidCapturedFunctionContext(pub CapturedFunctionContext);

/// A contextual signature demand in this checker session. Only source identity
/// and checking mode are public; its expected signature and mapper stay private.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContextualFunctionContext {
    pub expression: NativeExpressionKey,
    pub mode: NativeCheckMode,
    epoch: u64,
    slot: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidContextualFunctionContext(pub ContextualFunctionContext);

/// Exact source caller frame. `None` is declaration space, not inheritance
/// from an unrelated native query that happens to be active during reentry.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeCallerFrame {
    pub contextual: Option<ContextualFunctionContext>,
    pub captured: Option<CapturedFunctionContext>,
}

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
    /// Converted declaration and member headers, each with the provider's
    /// header revision it was read at (`NativeTypeProvider::header_revision`).
    headers: std::cell::RefCell<NativeHeaderCache>,
}

#[derive(Default)]
struct NativeHeaderCache {
    declarations: FxHashMap<bun_sema::check::native_named::NativeTypeDeclarationId, (u64, bun_sema::check::native_named::NativeTypeDeclarationHeader)>,
    members: FxHashMap<bun_sema::check::native_named::NativeTypeMemberId, (u64, bun_sema::check::native_named::NativeTypeMemberHeader)>,
}

#[derive(Default)]
struct TypeHandleState {
    types: Vec<TypeId>,
    slots: std::collections::HashMap<TypeId, u32>,
    contexts: Vec<NativeFunctionContext>,
    context_slots: std::collections::HashMap<NativeFunctionContext, u32>,
    captures: Vec<NativeCapturedFunctionContext>,
    capture_slots: std::collections::HashMap<NativeCapturedFunctionContext, u32>,
    packs: Vec<NativeResultPack>,
    pack_slots: std::collections::HashMap<TypeId, u32>,
}

impl TypeHandles {
    fn new() -> Self {
        static NEXT_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            epoch: NEXT_EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            state: Default::default(),
            headers: Default::default(),
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
    fn retain_context(&self, context: NativeFunctionContext) -> ContextualFunctionContext {
        let mut state = self.state.borrow_mut();
        let slot = match state.context_slots.get(&context).copied() {
            Some(slot) => slot,
            None => {
                let slot = u32::try_from(state.contexts.len()).expect("context handle capacity");
                state.contexts.push(context);
                state.context_slots.insert(context, slot);
                slot
            }
        };
        ContextualFunctionContext {
            expression: source_expression_key(context.expression),
            mode: context.mode,
            epoch: self.epoch,
            slot,
        }
    }
    fn resolve_context(&self, context: &ContextualFunctionContext) -> NativeFunctionContext {
        if context.epoch != self.epoch {
            std::panic::panic_any(InvalidContextualFunctionContext(*context));
        }
        self.state
            .borrow()
            .contexts
            .get(context.slot as usize)
            .copied()
            .filter(|native| {
                source_expression_key(native.expression) == context.expression
                    && native.mode == context.mode
            })
            .unwrap_or_else(|| std::panic::panic_any(InvalidContextualFunctionContext(*context)))
    }
}

impl TypeHandles {
    fn retain_capture(&self, context: NativeCapturedFunctionContext) -> CapturedFunctionContext {
        let mut state = self.state.borrow_mut();
        let slot = match state.capture_slots.get(&context).copied() {
            Some(slot) => slot,
            None => {
                let slot = u32::try_from(state.captures.len()).expect("capture handle capacity");
                state.captures.push(context);
                state.capture_slots.insert(context, slot);
                slot
            }
        };
        CapturedFunctionContext {
            epoch: self.epoch,
            slot,
        }
    }
    fn resolve_capture(&self, context: &CapturedFunctionContext) -> NativeCapturedFunctionContext {
        if context.epoch != self.epoch {
            std::panic::panic_any(InvalidCapturedFunctionContext(*context));
        }
        self.state
            .borrow()
            .captures
            .get(context.slot as usize)
            .copied()
            .unwrap_or_else(|| std::panic::panic_any(InvalidCapturedFunctionContext(*context)))
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
    RuntimeTable,
    RuntimeFunction,
    BooleanLiteral(bool),
    NumberLiteral(f64),
    StringLiteral(Vec<u8>),
    Union(Vec<Type>),
    Intersection(Vec<Type>),
    Shape(Vec<Property>),
    FreshShape(Vec<Property>),
    Table(TableType),
    Function(FunctionType),
    NativeFunction(NativeFunctionKey),
    NativeSignature(NativeSignatureKey),
    NativeTypeParameter(NativeTypeParameterKey),
    /// Original declaration reference. Import resolves transparent aliases in
    /// the native checker; lazy objects remain exact session capabilities.
    NativeNamedReference { declaration: NativeTypeDeclarationKey, arguments: Vec<Type> },
    SessionType(SessionTypeRef),
}

/// Source recipes may use thread-local interior state; release its borrows
/// before reentering the query capability supplied to a callback.
pub trait NativeTypeProvider {
    /// Exact selected original declaration identity for an opaque Lua runtime
    /// category. A matching user-written class name does not grant this fact.
    fn runtime_type_declaration(&self, _: NativeOpaqueRuntimeKind) -> Option<NativeTypeDeclarationKey> { None }
    /// A revision that changes whenever a declaration or member header may
    /// change, letting the session reuse converted headers. `None` (the
    /// default) reads every header afresh.
    fn header_revision(&self) -> Option<u64> { None }
    fn project_flow_provider(&self) -> Option<&dyn NativeFlowProvider> { None }
    fn contextual_type_owner(&self, _: NativeFunctionKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> ContextualTypeOwner { ContextualTypeOwner::Absent }
    fn function_calling_convention(&self, _: NativeFunctionKey) -> FunctionCallingConvention { FunctionCallingConvention::TypeScript }
    /// Exact source method declaration provenance. Function-valued fields stay false.
    fn function_is_method(&self, _: NativeFunctionKey) -> bool { false }
    fn signature_is_method(&self, _: NativeSignatureKey) -> bool { false }
    /// The initial standard-library environment whose primitive metatables are selected.
    /// This is independent of the current lexical or global variable named `string`.
    fn primitive_member_environment(&self) -> Option<NativeEnvironmentKey> { None }
    /// Standard bindings survive unsupported effects under this host policy.
    /// Explicit/proven writes and incomplete queries still take precedence.
    fn assumed_standard_environment(&self) -> Option<NativeEnvironmentKey> { None }
    /// The opt-in strict runtime-effect mode for heap provenance.
    fn strict_runtime_effects(&self) -> bool { false }
    fn primitive_member_binding(&self, _: NativeEnvironmentKey, _: NativePrimitiveMemberKind) -> Option<NativeGlobalBindingKey> { None }
    fn contains_global_environment(&self, _: NativeEnvironmentKey) -> bool { false }
    fn global_environment_binding(&self, _: NativeEnvironmentKey, _: &[u8]) -> Option<NativeGlobalBindingKey> { None }
    fn global_environment_self_binding(&self, _: NativeEnvironmentKey) -> Option<NativeGlobalBindingKey> { None }
    fn global_environment_bindings(&self, _: NativeEnvironmentKey) -> Vec<(Vec<u8>, NativeGlobalBindingKey)> { Vec::new() }
    fn contains_global_binding(&self, _: NativeGlobalBindingKey) -> bool { false }
    fn global_binding_declarations(&self, _: NativeGlobalBindingKey) -> Vec<NativeValueDeclarationKey> { Vec::new() }
    fn contains_value_declaration(&self, _: NativeValueDeclarationKey) -> bool { false }
    fn value_declaration_header(&self, _: NativeValueDeclarationKey) -> ValueDeclarationHeader { panic!("native global value provider is not installed") }
    /// Pure original callable heads; no runtime function or initializer exists.
    fn value_declaration_signatures(&self, _: NativeValueDeclarationKey) -> Option<Vec<NativeSignatureKey>> { None }
    /// Original function heads without classifying the declaration as bodyless.
    fn value_declaration_function_signatures(&self, declaration: NativeValueDeclarationKey) -> Option<Vec<NativeSignatureKey>> { self.value_declaration_signatures(declaration) }
    fn value_declaration_contract(&self, _: NativeValueDeclarationKey, _: &mut dyn TypeQueries) -> Option<Type> { None }
    fn value_declaration_initializer(&self, _: NativeValueDeclarationKey, _: &mut dyn TypeQueries) -> Type { panic!("native global initializer provider is not installed") }
    fn global_query_error(&self, _: GlobalQueryError, _: &mut dyn TypeQueries) {}
    fn contains_module(&self, _: NativeModuleKey) -> bool { false }
    fn module_header(&self, _: NativeModuleKey) -> NativeModuleHeader { panic!("native project provider is not installed") }
    fn module_inferred_result_pack(&self, _: NativeModuleKey, _: &mut dyn TypeQueries) -> ValuePack { panic!("native project provider is not installed") }
    fn project_query_error(&self, _: NativeProjectError) {}

    fn contains_class_definition(&self, _: NativeClassDefinitionKey) -> bool { false }
    fn type_declaration_self_parameter(&self, _: NativeTypeDeclarationKey) -> Option<NativeTypeParameterKey> { None }
    fn class_definition_header(&self, _: NativeClassDefinitionKey) -> ClassDefinitionHeader { panic!("class definition provider absent") }
    fn runtime_table_constructor(&self, _: NativeTypeDeclarationKey) -> Option<NativeExpressionKey> { None }
    fn runtime_table_constructor_owner(&self, _: NativeTypeDeclarationKey) -> Option<NativeFunctionKey> { None }
    fn runtime_table_seed(&self, _: NativeTypeDeclarationKey, _: &mut dyn TypeQueries) -> Option<Type> { None }
    fn runtime_store_receiver(&self, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Option<Type> { None }
    /// Raw written return annotation only. This hook must not inspect the body
    /// or demand its result count. Zero results project to Lua nil in scalar use.
    /// Context/capture variants also return declaration-space annotations; the
    /// native signature applies its mapper exactly once after normalization.
    fn function_declared_scalar_contract(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<ValuePack> { None }
    fn function_declared_scalar_contract_in_context(&self, key: NativeFunctionKey, _: &ContextualFunctionContext, queries: &mut dyn TypeQueries) -> Option<ValuePack> {
        self.function_declared_scalar_contract(key, queries)
    }
    fn function_declared_scalar_contract_in_capture(&self, key: NativeFunctionKey, _: &CapturedFunctionContext, queries: &mut dyn TypeQueries) -> Option<ValuePack> {
        self.function_declared_scalar_contract(key, queries)
    }
    fn contains_signature(&self, _: NativeSignatureKey) -> bool { false }
    fn signature_intrinsic(&self, _: NativeSignatureKey) -> Option<NativeIntrinsicSite> { None }
    fn signature_header(&self, _: NativeSignatureKey) -> SignatureHeader { panic!("native signature provider is not installed") }
    /// Original omitted doc-function result; calls still return a gradual pack.
    /// This is not set for runtime signatures whose results are inferred.
    fn signature_has_unconstrained_return(&self, _: NativeSignatureKey) -> bool { false }
    fn function_overload_signatures(&self, _: NativeFunctionKey) -> Vec<NativeSignatureKey> { Vec::new() }
    fn function_exposes_implementation(&self, _: NativeFunctionKey) -> bool { false }
    fn signature_type_parameters(&self, _: NativeSignatureKey) -> Vec<NativeTypeParameterKey> { Vec::new() }
    fn signature_outer_type_parameters(&self, _: NativeSignatureKey) -> Vec<NativeTypeParameterKey> { Vec::new() }
    /// Original mapped free parameter instances, independent of signature/body evaluation.
    fn signature_outer_type_parameter_instances(&self, _: NativeSignatureKey, _: &mut dyn TypeQueries) -> Vec<Type> { Vec::new() }
    fn signature_parameters(&self, _: NativeSignatureKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { panic!("native signature provider is not installed") }
    fn signature_rest_pack(&self, _: NativeSignatureKey, _: &mut dyn TypeQueries) -> Option<ValuePack> { None }
    fn signature_declared_return_pack(&self, _: NativeSignatureKey, _: &mut dyn TypeQueries) -> ValuePack { panic!("native signature provider is not installed") }
    fn signature_return_cycle(&self, _: NativeSignatureKey) {}
    fn signature_return_type_too_deep(&self, _: NativeSignatureKey) {}
    fn contains_type_declaration(&self, _: NativeTypeDeclarationKey) -> bool { false }
    fn type_declaration_owner_scope(&self, _: NativeTypeDeclarationKey) -> OwnerScopeRecipe {
        OwnerScopeRecipe::ResolvedHead
    }
    fn type_declaration_header(&self, _: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        panic!("native named declaration provider is not installed")
    }
    fn contains_type_member(&self, _: NativeTypeMemberKey) -> bool { false }
    fn type_member_header(&self, _: NativeTypeMemberKey) -> TypeMemberHeader {
        panic!("native named member provider is not installed")
    }
    fn type_member_origin(&self, _: NativeTypeMemberKey) -> TableFieldOrigin { TableFieldOrigin::Declared }
    fn type_member_effective_metadata(&self, member: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> TypeMemberEffectiveMetadata {
        TypeMemberEffectiveMetadata { origin: self.type_member_origin(member), is_assignment: self.type_member_is_assignment(member) }
    }
    fn type_member_owner_application(&self, _: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Option<MemberOwnerApplication> { None }
    /// A store declares a new member only when no inherited contract exists.
    fn type_member_is_assignment(&self, _: NativeTypeMemberKey) -> bool { false }
    fn type_member_activation(&self, _: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> TypeMemberActivation { TypeMemberActivation::Active }
    /// Pure original callable members; no runtime implementation is implied.
    fn type_member_signatures(&self, _: NativeTypeMemberKey) -> Option<Vec<NativeSignatureKey>> { None }
    fn type_declaration_members(&self, _: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> { Vec::new() }
    fn type_declaration_call_signatures(&self, _: NativeTypeDeclarationKey) -> Vec<NativeSignatureKey> { Vec::new() }
    /// Declared operator heads (`---@operator`), each taking the other operand.
    fn type_declaration_operator_signatures(&self, _: NativeTypeDeclarationKey, _: NativeOperator) -> Vec<NativeSignatureKey> { Vec::new() }
    fn type_alias_target(&self, _: NativeTypeDeclarationKey, _: &mut dyn TypeQueries) -> Type {
        panic!("native named alias provider is not installed")
    }
    fn type_member_type(&self, _: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        panic!("native named member provider is not installed")
    }
    fn type_indexer_key(&self, _: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        panic!("native named indexer provider is not installed")
    }
    fn type_declaration_bases(&self, _: NativeTypeDeclarationKey, _: &mut dyn TypeQueries) -> Vec<Type> { Vec::new() }
    fn type_parameter_owner(&self, _: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> { None }
    fn type_declaration_error(&self, _: NamedTypeError) {}
    fn check_expression_pack(
        &self,
        key: NativeExpressionKey,
        expected: Option<&ValuePack>,
        mode: NativeCheckMode,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        let expected = expected.map(|pack| queries.pack_slot(pack, 0));
        ValuePack::single(self.check_expression(key, expected.as_ref(), mode, queries))
    }
    fn function_has_result_pack(&self, _: NativeFunctionKey) -> bool {
        false
    }
    fn function_declared_return_pack(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.function_declared_return(key, queries)
            .map(ValuePack::single)
    }
    fn function_inferred_return_pack(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        ValuePack::single(self.function_inferred_return(key, queries))
    }
    fn function_declared_return_pack_in_context(
        &self,
        key: NativeFunctionKey,
        _: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.function_declared_return_pack(key, queries)
    }
    fn function_inferred_return_pack_in_context(
        &self,
        key: NativeFunctionKey,
        _: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        self.function_inferred_return_pack(key, queries)
    }
    fn function_declared_return_pack_in_capture(
        &self,
        key: NativeFunctionKey,
        _: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.function_declared_return_pack(key, queries)
    }
    fn function_inferred_return_pack_in_capture(
        &self,
        key: NativeFunctionKey,
        _: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        self.function_inferred_return_pack(key, queries)
    }
    fn function_rest_pack(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        None
    }
    fn function_rest_pack_in_context(
        &self,
        key: NativeFunctionKey,
        _: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.function_rest_pack(key, queries)
    }
    fn function_rest_pack_in_capture(
        &self,
        key: NativeFunctionKey,
        _: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.function_rest_pack(key, queries)
    }
    fn contextual_rest_parameter(&self, _: NativeFunctionKey) -> Option<usize> {
        None
    }
    fn contains_binding(&self, _: NativeBindingKey) -> bool {
        false
    }
    fn function_parameters_in_capture(
        &self,
        key: NativeFunctionKey,
        _: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        self.function_parameters(key, queries)
    }
    fn function_declared_return_in_capture(
        &self,
        key: NativeFunctionKey,
        _: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        self.function_declared_return(key, queries)
    }
    fn function_inferred_return_in_capture(
        &self,
        key: NativeFunctionKey,
        _: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        self.function_inferred_return(key, queries)
    }
    fn contains_expression(&self, _: NativeExpressionKey) -> bool {
        false
    }
    fn is_context_sensitive(&self, _: NativeExpressionKey) -> bool {
        false
    }
    fn has_context_sensitive_parameters(&self, _: NativeExpressionKey) -> bool {
        false
    }
    fn is_expression_descendant(
        &self,
        expression: NativeExpressionKey,
        ancestor: NativeExpressionKey,
    ) -> bool {
        expression == ancestor
    }
    fn check_expression(
        &self,
        _: NativeExpressionKey,
        _: Option<&Type>,
        _: NativeCheckMode,
        _: &mut dyn TypeQueries,
    ) -> Type {
        panic!("native expression provider is not installed")
    }
    /// `check_expression`, and whether the checker may reuse the result for the
    /// same expression, mode and function frame instead of checking it again.
    fn check_reusable_expression(
        &self,
        key: NativeExpressionKey,
        expected: Option<&Type>,
        mode: NativeCheckMode,
        queries: &mut dyn TypeQueries,
    ) -> (Type, bool) {
        (self.check_expression(key, expected, mode, queries), false)
    }
    fn contextual_parameter_indices(&self, _: NativeFunctionKey) -> Vec<usize> {
        Vec::new()
    }
    fn function_parameters_in_context(
        &self,
        key: NativeFunctionKey,
        _: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        self.function_parameters(key, queries)
    }
    fn function_declared_return_in_context(
        &self,
        key: NativeFunctionKey,
        _: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        self.function_declared_return(key, queries)
    }
    fn function_inferred_return_in_context(
        &self,
        key: NativeFunctionKey,
        _: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        self.function_inferred_return(key, queries)
    }
    fn contains_function(&self, key: NativeFunctionKey) -> bool;
    fn function_lexical_parent(&self, _: NativeFunctionKey) -> Option<NativeFunctionKey> { None }
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
    fn function_outer_type_parameter_instances(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Type> { Vec::new() }
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
    PackArity {
        index: usize,
        actual_minimum: usize,
        actual_maximum: Option<usize>,
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
    /// Whether this exact contextual instance is final source-body work, usable
    /// only by its current native query, or from an expired query scope.
    fn contextual_function_check_state(&mut self, context: &ContextualFunctionContext) -> NativeFunctionCheckState;
    /// Includes captured lexical-parent provenance, without demanding returns.
    fn function_check_state(&mut self, function: &Type) -> NativeFunctionCheckState;
    fn global_environment_type(&mut self, environment: NativeEnvironmentKey) -> Result<Type, GlobalQueryError>;
    fn global_environment_origin(&mut self, environment: &Type) -> Option<NativeEnvironmentKey>;
    /// Whether strict runtime effects were requested (`--strict-standard-effects`).
    fn strict_runtime_effects(&mut self) -> bool { false }
    fn global_binding_type(&mut self, binding: NativeGlobalBindingKey) -> Result<Type, GlobalQueryError>;
    fn value_declaration_type(&mut self, declaration: NativeValueDeclarationKey) -> Result<Type, GlobalQueryError>;
    fn module_result_pack(&mut self, key: NativeModuleKey) -> Result<ValuePack, NativeProjectError>;
    fn module_value(&mut self, key: NativeModuleKey, policy: NativeLoaderPolicy) -> Result<Type, NativeProjectError>;
    fn require_pack(&mut self, key: NativeModuleKey, policy: NativeLoaderPolicy) -> Result<ValuePack, NativeProjectError>;

    fn class_definition_context(&mut self, owner: NativeClassDefinitionKey, class_type: &Type) -> Result<Type, NamedTypeError>;
    fn class_definition_value(&mut self, owner: NativeClassDefinitionKey, class_type: &Type, actual: &Type) -> Result<Type, NamedTypeError>;
    /// A class declared in several places, reopened on a value that is already the class or
    /// its definition; any other value is an ordinary definition value.
    fn class_definition_reopen(&mut self, owner: NativeClassDefinitionKey, class_type: &Type, actual: &Type) -> Result<Type, NamedTypeError>;
    fn class_definition_member_contract(&mut self, owner: NativeClassDefinitionKey, actual: &Type) -> Option<Type>;
    fn assertion_overlap(&mut self, source: &Type, target: &Type) -> bool;
    /// The language service's members of a value: each string key a read of it accepts, with the
    /// type that read returns.
    fn type_members(&mut self, _: &Type) -> Vec<(String, Type)> { Vec::new() }
    /// TypeScript's comparable relation, in either direction, without widening: whether `==` can
    /// equate the two (`isTypeEqualityComparableTo`).
    fn types_comparable(&mut self, _: &Type, _: &Type) -> bool { true }
    fn exclude_type(&mut self, source: &Type, excluded: &Type) -> Type;
    fn exclude_pack(&mut self, source: &ValuePack, fixed: &[Type], tail: Option<&Type>) -> ValuePack;
    fn signature_type(&mut self, declaration: NativeSignatureKey) -> Type;
    /// Pure original-callee metadata; never infers a callable's return or body.
    fn function_intrinsic(&mut self, function: &Type) -> Option<NativeIntrinsicSite>;
    fn signature_group_type(&mut self, declarations: &[NativeSignatureKey]) -> Type;
    fn call_signature_types(&mut self, function: &Type) -> Vec<Type>;
    fn function_implementation_type(&mut self, function: &Type) -> Option<Type>;
    fn signature_implementation_compatible(&mut self, declaration: NativeSignatureKey, implementation: &Type) -> bool;
    fn pack_function(&mut self, function: &PackFunctionType) -> Type;
    fn declared_result_pack(&mut self, pack: &ValuePack) -> ValuePack;
    fn return_pack_has_fixed_cardinality(&mut self, pack: &ValuePack) -> bool;
    fn return_pack_with_cardinality(&mut self, declared: &ValuePack, actual: &ValuePack) -> ValuePack;
    fn function_declared_scalar_return(&mut self, function: &Type) -> Option<Type>;
    fn call_native_arguments_scalar(
        &mut self,
        callee: &Type,
        call: Option<NativeExpressionKey>,
        arguments: &[PackArgument],
        type_arguments: &[Type],
        expected_return: Option<&Type>,
    ) -> CallResult;
    fn call_native_arguments(
        &mut self,
        callee: &Type,
        call: Option<NativeExpressionKey>,
        arguments: &[PackArgument],
        type_arguments: &[Type],
        expected: Option<&ValuePack>,
    ) -> PackCallResult;
    fn call_native_arguments_scalar_in_frame(
        &mut self,
        callee: &Type,
        call: Option<NativeExpressionKey>,
        arguments: &[PackArgument],
        type_arguments: &[Type],
        expected_return: Option<&Type>,
        caller: NativeCallerFrame,
    ) -> CallResult;
    fn call_native_arguments_in_frame(
        &mut self,
        callee: &Type,
        call: Option<NativeExpressionKey>,
        arguments: &[PackArgument],
        type_arguments: &[Type],
        expected: Option<&ValuePack>,
        caller: NativeCallerFrame,
    ) -> PackCallResult;
    fn call_arguments_pack(
        &mut self,
        callee: &Type,
        call: Option<NativeExpressionKey>,
        arguments: &[PackArgument],
        type_arguments: &[Type],
        expected: Option<&ValuePack>,
    ) -> PackCallResult {
        self.call_native_arguments(callee, call, arguments, type_arguments, expected)
    }
    fn call_pack(
        &mut self,
        callee: &Type,
        arguments: &[Type],
        type_arguments: &[Type],
        expected: Option<&ValuePack>,
    ) -> PackCallResult;
    fn call_expression_pack(
        &mut self,
        callee: &Type,
        call: NativeExpressionKey,
        arguments: &[NativeExpressionKey],
        type_arguments: &[Type],
        expected: Option<&ValuePack>,
    ) -> PackCallResult;
    fn function_result_pack(&mut self, function: &Type) -> Option<ValuePack>;
    fn signature_rest_pack(&mut self, function: &Type) -> Option<ValuePack>;
    fn contextual_rest_pack(
        &mut self,
        context: &ContextualFunctionContext,
        fixed_count: usize,
    ) -> ValuePack;
    fn contextual_return_pack(&mut self, context: &ContextualFunctionContext) -> ValuePack;
    fn captured_rest_pack(&mut self, context: &CapturedFunctionContext) -> Option<ValuePack>;
    fn instantiate_pack_in_capture(
        &mut self,
        context: &CapturedFunctionContext,
        pack: &ValuePack,
    ) -> ValuePack;
    fn table_type(&mut self, table: &TableType) -> Result<Type, TableError>;
    fn table_type_in_context(
        &mut self,
        table: &TableType,
        expected: &Type,
    ) -> Result<Type, TableError>;
    fn table_with_sequence(
        &mut self,
        table: &TableType,
        start: i64,
        pack: &ValuePack,
        expected: Option<&Type>,
    ) -> Result<Type, TableError>;
    fn table_with_sequence_origin(
        &mut self,
        table: &TableType,
        start: i64,
        pack: &ValuePack,
        origin: TableFieldOrigin,
        expected: Option<&Type>,
    ) -> Result<Type, TableError>;
    fn table_sequence_context(&mut self, expected: &Type, start: i64) -> Option<ValuePack>;
    fn table_lookup(&mut self, table: &Type, key: &TableKey) -> Result<Type, TableError>;
    fn table_write(&mut self, table: &Type, key: &TableKey, value: &Type)
    -> Result<(), TableError>;
    /// Validate an original store without turning future computed writes into
    /// prospective members. Reached value evolution remains native flow work.
    fn table_write_at_flow(&mut self, _: &dyn NativeFlowProvider, _: NativeFlowKey, table: &Type, key: &TableKey, value: &Type) -> Result<(), TableError> { self.table_write(table, key, value) }
    fn definition_field_write(&mut self, table: &Type, key: &LuaKey, value: &Type) -> Result<Type, TableError>;
    fn table_value_context(&mut self, table: &Type, key: &TableKey) -> Option<Type>;
    fn array_type(&mut self, element: &Type) -> Type;
    fn retain_pack(&mut self, pack: &ValuePack) -> ValuePack;
    fn inspect_pack(&mut self, pack: &ValuePack) -> Option<Vec<PackAlternative>>;
    /// Display-only classification retains whole top/symbolic carriers when
    /// ordinary row inspection is unavailable. Semantic operations use the pack.
    fn inspect_pack_view(&mut self, pack: &ValuePack) -> PackInspection;
    fn pack_slot(&mut self, pack: &ValuePack, index: usize) -> Type;
    fn pack_contextual_slot(&mut self, pack: &ValuePack, index: usize) -> Option<Type>;
    fn adjust_pack(
        &mut self,
        pack: &ValuePack,
        count: usize,
    ) -> Result<ValuePack, PackAdjustmentError>;
    fn concat_pack(&mut self, prefix: &[Type], tail: &ValuePack) -> ValuePack;
    fn pack_contextual_tail(&mut self, expected: &ValuePack, prefix: &[Type]) -> Option<ValuePack>;
    fn join_packs(&mut self, packs: &[ValuePack]) -> ValuePack;
    fn aggregate_result_packs(
        &mut self,
        packs: &[ValuePack],
        has_fallthrough: bool,
        context: Option<&ValuePack>,
    ) -> ValuePack;
    fn pack_assignable(&mut self, actual: &ValuePack, declared: &ValuePack) -> bool;
    fn pack_allows_implicit_return(&mut self, pack: &ValuePack) -> bool;
    fn pack_with_implicit_empty(&mut self, pack: &ValuePack) -> ValuePack;
    fn captured_function_value(&mut self, context: &CapturedFunctionContext) -> Type;
    /// Actual implementation capability, including an ordinary declaration-space function.
    fn function_capture_context(&mut self, function: &Type) -> Option<CapturedFunctionContext>;
    /// Exact lexical creation frame retained by this function value.
    fn captured_function_parent(&mut self, context: &CapturedFunctionContext)
    -> Option<CapturedFunctionContext>;
    fn captured_function_context(&mut self, context: &CapturedFunctionContext)
    -> Option<ContextualFunctionContext>;
    fn native_function_key(&mut self, function: &Type) -> Option<NativeFunctionKey>;
    fn contextual_type_owner(&mut self, scope: NativeFunctionKey, input: NativeFlowKey) -> ContextualTypeOwner;
    fn contextual_owner_self_type(&mut self, owner: &Type) -> Option<Type>;
    fn instantiate_in_function(&mut self, function: &Type, declaration_type: &Type)
    -> Option<Type>;
    fn captured_parameter_type(
        &mut self,
        context: &CapturedFunctionContext,
        index: usize,
    ) -> Option<Type>;
    fn instantiate_in_capture(&mut self, context: &CapturedFunctionContext, ty: &Type) -> Type;
    fn captured_function(
        &mut self,
        function: NativeFunctionKey,
        bindings: &[(NativeBindingKey, Type)],
    ) -> Type {
        self.captured_function_with_parent(function, bindings, None)
    }
    /// `parent` must be attested against the original source's lexical owner.
    fn captured_function_with_parent(
        &mut self,
        function: NativeFunctionKey,
        bindings: &[(NativeBindingKey, Type)],
        parent: Option<&CapturedFunctionContext>,
    ) -> Type;
    fn captured_binding(
        &mut self,
        context: &CapturedFunctionContext,
        binding: NativeBindingKey,
    ) -> Option<Type>;
    fn current_captured_function(&mut self) -> Option<CapturedFunctionContext>;
    /// How many reusable argument results the checker holds; with
    /// `discard_argument_results_since`, forgets those recorded while a report
    /// that is then discarded was open, so their checks report again.
    fn argument_results_mark(&mut self) -> usize;
    fn discard_argument_results_since(&mut self, mark: usize);
    /// Select both original source frame options, including declaration-space
    /// absence. Frozen flow callbacks must not inherit an unrelated requester.
    fn with_native_function_frame(
        &mut self,
        frame: NativeCallerFrame,
        work: &mut dyn FnMut(&mut dyn TypeQueries),
    );
    fn with_captured_function(
        &mut self,
        context: &CapturedFunctionContext,
        work: &mut dyn FnMut(&mut dyn TypeQueries),
    );
    fn contextual_function_value(
        &mut self,
        function: &Type,
        expression: NativeExpressionKey,
        expected: &Type,
        mode: NativeCheckMode,
    ) -> Type;
    fn current_contextual_function(&mut self) -> Option<ContextualFunctionContext>;
    fn flow_alias_relation(&mut self, provider: &dyn NativeFlowProvider, left: NativeReferenceKey, left_at: NativeFlowKey, right: NativeValueKey, right_at: NativeFlowKey) -> NativeAliasRelation;
    fn flow_call_preservation(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, call: NativeValueKey, input: NativeFlowKey) -> NativeCallPreservation;
    /// Policy-aware standard-slot continuation, separate from a raw effect certificate.
    fn flow_standard_call_continuation(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, call: NativeValueKey, input: NativeFlowKey) -> NativeCallPreservation {
        self.flow_call_preservation(provider, reference, call, input)
    }
    fn flow_alias_intrinsic(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey) -> NativeAliasIntrinsic;
    fn flow_standard_operation(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey) -> NativeStandardOperation {
        match self.flow_alias_intrinsic(provider, value, input) {
            NativeAliasIntrinsic::Standard(site) => NativeStandardOperation::Standard(site),
            NativeAliasIntrinsic::Unknown => NativeStandardOperation::Ordinary,
            NativeAliasIntrinsic::Pending => NativeStandardOperation::Pending,
        }
    }
    /// Host-policy loader state for ordinary module typing, never physical proof.
    fn flow_standard_loader_state(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, reference: NativeReferenceKey) -> NativeFlowResult {
        self.flow_type(provider, at, reference, &Type::BooleanLiteral(false), &Type::BooleanLiteral(true))
    }
    fn with_contextual_function(
        &mut self,
        context: &ContextualFunctionContext,
        work: &mut dyn FnMut(&mut dyn TypeQueries),
    );
    /// An original value recipe at its frozen input, through the flow value
    /// cache (computed once per stable demand, as Bun memoises expressions).
    fn flow_value(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey) -> Type;
    fn flow_type(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        reference: NativeReferenceKey,
        declared: &Type,
        initial: &Type,
    ) -> NativeFlowResult;
    fn flow_capture_type(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        reference: NativeReferenceKey,
        declared: &Type,
        initial: &Type,
    ) -> NativeFlowResult;
    fn flow_reachable(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey) -> bool;
    fn flow_definitely_unreachable(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey) -> bool;
    fn flow_iterator_pack(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, frame: NativeReferenceKey) -> ValuePack;
    fn iterator_result(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, frame: NativeReferenceKey, index: usize) -> Type;
    fn iterator_result_with_status(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, frame: NativeReferenceKey, index: usize) -> NativeFlowResult;
    fn flow_iterator_projection(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, projection: NativeIteratorProjection, ordinary: &Type) -> NativeFlowResult;
    fn flow_pack_projection(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, projection: NativePackProjection, ordinary: &Type) -> NativeFlowResult;
    fn flow_correlated_pack(&mut self, provider: &dyn NativeFlowProvider, ordinary: &ValuePack, elements: &[Option<NativeFrozenReference>]) -> NativeFlowPackResult;
    fn contextual_function(
        &mut self,
        function: NativeFunctionKey,
        expression: NativeExpressionKey,
        expected: &Type,
        mode: NativeCheckMode,
    ) -> Type;
    fn contextual_parameter_type(
        &mut self,
        context: &ContextualFunctionContext,
        index: usize,
    ) -> Option<Type>;
    fn contextual_return_type(&mut self, context: &ContextualFunctionContext) -> Type;
    fn call_expression_native(
        &mut self,
        callee: &Type,
        call: NativeExpressionKey,
        arguments: &[NativeExpressionKey],
        type_arguments: &[Type],
        expected_return: Option<&Type>,
    ) -> CallResult;
    fn is_assignable(&mut self, source: &Type, target: &Type) -> bool;
    fn compare(&mut self, source: &Type, target: &Type) -> Relation;
    fn widen(&mut self, ty: &Type) -> Type;
    /// Native literal-kind metadata only; does not request signatures or members.
    fn is_primitive_literal(&mut self, ty: &Type) -> bool;
    /// Uses the native base-of-literal worker, including its union mapping.
    fn base_of_literal(&mut self, ty: &Type) -> Type;
    /// The annotation form of a literal, using TypeScript's regular-literal worker.
    fn regular_type(&mut self, ty: &Type) -> Type;
    /// Partition with the same Lua nil/false policy used by native graph flow.
    fn lua_truthiness_type(&mut self, ty: &Type, sense: bool) -> Type;
    fn union_reduced(&mut self, types: &[Type]) -> Type;
    fn runtime_value_union(&mut self, types: &[Type]) -> Type;
    fn property_type(&mut self, ty: &Type, name: &str) -> Option<Type>;
    fn property_type_checked(&mut self, ty: &Type, name: &str) -> Result<Option<Type>, TableError>;
    fn assignment_value(&mut self, declared: &Type, assigned: &Type) -> Type;
    /// Session-owned automatic declaration sentinel. Keep a separate original
    /// initial value when querying flow; this capability is not ordinary Any.
    fn auto_variable_constraint(&mut self) -> Type;
    fn is_auto_variable_constraint(&mut self, ty: &Type) -> bool;
    fn call(&mut self, callee: &Type, arguments: &[Type]) -> CallResult;
    /// `operator` applied to `receiver` (and `other`, for a binary operator) as
    /// `receiver`'s type declares it with `---@operator`; None if it declares none.
    fn operator_call(&mut self, operator: NativeOperator, receiver: &Type, other: Option<&Type>) -> Option<CallResult>;
    fn call_with_type_arguments(
        &mut self,
        callee: &Type,
        arguments: &[Type],
        type_arguments: &[Type],
        expected_return: Option<&Type>,
    ) -> CallResult;
    fn function_signature(&mut self, ty: &Type) -> Option<FunctionType>;
    fn inspect_type(&mut self, ty: &Type) -> Type;
    /// Exact backend recovery type, preserving its error identity across the owned boundary.
    fn error_type(&mut self) -> Type;
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
    fn named_type_reference(&mut self, declaration: NativeTypeDeclarationKey, arguments: &[Type]) -> Result<Type, NamedTypeError>;
    /// `checkTypeArgumentConstraints` for a written reference; resolution does not check them.
    fn named_type_constraint_error(&mut self, declaration: NativeTypeDeclarationKey, arguments: &[Type]) -> Option<NamedTypeError>;
    fn named_object_declaration(&mut self, ty: &Type) -> Option<NativeTypeDeclarationKey>;
    fn runtime_table_value(&mut self, declaration: NativeTypeDeclarationKey, actual: &Type) -> Result<Type, NamedTypeError>;
    fn runtime_table_declaration(&mut self, ty: &Type) -> Option<NativeTypeDeclarationKey>;
    fn runtime_table_schema(&mut self, ty: &Type) -> Option<Type>;
    fn named_declared_type(&mut self, declaration: NativeTypeDeclarationKey) -> Result<Type, NamedTypeError>;
    fn named_receiver_type(&mut self, class: &Type, receiver: &Type) -> Result<Type, NamedTypeError>;
    fn named_member_context(&mut self, member: NativeTypeMemberKey) -> Result<Option<Type>, NamedTypeError>;
    fn named_member_scope_owner(&mut self, member: NativeTypeMemberKey) -> Result<NativeTypeDeclarationKey, NamedTypeError>;
    fn named_member_owner(&mut self, member: NativeTypeMemberKey) -> Result<NamedMemberOwner, NamedTypeError>;
    fn validate_named_declaration(&mut self, declaration: NativeTypeDeclarationKey) -> Result<Type, NamedTypeError>;
    fn validate_named_member(&mut self, member: NativeTypeMemberKey) -> Result<Type, NamedTypeError>;
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
            checker.set_stack_limit(
                crate::CHECKER_STACK_BUDGET,
            );
            checker.enable_native_arrays();
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
            checker.set_stack_limit(
                crate::CHECKER_STACK_BUDGET,
            );
            checker.set_native_type_declaration_provider(&adapter);
            checker.set_native_globals_provider(&adapter);
            checker.set_native_project_provider(&adapter);
            checker.enable_native_arrays();
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
        self.with_session(|queries| {
            let result = queries.widen(ty);
            queries.inspect_type(&result)
        })
    }

    pub fn property_type(&self, ty: &Type, name: &str) -> Option<Type> {
        self.with_session(|queries| queries.property_type(ty, name).map(|ty| queries.inspect_type(&ty)))
    }

    /// The native checker's assignment transfer for a declared location,
    /// including literal widening of automatic declarations.
    pub fn assignment_value(&self, declared: &Type, assigned: &Type) -> Type {
        self.with_session(|queries| {
            let result = queries.assignment_value(declared, assigned);
            queries.inspect_type(&result)
        })
    }

    pub fn call(&self, callee: &Type, arguments: &[Type]) -> CallResult {
        self.with_session(|queries| {
            let result = queries.call(callee, arguments);
            describe_call_result(queries, result)
        })
    }

    pub fn call_with_type_arguments(
        &self,
        callee: &Type,
        arguments: &[Type],
        type_arguments: &[Type],
        expected_return: Option<&Type>,
    ) -> CallResult {
        self.with_session(|queries| {
            let result = queries.call_with_type_arguments(callee, arguments, type_arguments, expected_return);
            describe_call_result(queries, result)
        })
    }

    pub fn allows_implicit_return(&self, returned: &Type) -> bool {
        self.with_session(|queries| queries.allows_implicit_return(returned))
    }

    pub fn format(&self, ty: &Type) -> String {
        self.with_session(|queries| queries.format(ty))
    }
}

fn describe_call_result(queries: &mut dyn TypeQueries, mut result: CallResult) -> CallResult {
    result.return_type = queries.inspect_type(&result.return_type);
    for error in &mut result.errors {
        if let CallError::Argument { source, target, .. } | CallError::TypeArgument { source, target, .. } = error {
            *source = queries.inspect_type(source);
            *target = queries.inspect_type(target);
        }
    }
    result
}

struct SessionQueries<'a, 'p, 's> {
    checker: &'a mut Checker<'p, 's>,
    handles: &'a TypeHandles,
}

impl TypeQueries for SessionQueries<'_, '_, '_> {
    fn flow_value(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey) -> Type {
        let adapter = native_flow::FlowProviderAdapter { provider, handles: self.handles };
        let ty = self.checker.native_flow_value_cached(&adapter, value, input);
        export_type(self.checker, self.handles, ty)
    }
    fn contextual_function_check_state(&mut self, context: &ContextualFunctionContext) -> NativeFunctionCheckState {
        self.checker.native_contextual_function_check_state(self.handles.resolve_context(context))
    }
    fn function_check_state(&mut self, function: &Type) -> NativeFunctionCheckState {
        match function {
            Type::SessionType(reference) => self.checker.native_function_check_state(self.handles.resolve(*reference)),
            Type::NativeFunction(key) => {
                if !self.checker.contains_native_function(core_key(*key)) {
                    std::panic::panic_any(InvalidNativeFunctionKey(*key));
                }
                NativeFunctionCheckState::Final
            }
            Type::NativeSignature(key) => {
                if !self.checker.contains_native_signature(core_signature_key(*key)) {
                    std::panic::panic_any(InvalidNativeSignatureKey(*key));
                }
                NativeFunctionCheckState::Final
            }
            Type::Union(parts) | Type::Intersection(parts) => {
                let mut state = NativeFunctionCheckState::Final;
                for part in parts {
                    match self.function_check_state(part) {
                        NativeFunctionCheckState::Expired => return NativeFunctionCheckState::Expired,
                        NativeFunctionCheckState::Provisional => state = NativeFunctionCheckState::Provisional,
                        NativeFunctionCheckState::Final => {}
                    }
                }
                state
            }
            _ => NativeFunctionCheckState::Final,
        }
    }
    fn global_binding_type(&mut self, binding: NativeGlobalBindingKey) -> Result<Type, GlobalQueryError> { self.global_binding_type_native(binding) }
    fn global_environment_type(&mut self, environment: NativeEnvironmentKey) -> Result<Type, GlobalQueryError> { self.global_environment_type_native(environment) }
    fn global_environment_origin(&mut self, environment: &Type) -> Option<NativeEnvironmentKey> { self.global_environment_origin_native(environment) }
    fn strict_runtime_effects(&mut self) -> bool { self.checker.native_strict_runtime_effects() }
    fn value_declaration_type(&mut self, declaration: NativeValueDeclarationKey) -> Result<Type, GlobalQueryError> { self.value_declaration_type_native(declaration) }
    fn module_result_pack(&mut self, key: NativeModuleKey) -> Result<ValuePack, NativeProjectError> { self.module_result_pack_native(key) }
    fn module_value(&mut self, key: NativeModuleKey, policy: NativeLoaderPolicy) -> Result<Type, NativeProjectError> { self.module_value_native(key, policy) }
    fn require_pack(&mut self, key: NativeModuleKey, policy: NativeLoaderPolicy) -> Result<ValuePack, NativeProjectError> { self.require_pack_native(key, policy) }

    fn assertion_overlap(&mut self, source: &Type, target: &Type) -> bool { self.assertion_overlap_native(source, target) }
    fn type_members(&mut self, ty: &Type) -> Vec<(String, Type)> { self.type_members_native(ty) }
    fn types_comparable(&mut self, left: &Type, right: &Type) -> bool { self.types_comparable_native(left, right) }
    fn exclude_type(&mut self, source: &Type, excluded: &Type) -> Type { self.exclude_type_native(source, excluded) }
    fn exclude_pack(&mut self, source: &ValuePack, fixed: &[Type], tail: Option<&Type>) -> ValuePack { self.exclude_pack_native(source, fixed, tail) }
    fn signature_type(&mut self, declaration: NativeSignatureKey) -> Type { self.signature_type_native(declaration) }
    fn function_intrinsic(&mut self, function: &Type) -> Option<NativeIntrinsicSite> {
        let function = import_type(self.checker, self.handles, function);
        self.checker.native_function_intrinsic(function).map(native_signatures::source_intrinsic_site)
    }
    fn signature_group_type(&mut self, declarations: &[NativeSignatureKey]) -> Type { self.signature_group_type_native(declarations) }
    fn call_signature_types(&mut self, function: &Type) -> Vec<Type> {
        let function = import_type(self.checker, self.handles, function);
        self.checker.native_call_signature_types(function).into_iter().map(|ty| self.handles.retain(ty)).collect()
    }
    fn function_implementation_type(&mut self, function: &Type) -> Option<Type> {
        let function = import_type(self.checker, self.handles, function);
        let ty = self.checker.native_implementation_type(function)?;
        Some(self.handles.retain(ty))
    }
    fn signature_implementation_compatible(&mut self, declaration: NativeSignatureKey, implementation: &Type) -> bool {
        let implementation = import_type(self.checker, self.handles, implementation);
        self.checker.native_signature_implementation_compatible(core_signature_key(declaration), implementation)
    }
    fn pack_function(&mut self, function: &PackFunctionType) -> Type {
        self.pack_function_native(function)
    }
    fn declared_result_pack(&mut self, pack: &ValuePack) -> ValuePack {
        self.declared_result_pack_native(pack)
    }
    fn return_pack_has_fixed_cardinality(&mut self, pack: &ValuePack) -> bool {
        self.return_pack_has_fixed_cardinality_native(pack)
    }
    fn return_pack_with_cardinality(&mut self, declared: &ValuePack, actual: &ValuePack) -> ValuePack {
        self.return_pack_with_cardinality_native(declared, actual)
    }
    fn function_declared_scalar_return(&mut self, function: &Type) -> Option<Type> {
        self.function_declared_scalar_return_native(function)
    }
    fn call_native_arguments_scalar(
        &mut self,
        callee: &Type,
        call: Option<NativeExpressionKey>,
        arguments: &[PackArgument],
        type_arguments: &[Type],
        expected_return: Option<&Type>,
    ) -> CallResult {
        self.call_native_arguments_scalar_owned(callee, call, arguments, type_arguments, expected_return)
    }
    fn call_native_arguments(
        &mut self,
        callee: &Type,
        call: Option<NativeExpressionKey>,
        arguments: &[PackArgument],
        type_arguments: &[Type],
        expected: Option<&ValuePack>,
    ) -> PackCallResult {
        self.call_native_arguments_owned(callee, call, arguments, type_arguments, expected)
    }
    fn call_native_arguments_scalar_in_frame(
        &mut self,
        callee: &Type,
        call: Option<NativeExpressionKey>,
        arguments: &[PackArgument],
        type_arguments: &[Type],
        expected_return: Option<&Type>,
        caller: NativeCallerFrame,
    ) -> CallResult {
        let contextual = caller.contextual.as_ref().map(|context| self.handles.resolve_context(context));
        let captured = caller.captured.as_ref().map(|context| self.handles.resolve_capture(context));
        let handles = self.handles;
        self.checker.with_native_function_frame(contextual, captured, |checker| {
            SessionQueries { checker, handles }.call_native_arguments_scalar_owned(callee, call, arguments, type_arguments, expected_return)
        })
    }
    fn call_native_arguments_in_frame(
        &mut self,
        callee: &Type,
        call: Option<NativeExpressionKey>,
        arguments: &[PackArgument],
        type_arguments: &[Type],
        expected: Option<&ValuePack>,
        caller: NativeCallerFrame,
    ) -> PackCallResult {
        let contextual = caller.contextual.as_ref().map(|context| self.handles.resolve_context(context));
        let captured = caller.captured.as_ref().map(|context| self.handles.resolve_capture(context));
        let handles = self.handles;
        self.checker.with_native_function_frame(contextual, captured, |checker| {
            SessionQueries { checker, handles }.call_native_arguments_owned(callee, call, arguments, type_arguments, expected)
        })
    }
    fn call_pack(
        &mut self,
        callee: &Type,
        arguments: &[Type],
        type_arguments: &[Type],
        expected: Option<&ValuePack>,
    ) -> PackCallResult {
        self.call_pack_native(callee, arguments, type_arguments, expected)
    }
    fn call_expression_pack(
        &mut self,
        callee: &Type,
        call: NativeExpressionKey,
        arguments: &[NativeExpressionKey],
        type_arguments: &[Type],
        expected: Option<&ValuePack>,
    ) -> PackCallResult {
        self.call_expression_pack_native(callee, call, arguments, type_arguments, expected)
    }
    fn function_result_pack(&mut self, function: &Type) -> Option<ValuePack> {
        self.function_result_pack_native(function)
    }
    fn signature_rest_pack(&mut self, function: &Type) -> Option<ValuePack> {
        self.signature_rest_pack_native(function)
    }
    fn contextual_rest_pack(
        &mut self,
        context: &ContextualFunctionContext,
        fixed_count: usize,
    ) -> ValuePack {
        self.contextual_rest_pack_native(context, fixed_count)
    }
    fn contextual_return_pack(&mut self, context: &ContextualFunctionContext) -> ValuePack {
        self.contextual_return_pack_native(context)
    }
    fn captured_rest_pack(&mut self, context: &CapturedFunctionContext) -> Option<ValuePack> {
        self.captured_rest_pack_native(context)
    }
    fn instantiate_pack_in_capture(
        &mut self,
        context: &CapturedFunctionContext,
        pack: &ValuePack,
    ) -> ValuePack {
        self.instantiate_pack_in_capture_native(context, pack)
    }
    fn table_type(&mut self, table: &TableType) -> Result<Type, TableError> {
        self.table_type_native(table)
    }
    fn table_type_in_context(
        &mut self,
        table: &TableType,
        expected: &Type,
    ) -> Result<Type, TableError> {
        self.table_type_in_context_native(table, expected)
    }
    fn table_with_sequence(
        &mut self,
        table: &TableType,
        start: i64,
        pack: &ValuePack,
        expected: Option<&Type>,
    ) -> Result<Type, TableError> {
        self.table_with_sequence_native(table, start, pack, TableFieldOrigin::Inferred, expected)
    }
    fn table_with_sequence_origin(
        &mut self,
        table: &TableType,
        start: i64,
        pack: &ValuePack,
        origin: TableFieldOrigin,
        expected: Option<&Type>,
    ) -> Result<Type, TableError> {
        self.table_with_sequence_native(table, start, pack, origin, expected)
    }
    fn table_sequence_context(&mut self, expected: &Type, start: i64) -> Option<ValuePack> {
        self.table_sequence_context_native(expected, start)
    }
    fn table_lookup(&mut self, table: &Type, key: &TableKey) -> Result<Type, TableError> {
        self.table_lookup_native(table, key)
    }
    fn table_write(
        &mut self,
        table: &Type,
        key: &TableKey,
        value: &Type,
    ) -> Result<(), TableError> {
        self.table_write_native(table, key, value)
    }
    fn table_write_at_flow(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, table: &Type, key: &TableKey, value: &Type) -> Result<(), TableError> {
        self.table_write_at_flow_native(provider, at, table, key, value)
    }
    fn table_value_context(&mut self, table: &Type, key: &TableKey) -> Option<Type> {
        self.table_value_context_native(table, key)
    }
    fn definition_field_write(&mut self, table: &Type, key: &LuaKey, value: &Type) -> Result<Type, TableError> {
        self.definition_field_write_native(table, key, value)
    }
    fn array_type(&mut self, element: &Type) -> Type {
        self.array_type_native(element)
    }
    fn retain_pack(&mut self, pack: &ValuePack) -> ValuePack {
        self.retain_pack_native(pack)
    }
    fn inspect_pack(&mut self, pack: &ValuePack) -> Option<Vec<PackAlternative>> {
        self.inspect_pack_native(pack)
    }
    fn inspect_pack_view(&mut self, pack: &ValuePack) -> PackInspection {
        self.inspect_pack_view_native(pack)
    }
    fn pack_slot(&mut self, pack: &ValuePack, index: usize) -> Type {
        self.pack_slot_native(pack, index)
    }
    fn pack_contextual_slot(&mut self, pack: &ValuePack, index: usize) -> Option<Type> {
        self.pack_contextual_slot_native(pack, index)
    }
    fn adjust_pack(
        &mut self,
        pack: &ValuePack,
        count: usize,
    ) -> Result<ValuePack, PackAdjustmentError> {
        self.adjust_pack_native(pack, count)
    }
    fn concat_pack(&mut self, prefix: &[Type], tail: &ValuePack) -> ValuePack {
        self.concat_pack_native(prefix, tail)
    }
    fn pack_contextual_tail(&mut self, expected: &ValuePack, prefix: &[Type]) -> Option<ValuePack> {
        self.pack_contextual_tail_native(expected, prefix)
    }
    fn join_packs(&mut self, packs: &[ValuePack]) -> ValuePack {
        self.join_packs_native(packs)
    }
    fn aggregate_result_packs(
        &mut self,
        packs: &[ValuePack],
        has_fallthrough: bool,
        context: Option<&ValuePack>,
    ) -> ValuePack {
        self.aggregate_result_packs_native(packs, has_fallthrough, context)
    }
    fn pack_assignable(&mut self, actual: &ValuePack, declared: &ValuePack) -> bool {
        self.pack_assignable_native(actual, declared)
    }
    fn pack_allows_implicit_return(&mut self, pack: &ValuePack) -> bool {
        self.pack_allows_implicit_return_native(pack)
    }
    fn pack_with_implicit_empty(&mut self, pack: &ValuePack) -> ValuePack {
        self.pack_with_implicit_empty_native(pack)
    }
    fn captured_function_value(&mut self, context: &CapturedFunctionContext) -> Type {
        let context = self.handles.resolve_capture(context);
        let ty = self.checker.native_captured_function_value(context);
        self.handles.retain(ty)
    }
    fn function_capture_context(&mut self, function: &Type) -> Option<CapturedFunctionContext> {
        let function = import_type(self.checker, self.handles, function);
        self.checker
            .native_function_capture_context(function)
            .map(|context| self.handles.retain_capture(context))
    }
    fn captured_function_parent(
        &mut self,
        context: &CapturedFunctionContext,
    ) -> Option<CapturedFunctionContext> {
        let context = self.handles.resolve_capture(context);
        self.checker
            .native_captured_function_parent(context)
            .map(|parent| self.handles.retain_capture(parent))
    }
    fn captured_function_context(
        &mut self,
        context: &CapturedFunctionContext,
    ) -> Option<ContextualFunctionContext> {
        let context = self.handles.resolve_capture(context);
        self.checker
            .native_captured_function_context(context)
            .map(|context| self.handles.retain_context(context))
    }
    fn native_function_key(&mut self, function: &Type) -> Option<NativeFunctionKey> {
        let function = import_type(self.checker, self.handles, function);
        let signature = self.checker.native_implementation_signature(function)?;
        self.checker
            .sig_native_decl(signature)
            .map(|(key, _)| source_key(key))
    }
    fn contextual_type_owner(&mut self, scope: NativeFunctionKey, input: NativeFlowKey) -> ContextualTypeOwner {
        use bun_sema::check::native_named::NativeContextualTypeOwner as Core;
        match self.checker.native_contextual_type_owner(core_key(scope), input) {
            Core::Pending => ContextualTypeOwner::Pending,
            Core::Absent => ContextualTypeOwner::Absent,
            Core::Owner(owner) => ContextualTypeOwner::Owner(self.handles.retain(owner)),
        }
    }
    fn contextual_owner_self_type(&mut self, owner: &Type) -> Option<Type> {
        let owner = import_type(self.checker, self.handles, owner);
        let ty = self.checker.native_contextual_owner_self_type(owner)?;
        Some(self.handles.retain(ty))
    }
    fn instantiate_in_function(
        &mut self,
        function: &Type,
        declaration_type: &Type,
    ) -> Option<Type> {
        let function = import_type(self.checker, self.handles, function);
        let signature = self.checker.native_implementation_signature(function)?;
        let (_, mapper) = self.checker.sig_native_decl(signature)?;
        let ty = import_type(self.checker, self.handles, declaration_type);
        let ty = self.checker.instantiate(ty, mapper);
        Some(export_type(self.checker, self.handles, ty))
    }
    fn captured_parameter_type(
        &mut self,
        context: &CapturedFunctionContext,
        index: usize,
    ) -> Option<Type> {
        let context = self.handles.resolve_capture(context);
        self.checker
            .native_capture_parameter_type(context, index)
            .map(|ty| export_type(self.checker, self.handles, ty))
    }
    fn instantiate_in_capture(&mut self, context: &CapturedFunctionContext, ty: &Type) -> Type {
        let context = self.handles.resolve_capture(context);
        let ty = import_type(self.checker, self.handles, ty);
        let ty = self.checker.instantiate_in_native_capture(context, ty);
        export_type(self.checker, self.handles, ty)
    }
    fn captured_function_with_parent(
        &mut self,
        function: NativeFunctionKey,
        bindings: &[(NativeBindingKey, Type)],
        parent: Option<&CapturedFunctionContext>,
    ) -> Type {
        import_type(self.checker, self.handles, &Type::NativeFunction(function));
        let mut environment = Vec::new();
        for (binding, ty) in bindings {
            self.validate_binding(*binding);
            environment.push(NativeCapturedBinding {
                declaration: core_binding_key(*binding),
                ty: import_type(self.checker, self.handles, ty),
            });
        }
        let ty = self
            .checker
            .native_captured_function_with_parent(
                core_key(function),
                &environment,
                parent.map(|parent| self.handles.resolve_capture(parent)),
            )
            .unwrap_or_else(|| std::panic::panic_any(InvalidNativeFunctionKey(function)));
        self.handles.retain(ty)
    }
    fn captured_binding(
        &mut self,
        context: &CapturedFunctionContext,
        binding: NativeBindingKey,
    ) -> Option<Type> {
        let context = self.handles.resolve_capture(context);
        self.validate_binding(binding);
        self.checker
            .captured_binding_type(context, core_binding_key(binding))
            .map(|ty| export_type(self.checker, self.handles, ty))
    }
    fn current_captured_function(&mut self) -> Option<CapturedFunctionContext> {
        self.checker
            .current_native_captured_function()
            .map(|context| self.handles.retain_capture(context))
    }
    fn argument_results_mark(&mut self) -> usize {
        self.checker.native_argument_results_mark()
    }
    fn discard_argument_results_since(&mut self, mark: usize) {
        self.checker.discard_native_argument_results_since(mark);
    }
    fn with_native_function_frame(
        &mut self,
        frame: NativeCallerFrame,
        work: &mut dyn FnMut(&mut dyn TypeQueries),
    ) {
        let contextual = frame.contextual.as_ref().map(|context| self.handles.resolve_context(context));
        let captured = frame.captured.as_ref().map(|context| self.handles.resolve_capture(context));
        let handles = self.handles;
        self.checker.with_native_function_frame(contextual, captured, |checker| {
            work(&mut SessionQueries { checker, handles });
        });
    }
    fn with_captured_function(
        &mut self,
        context: &CapturedFunctionContext,
        work: &mut dyn FnMut(&mut dyn TypeQueries),
    ) {
        let context = self.handles.resolve_capture(context);
        let handles = self.handles;
        self.checker
            .with_native_capture_context(context, |checker| {
                work(&mut SessionQueries { checker, handles })
            });
    }
    fn contextual_function_value(
        &mut self,
        function: &Type,
        expression: NativeExpressionKey,
        expected: &Type,
        mode: NativeCheckMode,
    ) -> Type {
        self.validate_expression(expression);
        let function = import_type(self.checker, self.handles, function);
        let expected = import_type(self.checker, self.handles, expected);
        let ty = self
            .checker
            .native_contextual_function_value(
                function,
                core_expression_key(expression),
                expected,
                mode,
            )
            .unwrap_or(function);
        self.handles.retain(ty)
    }
    fn with_contextual_function(
        &mut self,
        context: &ContextualFunctionContext,
        work: &mut dyn FnMut(&mut dyn TypeQueries),
    ) {
        let context = self.handles.resolve_context(context);
        let handles = self.handles;
        self.checker
            .with_native_contextual_function_context(context, |checker| {
                work(&mut SessionQueries { checker, handles })
            });
    }
    fn current_contextual_function(&mut self) -> Option<ContextualFunctionContext> {
        self.checker
            .current_native_function_context()
            .map(|context| self.handles.retain_context(context))
    }
    fn flow_type(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        reference: NativeReferenceKey,
        declared: &Type,
        initial: &Type,
    ) -> NativeFlowResult {
        let declared = import_type(self.checker, self.handles, declared);
        let initial = import_type(self.checker, self.handles, initial);
        let adapter = native_flow::FlowProviderAdapter {
            provider,
            handles: self.handles,
        };
        let result = self
            .checker
            .native_flow_type(&adapter, at, reference, declared, initial);
        NativeFlowResult {
            ty: export_type(self.checker, self.handles, result.ty),
            incomplete: result.incomplete,
            unresolved_auto: result.unresolved_auto,
        }
    }
    fn flow_alias_relation(&mut self, provider: &dyn NativeFlowProvider, left: NativeReferenceKey, left_at: NativeFlowKey, right: NativeValueKey, right_at: NativeFlowKey) -> NativeAliasRelation {
        let adapter = native_flow::FlowProviderAdapter { provider, handles: self.handles };
        self.checker.native_flow_alias_relation(&adapter, left, left_at, right, right_at)
    }
    fn flow_call_preservation(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, call: NativeValueKey, input: NativeFlowKey) -> NativeCallPreservation {
        let adapter = native_flow::FlowProviderAdapter { provider, handles: self.handles };
        self.checker.native_flow_call_preservation(&adapter, reference, call, input)
    }
    fn flow_standard_call_continuation(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, call: NativeValueKey, input: NativeFlowKey) -> NativeCallPreservation {
        let adapter = native_flow::FlowProviderAdapter { provider, handles: self.handles };
        self.checker.native_flow_standard_call_continuation(&adapter, reference, call, input)
    }
    fn flow_alias_intrinsic(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey) -> NativeAliasIntrinsic {
        let adapter = native_flow::FlowProviderAdapter { provider, handles: self.handles };
        match self.checker.native_flow_alias_intrinsic(&adapter, value, input) {
            bun_sema::check::native_aliases::NativeAliasIntrinsic::Standard(site) => NativeAliasIntrinsic::Standard(native_signatures::source_intrinsic_site(site)),
            bun_sema::check::native_aliases::NativeAliasIntrinsic::Unknown => NativeAliasIntrinsic::Unknown,
            bun_sema::check::native_aliases::NativeAliasIntrinsic::Pending => NativeAliasIntrinsic::Pending,
        }
    }
    fn flow_standard_operation(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey) -> NativeStandardOperation {
        let adapter = native_flow::FlowProviderAdapter { provider, handles: self.handles };
        match self.checker.native_flow_standard_operation(&adapter, value, input) {
            bun_sema::check::native_aliases::NativeStandardOperation::Standard(site) => NativeStandardOperation::Standard(native_signatures::source_intrinsic_site(site)),
            bun_sema::check::native_aliases::NativeStandardOperation::Ordinary => NativeStandardOperation::Ordinary,
            bun_sema::check::native_aliases::NativeStandardOperation::Pending => NativeStandardOperation::Pending,
        }
    }
    fn flow_standard_loader_state(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, reference: NativeReferenceKey) -> NativeFlowResult {
        let adapter = native_flow::FlowProviderAdapter { provider, handles: self.handles };
        let result = self.checker.native_flow_standard_loader_state(&adapter, at, reference);
        // This query returns a boolean analysis state. Regular literal identity
        // must not turn its true/false value into an opaque runtime type handle.
        NativeFlowResult { ty: export_type_with_literal_policy(self.checker, self.handles, result.ty, false), incomplete: result.incomplete, unresolved_auto: result.unresolved_auto }
    }
    fn flow_capture_type(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        reference: NativeReferenceKey,
        declared: &Type,
        initial: &Type,
    ) -> NativeFlowResult {
        let declared = import_type(self.checker, self.handles, declared);
        let initial = import_type(self.checker, self.handles, initial);
        let adapter = native_flow::FlowProviderAdapter { provider, handles: self.handles };
        let result = self.checker.native_flow_capture_type(&adapter, at, reference, declared, initial);
        NativeFlowResult {
            ty: export_type(self.checker, self.handles, result.ty),
            incomplete: result.incomplete,
            unresolved_auto: result.unresolved_auto,
        }
    }
    fn flow_reachable(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey) -> bool {
        let adapter = native_flow::FlowProviderAdapter {
            provider,
            handles: self.handles,
        };
        self.checker.native_flow_reachable(&adapter, at)
    }
    fn flow_definitely_unreachable(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey) -> bool {
        self.checker.native_flow_definitely_unreachable(&native_flow::FlowProviderAdapter { provider, handles: self.handles }, at)
    }
    fn flow_iterator_pack(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, frame: NativeReferenceKey) -> ValuePack {
        self.flow_iterator_pack_native(provider, at, frame)
    }
    fn iterator_result(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, frame: NativeReferenceKey, index: usize) -> Type {
        self.iterator_result_native(provider, at, frame, index)
    }
    fn iterator_result_with_status(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, frame: NativeReferenceKey, index: usize) -> NativeFlowResult {
        self.iterator_result_with_status_native(provider, at, frame, index)
    }
    fn flow_iterator_projection(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, projection: NativeIteratorProjection, ordinary: &Type) -> NativeFlowResult {
        self.flow_iterator_projection_native(provider, at, projection, ordinary)
    }
    fn flow_pack_projection(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, projection: NativePackProjection, ordinary: &Type) -> NativeFlowResult {
        self.flow_pack_projection_native(provider, at, projection, ordinary)
    }
    fn flow_correlated_pack(&mut self, provider: &dyn NativeFlowProvider, ordinary: &ValuePack, elements: &[Option<NativeFrozenReference>]) -> NativeFlowPackResult {
        self.flow_correlated_pack_native(provider, ordinary, elements)
    }
    fn contextual_function(
        &mut self,
        function: NativeFunctionKey,
        expression: NativeExpressionKey,
        expected: &Type,
        mode: NativeCheckMode,
    ) -> Type {
        let ordinary = import_type(self.checker, self.handles, &Type::NativeFunction(function));
        self.validate_expression(expression);
        let expected = import_type(self.checker, self.handles, expected);
        let ty = self
            .checker
            .native_contextual_function(
                core_key(function),
                core_expression_key(expression),
                expected,
                mode,
            )
            .unwrap_or(ordinary);
        self.handles.retain(ty)
    }
    fn contextual_parameter_type(
        &mut self,
        context: &ContextualFunctionContext,
        index: usize,
    ) -> Option<Type> {
        let native = self.handles.resolve_context(context);
        self.checker
            .contextual_parameter_type(native, index)
            .map(|ty| export_type(self.checker, self.handles, ty))
    }
    fn contextual_return_type(&mut self, context: &ContextualFunctionContext) -> Type {
        let native = self.handles.resolve_context(context);
        let ty = self.checker.native_contextual_return_type(native);
        export_type(self.checker, self.handles, ty)
    }
    fn call_expression_native(
        &mut self,
        callee: &Type,
        call: NativeExpressionKey,
        arguments: &[NativeExpressionKey],
        type_arguments: &[Type],
        expected_return: Option<&Type>,
    ) -> CallResult {
        self.validate_expression(call);
        for argument in arguments {
            self.validate_expression(*argument);
        }
        let callee = import_type(self.checker, self.handles, callee);
        let arguments: Vec<_> = arguments.iter().copied().map(core_expression_key).collect();
        let type_arguments: Vec<_> = type_arguments
            .iter()
            .map(|ty| import_type(self.checker, self.handles, ty))
            .collect();
        let expected_return = expected_return.map(|ty| import_type(self.checker, self.handles, ty));
        let result = self.checker.call_expression_native(
            callee,
            core_expression_key(call),
            &arguments,
            &type_arguments,
            expected_return,
        );
        self.export_call_result(result)
    }
    fn inspect_type(&mut self, ty: &Type) -> Type {
        let id = import_type(self.checker, self.handles, ty);
        if matches!(self.checker.data(id), TypeData::NativeTable(table) if table.environment_origin.is_some()) {
            return self.handles.retain(id);
        }
        if matches!(self.checker.data(id), TypeData::NativeRuntimeTop { .. }) {
            return export_type_with_literal_policy(self.checker, self.handles, id, false);
        }
        let signatures = self.checker.signatures(id, false);
        if signatures.len() == 1 {
            Type::Function(export_signature_with_literal_policy(self.checker, self.handles, signatures[0], false))
        } else {
            export_type_with_literal_policy(self.checker, self.handles, id, false)
        }
    }
    fn error_type(&mut self) -> Type { self.handles.retain(TypeId::ERROR) }
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
        let signature = self.checker.native_implementation_signature(ty).expect("runtime native function has an implementation signature");
        let params = self.checker.sig_type_params(signature).into_vec();
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
        let signature = self.checker.native_implementation_signature(ty).expect("runtime native function has an implementation signature");
        export_signature(self.checker, self.handles, signature)
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
    fn is_primitive_literal(&mut self, ty: &Type) -> bool {
        match ty {
            Type::StringLiteral(_) | Type::NumberLiteral(_) | Type::BooleanLiteral(_) => true,
            Type::SessionType(reference) => {
                let ty = self.handles.resolve(*reference);
                matches!(self.checker.data(ty), TypeData::StringLit { .. }
                    | TypeData::NumberLit { .. } | TypeData::BoolLit { .. })
            }
            // Do not import another facade recipe just to classify it: named
            // and function imports may legitimately request source metadata.
            _ => false,
        }
    }
    fn base_of_literal(&mut self, ty: &Type) -> Type {
        let ty = import_type(self.checker, self.handles, ty);
        let ty = self.checker.base_of_literal(ty);
        export_type(self.checker, self.handles, ty)
    }
    fn regular_type(&mut self, ty: &Type) -> Type {
        let ty = import_type(self.checker, self.handles, ty);
        let ty = self.checker.regular(ty);
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

    fn lua_truthiness_type(&mut self, ty: &Type, sense: bool) -> Type {
        let ty = import_type(self.checker, self.handles, ty);
        let ty = self.checker.lua_truthiness_type(ty, sense);
        export_type(self.checker, self.handles, ty)
    }
    fn runtime_value_union(&mut self, types: &[Type]) -> Type {
        let types = types.iter().map(|ty| import_type(self.checker, self.handles, ty)).collect::<Vec<_>>();
        let ty = self.checker.native_runtime_value_union(&types);
        export_type(self.checker, self.handles, ty)
    }

    fn property_type(&mut self, ty: &Type, name: &str) -> Option<Type> {
        let ty = import_type(self.checker, self.handles, ty);
        let name = self.checker.atoms().intern(name.as_bytes());
        self.checker
            .type_of_property(ty, name)
            .map(|ty| export_type(self.checker, self.handles, ty))
    }

    fn property_type_checked(&mut self, ty: &Type, name: &str) -> Result<Option<Type>, TableError> {
        match self.table_lookup(ty, &TableKey::Literal(LuaKey::String(name.as_bytes().to_vec()))) {
            Ok(value) => Ok(Some(value)),
            Err(TableError::MissingKey { .. } | TableError::NotTable { .. }) => Ok(self.property_type(ty, name)),
            Err(error) => Err(error),
        }
    }

    fn assignment_value(&mut self, declared: &Type, assigned: &Type) -> Type {
        let declared = import_type(self.checker, self.handles, declared);
        let assigned = import_type(self.checker, self.handles, assigned);
        let value = if self.checker.native_is_auto_variable_constraint(declared) || self.checker.is_union(declared) {
            self.checker.resolved_assignment_type(declared, assigned)
        } else {
            declared
        };
        export_type(self.checker, self.handles, value)
    }

    fn auto_variable_constraint(&mut self) -> Type {
        self.handles.retain(self.checker.native_auto_variable_constraint())
    }

    fn is_auto_variable_constraint(&mut self, ty: &Type) -> bool {
        let ty = import_type(self.checker, self.handles, ty);
        self.checker.native_is_auto_variable_constraint(ty)
    }

    fn call(&mut self, callee: &Type, arguments: &[Type]) -> CallResult {
        self.call_with_type_arguments(callee, arguments, &[], None)
    }

    fn operator_call(&mut self, operator: NativeOperator, receiver: &Type, other: Option<&Type>) -> Option<CallResult> {
        let receiver = import_type(self.checker, self.handles, receiver);
        let other = other.map(|other| import_type(self.checker, self.handles, other));
        let result = self.checker.native_operator_call(operator, receiver, other)?;
        Some(self.export_call_result(result))
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
        self.export_call_result(result)
    }

    fn format(&mut self, ty: &Type) -> String {
        let ty = import_type(self.checker, self.handles, ty);
        String::from_utf8_lossy(&self.checker.native_type_to_string(ty)).into_owned()
    }
    fn class_definition_context(&mut self, owner: NativeClassDefinitionKey, class_type: &Type) -> Result<Type, NamedTypeError> {
        native_named::definition_context(self, owner, class_type)
    }
    fn class_definition_reopen(&mut self, owner: NativeClassDefinitionKey, class_type: &Type, actual: &Type) -> Result<Type, NamedTypeError> {
        native_named::definition_reopen(self, owner, class_type, actual)
    }
    fn class_definition_value(&mut self, owner: NativeClassDefinitionKey, class_type: &Type, actual: &Type) -> Result<Type, NamedTypeError> {
        native_named::definition_value(self, owner, class_type, actual)
    }
    fn class_definition_member_contract(&mut self, owner: NativeClassDefinitionKey, actual: &Type) -> Option<Type> {
        native_named::definition_member_contract(self, owner, actual)
    }
    fn named_type_reference(&mut self, declaration: NativeTypeDeclarationKey, arguments: &[Type]) -> Result<Type, NamedTypeError> {
        native_named::resolve(self, declaration, arguments)
    }
    fn named_type_constraint_error(&mut self, declaration: NativeTypeDeclarationKey, arguments: &[Type]) -> Option<NamedTypeError> {
        native_named::constraint_error(self, declaration, arguments)
    }
    fn named_object_declaration(&mut self, ty: &Type) -> Option<NativeTypeDeclarationKey> {
        let ty = import_type(self.checker, self.handles, ty);
        match self.checker.data(ty) {
            TypeData::NativeObject(reference) => Some(native_named::source_declaration(reference.declaration)),
            TypeData::NativeDefinition(reference) => Some(native_named::source_declaration(reference.class.declaration)),
            _ => None,
        }
    }
    fn runtime_table_value(&mut self, declaration: NativeTypeDeclarationKey, actual: &Type) -> Result<Type, NamedTypeError> {
        let actual = import_type(self.checker, self.handles, actual);
        let declaration = bun_sema::types::NativeTypeDeclarationId {snapshot:declaration.snapshot,index:declaration.index};
        self.checker.native_runtime_table_value(declaration, actual)
            .map(|ty| self.handles.retain(ty))
            .map_err(|error| native_named::export_error(self.checker, self.handles, error))
    }
    fn runtime_table_declaration(&mut self, ty: &Type) -> Option<NativeTypeDeclarationKey> {
        let ty = import_type(self.checker, self.handles, ty);
        let schema = self.checker.native_runtime_table_schema(ty)?;
        let TypeData::NativeObject(reference) = *self.checker.data(schema) else { unreachable!("runtime schema reference") };
        Some(native_named::source_declaration(reference.declaration))
    }
    fn runtime_table_schema(&mut self, ty: &Type) -> Option<Type> {
        let ty = import_type(self.checker, self.handles, ty);
        self.checker.native_runtime_table_schema(ty).map(|ty| self.handles.retain(ty))
    }
    fn named_declared_type(&mut self, declaration: NativeTypeDeclarationKey) -> Result<Type, NamedTypeError> {
        native_named::declared(self, declaration, false)
    }
    fn named_receiver_type(&mut self, class: &Type, receiver: &Type) -> Result<Type, NamedTypeError> { native_named::receiver(self, class, receiver) }
    fn named_member_context(&mut self, member: NativeTypeMemberKey) -> Result<Option<Type>, NamedTypeError> {
        native_named::member_context(self, member)
    }
    fn named_member_scope_owner(&mut self, member: NativeTypeMemberKey) -> Result<NativeTypeDeclarationKey, NamedTypeError> {
        native_named::member_scope_owner(self, member)
    }
    fn named_member_owner(&mut self, member: NativeTypeMemberKey) -> Result<NamedMemberOwner, NamedTypeError> {
        native_named::member_owner(self, member)
    }
    fn validate_named_declaration(&mut self, declaration: NativeTypeDeclarationKey) -> Result<Type, NamedTypeError> {
        native_named::declared(self, declaration, true)
    }
    fn validate_named_member(&mut self, member: NativeTypeMemberKey) -> Result<Type, NamedTypeError> {
        native_named::validate_member(self, member)
    }
}

impl SessionQueries<'_, '_, '_> {
    fn validate_binding(&self, key: NativeBindingKey) {
        if !self.checker.contains_native_binding(core_binding_key(key)) {
            std::panic::panic_any(InvalidNativeBindingKey(key));
        }
    }
    fn validate_expression(&self, key: NativeExpressionKey) {
        if !self
            .checker
            .contains_native_expression(core_expression_key(key))
        {
            std::panic::panic_any(InvalidNativeExpressionKey(key));
        }
    }
    fn export_call_result(&mut self, result: bun_sema::check::ResolvedTypeCall) -> CallResult {
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
                ResolvedTypeCallError::PackArity { index, actual_min, actual_max, min, max } => CallError::PackArity {
                    index,
                    actual_minimum: actual_min,
                    actual_maximum: actual_max,
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
        Type::Table(table) => {
            table
                .fields
                .iter()
                .any(|field| contains_native_function(&field.value))
                || table.indexers.iter().any(|indexer| {
                    contains_native_function(&indexer.key)
                        || contains_native_function(&indexer.value)
                })
                || table.sequence_tails.iter().any(|tail| contains_native_function(&tail.value))
        }
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
        checker.enable_native_arrays();
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
                    crate::CHECKER_STACK_BUDGET,
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

fn core_expression_key(key: NativeExpressionKey) -> NativeExpressionId {
    NativeExpressionId {
        snapshot: key.snapshot,
        index: key.node,
    }
}

fn source_expression_key(key: NativeExpressionId) -> NativeExpressionKey {
    NativeExpressionKey {
        snapshot: key.snapshot,
        node: key.index,
    }
}

fn core_binding_key(key: NativeBindingKey) -> NativeBindingId {
    NativeBindingId {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn source_binding_key(key: NativeBindingId) -> NativeBindingKey {
    NativeBindingKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}

struct ProviderAdapter<'a, P> {
    provider: &'a P,
    handles: &'a TypeHandles,
}

impl<P: NativeTypeProvider> NativeFunctionProvider for ProviderAdapter<'_, P> {
    fn project_flow_provider(&self) -> Option<&dyn bun_sema::check::native_flow::NativeFlowProvider> {
        self.provider.project_flow_provider().map(|_| self as &dyn bun_sema::check::native_flow::NativeFlowProvider)
    }
    fn contextual_type_owner(&self, checker: &mut Checker<'_, '_>, scope: NativeFunctionId, input: NativeFlowKey) -> bun_sema::check::native_named::NativeContextualTypeOwner {
        use bun_sema::check::native_named::NativeContextualTypeOwner as Core;
        match self.provider.contextual_type_owner(source_key(scope), input, &mut SessionQueries { checker, handles: self.handles }) {
            ContextualTypeOwner::Pending => Core::Pending,
            ContextualTypeOwner::Absent => Core::Absent,
            ContextualTypeOwner::Owner(owner) => Core::Owner(import_type(checker, self.handles, &owner)),
        }
    }
    fn function_is_method(&self, key: NativeFunctionId) -> bool { self.provider.function_is_method(source_key(key)) }
    fn signature_is_method(&self, key: bun_sema::types::NativeSignatureId) -> bool { self.provider.signature_is_method(source_signature_key(key)) }
    fn calling_convention(&self, key: NativeFunctionId) -> bun_sema::check::native_signatures::NativeCallingConvention {
        match self.provider.function_calling_convention(source_key(key)) {
            FunctionCallingConvention::TypeScript => bun_sema::check::native_signatures::NativeCallingConvention::TypeScript,
            FunctionCallingConvention::Lua => bun_sema::check::native_signatures::NativeCallingConvention::Lua,
        }
    }
    fn declared_result_pack_contract(&self, checker: &mut Checker<'_, '_>, key: NativeFunctionId) -> Option<NativePackResult> {
        let pack = self.provider.function_declared_scalar_contract(source_key(key), &mut SessionQueries { checker, handles: self.handles })?;
        Some(NativePackResult::Declaration(native_packs::import_declared_pack(checker, self.handles, &pack)))
    }
    fn declared_result_pack_contract_in_context(&self, checker: &mut Checker<'_, '_>, key: NativeFunctionId, context: NativeFunctionContext) -> Option<NativePackResult> {
        let context = self.handles.retain_context(context);
        let pack = self.provider.function_declared_scalar_contract_in_context(source_key(key), &context, &mut SessionQueries { checker, handles: self.handles })?;
        Some(NativePackResult::Declaration(native_packs::import_declared_pack(checker, self.handles, &pack)))
    }
    fn declared_result_pack_contract_in_capture(&self, checker: &mut Checker<'_, '_>, key: NativeFunctionId, context: NativeCapturedFunctionContext) -> Option<NativePackResult> {
        let context = self.handles.retain_capture(context);
        let pack = self.provider.function_declared_scalar_contract_in_capture(source_key(key), &context, &mut SessionQueries { checker, handles: self.handles })?;
        Some(NativePackResult::Declaration(native_packs::import_declared_pack(checker, self.handles, &pack)))
    }
    fn declared_scalar_return_type(&self, checker: &mut Checker<'_, '_>, key: NativeFunctionId) -> Option<TypeId> {
        let pack = self.provider.function_declared_scalar_contract(source_key(key), &mut SessionQueries { checker, handles: self.handles })?;
        let pack = native_packs::import_declared_pack(checker, self.handles, &pack);
        Some(checker.native_pack_slot(pack, 0))
    }
    fn declared_scalar_return_type_in_context(&self, checker: &mut Checker<'_, '_>, key: NativeFunctionId, context: NativeFunctionContext) -> Option<NativeTypeResult> {
        let context = self.handles.retain_context(context);
        let pack = self.provider.function_declared_scalar_contract_in_context(source_key(key), &context, &mut SessionQueries { checker, handles: self.handles })?;
        let pack = native_packs::import_declared_pack(checker, self.handles, &pack);
        Some(NativeTypeResult::Declaration(checker.native_pack_slot(pack, 0)))
    }
    fn declared_scalar_return_type_in_capture(&self, checker: &mut Checker<'_, '_>, key: NativeFunctionId, context: NativeCapturedFunctionContext) -> Option<NativeTypeResult> {
        let context = self.handles.retain_capture(context);
        let pack = self.provider.function_declared_scalar_contract_in_capture(source_key(key), &context, &mut SessionQueries { checker, handles: self.handles })?;
        let pack = native_packs::import_declared_pack(checker, self.handles, &pack);
        Some(NativeTypeResult::Declaration(checker.native_pack_slot(pack, 0)))
    }
    fn contains_signature(&self, key: bun_sema::types::NativeSignatureId) -> bool { self.provider.contains_signature(source_signature_key(key)) }
    fn signature_intrinsic(&self, key: bun_sema::types::NativeSignatureId) -> Option<bun_sema::check::native_intrinsics::NativeIntrinsicSite> {
        let site = self.provider.signature_intrinsic(source_signature_key(key))?;
        Some(bun_sema::check::native_intrinsics::NativeIntrinsicSite {
            binding: bun_sema::check::native_globals::NativeGlobalBindingId {
                environment: bun_sema::check::native_globals::NativeEnvironmentId { snapshot: site.binding.environment.snapshot, index: site.binding.environment.index },
                index: site.binding.index,
            },
            declaration: bun_sema::check::native_globals::NativeValueDeclarationId { snapshot: site.declaration.snapshot, index: site.declaration.index },
            kind: match site.kind {
                NativeIntrinsicKind::Assert => bun_sema::check::native_intrinsics::NativeIntrinsicKind::Assert,
                NativeIntrinsicKind::Type => bun_sema::check::native_intrinsics::NativeIntrinsicKind::Type,
                NativeIntrinsicKind::Require => bun_sema::check::native_intrinsics::NativeIntrinsicKind::Require,
                NativeIntrinsicKind::SetMetatable => bun_sema::check::native_intrinsics::NativeIntrinsicKind::SetMetatable,
            },
        })
    }
    fn signature_header(&self, key: bun_sema::types::NativeSignatureId) -> bun_sema::check::native_signatures::NativeSignatureHeader { self.signature_header_core(key) }
    fn signature_has_unconstrained_return(&self, key: bun_sema::types::NativeSignatureId) -> bool { self.provider.signature_has_unconstrained_return(source_signature_key(key)) }
    fn overload_signatures(&self, function: NativeFunctionId) -> Vec<bun_sema::types::NativeSignatureId> { self.provider.function_overload_signatures(source_key(function)).into_iter().map(core_signature_key).collect() }
    fn exposes_implementation(&self, function: NativeFunctionId) -> bool { self.provider.function_exposes_implementation(source_key(function)) }
    fn signature_parameters(&self, checker: &mut Checker<'_, '_>, key: bun_sema::types::NativeSignatureId) -> Vec<SigParam> { self.signature_parameters_core(checker, key) }
    fn signature_type_parameters(&self, checker: &mut Checker<'_, '_>, key: bun_sema::types::NativeSignatureId) -> Vec<TypeId> { self.provider.signature_type_parameters(source_signature_key(key)).into_iter().map(|key| import_type(checker, self.handles, &Type::NativeTypeParameter(key))).collect() }
    fn signature_outer_type_parameters(&self, key: bun_sema::types::NativeSignatureId) -> Vec<NativeTypeParamId> { self.provider.signature_outer_type_parameters(source_signature_key(key)).into_iter().map(core_parameter_key).collect() }
    fn signature_outer_type_parameter_instances(&self, checker: &mut Checker<'_, '_>, key: bun_sema::types::NativeSignatureId) -> Vec<TypeId> {
        let instances = self.provider.signature_outer_type_parameter_instances(source_signature_key(key), &mut SessionQueries { checker, handles: self.handles });
        instances.iter().map(|ty| import_type(checker, self.handles, ty)).collect()
    }
    fn signature_return_pack_result(&self, checker: &mut Checker<'_, '_>, key: bun_sema::types::NativeSignatureId) -> NativePackResult { self.signature_return_pack_core(checker, key) }
    fn circular_signature_return(&self, key: bun_sema::types::NativeSignatureId) { self.provider.signature_return_cycle(source_signature_key(key)); }
    fn signature_return_type_too_deep(&self, key: bun_sema::types::NativeSignatureId) { self.provider.signature_return_type_too_deep(source_signature_key(key)); }
    fn has_result_pack(&self, key: NativeFunctionId) -> bool {
        self.provider.function_has_result_pack(source_key(key))
    }
    fn contextual_rest_parameter(&self, key: NativeFunctionId) -> Option<usize> {
        self.provider.contextual_rest_parameter(source_key(key))
    }
    fn return_pack_result(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeFunctionId,
    ) -> NativePackResult {
        let key = source_key(key);
        let declared = self.provider.function_declared_return_pack(
            key,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        match declared {
            Some(pack) => NativePackResult::Declaration(native_packs::import_pack(
                checker,
                self.handles,
                &pack,
            )),
            None => {
                let pack = self.provider.function_inferred_return_pack(
                    key,
                    &mut SessionQueries {
                        checker,
                        handles: self.handles,
                    },
                );
                // An ordinary declaration body is checked with declaration-space
                // parameters. Its inferred pack follows the same mapper policy
                // as the existing scalar native return provider.
                NativePackResult::Declaration(native_packs::import_pack(
                    checker,
                    self.handles,
                    &pack,
                ))
            }
        }
    }
    fn return_pack_result_in_context(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeFunctionId,
        context: NativeFunctionContext,
    ) -> NativePackResult {
        let context = self.handles.retain_context(context);
        let key = source_key(key);
        let declared = self.provider.function_declared_return_pack_in_context(
            key,
            &context,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        match declared {
            Some(pack) => NativePackResult::Declaration(native_packs::import_pack(
                checker,
                self.handles,
                &pack,
            )),
            None => {
                let pack = self.provider.function_inferred_return_pack_in_context(
                    key,
                    &context,
                    &mut SessionQueries {
                        checker,
                        handles: self.handles,
                    },
                );
                NativePackResult::Checked(native_packs::import_pack(checker, self.handles, &pack))
            }
        }
    }
    fn return_pack_result_in_capture(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeFunctionId,
        context: NativeCapturedFunctionContext,
    ) -> NativePackResult {
        let context = self.handles.retain_capture(context);
        let key = source_key(key);
        let declared = self.provider.function_declared_return_pack_in_capture(
            key,
            &context,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        match declared {
            Some(pack) => NativePackResult::Declaration(native_packs::import_pack(
                checker,
                self.handles,
                &pack,
            )),
            None => {
                let pack = self.provider.function_inferred_return_pack_in_capture(
                    key,
                    &context,
                    &mut SessionQueries {
                        checker,
                        handles: self.handles,
                    },
                );
                NativePackResult::Checked(native_packs::import_pack(checker, self.handles, &pack))
            }
        }
    }
    fn return_type_result_in_capture(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeFunctionId,
        context: NativeCapturedFunctionContext,
    ) -> NativeTypeResult {
        let context = self.handles.retain_capture(context);
        let key = source_key(key);
        let declared = self.provider.function_declared_return_in_capture(
            key,
            &context,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        match declared {
            Some(ty) => NativeTypeResult::Declaration(import_type(checker, self.handles, &ty)),
            None => {
                let ty = self.provider.function_inferred_return_in_capture(
                    key,
                    &context,
                    &mut SessionQueries {
                        checker,
                        handles: self.handles,
                    },
                );
                NativeTypeResult::Checked(import_type(checker, self.handles, &ty))
            }
        }
    }
    fn return_type_result_in_context(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeFunctionId,
        context: NativeFunctionContext,
    ) -> NativeTypeResult {
        let context = self.handles.retain_context(context);
        let key = source_key(key);
        let declared = self.provider.function_declared_return_in_context(
            key,
            &context,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        match declared {
            Some(ty) => NativeTypeResult::Declaration(import_type(checker, self.handles, &ty)),
            None => {
                let ty = self.provider.function_inferred_return_in_context(
                    key,
                    &context,
                    &mut SessionQueries {
                        checker,
                        handles: self.handles,
                    },
                );
                NativeTypeResult::Checked(import_type(checker, self.handles, &ty))
            }
        }
    }
    fn contains_binding(&self, key: NativeBindingId) -> bool {
        self.provider.contains_binding(source_binding_key(key))
    }
    fn parameters_in_capture(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeFunctionId,
        context: NativeCapturedFunctionContext,
    ) -> Vec<SigParam> {
        let context = self.handles.retain_capture(context);
        let parameters = self.provider.function_parameters_in_capture(
            source_key(key),
            &context,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        let rest = self.provider.function_rest_pack_in_capture(
            source_key(key),
            &context,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        let parameters = import_parameters(checker, self.handles, parameters);
        append_rest_parameter(checker, self.handles, parameters, rest)
    }
    fn return_type_in_capture(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeFunctionId,
        context: NativeCapturedFunctionContext,
    ) -> TypeId {
        let context = self.handles.retain_capture(context);
        let key = source_key(key);
        let declared = self.provider.function_declared_return_in_capture(
            key,
            &context,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        let returned = match declared {
            Some(ty) => ty,
            None => self.provider.function_inferred_return_in_capture(
                key,
                &context,
                &mut SessionQueries {
                    checker,
                    handles: self.handles,
                },
            ),
        };
        import_type(checker, self.handles, &returned)
    }
    fn contains_expression(&self, key: NativeExpressionId) -> bool {
        self.provider
            .contains_expression(source_expression_key(key))
    }
    fn is_context_sensitive(&self, key: NativeExpressionId) -> bool {
        self.provider
            .is_context_sensitive(source_expression_key(key))
    }
    fn has_context_sensitive_parameters(&self, key: NativeExpressionId) -> bool {
        self.provider
            .has_context_sensitive_parameters(source_expression_key(key))
    }
    fn contextual_parameter_indices(&self, key: NativeFunctionId) -> Vec<usize> {
        self.provider.contextual_parameter_indices(source_key(key))
    }
    fn is_expression_descendant(
        &self,
        expression: NativeExpressionId,
        ancestor: NativeExpressionId,
    ) -> bool {
        self.provider.is_expression_descendant(
            source_expression_key(expression),
            source_expression_key(ancestor),
        )
    }
    fn check_expression_pack(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeExpressionId,
        expected: Option<NativeResultPack>,
        mode: NativeCheckMode,
    ) -> NativeResultPack {
        let key = source_expression_key(key);
        if !self.provider.contains_expression(key) {
            std::panic::panic_any(InvalidNativeExpressionKey(key));
        }
        let expected = expected.map(|pack| self.handles.retain_pack(checker, pack));
        let pack = self.provider.check_expression_pack(
            key, expected.as_ref(), mode,
            &mut SessionQueries { checker, handles: self.handles },
        );
        native_packs::import_pack(checker, self.handles, &pack)
    }
    fn check_expression(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeExpressionId,
        expected: Option<TypeId>,
        mode: NativeCheckMode,
    ) -> TypeId {
        let key = source_expression_key(key);
        if !self.provider.contains_expression(key) {
            std::panic::panic_any(InvalidNativeExpressionKey(key));
        }
        let expected = expected.map(|ty| self.handles.retain(ty));
        let ty = self.provider.check_expression(
            key,
            expected.as_ref(),
            mode,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        import_type(checker, self.handles, &ty)
    }
    fn check_reusable_expression(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeExpressionId,
        expected: Option<TypeId>,
        mode: NativeCheckMode,
    ) -> (TypeId, bool) {
        let key = source_expression_key(key);
        if !self.provider.contains_expression(key) {
            std::panic::panic_any(InvalidNativeExpressionKey(key));
        }
        let expected = expected.map(|ty| self.handles.retain(ty));
        let (ty, reusable) = self.provider.check_reusable_expression(
            key,
            expected.as_ref(),
            mode,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        (import_type(checker, self.handles, &ty), reusable)
    }
    fn parameters_in_context(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeFunctionId,
        context: NativeFunctionContext,
    ) -> Vec<SigParam> {
        let context = self.handles.retain_context(context);
        let parameters = self.provider.function_parameters_in_context(
            source_key(key),
            &context,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        let rest = self.provider.function_rest_pack_in_context(
            source_key(key),
            &context,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        let parameters = import_parameters(checker, self.handles, parameters);
        append_rest_parameter(checker, self.handles, parameters, rest)
    }
    fn return_type_in_context(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeFunctionId,
        context: NativeFunctionContext,
    ) -> TypeId {
        let context = self.handles.retain_context(context);
        let key = source_key(key);
        let declared = self.provider.function_declared_return_in_context(
            key,
            &context,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        let returned = match declared {
            Some(ty) => ty,
            None => self.provider.function_inferred_return_in_context(
                key,
                &context,
                &mut SessionQueries {
                    checker,
                    handles: self.handles,
                },
            ),
        };
        import_type(checker, self.handles, &returned)
    }
    fn outer_type_parameters(&self, key: NativeFunctionId) -> Vec<NativeTypeParamId> {
        self.provider
            .function_outer_type_parameters(source_key(key))
            .into_iter()
            .map(core_parameter_key)
            .collect()
    }
    fn outer_type_parameter_instances(&self, checker: &mut Checker<'_, '_>, key: NativeFunctionId) -> Vec<TypeId> {
        let instances = self.provider.function_outer_type_parameter_instances(source_key(key), &mut SessionQueries { checker, handles: self.handles });
        instances.iter().map(|ty| import_type(checker, self.handles, ty)).collect()
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
    fn lexical_parent(&self, key: NativeFunctionId) -> Option<NativeFunctionId> {
        self.provider.function_lexical_parent(source_key(key)).map(core_key)
    }

    fn parameters(&self, checker: &mut Checker<'_, '_>, key: NativeFunctionId) -> Vec<SigParam> {
        let parameters = self.provider.function_parameters(
            source_key(key),
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        let rest = self.provider.function_rest_pack(
            source_key(key),
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        let parameters = import_parameters(checker, self.handles, parameters);
        append_rest_parameter(checker, self.handles, parameters, rest)
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

fn append_rest_parameter(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    mut parameters: Vec<SigParam>,
    rest: Option<ValuePack>,
) -> Vec<SigParam> {
    if let Some(rest) = rest {
        let pack = native_packs::import_pack(checker, handles, &rest);
        parameters.push(SigParam {
            name: checker.atoms().intern(b"..."),
            ty: checker.native_pack_type(pack),
            optional: false,
            rest: true,
            is_required_rest: false,
            declaration: None,
        });
    }
    parameters
}

fn import_parameters(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    parameters: Vec<Parameter>,
) -> Vec<SigParam> {
    parameters
        .into_iter()
        .map(|parameter| {
            let mut ty = import_type(checker, handles, &parameter.ty);
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
        Type::RuntimeTable => checker.native_runtime_top(bun_sema::types::NativeRuntimeKind::Table),
        Type::RuntimeFunction => checker.native_runtime_top(bun_sema::types::NativeRuntimeKind::Function),
        Type::SessionType(reference) => handles.resolve(*reference),
        Type::Table(table) => native_tables::import_table(checker, handles, table),
        Type::BooleanLiteral(value) => checker.bool_literal(*value, true),
        Type::NumberLiteral(value) => checker.number_literal(*value, true),
        Type::StringLiteral(value) => checker.string_literal(checker.atoms().intern(value), true),
        Type::NativeFunction(key) => checker
            .native_function_type(core_key(*key))
            .unwrap_or_else(|| std::panic::panic_any(InvalidNativeFunctionKey(*key))),
        Type::NativeSignature(key) => checker.native_signature_type(core_signature_key(*key)).unwrap_or_else(|| std::panic::panic_any(InvalidNativeSignatureKey(*key))),
        Type::NativeTypeParameter(key) => checker
            .native_type_parameter(core_parameter_key(*key))
            .unwrap_or_else(|| std::panic::panic_any(InvalidNativeTypeParameterKey(*key))),
        Type::NativeNamedReference { declaration, arguments } => {
            let arguments: Vec<_> = arguments.iter().map(|ty| import_type(checker, handles, ty)).collect();
            checker.native_type_reference(native_named::core_declaration(*declaration), &arguments)
                .unwrap_or(TypeId::ERROR)
        }
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
    export_type_with_literal_policy(checker, handles, ty, true)
}

fn export_type_with_literal_policy(checker: &mut Checker<'_, '_>, handles: &TypeHandles, ty: TypeId, preserve_regular: bool) -> Type {
    if ty == TypeId::BOOLEAN {
        return Type::Boolean;
    }
    if preserve_regular && matches!(checker.data(ty), TypeData::BoolLit { fresh: false, .. } | TypeData::NumberLit { fresh: false, .. } | TypeData::StringLit { fresh: false, .. }) {
        return handles.retain(ty);
    }
    match checker.data(ty) {
        TypeData::NativeRuntimeTop { kind, .. } => match kind {
            bun_sema::types::NativeRuntimeKind::Table => Type::RuntimeTable,
            bun_sema::types::NativeRuntimeKind::Function => Type::RuntimeFunction,
        },
        TypeData::NativeObject(..) | TypeData::NativeDefinition(..) | TypeData::NativeRuntimeTable(..) => handles.retain(ty),
        TypeData::NativeTable(_) => native_tables::export_table(checker, handles, ty, preserve_regular)
            .map(Type::Table)
            .unwrap_or_else(|| handles.retain(ty)),
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
            // Flow uses these sentinels to retain provisional/unreachable
            // semantics. Reimporting a plain Never would erase that distinction.
            Intrinsic::SilentNever | Intrinsic::UnreachableNever => handles.retain(ty),
            Intrinsic::Null | Intrinsic::NullWidening => Type::Nil,
            Intrinsic::Undefined | Intrinsic::UndefinedWidening | Intrinsic::Missing => Type::Nil,
            Intrinsic::Void => Type::Void,
            Intrinsic::String => Type::String,
            Intrinsic::Number => Type::Number,
            _ => handles.retain(ty),
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
                    .map(|ty| export_type_with_literal_policy(checker, handles, ty, preserve_regular))
                    .collect(),
            )
        }
        TypeData::Intersection(types) => {
            let types = types.to_vec();
            Type::Intersection(
                types
                    .into_iter()
                    .map(|ty| export_type_with_literal_policy(checker, handles, ty, preserve_regular))
                    .collect(),
            )
        }
        TypeData::Synth(shape) if shape.call.len() == 1 && shape.props.is_empty() => {
            let sig = shape.call[0];
            if (checker.sig_has_native_result_pack(sig) && checker.sig_native_decl(sig).is_none())
                || checker.sig_native_context(sig).is_some()
                || matches!(
                    checker.types().sig(checker.types().sig_origin(sig)),
                    SigData::NativeCaptured { .. }
                )
            {
                return handles.retain(ty);
            }
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
                Type::Function(export_signature_with_literal_policy(checker, handles, sig, preserve_regular))
            }
        }
        TypeData::Synth(shape) => {
            if !shape.call.is_empty() || !shape.construct.is_empty() {
                return handles.retain(ty);
            }
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
                    ty: export_type_with_literal_policy(checker, handles, ty, preserve_regular),
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
    export_signature_with_literal_policy(checker, handles, sig, true)
}

fn export_signature_with_literal_policy(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    sig: bun_sema::types::SigId,
    preserve_regular: bool,
) -> FunctionType {
    let params = checker.sig_params(sig).into_vec();
    let returns = checker.sig_return(sig);
    FunctionType {
        params: params
            .into_iter()
            .map(|param| Parameter {
                name: String::from_utf8_lossy(checker.atoms().bytes(param.name)).into_owned(),
                ty: export_type_with_literal_policy(checker, handles, param.ty, preserve_regular),
                optional: param.optional,
            })
            .collect(),
        returns: Box::new(export_type_with_literal_policy(checker, handles, returns, preserve_regular)),
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
mod native_named_core_tests;
#[cfg(test)]
mod contextual_type_owner_tests;

#[cfg(test)]
mod class_nominal_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct ContextualProvider {
        parameters: std::cell::RefCell<Vec<(ContextualFunctionContext, Type)>>,
        base_parameters: std::cell::Cell<usize>,
        callback_modes: std::cell::RefCell<Vec<NativeCheckMode>>,
        captures: std::cell::RefCell<Vec<CapturedFunctionContext>>,
        parent_parameter_observations: std::cell::RefCell<Vec<Type>>,
        argument_frames: std::cell::RefCell<Vec<NativeCallerFrame>>,
        capture_frames: std::cell::RefCell<Vec<NativeCallerFrame>>,
    }
    impl ContextualProvider {
        fn expression(node: u32) -> NativeExpressionKey {
            NativeExpressionKey { snapshot: 92, node }
        }
        fn function(index: u32) -> NativeFunctionKey {
            NativeFunctionKey {
                snapshot: 92,
                index,
            }
        }
        fn binding() -> NativeBindingKey {
            NativeBindingKey {
                snapshot: 92,
                index: 0,
            }
        }
    }
    impl NativeTypeProvider for ContextualProvider {
        fn contains_function(&self, key: NativeFunctionKey) -> bool {
            if key.snapshot == 91 {
                StaticGenericProvider.contains_function(key)
            } else {
                key.snapshot == 92 && key.index < 8
            }
        }
        fn function_parameters(
            &self,
            key: NativeFunctionKey,
            queries: &mut dyn TypeQueries,
        ) -> Vec<Parameter> {
            if key.snapshot == 91 {
                return StaticGenericProvider.function_parameters(key, queries);
            }
            self.base_parameters.set(self.base_parameters.get() + 1);
            if matches!(key.index, 1 | 4) {
                Vec::new()
            } else {
                vec![StaticGenericProvider::argument("value", Type::Any)]
            }
        }
        fn function_declared_return(
            &self,
            key: NativeFunctionKey,
            queries: &mut dyn TypeQueries,
        ) -> Option<Type> {
            if key.snapshot == 91 {
                StaticGenericProvider.function_declared_return(key, queries)
            } else {
                None
            }
        }
        fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
            Type::Any
        }
        fn return_cycle(&self, _: NativeFunctionKey) {
            panic!("contextual fixture cannot cycle");
        }
        fn return_type_too_deep(&self, _: NativeFunctionKey) {
            panic!("contextual fixture exceeds depth");
        }
        fn function_type_parameters(
            &self,
            key: NativeFunctionKey,
            queries: &mut dyn TypeQueries,
        ) -> Vec<NativeTypeParameterKey> {
            if key.snapshot == 91 {
                StaticGenericProvider.function_type_parameters(key, queries)
            } else {
                Vec::new()
            }
        }
        fn function_outer_type_parameters(
            &self,
            key: NativeFunctionKey,
        ) -> Vec<NativeTypeParameterKey> {
            if key.snapshot == 91 {
                StaticGenericProvider.function_outer_type_parameters(key)
            } else {
                Vec::new()
            }
        }
        fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
            StaticGenericProvider.contains_type_parameter(key)
        }
        fn type_parameter_name(&self, key: NativeTypeParameterKey) -> String {
            StaticGenericProvider.type_parameter_name(key)
        }
        fn type_parameter_siblings(
            &self,
            key: NativeTypeParameterKey,
        ) -> Vec<NativeTypeParameterKey> {
            StaticGenericProvider.type_parameter_siblings(key)
        }
        fn contains_expression(&self, key: NativeExpressionKey) -> bool {
            key.snapshot == 92 && key.node < 16
        }
        fn is_context_sensitive(&self, key: NativeExpressionKey) -> bool {
            matches!(key.node, 2 | 4 | 5 | 7 | 11 | 13)
        }
        fn has_context_sensitive_parameters(&self, key: NativeExpressionKey) -> bool {
            matches!(key.node, 2 | 5 | 7 | 11 | 13)
        }
        fn contextual_parameter_indices(&self, key: NativeFunctionKey) -> Vec<usize> {
            if key.snapshot == 92 && !matches!(key.index, 1 | 4) {
                vec![0]
            } else {
                Vec::new()
            }
        }
        fn check_expression(
            &self,
            key: NativeExpressionKey,
            expected: Option<&Type>,
            mode: NativeCheckMode,
            queries: &mut dyn TypeQueries,
        ) -> Type {
            match key.node {
                14 | 15 => {
                    let frame = NativeCallerFrame {
                        contextual: queries.current_contextual_function(),
                        captured: queries.current_captured_function(),
                    };
                    self.argument_frames.borrow_mut().push(frame);
                    if key.node == 15 { return Type::NumberLiteral(42.0); }
                    queries.captured_binding(&frame.captured.expect("original argument retains its caller"), Self::binding()).unwrap()
                }
                1 => Type::Number,
                3 => Type::String,
                12 => StaticGenericProvider::variable(0),
                10 => {
                    let context = queries
                        .current_contextual_function()
                        .expect("nested call captures an active contextual body");
                    queries
                        .contextual_parameter_type(&context, 0)
                        .expect("enclosing parameter")
                }
                2 | 4 | 5 | 7 | 11 | 13 => {
                    self.callback_modes.borrow_mut().push(mode);
                    let function = Self::function(match key.node {
                        2 => 0,
                        4 => 1,
                        5 => 2,
                        11 => 6,
                        13 => 7,
                        _ => 3,
                    });
                    match expected {
                        Some(expected) => {
                            queries.contextual_function(function, key, expected, mode)
                        }
                        None => Type::NativeFunction(function),
                    }
                }
                _ => panic!("call sites are not checked as argument expressions"),
            }
        }
        fn function_parameters_in_context(
            &self,
            key: NativeFunctionKey,
            context: &ContextualFunctionContext,
            queries: &mut dyn TypeQueries,
        ) -> Vec<Parameter> {
            if matches!(key.index, 1 | 4) {
                return Vec::new();
            }
            let parameter = queries
                .contextual_parameter_type(context, 0)
                .expect("one contextual parameter");
            self.parameters
                .borrow_mut()
                .push((*context, parameter.clone()));
            assert!(queries.is_assignable(&parameter, &Type::Unknown));
            vec![StaticGenericProvider::argument("value", parameter)]
        }
        fn function_inferred_return_in_context(
            &self,
            key: NativeFunctionKey,
            context: &ContextualFunctionContext,
            queries: &mut dyn TypeQueries,
        ) -> Type {
            assert_eq!(queries.current_contextual_function(), Some(*context));
            match key.index {
                1 => queries.aggregate_returns(&[Type::Number], false, None),
                2 => Type::Void,
                3 => {
                    let result = queries.call_expression_native(
                        &Type::NativeFunction(StaticGenericProvider::function(4)),
                        Self::expression(8),
                        &[Self::expression(10), Self::expression(2)],
                        &[],
                        None,
                    );
                    assert!(result.errors.is_empty(), "{result:?}");
                    result.return_type
                }
                4 => {
                    let captured = queries
                        .current_captured_function()
                        .expect("combined facet retains capture frame");
                    queries
                        .captured_binding(&captured, Self::binding())
                        .unwrap()
                }
                5 => {
                    let captured = queries
                        .current_captured_function()
                        .expect("combined facet retains capture frame");
                    let binding = queries
                        .captured_binding(&captured, Self::binding())
                        .unwrap();
                    let argument = queries.contextual_parameter_type(context, 0).unwrap();
                    Type::Shape(vec![
                        Property::new("captured", binding),
                        Property::new("argument", argument),
                    ])
                }
                6 => {
                    let binding = queries.contextual_parameter_type(context, 0).unwrap();
                    let closure =
                        queries.captured_function(Self::function(4), &[(Self::binding(), binding)]);
                    queries.aggregate_returns(&[closure], false, None)
                }
                7 => {
                    let binding = queries.contextual_parameter_type(context, 0).unwrap();
                    let first =
                        queries.captured_function(Self::function(4), &[(Self::binding(), binding)]);
                    queries.captured_function(Self::function(4), &[(Self::binding(), first)])
                }
                _ => {
                    let returned = queries.contextual_parameter_type(context, 0).unwrap();
                    let expected = queries.contextual_return_type(context);
                    queries.aggregate_returns(&[returned], false, Some(&expected))
                }
            }
        }
        fn contains_binding(&self, key: NativeBindingKey) -> bool {
            key.snapshot == 92 && key.index < 2
        }
        fn function_parameters_in_capture(
            &self,
            key: NativeFunctionKey,
            context: &CapturedFunctionContext,
            queries: &mut dyn TypeQueries,
        ) -> Vec<Parameter> {
            self.capture_frames.borrow_mut().push(NativeCallerFrame {
                contextual: queries.current_contextual_function(),
                captured: queries.current_captured_function(),
            });
            if let Some(parent) = queries.captured_function_parent(context) {
                if let Some(value) = queries.captured_binding(&parent, Self::binding()) {
                    self.parent_parameter_observations.borrow_mut().push(value);
                }
            }
            if key.snapshot == 91 {
                return StaticGenericProvider.function_parameters(key, queries);
            }
            if key.index == 4 {
                Vec::new()
            } else {
                vec![StaticGenericProvider::argument("value", Type::Any)]
            }
        }
        fn function_inferred_return_in_capture(
            &self,
            key: NativeFunctionKey,
            context: &CapturedFunctionContext,
            queries: &mut dyn TypeQueries,
        ) -> Type {
            self.capture_frames.borrow_mut().push(NativeCallerFrame {
                contextual: queries.current_contextual_function(),
                captured: queries.current_captured_function(),
            });
            assert_eq!(queries.current_captured_function(), Some(*context));
            let function = queries.captured_function_value(context);
            assert_eq!(queries.native_function_key(&function), Some(key));
            self.captures.borrow_mut().push(*context);
            queries.captured_binding(context, Self::binding()).unwrap()
        }
    }

    #[test]
    fn captured_provider_callbacks_clear_foreign_context_and_restore_the_caller() {
        Engine::new().with_native_provider(ContextualProvider::default, |provider, queries| {
            let raw = queries.captured_function(ContextualProvider::function(4), &[(ContextualProvider::binding(), Type::Number)]);
            let expected = StaticGenericProvider::callback(Some(Type::String), Type::String);
            let foreign = queries.contextual_function(ContextualProvider::function(0), ContextualProvider::expression(2), &expected, NativeCheckMode::empty());
            let foreign = queries.function_capture_context(&foreign).unwrap();
            let contextual = queries.captured_function_context(&foreign).unwrap();
            provider.capture_frames.borrow_mut().clear();
            queries.with_captured_function(&foreign, &mut |queries| {
                queries.with_contextual_function(&contextual, &mut |queries| {
                    let result = queries.call(&raw, &[]);
                    assert!(result.errors.is_empty(), "{result:?}");
                    assert_eq!(result.return_type, Type::Number);
                    assert_eq!(queries.current_captured_function(), Some(foreign));
                    assert_eq!(queries.current_contextual_function(), Some(contextual));
                });
            });
            let frames = provider.capture_frames.borrow();
            assert!(!frames.is_empty());
            assert!(frames.iter().all(|frame| frame.contextual.is_none()), "{frames:?}");
            assert!(frames.iter().all(|frame| frame.captured != Some(foreign)), "{frames:?}");
        });
    }

    #[test]
    fn delayed_native_arguments_restore_caller_frames_for_scalar_pack_and_declaration_space() {
        Engine::new().with_native_provider(ContextualProvider::default, |provider, queries| {
            let mut frames = Vec::new();
            for (function, expression, value) in [(0, 2, Type::Number), (2, 5, Type::String)] {
                let raw = queries.captured_function(ContextualProvider::function(function), &[(ContextualProvider::binding(), value.clone())]);
                let expected = StaticGenericProvider::callback(Some(value), Type::Any);
                let value = queries.contextual_function_value(&raw, ContextualProvider::expression(expression), &expected, NativeCheckMode::empty());
                let captured = queries.function_capture_context(&value).unwrap();
                frames.push(NativeCallerFrame { contextual: queries.captured_function_context(&captured), captured: Some(captured) });
            }
            let caller = frames[0];
            let foreign = frames[1];
            let identity = Type::NativeFunction(StaticGenericProvider::function(0));
            queries.with_captured_function(&foreign.captured.unwrap(), &mut |queries| {
                queries.with_contextual_function(&foreign.contextual.unwrap(), &mut |queries| {
                    let scalar = queries.call_native_arguments_scalar_in_frame(&identity, None, &[PackArgument::Expression(ContextualProvider::expression(14))], &[], None, caller);
                    assert!(scalar.errors.is_empty(), "{scalar:?}");
                    assert_eq!(scalar.return_type, Type::Number);
                    let pack = queries.call_native_arguments_in_frame(&identity, None, &[PackArgument::ExpandExpression(ContextualProvider::expression(14))], &[], None, caller);
                    assert!(pack.errors.is_empty(), "{pack:?}");
                    assert_eq!(pack.return_type, Type::Number);
                    let literal = queries.call_native_arguments_scalar_in_frame(&identity, None, &[PackArgument::Expression(ContextualProvider::expression(15))], &[], None, NativeCallerFrame::default());
                    assert!(literal.errors.is_empty(), "{literal:?}");
                    assert!(queries.is_assignable(&literal.return_type, &Type::Number));
                    assert_eq!(queries.current_captured_function(), foreign.captured);
                    assert_eq!(queries.current_contextual_function(), foreign.contextual);
                });
            });
            let observed = provider.argument_frames.borrow();
            assert!(observed.iter().any(|frame| frame.captured == caller.captured && frame.contextual == caller.contextual));
            assert!(observed.iter().any(|frame| frame.captured.is_none() && frame.contextual.is_none()));
            assert!(observed.iter().all(|frame| frame.captured != foreign.captured && frame.contextual != foreign.contextual));
        });
    }

    #[test]
    fn native_captured_closures_keep_distinct_environments_and_compose_with_contexts() {
        Engine::new().with_native_provider(ContextualProvider::default, |provider, queries| {
            let map = Type::NativeFunction(StaticGenericProvider::function(4));
            let mut closures = Vec::new();
            for (node, expected) in [(1, Type::Number), (3, Type::String), (1, Type::Number)] {
                let result = queries.call_expression_native(
                    &map,
                    ContextualProvider::expression(0),
                    &[
                        ContextualProvider::expression(node),
                        ContextualProvider::expression(11),
                    ],
                    &[],
                    None,
                );
                assert!(result.errors.is_empty(), "{result:?}");
                assert!(matches!(result.return_type, Type::SessionType(_)));
                let returned = queries.call(&result.return_type, &[]);
                assert!(returned.errors.is_empty(), "{returned:?}");
                assert_eq!(returned.return_type, expected);
                closures.push(result.return_type);
            }
            assert_ne!(closures[0], closures[1]);
            assert_eq!(closures[0], closures[2]);
            assert_eq!(
                provider.captures.borrow().len(),
                2,
                "native captured returns are cached by environment"
            );
            let captured = queries.captured_function(
                ContextualProvider::function(5),
                &[(ContextualProvider::binding(), Type::String)],
            );
            let expected = StaticGenericProvider::callback(Some(Type::Number), Type::Any);
            let combined = queries.contextual_function_value(
                &captured,
                ContextualProvider::expression(2),
                &expected,
                NativeCheckMode::empty(),
            );
            let signature = queries.function_signature(&combined).unwrap();
            assert_eq!(signature.params[0].ty, Type::Number);
            let returned = &*signature.returns;
            assert_eq!(
                queries.property_type(returned, "argument"),
                Some(Type::Number)
            );
            assert_eq!(
                queries.property_type(returned, "captured"),
                Some(Type::String)
            );
            assert_eq!(queries.current_captured_function(), None);
            assert_eq!(queries.current_contextual_function(), None);
            assert_eq!(provider.base_parameters.get(), 0);
        });
    }

    #[test]
    fn native_lexical_parent_capabilities_survive_empty_captures_and_contextual_wrapping() {
        Engine::new().with_native_provider(ContextualProvider::default, |_, queries| {
            let plain = queries
                .function_capture_context(&Type::NativeFunction(ContextualProvider::function(5)))
                .unwrap();
            assert_eq!(queries.captured_function_parent(&plain), None);
            assert_eq!(queries.captured_function_context(&plain), None);
            let parent = queries.captured_function(
                ContextualProvider::function(5),
                &[(ContextualProvider::binding(), Type::String)],
            );
            let expected = StaticGenericProvider::callback(Some(Type::Number), Type::Any);
            let parent = queries.contextual_function_value(
                &parent,
                ContextualProvider::expression(2),
                &expected,
                NativeCheckMode::CONTEXTUAL,
            );
            let parent = queries.function_capture_context(&parent).unwrap();
            assert!(queries.captured_function_context(&parent).is_some());
            let child = queries.captured_function_with_parent(
                ContextualProvider::function(4),
                &[],
                Some(&parent),
            );
            let child_context = queries.function_capture_context(&child).unwrap();
            assert_eq!(queries.captured_function_parent(&child_context), Some(parent));
            let child = queries.contextual_function_value(
                &child,
                ContextualProvider::expression(2),
                &StaticGenericProvider::callback(None, Type::Any),
                NativeCheckMode::CONTEXTUAL,
            );
            let child_context = queries.function_capture_context(&child).unwrap();
            assert_eq!(queries.captured_function_parent(&child_context), Some(parent));
            assert_eq!(
                queries.captured_binding(&parent, ContextualProvider::binding()),
                Some(Type::String),
            );
            assert_eq!(queries.current_captured_function(), None);
            assert_eq!(queries.current_contextual_function(), None);
        });
    }

    #[test]
    fn native_lexical_parent_instantiation_maps_each_ancestor_once() {
        with_program(|program, handles| {
            let provider = ContextualProvider::default();
            let adapter = ProviderAdapter {
                provider: &provider,
                handles,
            };
            let mut checker = program.checker_with_native_provider(&adapter);
            let second_binding = NativeBindingKey { snapshot: 92, index: 1 };
            let mut queries = SessionQueries { checker: &mut checker, handles };
            let parent = queries.captured_function(
                ContextualProvider::function(5),
                &[
                    (ContextualProvider::binding(), StaticGenericProvider::variable(3)),
                    (second_binding, StaticGenericProvider::variable(4)),
                ],
            );
            let parent = queries.contextual_function_value(
                &parent,
                ContextualProvider::expression(2),
                &StaticGenericProvider::callback(Some(StaticGenericProvider::variable(4)), Type::Any),
                NativeCheckMode::CONTEXTUAL,
            );
            let original_parent = queries.function_capture_context(&parent).unwrap();
            let child = queries.captured_function_with_parent(
                ContextualProvider::function(4), &[], Some(&original_parent),
            );
            let raw = import_type(&mut checker, handles, &child);
            let t = import_type(&mut checker, handles, &StaticGenericProvider::variable(3));
            let u = import_type(&mut checker, handles, &StaticGenericProvider::variable(4));
            let mapper = checker.types().mapper(vec![(t, u), (u, TypeId::NUMBER)]);
            let mapped = checker.instantiate(raw, mapper);
            let mapped = export_type(&mut checker, handles, mapped);
            let mut queries = SessionQueries { checker: &mut checker, handles };
            let child = queries.function_capture_context(&mapped).unwrap();
            let parent = queries.captured_function_parent(&child).unwrap();
            assert_ne!(parent, original_parent);
            assert_eq!(
                queries.captured_binding(&parent, ContextualProvider::binding()),
                Some(StaticGenericProvider::variable(4)),
                "the simultaneous T -> U, U -> number substitution must not map T twice",
            );
            assert_eq!(queries.captured_binding(&parent, second_binding), Some(Type::Number));
            let contextual = queries.captured_function_context(&parent).unwrap();
            assert_eq!(queries.contextual_parameter_type(&contextual, 0), Some(Type::Number));
            assert_eq!(
                queries.captured_binding(&original_parent, ContextualProvider::binding()),
                Some(StaticGenericProvider::variable(3)),
                "mapping a child must leave the original parent capability immutable",
            );
        });
    }

    #[test]
    fn native_lexical_parent_survives_own_erasure_and_signature_target_reconstruction() {
        with_program(|program, handles| {
            let provider = ContextualProvider::default();
            let adapter = ProviderAdapter { provider: &provider, handles };
            let mut checker = program.checker_with_native_provider(&adapter);
            let mut queries = SessionQueries { checker: &mut checker, handles };
            let bindings = [
                (ContextualProvider::binding(), StaticGenericProvider::variable(3)),
                (NativeBindingKey { snapshot: 92, index: 1 }, StaticGenericProvider::variable(4)),
            ];
            let parent = queries.captured_function(ContextualProvider::function(4), &bindings);
            let parent = queries.function_capture_context(&parent).unwrap();
            let own_generic = queries.captured_function_with_parent(
                StaticGenericProvider::function(4), &[], Some(&parent),
            );
            let own_generic = import_type(&mut checker, handles, &own_generic);
            let signature = checker.native_implementation_signature(own_generic).unwrap();
            let erased = checker.erased_sig(signature);
            assert_ne!(erased, signature);
            let erased_parent = checker.native_captured_function_parent(
                NativeCapturedFunctionContext { signature: erased },
            ).unwrap();
            assert_eq!(erased_parent, handles.resolve_capture(&parent));
            let child = SessionQueries { checker: &mut checker, handles }
                .captured_function_with_parent(StaticGenericProvider::function(0), &bindings, Some(&parent));
            let raw = import_type(&mut checker, handles, &child);
            let own = import_type(&mut checker, handles, &StaticGenericProvider::variable(0));
            let t = import_type(&mut checker, handles, &StaticGenericProvider::variable(3));
            let u = import_type(&mut checker, handles, &StaticGenericProvider::variable(4));
            let mapper = checker.types().mapper(vec![(own, TypeId::NUMBER), (t, u), (u, TypeId::NUMBER)]);
            let mapped = checker.instantiate(raw, mapper);
            let mapped = export_type(&mut checker, handles, mapped);
            provider.parent_parameter_observations.borrow_mut().clear();
            let expected = StaticGenericProvider::callback(Some(Type::String), Type::Any);
            assert!(!SessionQueries { checker: &mut checker, handles }.is_assignable(&mapped, &expected));
            let observations = provider.parent_parameter_observations.borrow();
            assert!(observations.len() >= 2, "comparison must demand the instantiated signature and its generic target: {observations:?}");
            assert!(
                observations.iter().all(|value| *value == StaticGenericProvider::variable(4)),
                "Signature.target must not reapply T -> U, U -> number to the already mapped parent: {observations:?}",
            );
        });
    }

    #[test]
    fn native_captured_environment_substitution_uses_the_signature_mapper() {
        with_program(|program, handles| {
            let provider = ContextualProvider::default();
            let adapter = ProviderAdapter {
                provider: &provider,
                handles,
            };
            let mut checker = program.checker_with_native_provider(&adapter);
            let captured = SessionQueries {
                checker: &mut checker,
                handles,
            }
            .captured_function(
                ContextualProvider::function(4),
                &[(
                    ContextualProvider::binding(),
                    StaticGenericProvider::variable(3),
                )],
            );
            let raw = import_type(&mut checker, handles, &captured);
            let parameter = import_type(&mut checker, handles, &StaticGenericProvider::variable(3));
            let mapper = checker.types().mapper(vec![(parameter, TypeId::STRING)]);
            let mapped = checker.instantiate(raw, mapper);
            let mapped = export_type(&mut checker, handles, mapped);
            assert!(matches!(mapped, Type::SessionType(_)));
            let returned = SessionQueries {
                checker: &mut checker,
                handles,
            }
            .call(&mapped, &[]);
            assert!(returned.errors.is_empty(), "{returned:?}");
            assert_eq!(returned.return_type, Type::String);
            let annotation_only = SessionQueries {
                checker: &mut checker,
                handles,
            }
            .captured_function(StaticGenericProvider::function(3), &[]);
            let raw = import_type(&mut checker, handles, &annotation_only);
            let outer = import_type(&mut checker, handles, &StaticGenericProvider::variable(1));
            let mapper = checker.types().mapper(vec![(outer, TypeId::STRING)]);
            let mapped = checker.instantiate(raw, mapper);
            let mapped = export_type(&mut checker, handles, mapped);
            let signature = SessionQueries {
                checker: &mut checker,
                handles,
            }
            .function_signature(&mapped)
            .unwrap();
            assert_eq!(signature.params[0].ty, Type::String);
            assert_eq!(*signature.returns, Type::String);
        });
    }

    #[test]
    fn native_captured_outer_parameter_dependencies_do_not_fix_unrelated_return_inference() {
        Engine::new().with_native_provider(ContextualProvider::default, |_, queries| {
            let result = queries.call_expression_native(
                &Type::NativeFunction(StaticGenericProvider::function(4)),
                ContextualProvider::expression(0),
                &[
                    ContextualProvider::expression(12),
                    ContextualProvider::expression(13),
                ],
                &[],
                None,
            );
            assert!(result.errors.is_empty(), "{result:?}");
            let first = queries.call(&result.return_type, &[]);
            assert!(first.errors.is_empty(), "outer closure: {first:?}");
            let inner = queries.call(&first.return_type, &[]);
            assert!(inner.errors.is_empty(), "inner closure: {inner:?}");
            assert_eq!(inner.return_type, StaticGenericProvider::variable(0));
        });
    }

    #[test]
    fn native_captured_body_values_apply_simultaneous_substitution_once() {
        with_program(|program, handles| {
            let provider = ContextualProvider::default();
            let adapter = ProviderAdapter {
                provider: &provider,
                handles,
            };
            let mut checker = program.checker_with_native_provider(&adapter);
            let captured = SessionQueries {
                checker: &mut checker,
                handles,
            }
            .captured_function(
                ContextualProvider::function(4),
                &[
                    (
                        ContextualProvider::binding(),
                        StaticGenericProvider::variable(3),
                    ),
                    (
                        NativeBindingKey {
                            snapshot: 92,
                            index: 1,
                        },
                        StaticGenericProvider::variable(4),
                    ),
                ],
            );
            let raw = import_type(&mut checker, handles, &captured);
            let t = import_type(&mut checker, handles, &StaticGenericProvider::variable(3));
            let u = import_type(&mut checker, handles, &StaticGenericProvider::variable(4));
            let mapper = checker.types().mapper(vec![(t, u), (u, TypeId::NUMBER)]);
            let mapped = checker.instantiate(raw, mapper);
            let mapped = export_type(&mut checker, handles, mapped);
            let returned = SessionQueries {
                checker: &mut checker,
                handles,
            }
            .call(&mapped, &[]);
            assert!(returned.errors.is_empty(), "{returned:?}");
            assert_eq!(returned.return_type, StaticGenericProvider::variable(4));
        });
    }

    #[test]
    fn native_contextual_captured_parameters_keep_callsite_space_during_mapping_and_cloning() {
        with_program(|program, handles| {
            let provider = ContextualProvider::default();
            let adapter = ProviderAdapter {
                provider: &provider,
                handles,
            };
            let mut checker = program.checker_with_native_provider(&adapter);
            let captured = SessionQueries {
                checker: &mut checker,
                handles,
            }
            .captured_function(
                ContextualProvider::function(5),
                &[
                    (
                        ContextualProvider::binding(),
                        StaticGenericProvider::variable(3),
                    ),
                    (
                        NativeBindingKey {
                            snapshot: 92,
                            index: 1,
                        },
                        StaticGenericProvider::variable(4),
                    ),
                ],
            );
            let raw = import_type(&mut checker, handles, &captured);
            let t = import_type(&mut checker, handles, &StaticGenericProvider::variable(3));
            let u = import_type(&mut checker, handles, &StaticGenericProvider::variable(4));
            let mapper = checker.types().mapper(vec![(t, u), (u, TypeId::NUMBER)]);
            let source = checker.instantiate(raw, mapper);
            let source = export_type(&mut checker, handles, source);
            let expected = StaticGenericProvider::callback(
                Some(StaticGenericProvider::variable(4)),
                Type::Any,
            );
            let contextual = SessionQueries {
                checker: &mut checker,
                handles,
            }
            .contextual_function_value(
                &source,
                ContextualProvider::expression(2),
                &expected,
                NativeCheckMode::empty(),
            );
            let signature = SessionQueries {
                checker: &mut checker,
                handles,
            }
            .function_signature(&contextual)
            .unwrap();
            assert_eq!(signature.params[0].ty, StaticGenericProvider::variable(4));
            let raw = import_type(&mut checker, handles, &contextual);
            let ambient = checker.types().mapper(vec![(u, TypeId::STRING)]);
            let clone = checker.instantiate(raw, ambient);
            let clone = export_type(&mut checker, handles, clone);
            let mut queries = SessionQueries {
                checker: &mut checker,
                handles,
            };
            let signature = queries.function_signature(&clone).unwrap();
            assert_eq!(signature.params[0].ty, Type::String);
            assert_eq!(
                queries.property_type(&signature.returns, "argument"),
                Some(Type::String)
            );
            assert_eq!(
                queries.property_type(&signature.returns, "captured"),
                Some(Type::String)
            );
        });
    }

    #[test]
    fn native_provider_scopes_restore_frames_and_validate_capture_capabilities() {
        let capture =
            Engine::new().with_native_provider(ContextualProvider::default, |provider, queries| {
                let map = Type::NativeFunction(StaticGenericProvider::function(4));
                let returned = queries.call_expression_native(
                    &map,
                    ContextualProvider::expression(0),
                    &[
                        ContextualProvider::expression(1),
                        ContextualProvider::expression(11),
                    ],
                    &[],
                    None,
                );
                assert!(returned.errors.is_empty());
                assert!(queries.call(&returned.return_type, &[]).errors.is_empty());
                let context = provider.parameters.borrow().last().unwrap().0;
                let capture = provider.captures.borrow()[0];
                let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    queries.with_contextual_function(&context, &mut |queries| {
                        assert_eq!(queries.current_contextual_function(), Some(context));
                        queries.with_captured_function(&capture, &mut |queries| {
                            assert_eq!(queries.current_captured_function(), Some(capture));
                            std::panic::panic_any(String::from("scoped callback payload"));
                        });
                    })
                }))
                .unwrap_err();
                assert_eq!(
                    panic.downcast_ref::<String>().map(String::as_str),
                    Some("scoped callback payload")
                );
                assert_eq!(queries.current_captured_function(), None);
                assert_eq!(queries.current_contextual_function(), None);
                queries.with_contextual_function(&context, &mut |queries| {
                    let nested = queries.call_expression_native(
                        &map,
                        ContextualProvider::expression(8),
                        &[
                            ContextualProvider::expression(10),
                            ContextualProvider::expression(2),
                        ],
                        &[],
                        None,
                    );
                    assert!(nested.errors.is_empty(), "{nested:?}");
                    assert_eq!(nested.return_type, Type::Number);
                });
                assert_eq!(
                    queries.captured_binding(&capture, ContextualProvider::binding()),
                    Some(Type::Number)
                );
                capture
            });
        let panic = std::panic::catch_unwind(|| {
            Engine::new().with_session(|queries| {
                queries.captured_binding(&capture, ContextualProvider::binding())
            })
        })
        .unwrap_err();
        assert_eq!(
            panic.downcast_ref::<InvalidCapturedFunctionContext>(),
            Some(&InvalidCapturedFunctionContext(capture))
        );
        let foreign = NativeBindingKey {
            snapshot: 999,
            index: 0,
        };
        let panic = std::panic::catch_unwind(|| {
            Engine::new().with_native_provider(ContextualProvider::default, |_, queries| {
                queries
                    .captured_function(ContextualProvider::function(4), &[(foreign, Type::Number)])
            })
        })
        .unwrap_err();
        assert_eq!(
            panic.downcast_ref::<InvalidNativeBindingKey>(),
            Some(&InvalidNativeBindingKey(foreign))
        );
        assert!(Engine::new().is_assignable(&Type::Number, &Type::Number));
    }

    #[test]
    fn native_contextual_callbacks_fix_parameters_and_keep_distinct_contexts() {
        Engine::new().with_native_provider(ContextualProvider::default, |provider, queries| {
            let map = Type::NativeFunction(StaticGenericProvider::function(4));
            for (argument, expected) in [(1, Type::Number), (3, Type::String), (1, Type::Number)] {
                let result = queries.call_expression_native(
                    &map,
                    ContextualProvider::expression(0),
                    &[
                        ContextualProvider::expression(argument),
                        ContextualProvider::expression(2),
                    ],
                    &[],
                    None,
                );
                assert!(result.errors.is_empty(), "{result:?}");
                assert_eq!(result.return_type, expected);
                assert_eq!(queries.current_contextual_function(), None);
            }
            assert_eq!(provider.base_parameters.get(), 0);
            let parameters = provider.parameters.borrow();
            assert!(parameters.iter().any(|(_, ty)| *ty == Type::Number));
            assert!(parameters.iter().any(|(_, ty)| *ty == Type::String));
            assert!(parameters.windows(2).any(|pair| pair[0].0 != pair[1].0));
            assert!(
                provider
                    .callback_modes
                    .borrow()
                    .iter()
                    .all(|mode| !mode.contains(NativeCheckMode::SKIP_CONTEXT_SENSITIVE))
            );
        });
    }

    #[test]
    fn native_contextual_producer_consumer_and_nested_inference_share_one_checker() {
        Engine::new().with_native_provider(ContextualProvider::default, |provider, queries| {
            let consume = queries.call_expression_native(
                &Type::NativeFunction(StaticGenericProvider::function(5)),
                ContextualProvider::expression(6),
                &[
                    ContextualProvider::expression(4),
                    ContextualProvider::expression(5),
                ],
                &[],
                None,
            );
            assert!(consume.errors.is_empty(), "{consume:?}");
            assert_eq!(consume.return_type, Type::Number);
            let nested = queries.call_expression_native(
                &Type::NativeFunction(StaticGenericProvider::function(4)),
                ContextualProvider::expression(9),
                &[
                    ContextualProvider::expression(3),
                    ContextualProvider::expression(7),
                ],
                &[],
                None,
            );
            assert!(nested.errors.is_empty(), "{nested:?}");
            assert_eq!(nested.return_type, Type::String);
            assert_eq!(provider.base_parameters.get(), 0);
        });
    }

    #[test]
    fn native_contextual_capabilities_reject_another_session() {
        let context =
            Engine::new().with_native_provider(ContextualProvider::default, |provider, queries| {
                let result = queries.call_expression_native(
                    &Type::NativeFunction(StaticGenericProvider::function(4)),
                    ContextualProvider::expression(0),
                    &[
                        ContextualProvider::expression(1),
                        ContextualProvider::expression(2),
                    ],
                    &[],
                    None,
                );
                assert!(result.errors.is_empty());
                provider.parameters.borrow()[0].0
            });
        let panic = std::panic::catch_unwind(|| {
            Engine::new().with_session(|queries| queries.contextual_parameter_type(&context, 0))
        })
        .unwrap_err();
        assert_eq!(
            panic.downcast_ref::<InvalidContextualFunctionContext>(),
            Some(&InvalidContextualFunctionContext(context))
        );
        assert!(Engine::new().is_assignable(&Type::Number, &Type::Number));
    }

    #[test]
    fn native_expression_keys_validate_snapshots_and_noncallable_contexts_remain_errors() {
        let invalid = NativeExpressionKey {
            snapshot: 999,
            node: 0,
        };
        let panic = std::panic::catch_unwind(|| {
            Engine::new().with_native_provider(ContextualProvider::default, |_, queries| {
                queries.call_expression_native(
                    &Type::NativeFunction(StaticGenericProvider::function(4)),
                    invalid,
                    &[
                        ContextualProvider::expression(1),
                        ContextualProvider::expression(2),
                    ],
                    &[],
                    None,
                )
            })
        })
        .unwrap_err();
        assert_eq!(
            panic.downcast_ref::<InvalidNativeExpressionKey>(),
            Some(&InvalidNativeExpressionKey(invalid))
        );
        Engine::new().with_native_provider(ContextualProvider::default, |_, queries| {
            let result = queries.call_expression_native(
                &Type::Function(FunctionType {
                    params: vec![StaticGenericProvider::argument("number", Type::Number)],
                    returns: Box::new(Type::Void),
                }),
                ContextualProvider::expression(0),
                &[ContextualProvider::expression(2)],
                &[],
                None,
            );
            assert!(
                matches!(
                    result.errors.as_slice(),
                    [CallError::Argument { index: 0, .. }]
                ),
                "{result:?}"
            );
        });
    }

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
    fn native_function_identity_and_declared_type_views_do_not_demand_inferred_returns() {
        Engine::new().with_native_provider(SourceProvider::new, |provider, queries| {
            let recursive = Type::NativeFunction(SourceProvider::key(2));
            assert_eq!(
                queries.native_function_key(&recursive),
                Some(SourceProvider::key(2))
            );
            assert_eq!(
                queries.instantiate_in_function(&recursive, &Type::Never),
                Some(Type::Never)
            );
            assert_eq!(provider.calls[2].get(), 0);
            assert_eq!(provider.parameters[2].get(), 0);
            assert_eq!(provider.cycles.get(), 0);
        });
        with_program(|program, handles| {
            let provider = ContextualProvider::default();
            let adapter = ProviderAdapter {
                provider: &provider,
                handles,
            };
            let mut checker = program.checker_with_native_provider(&adapter);
            let captured = SessionQueries {
                checker: &mut checker,
                handles,
            }
            .captured_function(StaticGenericProvider::function(3), &[]);
            let raw = import_type(&mut checker, handles, &captured);
            let t = import_type(&mut checker, handles, &StaticGenericProvider::variable(1));
            let mapper = checker.types().mapper(vec![(t, TypeId::NEVER)]);
            let mapped = checker.instantiate(raw, mapper);
            let mapped = export_type(&mut checker, handles, mapped);
            let mut queries = SessionQueries {
                checker: &mut checker,
                handles,
            };
            assert_eq!(
                queries.native_function_key(&mapped),
                Some(StaticGenericProvider::function(3))
            );
            assert_eq!(
                queries.instantiate_in_function(&mapped, &StaticGenericProvider::variable(1)),
                Some(Type::Never)
            );
            assert!(provider.captures.borrow().is_empty());
        });
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
    fn literal_kind_and_base_queries_preserve_regular_and_union_boundaries() {
        Engine::new().with_session(|queries| {
            for (literal, expected) in [
                (Type::StringLiteral(b"tag".to_vec()), Type::String),
                (Type::NumberLiteral(1.0), Type::Number),
                (Type::BooleanLiteral(true), Type::Boolean),
            ] {
                let regular = queries.regular_type(&literal);
                assert!(queries.is_primitive_literal(&literal));
                assert!(queries.is_primitive_literal(&regular));
                assert_eq!(queries.base_of_literal(&literal), expected);
                assert_eq!(queries.base_of_literal(&regular), expected);
            }
            let one = queries.regular_type(&Type::NumberLiteral(1.0));
            let two = queries.regular_type(&Type::NumberLiteral(2.0));
            let union = queries.union_reduced(&[one.clone(), two]);
            assert!(!queries.is_primitive_literal(&union));
            assert_eq!(queries.base_of_literal(&union), Type::Number);
            let table = queries.table_type(&TableType {
                fields: vec![TableField::new(LuaKey::String(b"value".to_vec()), one)],
                ..TableType::default()
            }).unwrap();
            assert!(!queries.is_primitive_literal(&table));
            let untouched = queries.base_of_literal(&table);
            let field = queries.table_lookup(&untouched, &TableKey::Literal(LuaKey::String(b"value".to_vec()))).unwrap();
            assert!(queries.is_assignable(&field, &Type::NumberLiteral(1.0)));
            assert!(!queries.is_assignable(&field, &Type::NumberLiteral(2.0)));
        });
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
    fn native_calls_report_noncallable_and_choose_overloads() {
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
        let number = engine.call(&overloaded, &[Type::Number]);
        assert!(number.errors.is_empty());
        assert_eq!(number.return_type, Type::String);
        let string = engine.call(&overloaded, &[Type::String]);
        assert!(string.errors.is_empty());
        assert_eq!(string.return_type, Type::Number);
        assert!(!engine.call(&overloaded, &[Type::Boolean]).errors.is_empty());
    }

    #[test]
    fn native_call_boundary_rejects_this_and_checks_correlated_rest_metadata() {
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
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            assert_eq!(result.ret, TypeId::STRING);
            let alternate = checker.call_resolved_types(with_rest, &[TypeId::NUMBER, TypeId::STRING]);
            assert!(alternate.errors.is_empty(), "{:?}", alternate.errors);
            let crossed = checker.call_resolved_types(with_rest, &[TypeId::NUMBER, TypeId::NUMBER]);
            assert!(matches!(crossed.errors.as_slice(), [ResolvedTypeCallError::Argument { .. }]));
            let missing = checker.call_resolved_types(with_rest, &[TypeId::STRING]);
            assert!(matches!(missing.errors.as_slice(), [ResolvedTypeCallError::Arity { actual: 1, min: 2, max: Some(2) }]));
        });
    }
}

#[cfg(test)]
mod native_publication_tests;
