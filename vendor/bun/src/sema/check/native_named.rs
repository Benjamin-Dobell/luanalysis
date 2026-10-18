//! Lazy named declaration queries for a foreign frontend's original identities.
//!
//! Aliases resolve to their actual primitive, union or object TypeId. Structural object heads
//! carry a declaration and MapperId; they are published before any child annotation demand.
//! The ordinary query enter/leave/provisional/Stored machinery detects unproductive alias cycles.
//!
//! Raw member annotations remain in declaration space and are substituted once for an object
//! mapper. Literal member reads request only the selected field. Full structural demands build
//! ordinary NativeTable descriptions, preserving Lua numeric, boolean and typed key domains.
//! Source array heads retain their structural sequence form; intrinsic NativeArray remains available
//! to the existing rest/tuple normalization workers. Generic applications use fill_type_args,
//! mapper_from and type_alias_instantiation, including the ordinary instantiation cache.
//!
//! Source providers index names, scopes and recipes; they do not expand aliases or implement a
//! relation solver. Native members use the common recursive relation and invokeOnce inference
//! guards. Query::Shape and shape_memo own the string/index/signature view required by TS workers.
//!
//! Class heads retain their original declaration origin through native table materialization and
//! recursive relation workers. A class target requires that origin before ordinary member checks;
//! structural targets still accept compatible classes. Module heads, base inheritance and non-public
//! member visibility remain explicit unsupported errors. Explicit-owner augmentations resolve aliases through
//! the same declared-head query, retain ambiguity errors and skip unrelated canonical owners.

use crate::atom::Atom;
use crate::check::{Checker, Query};
use crate::types::{CloneIn, MapperId, NativeFunctionId, NativeSignatureId, NativeTableKey, NativeTypeParamId, Shape, SigData, SigId, TypeId, TypeData, NativeTableField, NativeTableIndexer, NativeTableFlavor};
use super::native_tables::{NativeTableAccessKey, NativeTableError};
use super::relate::{Relation, Relater, STATE_NONE};
use super::task::Stored;
use crate::util::FxHashMap;
#[path = "native_runtime_tables.rs"]
mod runtime_tables;

/// Original named declaration, or an anonymous object annotation head registered by the source.
/// File-local syntax ids must be mapped explicitly into this provider-owned namespace.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeTypeDeclarationId {
    pub snapshot: u64,
    pub index: u32,
}

impl crate::local::MaybeLocal for NativeTypeDeclarationId {
    fn is_local(&self) -> bool { false }
}

/// Original field/indexer declaration, including explicit-owner augmentations.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeTypeMemberId {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeDefinitionId { pub snapshot: u64, pub index: u32 }

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeDefinitionHeader { pub declaration: NativeTypeDeclarationId }

/// A definition value is an explicitly owned partial runtime table, not the full class schema.
/// `view` is immutable checked value space; its identity retains contextual/captured facets.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeDefinitionRef {
    pub owner: NativeDefinitionId,
    pub class: NativeObjectReference,
    pub view: TypeId,
}

/// Original Lua allocation schema and its distinct, immutable value view.
/// This carrier is structural; the schema identity never creates nominality.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeRuntimeTableRef {
    pub schema: NativeObjectReference,
    pub view: TypeId,
    /// The original checked constructor keeps written storage bounds even
    /// when the reached view narrows or deletes their current values.
    pub bounds: Option<TypeId>,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeRuntimeTableContext {
    pub contextual: Option<crate::types::NativeFunctionContext>,
    pub captured: Option<crate::types::NativeCapturedFunctionContext>,
}

/// Generic annotation scope. A named type does not borrow a fabricated function identity.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum NativeTypeDeclarationOwner {
    Function(NativeFunctionId),
    NamedType(NativeTypeDeclarationId),
    Signature(NativeSignatureId),
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum NativeTypeDeclarationKind {
    Alias,
    Shape,
    Class,
    Module,
    RuntimeTable,
}

impl NativeTypeDeclarationKind {
    pub fn is_object(self) -> bool {
        !matches!(self, Self::Alias)
    }

    pub fn is_nominal(self) -> bool {
        matches!(self, Self::Class)
    }
}

/// Original object annotation form, available before resolving any child type. A malformed
/// builtin table application must not become an empty object merely because it has no members.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum NativeObjectDeclarationForm {
    Record,
    Array,
    Table { type_argument_count: usize },
}

/// Metadata that may be requested without resolving aliases, member annotations or bases.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeTypeDeclarationHeader {
    pub kind: NativeTypeDeclarationKind,
    pub name: Atom,
    pub object_form: Option<NativeObjectDeclarationForm>,
    /// Actual existing native parameter ids, allocated across Function|NamedType owners.
    pub parameters: Vec<NativeTypeParamId>,
    pub outer_parameters: Vec<NativeTypeParamId>,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum NativeTypeMemberVisibility {
    Public,
    Protected,
    Private,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum NativeTypeMemberKey {
    Fixed(NativeTableKey),
    /// A typed Lua key domain. It never denotes singleton runtime reference identity.
    Indexer { sparse: bool },
}

/// Member origin is preserved independently of the object where an inherited member is read.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum NativeTypeMemberOwner {
    Declared(NativeTypeDeclarationId),
    /// An explicit-owner augmentation supplies original name-binding candidates. Resolving aliases
    /// and validating the resulting object owner belongs to the backend declaration query.
    Reference { name: Atom, candidates: Vec<NativeTypeDeclarationId> },
    RuntimeStore(super::native_flow::NativeFlowKey),
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeTypeMemberHeader {
    pub owner: NativeTypeMemberOwner,
    pub key: NativeTypeMemberKey,
    pub visibility: NativeTypeMemberVisibility,
    pub optional: bool,
    pub readonly: bool,
}

/// Integration keys for the actual Checker Query adapter. Mapping does not change the raw
/// declaration recipe; instantiated object shapes remain keyed by their native object TypeId.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum NativeTypeDeclarationQuery {
    OwnerScope(NativeTypeDeclarationId),
    DeclaredHead(NativeTypeDeclarationId),
    MemberType(NativeTypeMemberId),
    MemberContext(NativeTypeMemberId),
    IndexerKey(NativeTypeMemberId),
    MemberOwnerApplication(NativeTypeMemberId),
    MemberOwnerMatch { member: NativeTypeMemberId, owner: TypeId },
    Bases(NativeTypeDeclarationId),
    RuntimeDomain(NativeTypeDeclarationId),
    ObjectMembers(TypeId),
    ObjectValidation(TypeId),
    DefinitionContext(TypeId),
    RuntimeTableSeed(TypeId),
    RuntimeTableMember(TypeId, NativeTypeMemberId, bool),
    RuntimeTableWrittenContext(TypeId, NativeTableKey),
    RuntimeTableNumericContract(TypeId),
}

/// One original alias-head edge. The provider never follows the target alias.
pub enum NativeOwnerScopeRecipe {
    ResolvedHead,
    Original(NativeTypeDeclarationKind),
    AliasReference { candidates: Vec<NativeTypeDeclarationId> },
    UnsupportedAlias,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum NativeNamedTypeError {
    PendingContext { member: NativeTypeMemberId },
    InvalidDefinition(NativeDefinitionId),
    InvalidReceiver { declaration: NativeTypeDeclarationId, expected: TypeId, actual: TypeId },
    UnsupportedReceiverType { actual: TypeId },
    DefinitionOwner { owner: NativeDefinitionId, expected: NativeTypeDeclarationId, actual: TypeId },
    DefinitionInitializer { owner: NativeDefinitionId, actual: TypeId },
    DefinitionField { owner: NativeDefinitionId, error: NativeTableError },
    InvalidDeclaration(NativeTypeDeclarationId),
    InvalidMember(NativeTypeMemberId),
    InvalidMemberSignatures { member: NativeTypeMemberId, signature: Option<NativeSignatureId> },
    InvalidTypeParameter(NativeTypeParamId),
    UnknownOwner { member: NativeTypeMemberId, name: Atom },
    AmbiguousOwner { member: NativeTypeMemberId, name: Atom },
    TypeArgumentArity { declaration: NativeTypeDeclarationId, actual: usize, minimum: usize, maximum: usize },
    TypeArgumentConstraint { declaration: NativeTypeDeclarationId, index: usize, actual: TypeId, expected: TypeId },
    CircularAlias(NativeTypeDeclarationId),
    CircularBase(NativeTypeDeclarationId),
    InvalidBase { declaration: NativeTypeDeclarationId, index: usize, base: TypeId },
    IncompatibleOverride { declaration: NativeTypeDeclarationId, member: NativeTypeMemberId, base: TypeId, expected: TypeId, actual: TypeId },
    IncompatibleBase { declaration: NativeTypeDeclarationId, index: usize, base: TypeId, expected: TypeId, actual: TypeId },
    TooDeep(NativeTypeDeclarationQuery),
    UnsupportedKind { declaration: NativeTypeDeclarationId, kind: NativeTypeDeclarationKind },
    UnsupportedBases(NativeTypeDeclarationId),
    UnsupportedVisibility(NativeTypeMemberId),
    UnsupportedOwnerInstantiation(NativeTypeMemberId),
    Table { declaration: NativeTypeDeclarationId, error: NativeTableError },
}

/// A Lua operator a named type may declare (`---@operator`). Comparison
/// operators are `Less` and `LessEqual`; `>` and `>=` swap their operands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    FloorDivide,
    Modulo,
    Power,
    Concat,
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    ShiftLeft,
    ShiftRight,
    Negate,
    BitwiseNot,
    Length,
    Less,
    LessEqual,
}

/// Declaration provider contract for native named-type integration. Every returned type
/// is in declaration space, including member/indexer/bases queries. The backend owns resolving,
/// substitution, recursive publication, inference, relation checks and semantic diagnostics.
pub trait NativeTypeDeclarationProvider {
    /// Selected original nominal roots. Display names and object structure do
    /// not attest a Lua runtime category.
    fn runtime_type_declaration(&self, _: super::native_flow::NativeOpaqueRuntimeKind) -> Option<NativeTypeDeclarationId> { None }
    fn type_member_activation(&self, _: &mut Checker<'_, '_>, _: NativeTypeMemberId) -> NativeTypeMemberActivation { NativeTypeMemberActivation::Active }
    fn contains_class_definition(&self, _: NativeDefinitionId) -> bool { false }
    fn class_definition_header(&self, _: NativeDefinitionId) -> NativeDefinitionHeader { panic!("class definition provider absent") }
    fn runtime_table_constructor(&self, _: NativeTypeDeclarationId) -> Option<crate::types::NativeExpressionId> { None }
    fn runtime_table_constructor_owner(&self, _: NativeTypeDeclarationId) -> Option<crate::types::NativeFunctionId> { None }
    fn runtime_table_seed(&self, _: &mut Checker<'_, '_>, _: NativeTypeDeclarationId) -> Option<TypeId> { None }
    fn runtime_store_receiver(&self, _: &mut Checker<'_, '_>, _: super::native_flow::NativeFlowKey) -> Option<TypeId> { None }
    fn contains_type_declaration(&self, declaration: NativeTypeDeclarationId) -> bool;
    fn type_declaration_self_parameter(&self, _: NativeTypeDeclarationId) -> Option<NativeTypeParamId> { None }
    fn type_declaration_header(&self, checker: &Checker<'_, '_>, declaration: NativeTypeDeclarationId) -> NativeTypeDeclarationHeader;
    /// `type_declaration_header(..).kind`, for callers that need nothing else.
    fn type_declaration_kind(&self, checker: &Checker<'_, '_>, declaration: NativeTypeDeclarationId) -> NativeTypeDeclarationKind {
        self.type_declaration_header(checker, declaration).kind
    }
    fn type_declaration_owner_scope(&self, checker: &Checker<'_, '_>, declaration: NativeTypeDeclarationId) -> NativeOwnerScopeRecipe {
        let _ = (checker, declaration);
        NativeOwnerScopeRecipe::ResolvedHead
    }

    fn contains_type_member(&self, declaration: NativeTypeMemberId) -> bool;
    fn type_member_header(&self, checker: &Checker<'_, '_>, declaration: NativeTypeMemberId) -> NativeTypeMemberHeader;
    fn type_member_origin(&self, _: NativeTypeMemberId) -> crate::types::NativeTableFieldOrigin { crate::types::NativeTableFieldOrigin::Declared }
    fn type_member_effective_metadata(&self, _: &mut Checker<'_, '_>, member: NativeTypeMemberId) -> NativeTypeMemberEffectiveMetadata {
        NativeTypeMemberEffectiveMetadata { origin: self.type_member_origin(member), is_assignment: self.type_member_is_assignment(member) }
    }
    fn type_member_owner_application(&self, _: &mut Checker<'_, '_>, _: NativeTypeMemberId) -> Option<NativeMemberOwnerApplication> { None }
    fn type_member_is_assignment(&self, _: NativeTypeMemberId) -> bool { false }
    /// Pure written callable members, with no runtime implementation or body.
    fn type_member_signatures(&self, _: NativeTypeMemberId) -> Option<Vec<NativeSignatureId>> { None }
    /// Own declarations plus potential explicit-owner augmentations; the backend validates each
    /// original owner candidate before publishing or inheriting its member origin.
    fn type_declaration_members(&self, declaration: NativeTypeDeclarationId) -> Vec<NativeTypeMemberId>;
    fn type_declaration_call_signatures(&self, _: NativeTypeDeclarationId) -> Vec<NativeSignatureId> { Vec::new() }
    /// Declared operator heads (`---@operator add(Other): Result`), each with the
    /// other operand as its only parameter, or none for a unary operator.
    fn type_declaration_operator_signatures(&self, _: NativeTypeDeclarationId, _: NativeOperator) -> Vec<NativeSignatureId> { Vec::new() }

    /// Alias target only. Record/class/module heads are created by the checker from their header
    /// before it requests members, rather than supplied as an eagerly expanded source table.
    fn type_alias_target(&self, checker: &mut Checker<'_, '_>, declaration: NativeTypeDeclarationId) -> TypeId;
    fn type_member_type(&self, checker: &mut Checker<'_, '_>, declaration: NativeTypeMemberId) -> TypeId;
    fn type_indexer_key(&self, checker: &mut Checker<'_, '_>, declaration: NativeTypeMemberId) -> TypeId;

