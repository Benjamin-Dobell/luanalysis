//! Guarded original declaration values and environment-qualified Lua globals.
//! Providers index source declarations; merging, cycles and relations stay here.

use super::{Checker, Query};
use super::task::Stored;
use crate::types::{NativeSignatureId, TypeId};
use crate::types::CloneIn;
use crate::util::FxHashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeEnvironmentId { pub snapshot: u64, pub index: u32 }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeGlobalBindingId { pub environment: NativeEnvironmentId, pub index: u32 }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeValueDeclarationId { pub snapshot: u64, pub index: u32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativePrimitiveMemberKind { String }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeGlobalSourceRole { Implementation, Declaration }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeGlobalValueKind { Variable, Function, Member }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeValueDeclarationHeader { pub role: NativeGlobalSourceRole, pub kind: NativeGlobalValueKind }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeGlobalsQuery {
    ValueDeclarationContract(NativeValueDeclarationId),
    ValueDeclaration(NativeValueDeclarationId),
    GlobalBinding(NativeGlobalBindingId),
}

impl crate::local::MaybeLocal for NativeGlobalsQuery {
    fn is_local(&self) -> bool { false }
}

impl crate::types::Follow for NativeGlobalsQuery {
    fn visit<V: crate::types::Visitor>(&self, visitor: &mut V) {
        visitor.plain(&std::mem::discriminant(self));
        match self {
            Self::ValueDeclarationContract(NativeValueDeclarationId { snapshot, index })
            | Self::ValueDeclaration(NativeValueDeclarationId { snapshot, index }) => {
                visitor.plain(snapshot); visitor.plain(index);
            }
            Self::GlobalBinding(NativeGlobalBindingId { environment: NativeEnvironmentId { snapshot, index }, index: binding }) => {
                visitor.plain(snapshot); visitor.plain(index); visitor.plain(binding);
            }
        }
    }
    fn follow(&mut self, _: &crate::types::Link) {
        match self {
            Self::ValueDeclarationContract(NativeValueDeclarationId { snapshot: _, index: _ })
            | Self::ValueDeclaration(NativeValueDeclarationId { snapshot: _, index: _ })
            | Self::GlobalBinding(NativeGlobalBindingId { environment: NativeEnvironmentId { snapshot: _, index: _ }, index: _ }) => {}
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeGlobalQueryError {
    InvalidEnvironment(NativeEnvironmentId),
    InvalidBinding(NativeGlobalBindingId),
    InvalidDeclaration(NativeValueDeclarationId),
    InvalidPrimitiveMemberNamespace { binding: NativeGlobalBindingId, kind: NativePrimitiveMemberKind },
    MissingDeclarationContract(NativeValueDeclarationId),
    ConflictingDeclarations { binding: NativeGlobalBindingId, declarations: Vec<NativeValueDeclarationId> },
    MixedCallableDeclarations { binding: NativeGlobalBindingId, declarations: Vec<NativeValueDeclarationId> },
    InvalidDeclarationSignatures { declaration: NativeValueDeclarationId, signature: Option<NativeSignatureId> },
    CircularQuery(NativeGlobalsQuery),
    TooDeep(NativeGlobalsQuery),
    UnresolvedQuery(NativeGlobalsQuery),
}

pub trait NativeGlobalsProvider {
    /// Host-selected ordinary analysis policy. Unknown effects preserve the
    /// reached standard binding state; this supplies no heap identity or
    /// purity certificate and never overrides an observed assignment.
    fn assumed_standard_environment(&self) -> Option<NativeEnvironmentId> { None }
    /// The opt-in strict runtime-effect mode: an unresolved call is an effect
    /// barrier for heap provenance. Ordinary checking is useful rather than
    /// sound and preserves what such a call does not visibly write.
    fn strict_runtime_effects(&self) -> bool { false }
    /// Frozen initial-library capability, never a lookup of the current variable spelling.
    fn primitive_member_environment(&self) -> Option<NativeEnvironmentId> { None }
    fn primitive_member_binding(&self, _: NativeEnvironmentId, _: NativePrimitiveMemberKind) -> Option<NativeGlobalBindingId> { None }
    fn contains_global_environment(&self, environment: NativeEnvironmentId) -> bool;
    fn global_environment_binding(&self, _: NativeEnvironmentId, _: &[u8]) -> Option<NativeGlobalBindingId> { None }
    /// The selected initial self-reference, attested by declaration provenance.
    fn global_environment_self_binding(&self, _: NativeEnvironmentId) -> Option<NativeGlobalBindingId> { None }
    /// Names and original identities only; obtaining a table never reads all contracts.
    fn global_environment_bindings(&self, _: NativeEnvironmentId) -> Vec<(Vec<u8>, NativeGlobalBindingId)> { Vec::new() }
    fn contains_global_binding(&self, binding: NativeGlobalBindingId) -> bool;
    fn global_binding_declarations(&self, binding: NativeGlobalBindingId) -> Vec<NativeValueDeclarationId>;
    fn contains_value_declaration(&self, declaration: NativeValueDeclarationId) -> bool;
    fn value_declaration_header(&self, declaration: NativeValueDeclarationId) -> NativeValueDeclarationHeader;
    /// Pure original callable heads, independent of any runtime function/body.
    /// `Some` must be nonempty; `None` retains ordinary value declaration rules.
    fn value_declaration_signatures(&self, _: NativeValueDeclarationId) -> Option<Vec<NativeSignatureId>> { None }
    /// Pure original function heads, including ambient long-form functions.
    /// This does not classify a global as a bodyless callable declaration.
    fn value_declaration_function_signatures(&self, declaration: NativeValueDeclarationId) -> Option<Vec<NativeSignatureId>> { self.value_declaration_signatures(declaration) }
    /// Original annotation space; this must never inspect an ambient stub body.
    fn value_declaration_contract(&self, checker: &mut Checker<'_, '_>, declaration: NativeValueDeclarationId) -> Option<TypeId>;
    /// Executable source values only. The requesting checker owns every demand.
    fn value_declaration_initializer(&self, checker: &mut Checker<'_, '_>, declaration: NativeValueDeclarationId) -> TypeId;
    fn global_query_error(&self, _: &mut Checker<'_, '_>, _: NativeGlobalQueryError) {}
}

#[derive(Clone, Copy)]
enum NativeGlobalsValue { Type(TypeId), NoContract }
impl NativeGlobalsValue {
    fn encode(self) -> u64 { match self { Self::Type(ty) => u64::from(ty.0) + 1, Self::NoContract => 0 } }
    fn into_type(self) -> TypeId { match self { Self::Type(ty) => ty, Self::NoContract => unreachable!("value query returned an absent contract") } }
}

#[derive(Default)]
pub(super) struct NativeGlobalsCache {
    values: FxHashMap<NativeGlobalsQuery, Result<NativeGlobalsValue, NativeGlobalQueryError>>,
}
impl NativeGlobalsCache {
    fn store(&mut self, query: NativeGlobalsQuery, value: Result<NativeGlobalsValue, NativeGlobalQueryError>, _: Stored) {
        self.values.insert(query, value);
    }
}

#[derive(Clone, Copy)]
struct NativeGlobalDeclarationContext {
    serial: u64,
    incomplete_flow: bool,
    cuts: u64,
}

impl<'p, 's> Checker<'p, 's> {
    /// The environment is an open Lua table, interned by its original environment identity.
    /// Creating it does not enter any declaration query, including its own `_G` binding.
    pub fn native_global_environment_type(&mut self, environment: NativeEnvironmentId) -> Result<TypeId, NativeGlobalQueryError> {
        if !self.native_globals_provider.is_some_and(|provider| provider.contains_global_environment(environment)) {
            let error = NativeGlobalQueryError::InvalidEnvironment(environment);
            self.report_native_global(error.clone());
            return Err(error);
        }
        let table = self.native_table(&[], &[crate::types::NativeTableIndexer {
            key: TypeId::ANY, value: TypeId::ANY, readonly: false, sparse: true,
            origin: crate::types::NativeTableFieldOrigin::Declared,
        }], crate::types::NativeTableFlavor::Declared).expect("an environment has a valid open indexer");
        let crate::types::TypeData::NativeTable(data) = self.data(table) else { unreachable!() };
        let mut data = (**data).clone_in(self.arena);
        data.environment_origin = Some(environment);
        Ok(self.intern(crate::types::TypeData::NativeTable(self.boxed(data))))
    }

    pub(super) fn native_global_environment_member(&mut self, environment: NativeEnvironmentId, key: crate::types::NativeTableAccessKey) -> TypeId {
        let Some(provider) = self.native_globals_provider else { return TypeId::ERROR };
        let crate::types::NativeTableAccessKey::Literal(crate::types::NativeTableKey::String(name)) = key else { return TypeId::ANY };
        let Some(binding) = provider.global_environment_binding(environment, self.atoms().bytes(name)) else { return TypeId::ANY };
        if binding.environment != environment || !provider.contains_global_binding(binding) {
            self.report_native_global(NativeGlobalQueryError::InvalidBinding(binding));
            return TypeId::ERROR;
        }
        self.native_global_environment_binding_type(binding)
    }

    pub(super) fn native_global_environment_binding_type(&mut self, binding: NativeGlobalBindingId) -> TypeId {
        let Some(provider) = self.native_globals_provider else { return TypeId::ERROR };
        let environment = binding.environment;
        if !provider.contains_global_environment(environment) || !provider.contains_global_binding(binding) {
            self.report_native_global(NativeGlobalQueryError::InvalidBinding(binding));
            return TypeId::ERROR;
        }
        if provider.global_environment_self_binding(environment) == Some(binding) {
            return self.native_global_environment_type(environment).unwrap_or(TypeId::ERROR);
        }
        self.native_global_binding_type(binding).unwrap_or(TypeId::ERROR)
    }

    pub(super) fn native_global_environment_properties(&mut self, environment: NativeEnvironmentId) -> Vec<(crate::atom::Atom, NativeGlobalBindingId)> {
        let Some(provider) = self.native_globals_provider else { return Vec::new() };
        provider.global_environment_bindings(environment).into_iter().filter_map(|(name, binding)| {
            if binding.environment != environment || !provider.contains_global_binding(binding) {
                self.report_native_global(NativeGlobalQueryError::InvalidBinding(binding));
                None
            } else { Some((self.atoms().intern(&name), binding)) }
        }).collect()
    }

    /// Full shape demand is explicit: preserve original global contracts in native inference
    /// and structural relations, while ordinary property reads remain one-binding demands.
    pub(super) fn native_global_environment_fields(&mut self, environment: NativeEnvironmentId) -> Vec<crate::types::NativeTableField> {
        let Some(provider) = self.native_globals_provider else { return Vec::new() };
        let mut fields = Vec::new();
        for (name, binding) in provider.global_environment_bindings(environment) {
            if binding.environment != environment {
                self.report_native_global(NativeGlobalQueryError::InvalidBinding(binding));
                continue;
            }
            let value = self.native_global_environment_binding_type(binding);
            fields.push(crate::types::NativeTableField {
                key: crate::types::NativeTableKey::String(self.atoms().intern(&name)), value,
                optional: false, readonly: false, origin: crate::types::NativeTableFieldOrigin::Declared,
            });
        }
        fields
    }
    /// None is the ordinary backend property path. A selected Lua namespace
    /// keeps its typed lookup errors, including provisional member context.
    pub(super) fn native_primitive_property(&mut self, ty: TypeId, name: crate::atom::Atom) -> Option<Result<TypeId, super::native_tables::NativeTableError>> {
        use super::native_tables::NativeTableError;
        use crate::types::{NativeTableAccessKey, NativeTableKey};
        let provider = self.native_globals_provider?;
        let environment = provider.primitive_member_environment()?;
        let key = NativeTableAccessKey::Literal(NativeTableKey::String(name));
        let apparent = self.base_constraint_of(ty).unwrap_or(ty);
        if self.is_union(apparent) {
            let parts = self.parts(apparent).to_vec();
            if !parts.iter().any(|&part| {
                let apparent = self.base_constraint_of(part).unwrap_or(part);
                self.flags(apparent) & crate::types::tf::STRING_LIKE != 0
            }) { return None }
            let mut values = Vec::with_capacity(parts.len());
            for part in parts {
                let value = if let Some(value) = self.native_primitive_property(part, name) {
                    value
                } else if matches!(self.data(part), crate::types::TypeData::NativeTable(_) | crate::types::TypeData::NativeObject(_) | crate::types::TypeData::NativeDefinition(_)) {
                    self.native_table_read(part, key)
                } else {
                    self.type_of_property(part, name).ok_or(NativeTableError::MissingKey { key })
                };
                match value {
                    Ok(value) => values.push(value),
                    Err(error) => return Some(Err(error)),
                }
            }
            return Some(Ok(self.union(&values)));
        }
        if self.flags(apparent) & crate::types::tf::STRING_LIKE == 0 { return None }
        if !provider.contains_global_environment(environment) {
            self.report_native_global(NativeGlobalQueryError::InvalidEnvironment(environment));
            return Some(Err(NativeTableError::MissingKey { key }));
        }
        let Some(binding) = provider.primitive_member_binding(environment, NativePrimitiveMemberKind::String) else { return None };
        if binding.environment != environment {
            self.report_native_global(NativeGlobalQueryError::InvalidBinding(binding));
            return Some(Err(NativeTableError::MissingKey { key }));
        }
        let Ok(namespace) = self.native_global_binding_type(binding) else { return Some(Err(NativeTableError::MissingKey { key })) };
        // Only original native table/object namespaces qualify. In particular, a
        // string/any contract cannot recurse back into this primitive lookup.
        if !matches!(self.data(namespace), crate::types::TypeData::NativeTable(_) | crate::types::TypeData::NativeObject(_)) {
            self.report_native_global(NativeGlobalQueryError::InvalidPrimitiveMemberNamespace { binding, kind: NativePrimitiveMemberKind::String });
            return Some(Err(NativeTableError::MissingKey { key }));
        }
        Some(self.native_table_read(namespace, key))
    }

    /// The namespace a string's metatable indexes, the string library, whose class can declare
    /// the operators strings have (LuaLS's `---@class string` with `---@operator`).
    pub(super) fn native_primitive_namespace(&mut self, ty: TypeId) -> Option<TypeId> {
        let provider = self.native_globals_provider?;
        let environment = provider.primitive_member_environment()?;
        let apparent = self.base_constraint_of(ty).unwrap_or(ty);
        if self.is_union(apparent) || self.flags(apparent) & crate::types::tf::STRING_LIKE == 0 { return None }
        if !provider.contains_global_environment(environment) { return None }
        let binding = provider.primitive_member_binding(environment, NativePrimitiveMemberKind::String)?;
        if binding.environment != environment { return None }
        let namespace = self.native_global_binding_type(binding).ok()?;
        matches!(self.data(namespace), crate::types::TypeData::NativeTable(_) | crate::types::TypeData::NativeObject(_)).then_some(namespace)
    }

    pub fn set_native_globals_provider(&mut self, provider: &'p dyn NativeGlobalsProvider) {
        assert!(self.native_globals_provider.is_none(), "native globals provider already installed");
        self.native_globals_provider = Some(provider);
    }

    pub(super) fn report_native_global(&mut self, error: NativeGlobalQueryError) {
        if let Some(provider) = self.native_globals_provider { provider.global_query_error(self, error); }
    }

    fn native_globals_demand(
        &mut self,
        demand: NativeGlobalsQuery,
        compute: impl FnOnce(&mut Self, &'p dyn NativeGlobalsProvider) -> Result<NativeGlobalsValue, NativeGlobalQueryError>,
    ) -> Result<NativeGlobalsValue, NativeGlobalQueryError> {
        let invalid = match demand {
            NativeGlobalsQuery::ValueDeclarationContract(declaration)
            | NativeGlobalsQuery::ValueDeclaration(declaration) => NativeGlobalQueryError::InvalidDeclaration(declaration),
            NativeGlobalsQuery::GlobalBinding(binding) => NativeGlobalQueryError::InvalidBinding(binding),
        };
        let Some(provider) = self.native_globals_provider else { return Err(invalid); };
        if let NativeGlobalsQuery::GlobalBinding(binding) = demand
            && !provider.contains_global_environment(binding.environment) {
            let error = NativeGlobalQueryError::InvalidEnvironment(binding.environment);
            self.report_native_global(error.clone());
            return Err(error);
        }
        let valid = match demand {
            NativeGlobalsQuery::ValueDeclarationContract(declaration)
            | NativeGlobalsQuery::ValueDeclaration(declaration) => provider.contains_value_declaration(declaration),
            NativeGlobalsQuery::GlobalBinding(binding) => provider.contains_global_binding(binding),
        };
        if !valid { self.report_native_global(invalid.clone()); return Err(invalid); }
        if let Some(known) = self.p.native_global_results.get(&self.task, &demand) {
            return Ok(known.map_or(NativeGlobalsValue::NoContract, NativeGlobalsValue::Type));
        }
        if let Some(result) = self.native_globals_cache.values.get(&demand) { return result.clone(); }
        let query = Query::NativeGlobals(demand);
        // Only unresolved computations enter this provisional table. A hit
        // retains their dependency/lifetime effects, not permission to use the
        // fallback as a finished declaration. Current-only pure contracts take
        // the uncached completion path below and never create such an entry.
        if self.provisional(query).is_some() { return Err(NativeGlobalQueryError::UnresolvedQuery(demand)); }
        if !self.enter(query) {
            let error = if self.found_cycle { NativeGlobalQueryError::CircularQuery(demand) } else { NativeGlobalQueryError::TooDeep(demand) };
            self.report_native_global(error.clone());
            return Err(error);
        }
        // A declaration can be demanded while its consumer is traversing an
        // incomplete loop, or under another context that is not cacheable. It is
        // computed at its own lexical root (`native_globals_at_lexical_root`), so
        // its result remains usable in that original context, without becoming a
        // completed cached declaration, when nothing it depends on is unsettled.
        let frame = self.frames.last().expect("entered global declaration query");
        let context = (frame.tainted && !frame.circular).then_some(NativeGlobalDeclarationContext {
            serial: frame.serial,
            incomplete_flow: frame.incomplete_flow,
            cuts: self.cuts(),
        });
        let (result, memoizable) = self.run_memoizable(|checker| compute(checker, provider));
        let left = self.leave(query);
        if self.left_a_cycle {
            let error = NativeGlobalQueryError::CircularQuery(demand);
            self.report_native_global(error.clone());
            self.native_globals_cache.store(demand, Err(error.clone()), self.cycle_result());
            return Err(error);
        }
        match left {
            Ok(stored) => {
                if let Ok(value) = &result {
                    let ty = match value { NativeGlobalsValue::Type(ty) => Some(*ty), NativeGlobalsValue::NoContract => None };
                    self.p.native_global_results.insert(&self.task, demand, ty, stored);
                }
                self.native_globals_cache.store(demand, result.clone(), stored);
            }
            Err(_) if result.is_ok() && context.is_some_and(|context| {
                memoizable.is_some() && self.cuts() == context.cuts
                    && self.lowest_taint_since(context.serial) == usize::MAX
                    && self.left_frame.as_ref().is_some_and(|frame| {
                        frame.serial == context.serial && !frame.circular
                            && frame.incomplete_flow == context.incomplete_flow
                    })
            }) => { self.left_frame.take(); },
            Err(open) => if let Ok(value) = result {
                self.cache_provisionally(query, value.encode(), open);
                let error = NativeGlobalQueryError::UnresolvedQuery(demand);
                self.report_native_global(error.clone());
                return Err(error);
            },
        }
        if let Err(error) = &result { self.report_native_global(error.clone()); }
        result
    }

    fn native_globals_at_lexical_root<R>(&mut self, compute: impl FnOnce(&mut Self) -> R) -> R {
        // Globals cannot inherit the first consumer's expected function or
        // captured environment. The active native query stack remains intact.
        let contextual = std::mem::take(&mut self.native_function_contexts);
        let captured = std::mem::take(&mut self.native_capture_contexts);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compute(self)));
        self.native_function_contexts = contextual;
        self.native_capture_contexts = captured;
        result.unwrap_or_else(|error| std::panic::resume_unwind(error))
    }

    fn native_value_declaration_contract(&mut self, declaration: NativeValueDeclarationId) -> Result<Option<TypeId>, NativeGlobalQueryError> {
        self.native_globals_demand(NativeGlobalsQuery::ValueDeclarationContract(declaration), |checker, provider| {
            checker.native_globals_at_lexical_root(|checker| {
                if let Some(signatures) = provider.value_declaration_signatures(declaration) {
                    let ty = checker.native_global_declaration_signatures(declaration, &signatures)?;
                    return Ok(NativeGlobalsValue::Type(ty));
                }
                Ok(provider.value_declaration_contract(checker, declaration)
                    .map_or(NativeGlobalsValue::NoContract, NativeGlobalsValue::Type))
            })
        }).map(|value| match value { NativeGlobalsValue::Type(ty) => Some(ty), NativeGlobalsValue::NoContract => None })
    }

    fn native_global_declaration_signatures(&mut self, declaration: NativeValueDeclarationId, signatures: &[NativeSignatureId]) -> Result<TypeId, NativeGlobalQueryError> {
        if signatures.is_empty() {
            return Err(NativeGlobalQueryError::InvalidDeclarationSignatures { declaration, signature: None });
        }
        self.native_signature_group_type(signatures).map_err(|signature| {
            NativeGlobalQueryError::InvalidDeclarationSignatures { declaration, signature: Some(signature) }
        })
    }

    pub fn native_value_declaration_type(&mut self, declaration: NativeValueDeclarationId) -> Result<TypeId, NativeGlobalQueryError> {
        self.native_globals_demand(NativeGlobalsQuery::ValueDeclaration(declaration), |checker, provider| {
            if let Some(contract) = checker.native_value_declaration_contract(declaration)? { return Ok(NativeGlobalsValue::Type(contract)); }
            if provider.value_declaration_header(declaration).role == NativeGlobalSourceRole::Declaration {
                return Err(NativeGlobalQueryError::MissingDeclarationContract(declaration));
            }
            checker.native_globals_at_lexical_root(|checker| {
                let initializer = provider.value_declaration_initializer(checker, declaration);
                Ok(NativeGlobalsValue::Type(checker.widened(initializer)))
            })
        }).map(NativeGlobalsValue::into_type)
    }

    pub fn native_global_binding_type(&mut self, binding: NativeGlobalBindingId) -> Result<TypeId, NativeGlobalQueryError> {
        self.native_globals_demand(NativeGlobalsQuery::GlobalBinding(binding), |checker, provider| {
            let mut declarations = provider.global_binding_declarations(binding);
            declarations.sort_unstable_by_key(|key| (key.snapshot, key.index));
            declarations.dedup();
            if declarations.is_empty() { return Err(NativeGlobalQueryError::InvalidBinding(binding)); }
            let mut callable = Vec::new();
            let mut ordinary = false;
            for &declaration in &declarations {
                if !provider.contains_value_declaration(declaration) {
                    return Err(NativeGlobalQueryError::InvalidDeclaration(declaration));
                }
                if let Some(signatures) = provider.value_declaration_signatures(declaration) {
                    callable.push((declaration, signatures));
                } else {
                    ordinary = true;
                }
            }
            if !callable.is_empty() {
                if ordinary {
                    return Err(NativeGlobalQueryError::MixedCallableDeclarations { binding, declarations });
                }
                return checker.native_globals_at_lexical_root(|checker| {
                    let mut signatures = Vec::new();
                    for (declaration, heads) in callable {
                        // Validate each original owner independently so errors
                        // retain that declaration's source identity.
                        checker.native_global_declaration_signatures(declaration, &heads)?;
                        signatures.extend(heads);
                    }
                    let ty = checker.native_signature_group_type(&signatures).expect("validated original global heads");
                    Ok(NativeGlobalsValue::Type(ty))
                });
            }
            let mut values = Vec::with_capacity(declarations.len());
            for &declaration in &declarations {
                if let Some(contract) = checker.native_value_declaration_contract(declaration)? {
                    values.push((declaration, contract));
                } else if provider.value_declaration_header(declaration).role == NativeGlobalSourceRole::Declaration {
                    return Err(NativeGlobalQueryError::MissingDeclarationContract(declaration));
                }
            }
            // Written schemas are authoritative. Executable initializers/writes
            // are checked against them at their original expression sites; their
            // inferred narrower values must not become conflicting declarations.
            if values.is_empty() {
                for &declaration in &declarations { values.push((declaration, checker.native_value_declaration_type(declaration)?)); }
            }
            let mut conflicts = Vec::new();
            for left in 0..values.len() {
                for right in left + 1..values.len() {
                    if !checker.is_identical(values[left].1, values[right].1) {
                        conflicts.extend([values[left].0, values[right].0]);
                    }
                }
            }
            if !conflicts.is_empty() {
                conflicts.sort_unstable_by_key(|key| (key.snapshot, key.index));
                conflicts.dedup();
                return Err(NativeGlobalQueryError::ConflictingDeclarations { binding, declarations: conflicts });
            }
            Ok(NativeGlobalsValue::Type(values[0].1))
        }).map(NativeGlobalsValue::into_type)
    }
}
