//! Lazy named declaration queries for a foreign frontend's original identities.
//!
//! Aliases resolve to their actual primitive, union or object TypeId. Structural object heads
//! carry a declaration and MapperId; they are published before any child annotation demand.
//! The ordinary query enter/leave/provisional/Stored machinery detects unproductive alias cycles.
//!
//! Raw member annotations remain in declaration space and are substituted once for an object
//! mapper. Literal member reads request only the selected field. Full structural demands build
//! ordinary NativeTable descriptions, preserving Lua numeric, boolean and typed key domains.
//! Source array heads also describe Lua numeric tables; intrinsic NativeArray remains available
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
use crate::types::{CloneIn, MapperId, NativeFunctionId, NativeSignatureId, NativeTableKey, NativeTypeParamId, SigData, SigId, TypeId, TypeData, NativeTableField, NativeTableIndexer, NativeTableFlavor};
use super::native_tables::{NativeTableAccessKey, NativeTableError};
use super::relate::{Relation, Relater, STATE_NONE};
use super::task::Stored;
use crate::util::FxHashMap;

/// Original named declaration, or an anonymous object annotation head registered by the source.
/// File-local syntax ids must be mapped explicitly into this provider-owned namespace.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeTypeDeclarationId {
    pub snapshot: u64,
    pub index: u32,
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
    DeclaredHead(NativeTypeDeclarationId),
    MemberType(NativeTypeMemberId),
    IndexerKey(NativeTypeMemberId),
    Bases(NativeTypeDeclarationId),
    ObjectMembers(TypeId),
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum NativeNamedTypeError {
    InvalidDefinition(NativeDefinitionId),
    DefinitionOwner { owner: NativeDefinitionId, expected: NativeTypeDeclarationId, actual: TypeId },
    DefinitionInitializer { owner: NativeDefinitionId, actual: TypeId },
    DefinitionField { owner: NativeDefinitionId, error: NativeTableError },
    InvalidDeclaration(NativeTypeDeclarationId),
    InvalidMember(NativeTypeMemberId),
    InvalidTypeParameter(NativeTypeParamId),
    UnknownOwner { member: NativeTypeMemberId, name: Atom },
    AmbiguousOwner { member: NativeTypeMemberId, name: Atom },
    TypeArgumentArity { declaration: NativeTypeDeclarationId, actual: usize, minimum: usize, maximum: usize },
    TypeArgumentConstraint { declaration: NativeTypeDeclarationId, index: usize, actual: TypeId, expected: TypeId },
    CircularAlias(NativeTypeDeclarationId),
    CircularBase(NativeTypeDeclarationId),
    TooDeep(NativeTypeDeclarationQuery),
    UnsupportedKind { declaration: NativeTypeDeclarationId, kind: NativeTypeDeclarationKind },
    UnsupportedBases(NativeTypeDeclarationId),
    UnsupportedVisibility(NativeTypeMemberId),
    UnsupportedOwnerInstantiation(NativeTypeMemberId),
    Table { declaration: NativeTypeDeclarationId, error: NativeTableError },
}

/// Declaration provider contract for native named-type integration. Every returned type
/// is in declaration space, including member/indexer/bases queries. The backend owns resolving,
/// substitution, recursive publication, inference, relation checks and semantic diagnostics.
pub trait NativeTypeDeclarationProvider {
    fn contains_class_definition(&self, _: NativeDefinitionId) -> bool { false }
    fn class_definition_header(&self, _: NativeDefinitionId) -> NativeDefinitionHeader { panic!("class definition provider absent") }
    fn contains_type_declaration(&self, declaration: NativeTypeDeclarationId) -> bool;
    fn type_declaration_header(&self, checker: &Checker<'_, '_>, declaration: NativeTypeDeclarationId) -> NativeTypeDeclarationHeader;