    fn type_declaration_bases(&self, _: &mut Checker<'_, '_>, _: NativeTypeDeclarationId) -> Vec<TypeId> {
        Vec::new()
    }

    /// Original scope metadata used by the same native parameter provider for constraints and
    /// defaults. Scope/name binding stays in the source registry; there is no frontend solver.
    fn type_parameter_owner(&self, declaration: NativeTypeParamId) -> Option<NativeTypeDeclarationOwner>;

    fn circular_type_alias(&self, _: NativeTypeDeclarationId) {}
    fn circular_type_base(&self, _: NativeTypeDeclarationId) {}
    fn type_declaration_too_deep(&self, _: NativeTypeDeclarationQuery) {}
    fn type_declaration_error(&self, _: &mut Checker<'_, '_>, _: NativeNamedTypeError) {}
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum NativeContextualTypeOwner { Pending, Absent, Owner(TypeId) }

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum NativeTypeMemberActivation { Pending, Active, Inactive }

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct NativeTypeMemberEffectiveMetadata {
    pub origin: crate::types::NativeTableFieldOrigin,
    pub is_assignment: bool,
}

impl NativeNamedTypeError {
    pub(super) fn table_error(self, ty: TypeId) -> NativeTableError {
        match self {
            Self::PendingContext { .. } => NativeTableError::PendingContext,
            _ => NativeTableError::NotTable { ty },
        }
    }
}

/// Constructor metadata for the native object TypeData. The mapper must preserve
/// identity pairs for all own/outer declaration parameters until actual substitutions are known.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeObjectReference {
    pub declaration: NativeTypeDeclarationId,
    pub mapper: MapperId,
    pub self_argument: Option<TypeId>,
    pub runtime_context: Option<NativeRuntimeTableContext>,
}

#[derive(Clone, Debug)]
pub struct NativeMemberOwnerApplication {
    pub reference: TypeId,
    pub parameters: Vec<NativeTypeParamId>,
}


/// These entries are checker-local because their provider belongs to that checker. Publication is
/// still permitted only by the common query machinery's Stored token. No unfinished member result
/// is silently retained as a completed declaration.
#[derive(Default)]
pub(super) struct NativeNamedCache {
    owner_scopes: FxHashMap<NativeTypeDeclarationId, Result<NativeTypeDeclarationId, NativeNamedTypeError>>,
    heads: FxHashMap<NativeTypeDeclarationId, Result<TypeId, NativeNamedTypeError>>,
    types: FxHashMap<NativeTypeDeclarationQuery, TypeId>,
    member_contexts: FxHashMap<NativeTypeMemberId, Option<TypeId>>,
    definition_contexts: FxHashMap<TypeId, Result<TypeId, NativeNamedTypeError>>,
    objects: FxHashMap<TypeId, Result<TypeId, NativeNamedTypeError>>,
    schemas: FxHashMap<TypeId, Result<TypeId, NativeNamedTypeError>>,
    bases: FxHashMap<NativeTypeDeclarationId, Result<Vec<TypeId>, NativeNamedTypeError>>,
    mapped_bases: FxHashMap<TypeId, Vec<TypeId>>,
    members: FxHashMap<TypeId, Vec<NativeObjectMember>>,
    owner_applications: FxHashMap<NativeTypeMemberId, Option<NativeMemberOwnerApplication>>,
}

#[derive(Clone)]
struct NativeObjectMember {
    member: NativeTypeMemberId,
    /// Further original declarations of the same pure callable own member.
    /// No synthetic member identity replaces these source origins.
    overloads: Vec<NativeTypeMemberId>,
    owner: NativeObjectReference,
    via: Option<(usize, TypeId)>,
}

impl NativeNamedCache {
    fn owner_scope(&mut self, declaration: NativeTypeDeclarationId, value: Result<NativeTypeDeclarationId, NativeNamedTypeError>, _: Stored) {
        self.owner_scopes.insert(declaration, value);
    }
    fn head(&mut self, key: NativeTypeDeclarationId, value: Result<TypeId, NativeNamedTypeError>, _: Stored) {
        self.heads.insert(key, value);
    }
    fn ty(&mut self, key: NativeTypeDeclarationQuery, value: TypeId, _: Stored) {
        self.types.insert(key, value);
    }
    fn member_context(&mut self, key: NativeTypeMemberId, value: Option<TypeId>, _: Stored) {
        self.member_contexts.insert(key, value);
    }
    fn definition_context(&mut self, key: TypeId, value: Result<TypeId, NativeNamedTypeError>, _: Stored) {
        self.definition_contexts.insert(key, value);
    }
    fn object(&mut self, key: TypeId, value: Result<TypeId, NativeNamedTypeError>, _: Stored) {
        self.objects.insert(key, value);
    }
    fn schema(&mut self, key: TypeId, value: Result<TypeId, NativeNamedTypeError>, _: Stored) {
        self.schemas.insert(key, value);
    }
    fn bases(&mut self, key: NativeTypeDeclarationId, value: Result<Vec<TypeId>, NativeNamedTypeError>, _: Stored) { self.bases.insert(key, value); }
    fn mapped_bases(&mut self, key: TypeId, value: Vec<TypeId>, _: Stored) { self.mapped_bases.insert(key, value); }
    fn members(&mut self, key: TypeId, value: Vec<NativeObjectMember>, _: Stored) { self.members.insert(key, value); }
}

impl<'p, 's> Checker<'p, 's> {
    pub fn set_native_type_declaration_provider(&mut self, provider: &'p dyn NativeTypeDeclarationProvider) {
        // Changing provider after a demand would invalidate the meaning of every native identity.
        assert!(self.native_type_declaration_provider.is_none(), "native declaration provider already installed");
        self.native_type_declaration_provider = Some(provider);
    }

    pub(super) fn native_named_provider(&self, declaration: NativeTypeDeclarationId) -> Result<&'p dyn NativeTypeDeclarationProvider, NativeNamedTypeError> {
        self.native_type_declaration_provider.filter(|p| p.contains_type_declaration(declaration))
            .ok_or(NativeNamedTypeError::InvalidDeclaration(declaration))
    }

    /// The original frozen owner recipe runs under the ordinary query guard.
    /// Pending is neither a negative owner result nor permission to publish a schema.
    pub fn native_contextual_type_owner(&mut self, scope: NativeFunctionId, input: super::native_flow::NativeFlowKey) -> NativeContextualTypeOwner {
        let Some(provider) = self.native_function_provider.filter(|provider| provider.contains_function(scope)) else { return NativeContextualTypeOwner::Absent };
        let epoch = self.flow_memo.native_state.effect_epoch;
        let key = (scope, input, epoch);
        if let Some(&owner) = self.flow_memo.native_state.contextual_owners.get(&key) {
            return owner.map_or(NativeContextualTypeOwner::Absent, NativeContextualTypeOwner::Owner);
        }
        let query = Query::NativeContextualTypeOwner(scope, input, epoch);
        if !self.enter(query) {
            self.mark_tainted_from(0);
            return NativeContextualTypeOwner::Pending;
        }
        let begun = self.begin_memoizable();
        let owner = provider.contextual_type_owner(self, scope, input);
        if owner == NativeContextualTypeOwner::Pending { self.mark_tainted_from(0); }
        let stored = self.end_memoizable(begun);
        let left = self.leave(query);
        if left.is_err() || stored.is_none() || self.is_stack_low() {
            self.mark_tainted_from(0);
            return NativeContextualTypeOwner::Pending;
        }
        let complete = match owner {
            NativeContextualTypeOwner::Pending => return owner,
            NativeContextualTypeOwner::Absent => None,
            NativeContextualTypeOwner::Owner(owner) => Some(owner),
        };
        self.flow_memo.native_state.contextual_owners.insert(key, complete);
        owner
    }

    /// Reuse the owner's existing semantic-self parameter and exact mapper.
    pub fn native_contextual_owner_self_type(&mut self, owner: TypeId) -> Option<TypeId> {
        let owner = self.native_runtime_carrier(owner);
        let reference = match *self.data(owner) {
            TypeData::NativeObject(reference) => reference,
            TypeData::NativeDefinition(reference) => reference.class,
            TypeData::NativeTypeParam { declaration, .. } => {
                let provider = self.native_type_declaration_provider?;
                let super::native_named::NativeTypeDeclarationOwner::NamedType(class) = provider.type_parameter_owner(declaration)? else { return None };
                return (provider.type_declaration_self_parameter(class) == Some(declaration)).then_some(owner);
            }
            _ => return None,
        };
        if let Some(receiver) = reference.self_argument { return Some(receiver) }
        let parameter = self.native_named_provider(reference.declaration).ok()?.type_declaration_self_parameter(reference.declaration)?;
        let mapping = self.types().mapping(reference.mapper).iter().copied().filter(|(from, to)| from != to).collect();
        Some(self.cloned_native_type_param(parameter, self.types().mapper(mapping)))
    }

    pub(super) fn report_native_named(&mut self, error: NativeNamedTypeError) {
        let Some(provider) = self.native_type_declaration_provider else { return };
        match error {
            NativeNamedTypeError::CircularAlias(id) => provider.circular_type_alias(id),
            NativeNamedTypeError::CircularBase(id) => provider.circular_type_base(id),
            NativeNamedTypeError::TooDeep(query) => provider.type_declaration_too_deep(query),
            _ => {}
        }
        provider.type_declaration_error(self, error);
    }

    fn native_owner_scope(&mut self, declaration: NativeTypeDeclarationId) -> Result<NativeTypeDeclarationId, NativeNamedTypeError> {
        if let Some(&known) = self.native_named_cache.owner_scopes.get(&declaration) { return known }
        let provider = self.native_named_provider(declaration)?;
        let demand = NativeTypeDeclarationQuery::OwnerScope(declaration);
        let query = Query::NativeNamed(demand);
        if !self.enter(query) {
            let error = if self.found_cycle { NativeNamedTypeError::CircularAlias(declaration) }
                else { NativeNamedTypeError::TooDeep(demand) };
            self.report_native_named(error);
            return Err(error);
        }
        let result = match provider.type_declaration_owner_scope(self, declaration) {
            NativeOwnerScopeRecipe::ResolvedHead => self.native_declared_head(declaration).and_then(|head| match self.data(head) {
                TypeData::NativeObject(reference) => Ok(reference.declaration),
                _ => Err(NativeNamedTypeError::InvalidDeclaration(declaration)),
            }),
            NativeOwnerScopeRecipe::Original(NativeTypeDeclarationKind::Class | NativeTypeDeclarationKind::Shape) => Ok(declaration),
            NativeOwnerScopeRecipe::Original(kind) => Err(NativeNamedTypeError::UnsupportedKind { declaration, kind }),
            NativeOwnerScopeRecipe::AliasReference { candidates } => match candidates.as_slice() {
                [target] => self.native_owner_scope(*target),
                _ => Err(NativeNamedTypeError::InvalidDeclaration(declaration)),
            },
            NativeOwnerScopeRecipe::UnsupportedAlias => Err(NativeNamedTypeError::InvalidDeclaration(declaration)),
        };
        let left = self.leave(query);
        let result = if self.left_a_cycle { Err(NativeNamedTypeError::CircularAlias(declaration)) } else { result };
        if let Ok(stored) = left {
            self.native_named_cache.owner_scope(declaration, result, stored);
        }
        if let Err(error) = result { self.report_native_named(error); }
        result
    }

    pub fn native_named_member_scope_owner(&mut self, member: NativeTypeMemberId) -> Result<NativeTypeDeclarationId, NativeNamedTypeError> {
        let result = self.native_named_member_scope_owner_unreported(member);
        if let Err(error) = result { self.report_native_named(error); }
        result
    }

    /// The member's scope owner without reporting a failure: the member's own
    /// validation reports it once. Probing whether another file's candidate
    /// belongs to an object must not report on every probe.
    fn native_named_member_scope_owner_unreported(&mut self, member: NativeTypeMemberId) -> Result<NativeTypeDeclarationId, NativeNamedTypeError> {
        let provider = self.native_type_declaration_provider.filter(|provider| provider.contains_type_member(member))
            .ok_or(NativeNamedTypeError::InvalidMember(member))?;
        let header = provider.type_member_header(self, member);
        let declaration = match header.owner {
            NativeTypeMemberOwner::Declared(owner) => owner,
            NativeTypeMemberOwner::RuntimeStore(_) => return Ok(self.native_named_member_origin(member)?.0.declaration),
            NativeTypeMemberOwner::Reference { name, candidates } => match candidates.as_slice() {
                [owner] => *owner,
                [] => return Err(NativeNamedTypeError::UnknownOwner { member, name }),
                _ => return Err(NativeNamedTypeError::AmbiguousOwner { member, name }),
            },
        };
        self.native_owner_scope(declaration)
    }

    pub fn native_named_member_owner(&mut self, member: NativeTypeMemberId) -> Result<(NativeTypeDeclarationId, TypeId), NativeNamedTypeError> {
        let result = self.native_named_member_origin(member)
            .map(|(owner, _)| (owner.declaration, self.intern(TypeData::NativeObject(owner))));
        if let Err(error) = result { self.report_native_named(error); }
        result
    }

    pub fn native_declared_head(&mut self, declaration: NativeTypeDeclarationId) -> Result<TypeId, NativeNamedTypeError> {
        let provider = self.native_named_provider(declaration).map_err(|error| { self.report_native_named(error); error })?;
        if let Some(known) = self.p.native_declared_heads.get(&self.task, &declaration) { return Ok(known) }
        if let Some(&known) = self.native_named_cache.heads.get(&declaration) { return known }
        let query = Query::NativeNamed(NativeTypeDeclarationQuery::DeclaredHead(declaration));
        if let Some(raw) = self.provisional(query) { return Ok(TypeId(raw as u32)) }
        if !self.enter(query) {
            let error = if self.found_cycle { NativeNamedTypeError::CircularAlias(declaration) }
                else { NativeNamedTypeError::TooDeep(NativeTypeDeclarationQuery::DeclaredHead(declaration)) };
            if !self.found_cycle { self.report_native_named(error) }
            return Err(error);
        }
        let header = provider.type_declaration_header(self, declaration);
        let result = match header.kind {
            NativeTypeDeclarationKind::Alias => Ok(provider.type_alias_target(self, declaration)),
            NativeTypeDeclarationKind::Shape | NativeTypeDeclarationKind::Class | NativeTypeDeclarationKind::RuntimeTable => {
                if header.kind == NativeTypeDeclarationKind::RuntimeTable && provider.runtime_table_constructor(declaration).is_none() {
                    Err(NativeNamedTypeError::InvalidDeclaration(declaration))
                } else if let Some(NativeObjectDeclarationForm::Table { type_argument_count }) = header.object_form
                    && type_argument_count != 2 {
                    Err(NativeNamedTypeError::TypeArgumentArity { declaration, actual: type_argument_count, minimum: 2, maximum: 2 })
                } else {
                let parameters: Result<Vec<_>, _> = header.outer_parameters.iter().chain(&header.parameters)
                    .map(|&id| self.native_type_parameter(id).ok_or(NativeNamedTypeError::InvalidTypeParameter(id))).collect();
                parameters.map(|parameters| {
                    let mapper = self.mapper_from(&parameters, &parameters);
                    self.intern(TypeData::NativeObject(NativeObjectReference { declaration, mapper, self_argument: None, runtime_context: None }))
                })
                }
            }
            kind => Err(NativeNamedTypeError::UnsupportedKind { declaration, kind }),
        };
        let left = self.leave(query);
        if self.left_a_cycle {
            let error = NativeNamedTypeError::CircularAlias(declaration);
            self.report_native_named(error);
            self.native_named_cache.head(declaration, Err(error), self.cycle_result());
            return Err(error);
        }
        match left {
            Ok(stored) => {
                if let Ok(ty) = result { self.p.native_declared_heads.insert(&self.task, declaration, ty, stored); }
                self.native_named_cache.head(declaration, result, stored);
            }
            Err(open) => if let Ok(ty) = result { self.cache_provisionally(query, u64::from(ty.0), open) },
        }
        if let Err(error) = result { self.report_native_named(error) }
        result
    }

    /// Own parameters receive arguments; outer parameters pass through unchanged. Default filling,
    /// constraint substitution and alias instantiation are the same workers used for TS aliases.
    pub fn native_type_reference(&mut self, declaration: NativeTypeDeclarationId, arguments: &[TypeId]) -> Result<TypeId, NativeNamedTypeError> {
        let provider = self.native_named_provider(declaration).map_err(|error| { self.report_native_named(error); error })?;
        let header = provider.type_declaration_header(self, declaration);
        let parameters: Vec<_> = header.parameters.iter().map(|&id| self.native_type_parameter(id)
            .ok_or(NativeNamedTypeError::InvalidTypeParameter(id))).collect::<Result<_, _>>()
            .map_err(|error| { self.report_native_named(error); error })?;
        let minimum = parameters.iter().rposition(|&param| self.default_of_type_param(param).is_none()).map_or(0, |i| i + 1);
        if arguments.len() < minimum || arguments.len() > parameters.len() {
            let error = NativeNamedTypeError::TypeArgumentArity { declaration, actual: arguments.len(), minimum, maximum: parameters.len() };
            self.report_native_named(error);
            return Err(error);
        }
        let filled = self.fill_type_args(&parameters, arguments);
        let mapper = self.mapper_from(&parameters, &filled);
        // As `getTypeFromTypeReference`, resolution does not check constraints: that is the
        // separate `checkTypeArgumentConstraints` of the reference's own check
        // (`native_type_reference_constraint_error`). Checking here made a reference resolved
        // inside a cycle fail as a whole, so the result depended on evaluation order.
        let declared = self.native_declared_head(declaration)?;
        Ok(self.type_alias_instantiation(declared, mapper))
    }

    /// `checkTypeArgumentConstraints` for a written reference: the first argument that does not
    /// satisfy its parameter's constraint. Arity errors belong to resolution.
    pub fn native_type_reference_constraint_error(&mut self, declaration: NativeTypeDeclarationId, arguments: &[TypeId]) -> Option<NativeNamedTypeError> {
        let provider = self.native_named_provider(declaration).ok()?;
        let header = provider.type_declaration_header(self, declaration);
        let parameters: Vec<_> = header.parameters.iter().map(|&id| self.native_type_parameter(id)).collect::<Option<_>>()?;
        let filled = self.fill_type_args(&parameters, arguments);
        let mapper = self.mapper_from(&parameters, &filled);
        for (index, (&parameter, &actual)) in parameters.iter().zip(&filled).enumerate() {
            // A declaration parameter is assumed to satisfy its own bound (Node<T>'s F-bound).
            if actual == parameter { continue }
            if let Some(constraint) = self.constraint_of_type_param(parameter) {
                let expected = self.instantiate(constraint, mapper);
                if !self.is_assignable(actual, expected) {
                    return Some(NativeNamedTypeError::TypeArgumentConstraint { declaration, index, actual, expected });
                }
            }
        }
        None
    }

    /// Validate a declaration template without requiring actual arguments for its own parameters.
    /// Head and member demands still use their normal lazy queries, and recursive child references
    /// obtain the published head rather than recursively expanding the declaration.
    pub fn validate_native_type_declaration(&mut self, declaration: NativeTypeDeclarationId) -> Result<TypeId, NativeNamedTypeError> {
        let head = self.native_declared_head(declaration)?;
        let provider = self.native_named_provider(declaration)?;
        for id in provider.type_declaration_header(self, declaration).parameters {
            let parameter = self.native_type_parameter(id).ok_or(NativeNamedTypeError::InvalidTypeParameter(id))?;
            self.constraint_of_type_param(parameter);
            self.default_of_type_param(parameter);
        }
        self.native_object_resolved(head)?;
        Ok(head)
    }

    /// Stores reuse existing written contracts from their owner and its bases,
    /// never their own initializer type. Keeping this query separate avoids a
    /// self-referential RHS demand while preserving each owner's mapper.
    pub fn native_named_member_context(&mut self, member: NativeTypeMemberId) -> Result<Option<TypeId>, NativeNamedTypeError> {
        if self.native_type_declaration_provider.is_some_and(|provider| matches!(provider.type_member_header(self, member).owner, NativeTypeMemberOwner::RuntimeStore(_))) { return Ok(None) }
        if let Some(&known) = self.native_named_cache.member_contexts.get(&member) { return Ok(known) }
        let native_query = NativeTypeDeclarationQuery::MemberContext(member);
        let query = Query::NativeNamed(native_query);
        if !self.enter(query) { return Err(NativeNamedTypeError::TooDeep(native_query)) }
        let result = (|| {
            let (owner, header) = self.native_named_member_origin(member)?;
            let provider = self.native_named_provider(owner.declaration)?;
            let key = match header.key {
                NativeTypeMemberKey::Fixed(key) => NativeTableAccessKey::Literal(key),
                NativeTypeMemberKey::Indexer { .. } => {
                    let raw = self.native_named_member_demand(member, true)?;
                    NativeTableAccessKey::Type(self.instantiate(raw, owner.mapper))
                }
            };
            let mut contexts = Vec::new();
            if provider.type_member_is_assignment(member)
                && let NativeTableAccessKey::Literal(key) = key
            {
                let key = key.normalize().map_err(|error| NativeNamedTypeError::Table { declaration: owner.declaration, error })?;
                for declared in provider.type_declaration_members(owner.declaration) {
                    if declared == member || provider.type_member_is_assignment(declared)
                        || provider.type_member_origin(declared) != crate::types::NativeTableFieldOrigin::Declared { continue }
                    let metadata = provider.type_member_header(self, declared);
                    let matches = match metadata.key {
                        NativeTypeMemberKey::Fixed(other) => other.normalize() == Ok(key)
                            && self.native_named_member_header(owner, declared)?.is_some(),
                        NativeTypeMemberKey::Indexer { .. } => {
                            if self.native_named_member_header(owner, declared)?.is_none() { continue }
                            let Some(capture) = self.native_member_capture_mapper(owner, declared)? else { continue };
                            let raw = self.native_named_member_demand(declared, true)?;
                            let captured = self.instantiate(raw, capture);
                            let expected = self.native_member_type(captured, owner);
                            self.is_assignable(self.native_table_key_type(key), expected)
                        },
                    };
                    if !matches { continue }
                    let origin = NativeObjectMember { member: declared, overloads: Vec::new(), owner, via: None };
                    if let Some(raw) = self.native_object_member_demand(&origin)? {
                        let value = self.native_member_type(raw, owner);
                        contexts.push(if metadata.optional { self.optional(value) } else { value });
                    }
                }
            }
            for base in self.native_object_bases(owner)? {
                if let Some(context) = self.native_table_value_context(base, key) { contexts.push(context); }
            }
            Ok(if contexts.is_empty() { None } else { Some(self.intersection(&contexts)) })
        })();
        if let Ok(stored) = self.leave(query) {
            if let Ok(context) = result { self.native_named_cache.member_context(member, context, stored); }
        }
        if let Err(error) = result { self.report_native_named(error) }
        result
    }

    fn native_named_member_demand(&mut self, member: NativeTypeMemberId, key: bool) -> Result<TypeId, NativeNamedTypeError> {
        let provider = self.native_type_declaration_provider.filter(|p| p.contains_type_member(member))
            .ok_or(NativeNamedTypeError::InvalidMember(member))?;
        let native_query = if key { NativeTypeDeclarationQuery::IndexerKey(member) } else { NativeTypeDeclarationQuery::MemberType(member) };
        if let Some(&known) = self.native_named_cache.types.get(&native_query) { return Ok(known) }
        let query = Query::NativeNamed(native_query);
        if let Some(raw) = self.provisional(query) { return Ok(TypeId(raw as u32)) }
        if !self.enter(query) {
            // tsgo resolves a member's type only where it is read, so a class's definition table,
            // built for a read in one of the class's own stores (`m.x = m.y`), never requests the
            // member being inferred. Here the table is built whole, and that member is unresolved
            // in it: the cycle leaves what is built in the meantime provisional.
            if !key && self.is_reentry(query) { return Ok(TypeId::UNRESOLVED) }
            let error = NativeNamedTypeError::TooDeep(native_query);
            self.report_native_named(error);
            return Err(error);
        }
        let result = if key { Ok(provider.type_indexer_key(self, member)) } else if let Some(signatures) = provider.type_member_signatures(member) {
            if signatures.is_empty() {
                Err(NativeNamedTypeError::InvalidMemberSignatures { member, signature: None })
            } else {
                self.native_signature_group_type(&signatures).map_err(|signature| NativeNamedTypeError::InvalidMemberSignatures { member, signature: Some(signature) })
            }
        } else { Ok(provider.type_member_type(self, member)) };
        match self.leave(query) {
            Ok(stored) => if let Ok(result) = result { self.native_named_cache.ty(native_query, result, stored) },
            Err(open) => if let Ok(result) = result { self.cache_provisionally(query, u64::from(result.0), open) },
        }
        if let Err(error) = result { self.report_native_named(error); }
        result
    }

    fn native_named_member_active(&mut self, member: NativeTypeMemberId) -> Result<bool, NativeNamedTypeError> {
        let provider = self.native_type_declaration_provider.ok_or(NativeNamedTypeError::InvalidMember(member))?;
        let scope = self.begin_scope();
        let (activation, stored) = self.run_memoizable(|checker| provider.type_member_activation(checker, member));
        let settled = self.end_scope(scope).is_ok();
        match if settled && stored.is_some() { activation } else { NativeTypeMemberActivation::Pending } {
            NativeTypeMemberActivation::Active => Ok(true),
            NativeTypeMemberActivation::Inactive => Ok(false),
            NativeTypeMemberActivation::Pending => {
                self.mark_tainted_from(0);
                Err(NativeNamedTypeError::PendingContext { member })
            }
        }
    }

    fn native_named_member_effective_metadata(&mut self, member: NativeTypeMemberId) -> Result<NativeTypeMemberEffectiveMetadata, NativeNamedTypeError> {
        let provider = self.native_type_declaration_provider.ok_or(NativeNamedTypeError::InvalidMember(member))?;
        let scope = self.begin_scope();
        let (metadata, stored) = self.run_memoizable(|checker| provider.type_member_effective_metadata(checker, member));
        if self.end_scope(scope).is_err() || stored.is_none() {
            self.mark_tainted_from(0);
            return Err(NativeNamedTypeError::PendingContext { member });
        }
        Ok(metadata)
    }

    fn native_member_owner_application(&mut self, member: NativeTypeMemberId) -> Result<Option<NativeMemberOwnerApplication>, NativeNamedTypeError> {
        if let Some(known) = self.native_named_cache.owner_applications.get(&member) { return Ok(known.clone()) }
        let provider = self.native_type_declaration_provider.ok_or(NativeNamedTypeError::InvalidMember(member))?;
        let native_query = NativeTypeDeclarationQuery::MemberOwnerApplication(member);
        let query = Query::NativeNamed(native_query);
        if !self.enter(query) {
            let error = if self.found_cycle { NativeNamedTypeError::PendingContext { member } } else { NativeNamedTypeError::TooDeep(native_query) };
            self.mark_tainted_from(0);
            return Err(error);
        }
        let serial = self.work;
        let (application, completed) = self.run_memoizable(|checker| provider.type_member_owner_application(checker, member));
        match self.leave(query) {
            Ok(_) if completed.is_some() => { self.native_named_cache.owner_applications.insert(member, application.clone()); }
            // Completed under a cycle that is still in progress further down, an application it
            // found is provisional, as a member's type is (`native_named_member_demand`), and the
            // cycle has left its callers provisional too. Finding none there is no evidence that
            // the member has none. Under a context that is not cacheable, with nothing unsettled
            // since, the application is only not cacheable.
            Err(_) if completed.is_some() && (application.is_some() || self.lowest_taint_since(serial) == usize::MAX) => {}
            _ => {
                // What that cycle left in progress above it is provisional already. Nothing
                // below it depends on this application, or any other unsettled one.
                let from = match self.lowest_taint_since(serial) { usize::MAX => 0, from => from.min(self.frames.len()) };
                self.mark_tainted_from(from);
                return Err(NativeNamedTypeError::PendingContext { member });
            }
        }
        Ok(application)
    }

    fn native_member_capture_mapper(&mut self, reference: NativeObjectReference, member: NativeTypeMemberId) -> Result<Option<MapperId>, NativeNamedTypeError> {
        let provider = self.native_type_declaration_provider.ok_or(NativeNamedTypeError::InvalidMember(member))?;
        let metadata = provider.type_member_header(self, member);
        // A canonical written owner with no application needs no matching
        // query. The ordinary member mapper supplies its class arguments.
        // Keep the application demand guarded: a typed-local contribution may
        // share this owner while still requiring its captured application.
        if matches!(metadata.owner, NativeTypeMemberOwner::Declared(owner) if owner == reference.declaration)
            && self.native_member_owner_application(member)?.is_none()
        {
            return Ok(Some(MapperId::IDENTITY));
        }
        let owner = self.intern(TypeData::NativeObject(reference));
        let query = Query::NativeNamed(NativeTypeDeclarationQuery::MemberOwnerMatch { member, owner });
        if !self.enter(query) {
            self.mark_tainted_from(0);
            return Err(NativeNamedTypeError::PendingContext { member });
        }
        let (result, completed) = self.run_memoizable(|checker| checker.native_member_capture_mapper_uncached(reference, member));
        let ended = self.leave(query);
        if result.is_ok() && (completed.is_none() || ended.is_err()) {
            self.mark_tainted_from(0);
            return Err(NativeNamedTypeError::PendingContext { member });
        }
        result
    }

    fn native_member_capture_mapper_uncached(&mut self, reference: NativeObjectReference, member: NativeTypeMemberId) -> Result<Option<MapperId>, NativeNamedTypeError> {
        use super::infer::{Inference, PRIORITY_ALWAYS_STRICT, PRIORITY_NO_CONSTRAINTS};
        let provider = self.native_type_declaration_provider.ok_or(NativeNamedTypeError::InvalidMember(member))?;
        let metadata = provider.type_member_header(self, member);
        if matches!(metadata.owner, NativeTypeMemberOwner::RuntimeStore(_)) { return Ok(Some(MapperId::IDENTITY)) }
        let application = self.native_member_owner_application(member)?;
        // Ordinary class-owned written contracts already use the canonical
        // class formals. The normal member mapper supplies their arguments;
        // discovering implementations or inferring an owner is unnecessary.
        if application.is_none() && matches!(metadata.owner, NativeTypeMemberOwner::Declared(owner) if owner == reference.declaration) {
            return Ok(Some(MapperId::IDENTITY));
        }
        let (owner, _) = self.native_named_member_origin(member)?;
        if owner.declaration != reference.declaration { return Ok(None) }
        let has_application = application.is_some();
        let eligible = if let Some(application) = application {
            application.parameters
        } else {
            match metadata.owner {
                NativeTypeMemberOwner::Reference { candidates, .. } => match candidates.as_slice() {
                    [alias] => provider.type_declaration_header(self, *alias).outer_parameters,
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            }
        };
        for &parameter in &eligible {
            self.native_type_parameter(parameter).ok_or(NativeNamedTypeError::InvalidTypeParameter(parameter))?;
        }
        let header = provider.type_declaration_header(self, reference.declaration);
        let mut class_parameters = header.parameters;
        class_parameters.extend(header.outer_parameters);
        let mut excluded = class_parameters.clone();
        excluded.extend(provider.type_declaration_self_parameter(reference.declaration));
        let mut equations = Vec::new();
        let mut parameters = Vec::new();
        for parameter in class_parameters {
            let formal = self.native_type_parameter(parameter).ok_or(NativeNamedTypeError::InvalidTypeParameter(parameter))?;
            let actual = self.instantiate(formal, reference.mapper);
            let written = self.instantiate(formal, owner.mapper);
            let written = self.instantiate(written, reference.mapper);
            equations.push((actual, written));
            if self.related(actual, written, Relation::Identity) { continue }
            self.any_type_in(written, true, |part| {
                if let TypeData::NativeTypeParam { declaration, .. } = self.data(part)
                    && eligible.contains(declaration) && !excluded.contains(declaration)
                    && !parameters.contains(&part)
                { parameters.push(part); }
                false
            });
        }
        let mut inference = Inference::for_params(&parameters, None);
        inference.around = reference.mapper;
        for &(actual, written) in &equations {
            self.infer(&mut inference, actual, written, PRIORITY_NO_CONSTRAINTS | PRIORITY_ALWAYS_STRICT);
        }
        let mut pairs = Vec::new();
        for (index, &parameter) in parameters.iter().enumerate() {
            let candidate = &inference.candidates[index];
            if candidate.covariant.is_empty() && candidate.contravariant.is_empty() && candidate.fixed.is_none() { return Ok(None) }
            // A declared subclass satisfies its ancestor without comparing members: a structural
            // comparison here needed the member schema that is being resolved.
            let inferred = self.get_inferred_type_comparing(&inference, index, false, &mut |checker, source, target| {
                checker.native_nominally_derived(source, target) || checker.is_assignable(source, target)
            });
            pairs.push((parameter, inferred));
            if let TypeData::NativeTypeParam { declaration, .. } = *self.data(parameter) {
                let original = self.native_type_parameter(declaration).ok_or(NativeNamedTypeError::InvalidTypeParameter(declaration))?;
                if original != parameter && !pairs.iter().any(|pair| pair.0 == original) { pairs.push((original, inferred)); }
            }
        }
        // An applied instance's physical receiver/return annotation can use an
        // existing mapped semantic-self parameter. Move that exact instance to
        // the accessed class scope; the ordinary member mapper binds its self
        // to the actual (possibly derived) receiver once, after this mapping.
        if has_application && let Some(self_id) = provider.type_declaration_self_parameter(owner.declaration) {
            let declared = self.native_type_parameter(self_id).ok_or(NativeNamedTypeError::InvalidTypeParameter(self_id))?;
            let owner_mapping = self.types().mapping(owner.mapper).iter().copied().filter(|(from, to)| from != to).collect();
            let original = self.cloned_native_type_param(self_id, self.types().mapper(owner_mapping));
            if original != declared {
                let accessed = self.cloned_native_type_param(self_id, reference.mapper);
                pairs.push((original, accessed));
            }
        }
        let mapper = self.types().mapper(pairs);
        for (actual, written) in equations {
            let matched = self.instantiate(written, mapper);
            if !self.related(actual, matched, Relation::Identity) { return Ok(None) }
        }
        Ok(Some(mapper))
    }

    fn native_object_member_effective_origin(&mut self, origin: &NativeObjectMember) -> Result<Option<crate::types::NativeTableFieldOrigin>, NativeNamedTypeError> {
        use crate::types::NativeTableFieldOrigin::{Declared, Inferred};
        let mut effective = None;
        for member in std::iter::once(origin.member).chain(origin.overloads.iter().copied()) {
            if !self.native_named_member_active(member)? { continue }
            let metadata = self.native_named_member_effective_metadata(member)?;
            if metadata.is_assignment && self.native_named_member_context(member)?.is_some() { continue }
            effective = Some(if metadata.origin == Declared || effective == Some(Declared) { Declared } else { Inferred });
        }
        Ok(effective)
    }

    fn native_object_member_demand(&mut self, origin: &NativeObjectMember) -> Result<Option<TypeId>, NativeNamedTypeError> {
        let mut active = Vec::new();
        for member in std::iter::once(origin.member).chain(origin.overloads.iter().copied()) {
            if !self.native_named_member_active(member)? { continue }
            // A store governed by an existing contract is not another schema declaration.
            if self.native_named_member_effective_metadata(member)?.is_assignment && self.native_named_member_context(member)?.is_some() { continue }
            let Some(mapper) = self.native_member_capture_mapper(origin.owner, member)? else { continue };
            active.push((member, mapper));
        }
        let Some(&(first, mapper)) = active.first() else { return Ok(None) };
        if origin.overloads.is_empty() {
            let raw = self.native_named_member_demand(first, false)?;
            return Ok(Some(self.instantiate(raw, mapper)));
        }
        let mut shape = Shape::new_in(self.arena);
        for (member, mapper) in active {
            // Each contributor retains its original guarded query, including
            // diagnostics for an invalid signature from a later source file.
            let raw = self.native_named_member_demand(member, false)?;
            let mapped = self.instantiate(raw, mapper);
            shape.call.extend(self.signatures(mapped, false).iter().copied());
        }
        Ok(Some(self.synth(shape)))
    }

    fn native_named_member_origin(&mut self, member: NativeTypeMemberId) -> Result<(NativeObjectReference, NativeTypeMemberHeader), NativeNamedTypeError> {
        let provider = self.native_type_declaration_provider.ok_or(NativeNamedTypeError::InvalidMember(member))?;
        if !provider.contains_type_member(member) { return Err(NativeNamedTypeError::InvalidMember(member)) }
        let header = provider.type_member_header(self, member);
        if header.visibility != NativeTypeMemberVisibility::Public { return Err(NativeNamedTypeError::UnsupportedVisibility(member)) }
        let mut owner = match &header.owner {
            NativeTypeMemberOwner::Declared(owner) => {
                let head = self.native_declared_head(*owner)?;
                match *self.data(head) {
                    TypeData::NativeObject(reference) => reference,
                    _ => return Err(NativeNamedTypeError::InvalidMember(member)),
                }
            },
            NativeTypeMemberOwner::Reference { name, candidates } => match candidates.as_slice() {
                [] => return Err(NativeNamedTypeError::UnknownOwner { member, name: *name }),
                [owner] => {
                    let declared = provider.type_declaration_header(self, *owner);
                    if declared.kind == NativeTypeDeclarationKind::Alias && !declared.parameters.is_empty() {
                        return Err(NativeNamedTypeError::UnsupportedOwnerInstantiation(member));
                    }
                    let target = self.native_declared_head(*owner)?;
                    match self.data(target) {
                        TypeData::NativeObject(reference) => {
                            let reference = *reference;
                            let scope = self.native_owner_scope(*owner)?;
                            if reference.declaration != scope {
                                return Err(NativeNamedTypeError::InvalidMember(member));
                            }
                            reference
                        },
                        _ => return Err(NativeNamedTypeError::UnknownOwner { member, name: *name }),
                    }
                }
                _ => return Err(NativeNamedTypeError::AmbiguousOwner { member, name: *name }),
            },
            NativeTypeMemberOwner::RuntimeStore(store) => {
                let receiver = provider.runtime_store_receiver(self, *store).ok_or(NativeNamedTypeError::InvalidMember(member))?;
                self.native_runtime_table_ref(receiver).ok_or(NativeNamedTypeError::InvalidMember(member))?.schema
            },
        };
        if let Some(application) = self.native_member_owner_application(member)? {
            let TypeData::NativeObject(reference) = *self.data(application.reference) else { return Err(NativeNamedTypeError::InvalidMember(member)) };
            if reference.declaration != owner.declaration { return Err(NativeNamedTypeError::InvalidMember(member)) }
            owner = reference;
        }
        Ok((owner, header))
    }

    fn native_named_member_header(&mut self, reference: NativeObjectReference, member: NativeTypeMemberId) -> Result<Option<NativeTypeMemberHeader>, NativeNamedTypeError> {
        let provider = self.native_type_declaration_provider.ok_or(NativeNamedTypeError::InvalidMember(member))?;
        let header = provider.type_member_header(self, member);
        if let NativeTypeMemberOwner::RuntimeStore(store) = header.owner {
            let receiver = self.with_runtime_table_context(reference, |checker| provider.runtime_store_receiver(checker, store)).ok_or(NativeNamedTypeError::InvalidMember(member))?;
            return Ok(self.native_runtime_receiver_matches(receiver, reference).then_some(header));
        }
        // Unrelated augmentation arguments must not be resolved while checking
        // another owner's constraints. Their original scope identity is enough.
        let candidate = match &header.owner {
            NativeTypeMemberOwner::Declared(owner) => Some(*owner),
            NativeTypeMemberOwner::Reference { candidates, .. } => match candidates.as_slice() { [owner] => Some(*owner), _ => None },
            NativeTypeMemberOwner::RuntimeStore(_) => None,
        };
        // A candidate whose scope owner cannot be determined (an augmentation of
        // an unsupported alias) is not this owner's member. Its own declaration
        // check reports the error; propagating it here failed every object
        // resolved while that candidate was listed.
        if let Some(candidate) = candidate
            && !matches!(provider.type_declaration_owner_scope(self, candidate), NativeOwnerScopeRecipe::ResolvedHead)
            && self.native_named_member_scope_owner_unreported(member).ok() != Some(reference.declaration)
        { return Ok(None) }
        let (owner, _) = self.native_named_member_origin(member)?;
        if owner.declaration != reference.declaration { return Ok(None) }
        Ok(Some(header))
    }

    /// Validate a raw field or augmentation even if no object demand would otherwise reach it.
    /// Alias owners resolve through the same declared-head query as ordinary references.
    pub fn validate_native_type_member(&mut self, member: NativeTypeMemberId) -> Result<TypeId, NativeNamedTypeError> {
        let result = (|| {
            let (owner, header) = self.native_named_member_origin(member)?;
            let raw = self.native_named_member_demand(member, false)?;
            let value = self.instantiate(raw, owner.mapper);
            if let NativeTypeMemberKey::Indexer { sparse } = header.key {
                let raw_key = self.native_named_member_demand(member, true)?;
                let key = self.instantiate(raw_key, owner.mapper);
                self.native_table(&[], &[NativeTableIndexer { key, value, readonly: header.readonly, sparse, origin: crate::types::NativeTableFieldOrigin::Declared }], NativeTableFlavor::Declared)
                    .map_err(|error| NativeNamedTypeError::Table { declaration: owner.declaration, error })?;
            }
            Ok(value)
        })();
        if let Err(error) = result { self.report_native_named(error) }
        result
    }

    pub(super) fn native_object_is_array(&self, reference: NativeObjectReference) -> bool {
        self.native_named_provider(reference.declaration).is_ok_and(|p| p.type_declaration_header(self, reference.declaration).object_form == Some(NativeObjectDeclarationForm::Array))
    }

    /// Nominal provenance is a declaration fact, never inferred from object fields. Materialized
    /// class descriptions retain the same fact so native key relations cannot erase it.
    pub(super) fn native_nominal_origin(&self, ty: TypeId) -> Option<NativeObjectReference> {
        match *self.data(ty) {
            TypeData::NativeDefinition(reference) => Some(reference.class),
            TypeData::NativeObject(reference) => self.native_named_provider(reference.declaration).ok()
                .filter(|p| p.type_declaration_kind(self, reference.declaration).is_nominal())
                .map(|_| reference),
            TypeData::NativeTable(ref table) => table.nominal_origin,
            _ => None,
        }
    }

    pub(super) fn native_with_receiver(&mut self, ty: TypeId, receiver: TypeId) -> TypeId {
        match *self.data(ty) {
            TypeData::NativeObject(reference) if reference.self_argument.is_none() => self.intern(TypeData::NativeObject(NativeObjectReference { self_argument: Some(receiver), ..reference })),
            TypeData::NativeDefinition(reference) if reference.class.self_argument.is_none() => self.intern(TypeData::NativeDefinition(NativeDefinitionRef { class: NativeObjectReference { self_argument: Some(receiver), ..reference.class }, ..reference })),
            TypeData::NativeTable(ref table) => {
                if let Some(reference) = table.definition_origin && reference.class.self_argument.is_none() {
                    return self.native_definition_resolved(NativeDefinitionRef { class: NativeObjectReference { self_argument: Some(receiver), ..reference.class }, ..reference });
                }
                if let Some(reference) = table.nominal_origin && reference.self_argument.is_none() {
                    let object = self.intern(TypeData::NativeObject(NativeObjectReference { self_argument: Some(receiver), ..reference }));
                    return self.native_object_schema_resolved(object).unwrap_or(TypeId::ERROR);
                }
                ty
            }
            _ => ty,
        }
    }

    fn native_member_mapper(&mut self, reference: NativeObjectReference) -> MapperId {
        let Some(id) = self.native_named_provider(reference.declaration).ok().and_then(|p| p.type_declaration_self_parameter(reference.declaration)) else { return reference.mapper };
        let Some(param) = self.native_type_parameter(id) else { return reference.mapper };
        let receiver = reference.self_argument.unwrap_or_else(|| self.intern(TypeData::NativeObject(reference)));
        let mapped = self.cloned_native_type_param(id, reference.mapper);
        let mut pairs = vec![(param, receiver)];
        if mapped != param { pairs.push((mapped, receiver)); }
        let mapper = self.types().mapper(pairs);
        self.compose(reference.mapper, mapper)
    }

    fn native_member_type(&mut self, raw: TypeId, reference: NativeObjectReference) -> TypeId {
        let mapper = self.native_member_mapper(reference);
        self.instantiate(raw, mapper)
    }

    fn native_member_read(&mut self, raw: TypeId, reference: NativeObjectReference) -> Result<TypeId, NativeTableError> {
        let ordinary = self.instantiate(raw, reference.mapper);
        let value = self.native_member_type(raw, reference);
        let receiver = reference.self_argument.unwrap_or_else(|| self.intern(TypeData::NativeObject(reference)));
        let mapper = self.native_receiver_mapper(reference, receiver);
        if self.native_uses_receiver(ordinary, mapper) {
            let expected = self.native_receiver_expected(reference);
            if !self.is_assignable(receiver, expected) { return Err(NativeTableError::ValueType { value: receiver, expected }) }
        }
        Ok(value)
    }

    fn native_receiver_mapper(&mut self, reference: NativeObjectReference, receiver: TypeId) -> MapperId {
        let mut pairs = Vec::new();
        self.native_receiver_pairs(reference, receiver, &mut pairs);
        self.types().mapper(pairs)
    }

    fn native_uses_receiver(&self, ty: TypeId, mapper: MapperId) -> bool {
        // Existing exact metadata traversal visits signature mapper values and captures without
        // requesting a native method's parameters, inferred returns or body.
        self.any_type_in(ty, true, |part| self.types().map(mapper, part).is_some_and(|mapped| mapped != part))
    }

    fn native_receiver_pairs(&mut self, reference: NativeObjectReference, receiver: TypeId, pairs: &mut Vec<(TypeId, TypeId)>) {
        if let Some(id) = self.native_named_provider(reference.declaration).ok().and_then(|p| p.type_declaration_self_parameter(reference.declaration)) {
            if let Some(param) = self.native_type_parameter(id) {
                for param in [param, self.cloned_native_type_param(id, reference.mapper)] {
                    if !pairs.iter().any(|pair| pair.0 == param) { pairs.push((param, receiver)); }
                }
            }
        }
        if let Ok(bases) = self.native_object_bases(reference) {
            for base in bases { if let TypeData::NativeObject(reference) = *self.data(base) { self.native_receiver_pairs(reference, receiver, pairs); } }
        }
    }

    fn native_receiver_expected(&self, reference: NativeObjectReference) -> TypeId {
        self.intern(TypeData::NativeObject(NativeObjectReference { self_argument: None, ..reference }))
    }

    pub(super) fn native_receiver_relation_target(&mut self, source: TypeId, target: TypeId) -> TypeId {
        let Some(origin) = self.native_nominal_origin(source) else { return target };
        let reference = match *self.data(target) {
            TypeData::NativeObject(reference) => reference,
            TypeData::NativeDefinition(reference) => reference.class,
            TypeData::NativeTable(ref table) => match table.nominal_origin { Some(reference) => reference, None => return target },
            _ => return target,
        };
        if reference.self_argument.is_some() { return target }
        let own_self = self.native_named_provider(reference.declaration).is_ok_and(|p| p.type_declaration_self_parameter(reference.declaration).is_some());
        if !own_self || !self.native_nominal_reference_compatible(origin, reference, false) { return target }
        let receiver = origin.self_argument.unwrap_or(source);
        self.native_with_receiver(target, receiver)
    }

    pub fn native_receiver_type(&mut self, ty: TypeId, receiver: TypeId) -> Result<TypeId, NativeNamedTypeError> {
        let base = self.base_constraint_or_type(ty);
        let reference = match *self.data(base) {
            TypeData::NativeObject(reference) => reference,
            TypeData::NativeDefinition(reference) => reference.class,
            TypeData::NativeTable(ref table) => table.nominal_origin.ok_or(NativeNamedTypeError::UnsupportedReceiverType { actual: ty })?,
            _ => return Err(NativeNamedTypeError::UnsupportedReceiverType { actual: ty }),
        };
        let expected = self.native_receiver_expected(reference);
        if !self.is_assignable(receiver, expected) { return Err(NativeNamedTypeError::InvalidReceiver { declaration: reference.declaration, expected, actual: receiver }) }
        Ok(if base != ty { ty } else { self.native_with_receiver(ty, receiver) })
    }

    pub(super) fn native_definition_member_read(&mut self, reference: NativeDefinitionRef, receiver: TypeId, key: NativeTableAccessKey) -> Result<TypeId, NativeTableError> {
        // A member the definition table has not been given at this point, one its class
        // declares, and one it inherits read as the class's member: as v1 types a class table,
        // and as TypeScript types a property assigned anywhere, rather than as possibly nil.
        let raw = match self.native_table_read(reference.view, key) {
            Ok(raw) if !self.contains_undefined(raw) => raw,
            other => match self.native_named_read(reference.class, key) {
                Ok(value) => return Ok(value),
                Err(_) => other?,
            },
        };
        let receiver = reference.class.self_argument.unwrap_or(receiver);
        let mapper = self.native_receiver_mapper(reference.class, receiver);
        let value = self.instantiate(raw, mapper);
        if self.native_uses_receiver(raw, mapper) {
            let expected = self.native_receiver_expected(reference.class);
            if !self.is_assignable(receiver, expected) { return Err(NativeTableError::ValueType { value: receiver, expected }) }
        }
        Ok(value)
    }

    pub(super) fn native_named_bases(&mut self, declaration: NativeTypeDeclarationId) -> Result<Vec<TypeId>, NativeNamedTypeError> {
        if let Some(known) = self.native_named_cache.bases.get(&declaration) { return known.clone() }
        let provider = self.native_named_provider(declaration)?;
        let native_query = NativeTypeDeclarationQuery::Bases(declaration);
        let query = Query::NativeNamed(native_query);
        if !self.enter(query) {
            let error = if self.found_cycle { NativeNamedTypeError::CircularBase(declaration) } else { NativeNamedTypeError::TooDeep(native_query) };
            self.report_native_named(error);
            return Err(error);
        }
        let result = (|| {
            let bases = provider.type_declaration_bases(self, declaration);
            for (index, &base) in bases.iter().enumerate() {
                if self.is_error_type(base) { continue }
                let valid = match *self.data(base) {
                    TypeData::NativeObject(reference) => self.native_named_provider(reference.declaration).is_ok_and(|p| {
                        let header = p.type_declaration_header(self, reference.declaration);
                        matches!(header.kind, NativeTypeDeclarationKind::Shape | NativeTypeDeclarationKind::Class)
                            && header.object_form == Some(NativeObjectDeclarationForm::Record)
                    }),
                    _ => false,
                };
                if !valid { return Err(NativeNamedTypeError::InvalidBase { declaration, index, base }) }
                let TypeData::NativeObject(reference) = *self.data(base) else { unreachable!() };
                // Declaration identity bounds cycles even when each recursive edge grows its arguments.
                self.native_named_bases(reference.declaration)?;
            }
            Ok(bases)
        })();
        if let Ok(stored) = self.leave(query) { self.native_named_cache.bases(declaration, result.clone(), stored); }
        if let Err(error) = result { self.report_native_named(error) }
        result
    }

    fn native_object_bases(&mut self, reference: NativeObjectReference) -> Result<Vec<TypeId>, NativeNamedTypeError> {
        let ty = self.intern(TypeData::NativeObject(reference));
        if let Some(known) = self.native_named_cache.mapped_bases.get(&ty) { return Ok(known.clone()) }
        let raw = self.native_named_bases(reference.declaration)?;
        let query = Query::NativeNamed(NativeTypeDeclarationQuery::Bases(reference.declaration));
        if !self.enter(query) { return Err(NativeNamedTypeError::CircularBase(reference.declaration)) }
        let receiver = reference.self_argument.unwrap_or(ty);
        let bases: Vec<_> = raw.into_iter().map(|base| { let base = self.instantiate(base, reference.mapper); self.native_with_receiver(base, receiver) }).collect();
        if let Ok(stored) = self.leave(query) { self.native_named_cache.mapped_bases(ty, bases.clone(), stored); }
        Ok(bases)
    }

    fn native_object_members(&mut self, reference: NativeObjectReference) -> Result<Vec<NativeObjectMember>, NativeNamedTypeError> {
        let ty = self.intern(TypeData::NativeObject(reference));
        if let Some(known) = self.native_named_cache.members.get(&ty) { return Ok(known.clone()) }
        let bases = self.native_object_bases(reference)?;
        let query = Query::NativeNamed(NativeTypeDeclarationQuery::ObjectMembers(ty));
        if !self.enter(query) { return Err(NativeNamedTypeError::TooDeep(NativeTypeDeclarationQuery::ObjectMembers(ty))) }
        let result = (|| {
            let provider = self.native_named_provider(reference.declaration)?;
            let mut members: Vec<NativeObjectMember> = Vec::new();
            for member in provider.type_declaration_members(reference.declaration) {
                let Some(metadata) = self.native_named_member_header(reference, member)? else { continue };
                if provider.type_member_signatures(member).is_some()
                    && provider.type_member_origin(member) == crate::types::NativeTableFieldOrigin::Declared
                    && let NativeTypeMemberKey::Fixed(key) = metadata.key
                {
                    let key = key.normalize().map_err(|error| NativeNamedTypeError::Table { declaration: reference.declaration, error })?;
                    let previous = members.iter_mut().find(|previous| {
                        let previous_metadata = provider.type_member_header(self, previous.member);
                        matches!(previous_metadata.key, NativeTypeMemberKey::Fixed(previous_key) if previous_key.normalize() == Ok(key))
                            && previous_metadata.optional == metadata.optional
                            && previous_metadata.readonly == metadata.readonly
                            && previous_metadata.visibility == metadata.visibility
                            && provider.type_member_origin(previous.member) == crate::types::NativeTableFieldOrigin::Declared
                            && provider.type_member_signatures(previous.member).is_some()
                    });
                    if let Some(previous) = previous {
                        if previous.member != member && !previous.overloads.contains(&member) { previous.overloads.push(member); }
                        continue;
                    }
                }
                members.push(NativeObjectMember { member, overloads: Vec::new(), owner: reference, via: None });
            }
            for (index, base) in bases.into_iter().enumerate() {
                if self.is_error_type(base) { continue }
                let TypeData::NativeObject(owner) = *self.data(base) else { unreachable!() };
                for member in self.native_object_members(owner)? {
                    // Candidate origins are immutable. An inactive override
                    // must not hide its inherited active fallback.
                    members.push(NativeObjectMember { via: Some((index, base)), ..member });
                }
            }
            Ok(members)
        })();
        if let Ok(stored) = self.leave(query) { if let Ok(members) = &result { self.native_named_cache.members(ty, members.clone(), stored); } }
        if let Err(error) = result { self.report_native_named(error) }
        result
    }

    /// The string keys of an object's own and inherited members, for the language service.
    pub(super) fn native_object_member_names(&mut self, reference: NativeObjectReference) -> Vec<Atom> {
        let Ok(members) = self.native_object_members(reference) else { return Vec::new() };
        let Ok(provider) = self.native_named_provider(reference.declaration) else { return Vec::new() };
        members.iter().filter_map(|member| match provider.type_member_header(self, member.member).key {
            NativeTypeMemberKey::Fixed(key) => match key.normalize() {
                Ok(NativeTableKey::String(name)) => Some(name),
                _ => None,
            },
            NativeTypeMemberKey::Indexer { .. } => None,
        }).collect()
    }

    /// Written value context needs only this key's declared recipes. Resolving
    /// an inferred store's receiver here would let an initializer demand its
    /// own prospective schema before its actual allocation is published.
    fn native_object_written_fixed_members(&mut self, reference: NativeObjectReference, key: NativeTableKey) -> Result<Vec<NativeObjectMember>, NativeNamedTypeError> {
        let provider = self.native_named_provider(reference.declaration)?;
        let mut members = Vec::new();
        for member in provider.type_declaration_members(reference.declaration) {
            if provider.type_member_origin(member) != crate::types::NativeTableFieldOrigin::Declared { continue }
            let metadata = provider.type_member_header(self, member);
            if !matches!(metadata.key, NativeTypeMemberKey::Fixed(other) if other.normalize() == Ok(key)) { continue }
            if self.native_named_member_header(reference, member)?.is_none() { continue }
            members.push(NativeObjectMember { member, overloads: Vec::new(), owner: reference, via: None });
        }
        for (index, base) in self.native_object_bases(reference)?.into_iter().enumerate() {
            if self.is_error_type(base) { continue }
            let TypeData::NativeObject(owner) = *self.data(base) else { unreachable!() };
            for member in self.native_object_written_fixed_members(owner, key)? {
                members.push(NativeObjectMember { via: Some((index, base)), ..member });
            }
        }
        Ok(members)
    }

    /// Whether `target` is `source` or one of its declared, mapped bases. A declared subclass is
    /// assignable to its ancestors (incompatible overrides are the declaration's own error).
    pub(super) fn native_nominally_derived(&mut self, source: TypeId, target: TypeId) -> bool {
        if source == target { return true }
        if !matches!(self.data(target), TypeData::NativeObject(_)) { return false }
        let TypeData::NativeObject(reference) = *self.data(source) else { return false };
        let Ok(bases) = self.native_object_bases(reference) else { return false };
        bases.into_iter().any(|base| self.native_nominally_derived(base, target))
    }

    pub(super) fn native_nominal_reference_compatible(&mut self, source: NativeObjectReference, target: NativeObjectReference, identity: bool) -> bool {
        if source.declaration == target.declaration { return true }
        if identity { return false }
        let Ok(bases) = self.native_object_bases(source) else { return false };
        bases.into_iter().any(|base| {
            let TypeData::NativeObject(reference) = *self.data(base) else { return false };
            let nominal = self.native_named_provider(reference.declaration).is_ok_and(|p| p.type_declaration_kind(self, reference.declaration) == NativeTypeDeclarationKind::Class);
            nominal && self.native_nominal_reference_compatible(reference, target, false)
        })
    }

    /// Materialize a declaration-space description once per object mapper. Recursive member types
    /// only request the already-published head, so productive recursion does not recurse into fields.
    pub fn native_object_resolved(&mut self, ty: TypeId) -> Result<TypeId, NativeNamedTypeError> {
        if let TypeData::NativeRuntimeTable(reference) = *self.data(ty) { return Ok(reference.view) }
        if let TypeData::NativeDefinition(reference) = *self.data(ty) { return Ok(self.native_definition_resolved(reference)) }
        let TypeData::NativeObject(reference) = *self.data(ty) else { return Ok(ty) };
        if let Some(&known) = self.native_named_cache.objects.get(&ty) { return known }
        // Publish the complete declaration shape before comparing overrides.
        // A method parameter can refer to this same class: the shared relation
        // worker needs its shape without recursively validating the declaration.
        let schema = self.native_object_schema_resolved(ty)?;
        let members = self.native_object_members(reference)?;
        let native_query = NativeTypeDeclarationQuery::ObjectValidation(ty);
        let query = Query::NativeNamed(native_query);
        if !self.enter(query) {
            // Requested again while it is being validated, as a member's inference builds its
            // class's definition table, the object is the shape published above.
            if self.is_reentry(query) { return Ok(schema) }
            let error = NativeNamedTypeError::TooDeep(native_query);
            self.report_native_named(error);
            return Err(error);
        }
        let result = self.native_object_resolved_uncached(reference, &members, true);
        match self.leave(query) {
            Ok(stored) => self.native_named_cache.object(ty, result, stored),
            Err(_) => {},
        }
        if let Err(error) = result { self.report_native_named(error) }
        result
    }

    /// Complete declaration-space members for structural queries. Compatibility
    /// validation is a separate demand, so recursive member types can use the
    /// ordinary relation/inference guards after this shape has been published.
    pub(super) fn native_object_schema_resolved(&mut self, ty: TypeId) -> Result<TypeId, NativeNamedTypeError> {
        if let TypeData::NativeRuntimeTable(reference) = *self.data(ty) { return Ok(reference.view) }
        if let TypeData::NativeDefinition(reference) = *self.data(ty) { return Ok(self.native_definition_resolved(reference)) }
        let TypeData::NativeObject(reference) = *self.data(ty) else { return Ok(ty) };
        if let Some(&known) = self.native_named_cache.schemas.get(&ty) { return known }
        let members = self.native_object_members(reference)?;
        let native_query = NativeTypeDeclarationQuery::ObjectMembers(ty);
        let query = Query::NativeNamed(native_query);
        if !self.enter(query) {
            if self.is_reentry(query) { return Ok(self.native_object_provisional_schema(reference, &members)) }
            let error = NativeNamedTypeError::TooDeep(native_query);
            self.report_native_named(error);
            return Err(error);
        }
        let result = self.native_object_resolved_uncached(reference, &members, false);
        if let Ok(stored) = self.leave(query) { self.native_named_cache.schema(ty, result, stored); }
        if let Err(error) = result { self.report_native_named(error); }
        result
    }

    /// The members of an object requested again while they are being resolved, as a member's
    /// inference reads or relates its own class: TypeScript has a class's member symbols before
    /// their types, and each member here has the type it already has, or none yet. Nothing is
    /// resolved for it, and the cycle leaves what is built from it provisional.
    fn native_object_provisional_schema(&mut self, reference: NativeObjectReference, members: &[NativeObjectMember]) -> TypeId {
        let Ok(provider) = self.native_named_provider(reference.declaration) else { return TypeId::UNRESOLVED };
        let mut fields: Vec<NativeTableField> = Vec::new();
        for origin in members {
            let header = provider.type_member_header(self, origin.member);
            let NativeTypeMemberKey::Fixed(key) = header.key else { continue };
            let Ok(key) = key.normalize() else { continue };
            if fields.iter().any(|field| field.key == key) { continue }
            let value = match self.native_named_cache.types.get(&NativeTypeDeclarationQuery::MemberType(origin.member)) {
                Some(&raw) if origin.via.is_none() => self.native_member_type(raw, origin.owner),
                _ => TypeId::UNRESOLVED,
            };
            fields.push(NativeTableField { key, value, optional: header.optional, readonly: header.readonly, origin: provider.type_member_origin(origin.member) });
        }
        self.native_table_with_context_and_sequence_policy(&fields, &[], &[], NativeTableFlavor::Declared, false, None)
            .unwrap_or(TypeId::UNRESOLVED)
    }

    /// Callable named objects keep their original signature view in recursive
    /// relations; exact Lua fields/keys still use the resolved table view.
    pub(super) fn native_object_relation_view(&mut self, ty: TypeId) -> Result<TypeId, NativeNamedTypeError> {
        let resolved = self.native_object_schema_resolved(ty)?;
        if self.native_object_has_call_signatures(ty) { return Ok(ty) }
        Ok(resolved)
    }

    /// A definition's call contract comes only from its explicitly attached class. Its actual
    /// member view remains separate, including after materialization and mapper substitution.
    fn native_object_call_reference(&self, ty: TypeId) -> Option<NativeObjectReference> {
        match *self.data(ty) {
            TypeData::NativeObject(reference) => Some(reference),
            TypeData::NativeDefinition(reference) => Some(reference.class),
            TypeData::NativeTable(ref table) => table.definition_origin.map(|reference| reference.class).or(table.nominal_origin),
            _ => None,
        }
    }

    pub(super) fn native_object_has_call_signatures(&mut self, ty: TypeId) -> bool {
        !self.native_object_call_heads(ty).is_empty()
    }

    /// Signature classification needs the presence of original members, not
    /// their types. A member type may itself be a class whose override contract
    /// is currently being compared, so materializing it here would recurse into
    /// the declaration's own unfinished schema.
    pub(super) fn native_object_has_non_signature_members(&mut self, ty: TypeId) -> Result<bool, NativeNamedTypeError> {
        match *self.data(ty) {
            TypeData::NativeObject(reference) => Ok(!self.native_object_members(reference)?.is_empty()),
            TypeData::NativeDefinition(reference) => {
                let TypeData::NativeTable(table) = self.data(reference.view) else { unreachable!("definition has its original table view") };
                Ok(!table.fields.is_empty() || !table.indexers.is_empty() || !table.sequence_tails.is_empty())
            }
            _ => unreachable!("native object signature classification"),
        }
    }

    /// The declared heads of `operator` for an object, with its own before its
    /// bases', as for call heads. An undeclared operator has none.
    pub(super) fn native_object_operator_heads(&mut self, ty: TypeId, operator: NativeOperator) -> Vec<SigId> {
        let Some(reference) = self.native_object_call_reference(ty) else { return Vec::new() };
        let Ok(provider) = self.native_named_provider(reference.declaration) else { return Vec::new() };
        let mapper = self.native_member_mapper(reference);
        let mut heads: Vec<_> = provider.type_declaration_operator_signatures(reference.declaration, operator).into_iter().map(|declaration| {
            self.native_signature_provider_for(declaration);
            self.types().intern_sig(SigData::NativeDeclarationSignature { declaration, mapper, implementation: None })
        }).collect();
        if let Ok(bases) = self.native_object_bases(reference) {
            for base in bases { for head in self.native_object_operator_heads(base, operator) { if !heads.contains(&head) { heads.push(head); } } }
        }
        heads
    }

    pub(super) fn native_object_call_heads(&mut self, ty: TypeId) -> Vec<SigId> {
        let Some(reference) = self.native_object_call_reference(ty) else { return Vec::new() };
        let Ok(provider) = self.native_named_provider(reference.declaration) else { return Vec::new() };
        let mapper = self.native_member_mapper(reference);
        let mut heads: Vec<_> = provider.type_declaration_call_signatures(reference.declaration).into_iter().map(|declaration| {
            self.native_signature_provider_for(declaration);
            self.types().intern_sig(SigData::NativeDeclarationSignature { declaration, mapper, implementation: None })
        }).collect();
        if let Ok(bases) = self.native_object_bases(reference) {
            for base in bases { for head in self.native_object_call_heads(base) { if !heads.contains(&head) { heads.push(head); } } }
        }
        heads
    }

    fn native_object_resolved_uncached(&mut self, reference: NativeObjectReference, members: &[NativeObjectMember], validate: bool) -> Result<TypeId, NativeNamedTypeError> {
        let declaration = reference.declaration;
        let provider = self.native_named_provider(declaration)?;
        let header = provider.type_declaration_header(self, declaration);
        if header.kind == NativeTypeDeclarationKind::RuntimeTable { return self.native_runtime_schema_resolved(reference, members) }
        for base in self.native_object_bases(reference)? {
            if validate { self.native_object_resolved(base)?; }
            else { self.native_object_schema_resolved(base)?; }
        }
        if let Some(NativeObjectDeclarationForm::Table { type_argument_count }) = header.object_form
            && type_argument_count != 2 {
            return Err(NativeNamedTypeError::TypeArgumentArity { declaration, actual: type_argument_count, minimum: 2, maximum: 2 });
        }
        let mut fields = Vec::new();
        let mut field_origins: Vec<NativeObjectMember> = Vec::new();
        let mut indexers = Vec::new();
        let mut indexer_origins: Vec<NativeObjectMember> = Vec::new();
        for origin in members.iter().cloned() {
            let metadata = provider.type_member_header(self, origin.member);
            let Some(raw) = self.native_object_member_demand(&origin)? else { continue };
            let Some(value_origin) = self.native_object_member_effective_origin(&origin)? else { continue };
            let value = self.native_member_type(raw, origin.owner);
            if validate && header.kind != NativeTypeDeclarationKind::Class {
                if let Err(NativeTableError::ValueType { value: actual, expected }) = self.native_member_read(raw, origin.owner) {
                    return Err(NativeNamedTypeError::InvalidReceiver { declaration, actual, expected });
                }
            }
            match metadata.key {
                NativeTypeMemberKey::Fixed(key) => {
                    let field = NativeTableField { key: key.normalize().map_err(|error| NativeNamedTypeError::Table { declaration, error })?, value, optional: metadata.optional, readonly: metadata.readonly, origin: value_origin };
                    if let Some(position) = fields.iter().position(|other: &NativeTableField| other.key == field.key) {
                        let previous = fields[position];
                        let previous_origin = &field_origins[position];
                        // The direct base has already checked its effective
                        // override layers; these are candidates from that same base.
                        if origin.via.is_some() && origin.via == previous_origin.via { continue }
                        let expected = if field.optional { self.optional(field.value) } else { field.value };
                        let actual = if previous.optional { self.optional(previous.value) } else { previous.value };
                        if origin.via.is_none() {
                            // A written field types the member that a definition table's value for
                            // it is checked against, in whichever partial declaration each is.
                            let declared = |field: &NativeTableField| field.origin == crate::types::NativeTableFieldOrigin::Declared;
                            if declared(&field) != declared(&previous) {
                                if declared(&field) { fields[position] = field; field_origins[position] = origin; }
                                continue;
                            }
                            // As TypeScript merges declarations, an identical redeclaration is the
                            // same member; a different one is reported once, at its declaration,
                            // and the first is used.
                            let identical = previous.optional == field.optional && previous.readonly == field.readonly
                                && self.related(previous.value, field.value, Relation::Identity);
                            if identical || !validate { continue }
                            return Err(NativeNamedTypeError::Table { declaration, error: NativeTableError::DuplicateDeclaredField { key: field.key } })
                        }
                        let (index, base) = origin.via.unwrap();
                        if !validate { continue; }
                        if previous_origin.via.is_none() {
                            if previous.optional && !field.optional || !self.is_assignable(actual, expected) {
                                return Err(NativeNamedTypeError::IncompatibleOverride { declaration, member: previous_origin.member, base, expected, actual });
                            }
                        } else if previous.optional != field.optional || previous.readonly != field.readonly || !self.related(actual, expected, Relation::Identity) {
                            return Err(NativeNamedTypeError::IncompatibleBase { declaration, index, base, expected, actual });
                        }
                    } else { fields.push(field); field_origins.push(origin); }
                }
                NativeTypeMemberKey::Indexer { sparse } => {
                    let raw_key = self.native_named_member_demand(origin.member, true)?;
                    let Some(capture) = self.native_member_capture_mapper(origin.owner, origin.member)? else { continue };
                    let raw_key = self.instantiate(raw_key, capture);
                    let key = self.native_member_type(raw_key, origin.owner);
                    indexers.push(NativeTableIndexer { key, value, readonly: metadata.readonly, sparse, origin: value_origin });
                    indexer_origins.push(origin);
                }
            }
        }
        // Constructor indexers describe inferred stores, not a written promise
        // that every overlapping named property has the initializer's type.
        // Include the original fixed contracts in their inferred domain while
        // retaining each fixed property's more precise type. The actual dynamic
        // store is independently checked against those contracts at construction.
        for indexer in &mut indexers {
            if indexer.origin != crate::types::NativeTableFieldOrigin::Inferred { continue }
            let mut values = vec![indexer.value];
            for field in &fields {
                if self.native_table_key_domains_overlap(self.native_table_key_type(field.key), indexer.key) { values.push(field.value); }
            }
            indexer.value = self.union(&values);
        }
        for (position, indexer) in indexers.iter().enumerate().filter(|_| validate) {
            let origin = &indexer_origins[position];
            for (other_position, expected) in indexers.iter().enumerate().take(position) {
                let other = &indexer_origins[other_position];
                if !self.native_table_key_domains_overlap(indexer.key, expected.key) { continue }
                if origin.via.is_some() && other.via.is_none() && !self.is_assignable(expected.value, indexer.value) {
                    return Err(NativeNamedTypeError::IncompatibleOverride { declaration, member: other.member, base: origin.via.unwrap().1, expected: indexer.value, actual: expected.value });
                }
                if let (Some((index, base)), Some(_)) = (origin.via, other.via)
                    && origin.via != other.via
                    && !self.related(expected.value, indexer.value, Relation::Identity) {
                    return Err(NativeNamedTypeError::IncompatibleBase { declaration, index, base, expected: indexer.value, actual: expected.value });
                }
            }
            for (field_position, field) in fields.iter().enumerate() {
                let field_origin = &field_origins[field_position];
                // One direct base already checked its own effective contracts. Keep its mapped
                // overlapping domains, rather than treating its override layers as sibling bases.
                if origin.via.is_some() && origin.via == field_origin.via { continue }
                if !self.native_table_key_domains_overlap(self.native_table_key_type(field.key), indexer.key) { continue }
                if let Some((_, base)) = origin.via && field_origin.via.is_none() && !self.is_assignable(field.value, indexer.value) {
                    return Err(NativeNamedTypeError::IncompatibleOverride { declaration, member: field_origin.member, base, expected: indexer.value, actual: field.value });
                }
                if let Some((_, base)) = field_origin.via && origin.via.is_none() && !self.is_assignable(field.value, indexer.value) {
                    return Err(NativeNamedTypeError::IncompatibleOverride { declaration, member: origin.member, base, expected: indexer.value, actual: field.value });
                }
                if !self.is_assignable(field.value, indexer.value) {
                    if let Some((index, base)) = origin.via.or(field_origin.via) {
                        return Err(NativeNamedTypeError::IncompatibleBase { declaration, index, base, expected: indexer.value, actual: field.value });
                    }
                    return Err(NativeNamedTypeError::Table { declaration, error: NativeTableError::ValueType { value: field.value, expected: indexer.value } });
                }
            }
        }
        if header.object_form == Some(NativeObjectDeclarationForm::Array) {
            if indexers.len() != 1 || indexers[0].key != TypeId::NUMBER || !fields.is_empty() {
                return Err(NativeNamedTypeError::InvalidDeclaration(declaration));
            }
        }
        let table = if header.object_form == Some(NativeObjectDeclarationForm::Array) {
            self.native_array_table(indexers[0].value, indexers[0].readonly)
        } else { self.native_table(&fields, &indexers, NativeTableFlavor::Declared) }
            .map_err(|error| NativeNamedTypeError::Table { declaration, error })?;
        if header.kind != NativeTypeDeclarationKind::Class { return Ok(table) }
        let TypeData::NativeTable(data) = self.data(table) else { unreachable!() };
        let mut description = (**data).clone_in(self.arena);
        description.nominal_origin = Some(reference);
        Ok(self.intern(TypeData::NativeTable(self.boxed(description))))
    }

    /// A literal field read demands just that field, preserving laziness across sibling members.
    /// Indexers and full structural operations use the ordinary NativeTable description worker.
    pub(super) fn native_named_read(&mut self, reference: NativeObjectReference, key: NativeTableAccessKey) -> Result<TypeId, NativeTableError> {
        if self.native_named_provider(reference.declaration).is_ok_and(|provider| provider.type_declaration_kind(self, reference.declaration) == NativeTypeDeclarationKind::RuntimeTable) {
            let key = self.native_table_access(key)?;
            let object = self.intern(TypeData::NativeObject(reference));
            if let Some(value) = self.native_runtime_seed_read(reference, key).map_err(|error| error.table_error(object))? { return Ok(value) }
            // A literal key reads its own members and the declared key domains alone, as a
            // TypeScript property's type resolves alone. A store whose value reads another field
            // of the same table then demands that field, not the whole schema again.
            if let NativeTableAccessKey::Literal(literal) = key {
                let members = self.native_object_members(reference).map_err(|error| error.table_error(object))?;
                let provider = self.native_named_provider(reference.declaration).map_err(|error| error.table_error(object))?;
                let selected: Vec<_> = members.into_iter().filter(|origin| match provider.type_member_header(self, origin.member).key {
                    NativeTypeMemberKey::Fixed(member_key) => member_key.normalize() == Ok(literal),
                    NativeTypeMemberKey::Indexer { .. } => provider.type_member_origin(origin.member) == crate::types::NativeTableFieldOrigin::Declared,
                }).collect();
                let table = self.native_runtime_schema_resolved(reference, &selected).map_err(|error| error.table_error(object))?;
                return self.native_table_read(table, key);
            }
            let resolved = self.native_object_resolved(object).map_err(|error| error.table_error(object))?;
            return self.native_table_read(resolved, key);
        }
        if let NativeTableAccessKey::Literal(key) = key {
            let key = key.normalize()?;
            let members = self.native_object_members(reference).map_err(|error| error.table_error(TypeId::ERROR))?;
            let provider = self.native_named_provider(reference.declaration).map_err(|error| error.table_error(TypeId::ERROR))?;
            let mut found: Option<(NativeObjectMember, NativeTypeMemberHeader, TypeId)> = None;
            let mut found_declared = false;
            for origin in members {
                let metadata = provider.type_member_header(self, origin.member);
                if let NativeTypeMemberKey::Fixed(member_key) = metadata.key && member_key.normalize()? == key {
                    if let Some((previous, _, _)) = &found
                        && origin.via.is_some() && origin.via == previous.via { continue }
                    let Some(raw) = self.native_object_member_demand(&origin).map_err(|error| error.table_error(TypeId::ERROR))? else { continue };
                    let value = self.native_member_read(raw, origin.owner)?;
                    let declared_here = self.native_object_member_effective_origin(&origin).map_err(|error| error.table_error(TypeId::ERROR))? == Some(crate::types::NativeTableFieldOrigin::Declared);
                    let value = if declared_here { self.regular(value) } else { value };
                    let value = if let Some((_, base)) = origin.via {
                        let TypeData::NativeObject(owner) = *self.data(base) else { unreachable!() };
                        self.native_named_read(owner, NativeTableAccessKey::Literal(key))?
                    } else if metadata.optional { self.optional(value) } else { value };
                    if let Some((previous, previous_metadata, actual)) = &found {
                        let error = if let Some((index, base)) = origin.via {
                            if previous.via.is_none() {
                                (previous_metadata.optional && !metadata.optional || !self.is_assignable(*actual, value)).then_some(NativeNamedTypeError::IncompatibleOverride { declaration: reference.declaration, member: previous.member, base, expected: value, actual: *actual })
                            } else {
                                (previous_metadata.optional != metadata.optional || previous_metadata.readonly != metadata.readonly || !self.related(*actual, value, Relation::Identity)).then_some(NativeNamedTypeError::IncompatibleBase { declaration: reference.declaration, index, base, expected: value, actual: *actual })
                            }
                        } else {
                            // A written field outranks a definition table's value for it; otherwise a
                            // redeclared member reads as its first declaration, and a conflicting one
                            // is reported once, at its declaration.
                            if declared_here && !found_declared {
                                found = Some((origin, metadata, value));
                                found_declared = true;
                            }
                            None
                        };
                        if let Some(error) = error { self.report_native_named(error); return Err(NativeTableError::NotTable { ty: TypeId::ERROR }); }
                    } else { found = Some((origin, metadata, value)); found_declared = declared_here; }
                }
            }
            if let Some((_, _, value)) = found { return Ok(value) }
            // A member's inference can read its own class while the class's members are being
            // resolved (`m.x = m.y` in a method). A key that no field has then reads the indexers
            // alone, as a TypeScript property lookup does not resolve the other members.
            let object = self.intern(TypeData::NativeObject(reference));
            if self.stack.contains(&Query::NativeNamed(NativeTypeDeclarationQuery::ObjectMembers(object))) {
                let members = self.native_object_members(reference).map_err(|error| error.table_error(object))?;
                let provider = self.native_named_provider(reference.declaration).map_err(|error| error.table_error(object))?;
                let indexers: Vec<_> = members.into_iter()
                    .filter(|origin| matches!(provider.type_member_header(self, origin.member).key, NativeTypeMemberKey::Indexer { .. }))
                    .collect();
                let table = self.native_object_resolved_uncached(reference, &indexers, false).map_err(|error| error.table_error(object))?;
                return self.native_table_read(table, NativeTableAccessKey::Literal(key));
            }
        }
        let object = self.intern(TypeData::NativeObject(reference));
        let resolved = self.native_object_resolved(object).map_err(|error| error.table_error(object))?;
        self.native_table_read(resolved, key)
    }

    /// The type a contextual lookup sees for `key`, as `getTypeOfPropertyOfContextualType`: the
    /// member's mapped type, without the receiver and override relations of a read. Context is
    /// demanded while resolving members, and relating types there re-entered the schema being
    /// resolved.
    pub(super) fn native_named_context_value(&mut self, reference: NativeObjectReference, key: NativeTableAccessKey) -> Option<TypeId> {
        if let NativeTableAccessKey::Literal(literal) = key
            && self.native_named_provider(reference.declaration).is_ok_and(|provider| provider.type_declaration_kind(self, reference.declaration) != NativeTypeDeclarationKind::RuntimeTable)
        {
            let literal = literal.normalize().ok()?;
            let members = self.native_object_members(reference).ok()?;
            let provider = self.native_named_provider(reference.declaration).ok()?;
            for origin in members {
                let metadata = provider.type_member_header(self, origin.member);
                let NativeTypeMemberKey::Fixed(member_key) = metadata.key else { continue };
                if member_key.normalize().ok()? != literal { continue }
                if let Some((_, base)) = origin.via {
                    let TypeData::NativeObject(owner) = *self.data(base) else { continue };
                    return self.native_named_context_value(owner, key);
                }
                let Some(raw) = self.native_object_member_demand(&origin).ok()? else { continue };
                let value = self.native_member_type(raw, origin.owner);
                let value = if self.native_object_member_effective_origin(&origin).ok()? == Some(crate::types::NativeTableFieldOrigin::Declared) { self.regular(value) } else { value };
                return Some(if metadata.optional { self.optional(value) } else { value });
            }
        }
        let object = self.intern(TypeData::NativeObject(reference));
        let schema = self.native_object_schema_resolved(object).ok()?;
        self.native_table_read(schema, key).ok()
    }
}