    fn contains_type_member(&self, declaration: NativeTypeMemberId) -> bool;
    fn type_member_header(&self, checker: &Checker<'_, '_>, declaration: NativeTypeMemberId) -> NativeTypeMemberHeader;
    /// Own declarations plus potential explicit-owner augmentations; the backend validates each
    /// original owner candidate before publishing or inheriting its member origin.
    fn type_declaration_members(&self, declaration: NativeTypeDeclarationId) -> Vec<NativeTypeMemberId>;
    fn type_declaration_call_signatures(&self, _: NativeTypeDeclarationId) -> Vec<NativeSignatureId> { Vec::new() }

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

/// Constructor metadata for the native object TypeData. The mapper must preserve
/// identity pairs for all own/outer declaration parameters until actual substitutions are known.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeObjectReference {
    pub declaration: NativeTypeDeclarationId,
    pub mapper: MapperId,
}


/// These entries are checker-local because their provider belongs to that checker. Publication is
/// still permitted only by the common query machinery's Stored token. No unfinished member result
/// is silently retained as a completed declaration.
#[derive(Default)]
pub(super) struct NativeNamedCache {
    heads: FxHashMap<NativeTypeDeclarationId, Result<TypeId, NativeNamedTypeError>>,
    types: FxHashMap<NativeTypeDeclarationQuery, TypeId>,
    objects: FxHashMap<TypeId, Result<TypeId, NativeNamedTypeError>>,
}

impl NativeNamedCache {
    fn head(&mut self, key: NativeTypeDeclarationId, value: Result<TypeId, NativeNamedTypeError>, _: Stored) {
        self.heads.insert(key, value);
    }
    fn ty(&mut self, key: NativeTypeDeclarationQuery, value: TypeId, _: Stored) {
        self.types.insert(key, value);
    }
    fn object(&mut self, key: TypeId, value: Result<TypeId, NativeNamedTypeError>, _: Stored) {
        self.objects.insert(key, value);
    }
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

    fn report_native_named(&mut self, error: NativeNamedTypeError) {
        let Some(provider) = self.native_type_declaration_provider else { return };
        match error {
            NativeNamedTypeError::CircularAlias(id) => provider.circular_type_alias(id),
            NativeNamedTypeError::CircularBase(id) => provider.circular_type_base(id),
            NativeNamedTypeError::TooDeep(query) => provider.type_declaration_too_deep(query),
            _ => {}
        }
        provider.type_declaration_error(self, error);
    }