impl<'p, 's> Checker<'p, 's> {
    fn native_definition_class(&mut self, owner: NativeDefinitionId, class: TypeId) -> Result<NativeObjectReference, NativeNamedTypeError> {
        let provider = self.native_type_declaration_provider.filter(|provider| provider.contains_class_definition(owner))
            .ok_or(NativeNamedTypeError::InvalidDefinition(owner))?;
        let expected = provider.class_definition_header(owner).declaration;
        let valid = match *self.data(class) {
            TypeData::NativeObject(reference) => reference.declaration == expected
                && provider.contains_type_declaration(expected)
                && provider.type_declaration_kind(self, expected) == NativeTypeDeclarationKind::Class,
            _ => false,
        };
        if !valid { return Err(NativeNamedTypeError::DefinitionOwner { owner, expected, actual: class }) }
        let TypeData::NativeObject(reference) = *self.data(class) else { unreachable!() };
        Ok(reference)
    }

    fn native_definition_schema(&mut self, class: TypeId) -> Result<TypeId, NativeNamedTypeError> {
        let TypeData::NativeObject(reference) = *self.data(class) else { return Ok(class) };
        let Some(id) = self.native_named_provider(reference.declaration)?.type_declaration_self_parameter(reference.declaration) else { return Ok(class) };
        // A declared class head retains T -> T pairs as dependency evidence. They
        // do not change self's constraint, and must not create a distinct rigid
        // self parameter when an original method is stored on that definition.
        // Keep real substitutions so captured and instantiated bounds still map.
        let mapping = self.types().mapping(reference.mapper).iter().copied().filter(|(parameter, value)| parameter != value).collect();
        let formal = self.cloned_native_type_param(id, self.types().mapper(mapping));
        Ok(self.native_with_receiver(class, formal))
    }

    /// Contextual guidance is structural and optional. It confers neither nominal origin nor a
    /// definition value capability; only the checked value factory below does that.
    pub fn native_definition_context(&mut self, owner: NativeDefinitionId, class: TypeId) -> Result<TypeId, NativeNamedTypeError> {
        let reference = self.native_definition_class(owner, class)?;
        if let Some(&known) = self.native_named_cache.definition_contexts.get(&class) { return known }
        let native_query = NativeTypeDeclarationQuery::DefinitionContext(class);
        let query = Query::NativeNamed(native_query);
        if !self.enter(query) { return Err(NativeNamedTypeError::TooDeep(native_query)) }
        let result = self.native_definition_context_uncached(reference);
        if let Ok(stored) = self.leave(query) { self.native_named_cache.definition_context(class, result, stored); }
        if let Err(error) = result { self.report_native_named(error) }
        result
    }

    fn native_definition_context_uncached(&mut self, reference: NativeObjectReference) -> Result<TypeId, NativeNamedTypeError> {
        let class = self.intern(TypeData::NativeObject(reference));
        let schema_class = self.native_definition_schema(class)?;
        let TypeData::NativeObject(reference) = *self.data(schema_class) else { unreachable!("definition schema has its original class") };
        // An initializer receives written own contracts and inherited contracts.
        // Its own inferred recipes are results of that check, never the expected
        // type used to perform it. In particular a bad inferred override must not
        // discard an inherited callback's parameter/return context.
        let mut members = Vec::new();
        for origin in self.native_object_members(reference)? {
            if origin.via.is_some() || self.native_object_member_effective_origin(&origin)? == Some(crate::types::NativeTableFieldOrigin::Declared) { members.push(origin); }
        }
        let schema = self.native_object_resolved_uncached(reference, &members, true)?;
        let TypeData::NativeTable(data) = self.data(schema) else { unreachable!() };
        let fields: Vec<_> = data.fields.iter().map(|field| NativeTableField { optional: true, ..*field }).collect();
        let mut context = (**data).clone_in(self.arena);
        context.nominal_origin = None;
        context.definition_origin = None;
        context.fields = self.list(&fields);
        context.fresh = false;
        Ok(self.intern(TypeData::NativeTable(self.boxed(context))))
    }