    pub fn native_declared_head(&mut self, declaration: NativeTypeDeclarationId) -> Result<TypeId, NativeNamedTypeError> {
        if let Some(&known) = self.native_named_cache.heads.get(&declaration) { return known }
        let provider = self.native_named_provider(declaration).map_err(|error| { self.report_native_named(error); error })?;
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
            NativeTypeDeclarationKind::Shape | NativeTypeDeclarationKind::Class => {
                if let Some(NativeObjectDeclarationForm::Table { type_argument_count }) = header.object_form
                    && type_argument_count != 2 {
                    Err(NativeNamedTypeError::TypeArgumentArity { declaration, actual: type_argument_count, minimum: 2, maximum: 2 })
                } else {
                let parameters: Result<Vec<_>, _> = header.outer_parameters.iter().chain(&header.parameters)
                    .map(|&id| self.native_type_parameter(id).ok_or(NativeNamedTypeError::InvalidTypeParameter(id))).collect();
                parameters.map(|parameters| {
                    let mapper = self.mapper_from(&parameters, &parameters);
                    self.intern(TypeData::NativeObject(NativeObjectReference { declaration, mapper }))
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
            Ok(stored) => self.native_named_cache.head(declaration, result, stored),
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
        for (index, (&parameter, &actual)) in parameters.iter().zip(&filled).enumerate() {
            // A declaration parameter is assumed to satisfy its own bound. Demanding that bound
            // while resolving Node<T>'s F-bound Node<T> would otherwise manufacture a constraint
            // cycle before the structural head can be used by the ordinary relation worker.
            if actual == parameter { continue }
            if let Some(constraint) = self.constraint_of_type_param(parameter) {
                let expected = self.instantiate(constraint, mapper);
                if !self.is_assignable(actual, expected) {
                    let error = NativeNamedTypeError::TypeArgumentConstraint { declaration, index, actual, expected };
                    self.report_native_named(error);
                    return Err(error);
                }
            }
        }
        let declared = self.native_declared_head(declaration)?;
        Ok(self.type_alias_instantiation(declared, mapper))
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

    fn native_named_member_demand(&mut self, member: NativeTypeMemberId, key: bool) -> Result<TypeId, NativeNamedTypeError> {
        let provider = self.native_type_declaration_provider.filter(|p| p.contains_type_member(member))
            .ok_or(NativeNamedTypeError::InvalidMember(member))?;
        let native_query = if key { NativeTypeDeclarationQuery::IndexerKey(member) } else { NativeTypeDeclarationQuery::MemberType(member) };
        if let Some(&known) = self.native_named_cache.types.get(&native_query) { return Ok(known) }
        let query = Query::NativeNamed(native_query);
        if let Some(raw) = self.provisional(query) { return Ok(TypeId(raw as u32)) }
        if !self.enter(query) {
            let error = NativeNamedTypeError::TooDeep(native_query);
            self.report_native_named(error);
            return Err(error);
        }
        let result = if key { provider.type_indexer_key(self, member) } else { provider.type_member_type(self, member) };
        match self.leave(query) {
            Ok(stored) => self.native_named_cache.ty(native_query, result, stored),
            Err(open) => self.cache_provisionally(query, u64::from(result.0), open),
        }
        Ok(result)
    }

    fn native_named_member_origin(&mut self, member: NativeTypeMemberId) -> Result<(NativeObjectReference, NativeTypeMemberHeader), NativeNamedTypeError> {
        let provider = self.native_type_declaration_provider.ok_or(NativeNamedTypeError::InvalidMember(member))?;
        if !provider.contains_type_member(member) { return Err(NativeNamedTypeError::InvalidMember(member)) }
        let header = provider.type_member_header(self, member);
        if header.visibility != NativeTypeMemberVisibility::Public { return Err(NativeNamedTypeError::UnsupportedVisibility(member)) }
        let owner = match &header.owner {
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
                    let target = self.native_declared_head(*owner)?;
                    match self.data(target) {
                        TypeData::NativeObject(reference) => {
                            if self.types().mapping(reference.mapper).iter().any(|(from, to)| from != to) {
                                return Err(NativeNamedTypeError::UnsupportedOwnerInstantiation(member));
                            }
                            *reference
                        },
                        _ => return Err(NativeNamedTypeError::UnknownOwner { member, name: *name }),
                    }
                }
                _ => return Err(NativeNamedTypeError::AmbiguousOwner { member, name: *name }),
            },
        };
        Ok((owner, header))
    }

    fn native_named_member_header(&mut self, declaration: NativeTypeDeclarationId, member: NativeTypeMemberId) -> Result<Option<NativeTypeMemberHeader>, NativeNamedTypeError> {
        let (owner, header) = self.native_named_member_origin(member)?;
        Ok((owner.declaration == declaration).then_some(header))
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
                self.native_table(&[], &[NativeTableIndexer { key, value, readonly: header.readonly, sparse }], NativeTableFlavor::Declared)
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
                .filter(|p| p.type_declaration_header(self, reference.declaration).kind.is_nominal())
                .map(|_| reference),
            TypeData::NativeTable(ref table) => table.nominal_origin,
            _ => None,
        }
    }

    /// Materialize a declaration-space description once per object mapper. Recursive member types
    /// only request the already-published head, so productive recursion does not recurse into fields.
    pub fn native_object_resolved(&mut self, ty: TypeId) -> Result<TypeId, NativeNamedTypeError> {
        if let TypeData::NativeDefinition(reference) = *self.data(ty) { return Ok(self.native_definition_resolved(reference)) }
        let TypeData::NativeObject(reference) = *self.data(ty) else { return Ok(ty) };
        if let Some(&known) = self.native_named_cache.objects.get(&ty) { return known }
        let native_query = NativeTypeDeclarationQuery::ObjectMembers(ty);
        let query = Query::NativeNamed(native_query);
        if !self.enter(query) {
            let error = NativeNamedTypeError::TooDeep(native_query);
            self.report_native_named(error);
            return Err(error);
        }
        let result = self.native_object_resolved_uncached(reference);
        match self.leave(query) {
            Ok(stored) => self.native_named_cache.object(ty, result, stored),
            Err(_) => {},
        }
        if let Err(error) = result { self.report_native_named(error) }
        result
    }

    /// Callable named objects keep their original signature view in recursive
    /// relations; exact Lua fields/keys still use the resolved table view.
    pub(super) fn native_object_relation_view(&mut self, ty: TypeId) -> Result<TypeId, NativeNamedTypeError> {
        let resolved = self.native_object_resolved(ty)?;
        if self.native_object_has_call_signatures(ty) { return Ok(ty) }
        Ok(resolved)
    }

    /// A definition's call contract comes only from its explicitly attached class. Its actual
    /// member view remains separate, including after materialization and mapper substitution.
    fn native_object_call_reference(&self, ty: TypeId) -> Option<NativeObjectReference> {
        match *self.data(ty) {
            TypeData::NativeObject(reference) => Some(reference),
            TypeData::NativeDefinition(reference) => Some(reference.class),
            TypeData::NativeTable(ref table) => table.definition_origin.map(|reference| reference.class),
            _ => None,
        }
    }

    pub(super) fn native_object_has_call_signatures(&self, ty: TypeId) -> bool {
        self.native_object_call_reference(ty).is_some_and(|reference| self.native_named_provider(reference.declaration)
            .is_ok_and(|provider| !provider.type_declaration_call_signatures(reference.declaration).is_empty()))
    }

    pub(super) fn native_object_call_heads(&mut self, ty: TypeId) -> Vec<SigId> {
        let Some(reference) = self.native_object_call_reference(ty) else { return Vec::new() };
        let Ok(provider) = self.native_named_provider(reference.declaration) else { return Vec::new() };
        provider.type_declaration_call_signatures(reference.declaration).into_iter().map(|declaration| {
            self.native_signature_provider_for(declaration);
            self.types().intern_sig(SigData::NativeDeclarationSignature { declaration, mapper: reference.mapper, implementation: None })
        }).collect()
    }

    fn native_object_resolved_uncached(&mut self, reference: NativeObjectReference) -> Result<TypeId, NativeNamedTypeError> {
        let declaration = reference.declaration;
        let provider = self.native_named_provider(declaration)?;
        let header = provider.type_declaration_header(self, declaration);
        if !provider.type_declaration_bases(self, declaration).is_empty() { return Err(NativeNamedTypeError::UnsupportedBases(declaration)) }
        if let Some(NativeObjectDeclarationForm::Table { type_argument_count }) = header.object_form
            && type_argument_count != 2 {
            return Err(NativeNamedTypeError::TypeArgumentArity { declaration, actual: type_argument_count, minimum: 2, maximum: 2 });
        }
        let mut fields = Vec::new();
        let mut indexers = Vec::new();
        for member in provider.type_declaration_members(declaration) {
            let Some(metadata) = self.native_named_member_header(declaration, member)? else { continue };
            let raw = self.native_named_member_demand(member, false)?;
            let value = self.instantiate(raw, reference.mapper);
            match metadata.key {
                NativeTypeMemberKey::Fixed(key) => fields.push(NativeTableField { key, value, optional: metadata.optional, readonly: metadata.readonly }),
                NativeTypeMemberKey::Indexer { sparse } => {
                    let raw_key = self.native_named_member_demand(member, true)?;
                    let key = self.instantiate(raw_key, reference.mapper);
                    indexers.push(NativeTableIndexer { key, value, readonly: metadata.readonly, sparse });
                }
            }
        }
        if header.object_form == Some(NativeObjectDeclarationForm::Array) {
            if indexers.len() != 1 || indexers[0].key != TypeId::NUMBER || !fields.is_empty() {
                return Err(NativeNamedTypeError::InvalidDeclaration(declaration));
            }
        }
        let table = self.native_table(&fields, &indexers, NativeTableFlavor::Declared)
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
        if let NativeTableAccessKey::Literal(key) = key {
            let key = key.normalize()?;
            let provider = self.native_named_provider(reference.declaration).map_err(|_| NativeTableError::NotTable { ty: TypeId::ERROR })?;
            let mut found = None;
            for member in provider.type_declaration_members(reference.declaration) {
                let Some(metadata) = self.native_named_member_header(reference.declaration, member)
                    .map_err(|error| { self.report_native_named(error); NativeTableError::NotTable { ty: TypeId::ERROR } })? else { continue };
                if let NativeTypeMemberKey::Fixed(member_key) = metadata.key
                    && member_key.normalize()? == key {
                    if found.is_some() { return Err(NativeTableError::DuplicateDeclaredField { key }) }
                    found = Some((member, metadata));
                }
            }
            if let Some((member, metadata)) = found {
                let raw = self.native_named_member_demand(member, false).map_err(|_| NativeTableError::NotTable { ty: TypeId::ERROR })?;
                let value = self.instantiate(raw, reference.mapper);
                return Ok(if metadata.optional { self.optional(value) } else { value });
            }
        }
        let object = self.intern(TypeData::NativeObject(reference));
        let resolved = self.native_object_resolved(object).map_err(|_| NativeTableError::NotTable { ty: object })?;
        self.native_table_read(resolved, key)
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
                && provider.type_declaration_header(self, expected).kind == NativeTypeDeclarationKind::Class,
            _ => false,
        };
        if !valid { return Err(NativeNamedTypeError::DefinitionOwner { owner, expected, actual: class }) }
        let TypeData::NativeObject(reference) = *self.data(class) else { unreachable!() };
        self.native_object_resolved(class)?;
        Ok(reference)
    }

    /// Contextual guidance is structural and optional. It confers neither nominal origin nor a
    /// definition value capability; only the checked value factory below does that.
    pub fn native_definition_context(&mut self, owner: NativeDefinitionId, class: TypeId) -> Result<TypeId, NativeNamedTypeError> {
        self.native_definition_class(owner, class)?;
        let schema = self.native_object_resolved(class)?;
        let TypeData::NativeTable(data) = self.data(schema) else { unreachable!() };
        let fields: Vec<_> = data.fields.iter().map(|field| NativeTableField { optional: true, ..*field }).collect();
        let mut context = (**data).clone_in(self.arena);
        context.nominal_origin = None;
        context.definition_origin = None;
        context.fields = self.list(&fields);
        context.fresh = false;
        Ok(self.intern(TypeData::NativeTable(self.boxed(context))))
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
        let schema = self.native_object_resolved(class)?;
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
        indexers.extend(declared.indexers.iter().map(|indexer| NativeTableIndexer { sparse: true, ..*indexer }));
        let view = self.native_table_with_context_and_sequence_policy(&fields, &indexers, &supplied.sequence_tails, NativeTableFlavor::Declared, false, None)
            .map_err(|error| NativeNamedTypeError::DefinitionField { owner, error })?;
        Ok(self.intern(TypeData::NativeDefinition(NativeDefinitionRef { owner, class: reference, view })))
    }

    fn native_definition_resolved(&self, reference: NativeDefinitionRef) -> TypeId {
        let TypeData::NativeTable(view) = self.data(reference.view) else { unreachable!("definition has a native partial view") };
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

    /// Reads expose optional presence; writes use the original class member T, never T|nil.
    pub(super) fn native_definition_write(&mut self, reference: NativeDefinitionRef, key: NativeTableAccessKey, value: TypeId) -> Result<(), NativeTableError> {
        let key = self.native_table_access(key)?;
        if let NativeTableAccessKey::Type(ty) = key && self.is_union(ty) {
            for &part in self.parts(ty) { self.native_definition_write(reference, NativeTableAccessKey::Type(part), value)?; }
            return Ok(());
        }
        let class = self.intern(TypeData::NativeObject(reference.class));
        let schema = self.native_object_resolved(class).map_err(|_| NativeTableError::NotTable { ty: class })?;
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