    /// Reopening a class, as a repeated `---@class X` on another module's value does when
    /// a class is declared in several places: a definition of the class remains that
    /// definition, and the class itself starts a definition from all its members.
    pub fn native_definition_reopen(&mut self, owner: NativeDefinitionId, class: TypeId, actual: TypeId) -> Result<TypeId, NativeNamedTypeError> {
        let reference = self.native_definition_class(owner, class)?;
        match *self.data(actual) {
            TypeData::NativeDefinition(existing) if existing.class.declaration == reference.declaration => Ok(actual),
            TypeData::NativeObject(existing) if existing.declaration == reference.declaration => {
                let schema = self.native_object_resolved(actual)?;
                let (fields, indexers, tails) = {
                    let TypeData::NativeTable(data) = self.data(schema) else { return Ok(actual) };
                    (data.fields.to_vec(), data.indexers.to_vec(), data.sequence_tails.to_vec())
                };
                let view = self.native_table_with_context_and_sequence_policy(&fields, &indexers, &tails, NativeTableFlavor::Declared, false, None)
                    .map_err(|error| NativeNamedTypeError::DefinitionField { owner, error })?;
                Ok(self.intern(TypeData::NativeDefinition(NativeDefinitionRef { owner, class: reference, view })))
            }
            _ => self.native_definition_value(owner, class, actual),
        }
    }

    pub fn native_definition_value(&mut self, owner: NativeDefinitionId, class: TypeId, actual: TypeId) -> Result<TypeId, NativeNamedTypeError> {
        let reference = self.native_definition_class(owner, class)?;
        let TypeData::NativeTable(data) = self.data(actual) else {
            return Err(NativeNamedTypeError::DefinitionInitializer { owner, actual });
        };
        if data.nominal_origin.is_some() || data.definition_origin.is_some() {
            return Err(NativeNamedTypeError::DefinitionInitializer { owner, actual });
        }
        if let Some(error) = self.native_table_description_error(actual) {
            return Err(NativeNamedTypeError::DefinitionField { owner, error });
        }
        let mut supplied = (**data).clone_in(self.arena);
        supplied.fresh = false;
        let supplied = self.intern(TypeData::NativeTable(self.boxed(supplied)));
        let context = self.native_definition_context(owner, class)?;
        let TypeData::NativeTable(supplied_data) = self.data(supplied) else { unreachable!() };
        let TypeData::NativeTable(context_data) = self.data(context) else { unreachable!() };
        // This operation checks supplied members, rather than assigning a whole object to a weak
        // TS type. Inferred extras remain in the actual view even when no declared field is present.
        let mut relation = Relater::new(Relation::Assignable, self.cycles);
        if !self.native_tables_related::<false>(&mut relation, supplied_data, context_data, STATE_NONE).holds() {
            return Err(NativeNamedTypeError::DefinitionField { owner, error: NativeTableError::ValueType { value: actual, expected: context } });
        }
        let schema_class = self.native_definition_schema(class)?;
        let schema = self.native_object_resolved(schema_class)?;
        let TypeData::NativeTable(declared) = self.data(schema) else { unreachable!() };
        let TypeData::NativeTable(supplied) = self.data(supplied) else { unreachable!() };
        // A dynamic constructor store may supply (or overwrite) a fixed declared member. The
        // optional structural context alone cannot prove that candidate has the member's T.
        for field in &declared.fields {
            let literal = self.native_table_key_type(field.key);
            for indexer in &supplied.indexers {
                if !self.native_table_inference_key_matches(literal, indexer.key) { continue }
                let value = if supplied.collision_candidates { self.without_undefined(indexer.value) } else { indexer.value };
                if !self.is_assignable(value, field.value) {
                    return Err(NativeNamedTypeError::DefinitionField { owner, error: NativeTableError::ValueType { value, expected: field.value } });
                }
            }
            for tail in &supplied.sequence_tails {
                if !Self::native_sequence_contains(tail.start, field.key) { continue }
                let value = if supplied.collision_candidates { self.without_undefined(tail.value) } else { tail.value };
                if !self.is_assignable(value, field.value) {
                    return Err(NativeNamedTypeError::DefinitionField { owner, error: NativeTableError::ValueType { value, expected: field.value } });
                }
            }
        }
        let mut fields = supplied.fields.to_vec();
        for field in &declared.fields {
            if let Some(provided) = fields.iter_mut().find(|provided| provided.key == field.key) {
                provided.readonly |= field.readonly;
            } else {
                fields.push(NativeTableField { optional: true, ..*field });
            }
        }
        let mut indexers = supplied.indexers.to_vec();
        // A declared indexer answers for every key, assigned or not, as a TypeScript index
        // signature does; only fixed fields wait for their assignment.
        indexers.extend(declared.indexers.iter().copied());
        let view = self.native_table_with_context_and_sequence_policy(&fields, &indexers, &supplied.sequence_tails, NativeTableFlavor::Declared, false, None)
            .map_err(|error| NativeNamedTypeError::DefinitionField { owner, error })?;
        Ok(self.intern(TypeData::NativeDefinition(NativeDefinitionRef { owner, class: reference, view })))
    }

    fn native_definition_resolved(&mut self, reference: NativeDefinitionRef) -> TypeId {
        let receiver = reference.class.self_argument.unwrap_or_else(|| self.intern(TypeData::NativeDefinition(reference)));
        let mapper = self.native_receiver_mapper(reference.class, receiver);
        let view = self.instantiate(reference.view, mapper);
        let TypeData::NativeTable(view) = self.data(view) else { unreachable!("definition has a native partial view") };
        let mut data = (**view).clone_in(self.arena);
        data.nominal_origin = Some(reference.class);
        data.definition_origin = Some(reference);
        self.intern(TypeData::NativeTable(self.boxed(data)))
    }

    pub(super) fn native_definition_ref(&self, ty: TypeId) -> Option<NativeDefinitionRef> {
        match *self.data(ty) {
            TypeData::NativeDefinition(reference) => Some(reference),
            TypeData::NativeTable(ref table) => table.definition_origin,
            _ => None,
        }
    }

    /// A deferred body may use its original namespace's declared contract.
    /// This preserves the exact checked class facet without turning an alias,
    /// replacement definition or ordinary table into that namespace value.
    pub fn native_definition_member_contract(&self, owner: NativeDefinitionId, actual: TypeId) -> Option<TypeId> {
        let reference = self.native_definition_ref(actual)?;
        (reference.owner == owner).then(|| self.intern(TypeData::NativeObject(reference.class)))
    }

    /// A proven fixed store updates presence in an immutable actual view. It never reconstructs
    /// the full class instance or grants an ordinary table a declaration origin.
    pub fn native_definition_fixed_write(&mut self, table: TypeId, key: NativeTableKey, value: TypeId) -> Result<TypeId, NativeTableError> {
        let Some(reference) = self.native_definition_ref(table) else { return Err(NativeTableError::NotTable { ty: table }) };
        // A class's own table stored in one of its fields (`mt.__index = mt`) is the class.
        let value = match self.native_definition_ref(value) {
            Some(own) if own.class.declaration == reference.class.declaration => self.intern(TypeData::NativeObject(reference.class)),
            _ => value,
        };
        let key = key.normalize()?;
        self.native_definition_write(reference, NativeTableAccessKey::Literal(key), value)?;
        let declared = self.native_definition_value_context(reference, NativeTableAccessKey::Literal(key));
        let TypeData::NativeTable(data) = self.data(reference.view) else { unreachable!("definition has a native actual view") };
        let mut updated = (**data).clone_in(self.arena);
        let mut fields = data.fields.to_vec();
        let optional = self.contains_undefined(value);
        let present = self.without_undefined(value);
        if let Some(field) = fields.iter_mut().find(|field| field.key == key) {
            field.value = if field.origin == crate::types::NativeTableFieldOrigin::Declared {
                self.resolved_assignment_type(declared.unwrap_or(field.value), present)
            } else { present };
            field.optional = optional;
        } else {
            fields.push(NativeTableField { key, value: present, optional, readonly: false, origin: crate::types::NativeTableFieldOrigin::Inferred });
        }
        updated.fields = self.list(&fields);
        updated.fresh = false;
        let view = self.intern(TypeData::NativeTable(self.boxed(updated)));
        Ok(self.intern(TypeData::NativeDefinition(NativeDefinitionRef { view, ..reference })))
    }

    /// Descendant stores refine only existing definition carriers. Joins, reachability and loop
    /// publication remain in the shared flow walker; an invalid store cannot prove presence.
    pub(super) fn native_definition_flow_target(&self, table: TypeId) -> bool {
        if self.is_union(table) { return self.parts(table).iter().any(|&part| self.native_definition_flow_target(part)) }
        self.native_definition_ref(table).is_some() || self.native_runtime_table_ref(table).is_some()
    }

    pub(super) fn native_definition_flow_write(&mut self, table: TypeId, key: NativeTableKey, value: TypeId) -> TypeId {
        if table.is_never() || self.is_error_type(table) { return table }
        if self.is_union(table) { return self.map_type(table, |c, part| c.native_definition_flow_write(part, key, value)) }
        if self.native_runtime_table_ref(table).is_some() { return self.native_runtime_table_flow_write(table, key, value) }
        if self.native_definition_ref(table).is_none() { return table }
        self.native_definition_fixed_write(table, key, value).unwrap_or(table)
    }

    /// Reads expose optional presence; writes use the original class member T, never T|nil.
    pub(super) fn native_definition_write(&mut self, reference: NativeDefinitionRef, key: NativeTableAccessKey, value: TypeId) -> Result<(), NativeTableError> {
        let key = self.native_table_access(key)?;
        if let NativeTableAccessKey::Type(ty) = key && self.is_union(ty) {
            for &part in self.parts(ty) { self.native_definition_write(reference, NativeTableAccessKey::Type(part), value)?; }
            return Ok(());
        }
        let class = self.intern(TypeData::NativeObject(reference.class));
        let class = self.native_definition_schema(class).map_err(|error| error.table_error(class))?;
        let schema = self.native_object_resolved(class).map_err(|error| error.table_error(class))?;
        let TypeData::NativeTable(data) = self.data(schema) else { unreachable!() };
        if let NativeTableAccessKey::Literal(literal) = key
            && let Some(field) = data.fields.iter().find(|field| field.key == literal) {
            if field.readonly { return Err(NativeTableError::ReadOnly { key }) }
            return if self.is_assignable(value, field.value) { Ok(()) }
                else { Err(NativeTableError::ValueType { value, expected: field.value }) };
        }
        let key_type = match key { NativeTableAccessKey::Type(ty) => ty, NativeTableAccessKey::Literal(key) => self.native_table_key_type(key) };
        for field in &data.fields {
            if !self.native_table_inference_key_matches(self.native_table_key_type(field.key), key_type) { continue }
            if field.readonly { return Err(NativeTableError::ReadOnly { key }) }
            if !self.is_assignable(value, field.value) { return Err(NativeTableError::ValueType { value, expected: field.value }) }
        }
        let mut matched = false;
        for indexer in &data.indexers {
            if !self.is_assignable(key_type, indexer.key) { continue }
            matched = true;
            if indexer.readonly { return Err(NativeTableError::ReadOnly { key }) }
            if !self.is_assignable(value, indexer.value) { return Err(NativeTableError::ValueType { value, expected: indexer.value }) }
        }
        if matched { return Ok(()) }
        self.native_table_write(reference.view, key, value)
    }

    pub(super) fn native_definition_value_context(&mut self, reference: NativeDefinitionRef, key: NativeTableAccessKey) -> Option<TypeId> {
        let key = self.native_table_access(key).ok()?;
        let class = self.intern(TypeData::NativeObject(reference.class));
        let class = self.native_definition_schema(class).ok()?;
        let schema = self.native_object_resolved(class).ok()?;
        let TypeData::NativeTable(data) = self.data(schema) else { return None };
        if let NativeTableAccessKey::Literal(literal) = key
            && let Some(field) = data.fields.iter().find(|field| field.key == literal) { return Some(field.value) }
        let key_type = match key { NativeTableAccessKey::Type(ty) => ty, NativeTableAccessKey::Literal(key) => self.native_table_key_type(key) };
        let mut values: Vec<_> = data.fields.iter().filter(|field| self.native_table_inference_key_matches(self.native_table_key_type(field.key), key_type)).map(|field| field.value).collect();
        values.extend(data.indexers.iter().filter(|indexer| self.is_assignable(key_type, indexer.key)).map(|indexer| indexer.value));
        if !values.is_empty() { return Some(self.intersection(&values)) }
        self.native_table_value_context(reference.view, key)
    }
}
