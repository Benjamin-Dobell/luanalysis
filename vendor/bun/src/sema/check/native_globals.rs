//! Guarded original declaration values and environment-qualified Lua globals.
//! Providers index source declarations; merging, cycles and relations stay here.

use super::{Checker, Query};
use super::task::Stored;
use crate::types::TypeId;
use crate::util::FxHashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeEnvironmentId { pub snapshot: u64, pub index: u32 }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeGlobalBindingId { pub environment: NativeEnvironmentId, pub index: u32 }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeValueDeclarationId { pub snapshot: u64, pub index: u32 }

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeGlobalQueryError {
    InvalidEnvironment(NativeEnvironmentId),
    InvalidBinding(NativeGlobalBindingId),
    InvalidDeclaration(NativeValueDeclarationId),
    MissingDeclarationContract(NativeValueDeclarationId),
    ConflictingDeclarations { binding: NativeGlobalBindingId, declarations: Vec<NativeValueDeclarationId> },
    CircularQuery(NativeGlobalsQuery),
    TooDeep(NativeGlobalsQuery),
    UnresolvedQuery(NativeGlobalsQuery),
}

pub trait NativeGlobalsProvider {
    fn contains_global_environment(&self, environment: NativeEnvironmentId) -> bool;
    fn contains_global_binding(&self, binding: NativeGlobalBindingId) -> bool;
    fn global_binding_declarations(&self, binding: NativeGlobalBindingId) -> Vec<NativeValueDeclarationId>;
    fn contains_value_declaration(&self, declaration: NativeValueDeclarationId) -> bool;
    fn value_declaration_header(&self, declaration: NativeValueDeclarationId) -> NativeValueDeclarationHeader;
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
    fn decode(raw: u64) -> Self { if raw == 0 { Self::NoContract } else { Self::Type(TypeId((raw - 1) as u32)) } }
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

impl<'p, 's> Checker<'p, 's> {
    pub fn set_native_globals_provider(&mut self, provider: &'p dyn NativeGlobalsProvider) {
        assert!(self.native_globals_provider.is_none(), "native globals provider already installed");
        self.native_globals_provider = Some(provider);
    }

    fn report_native_global(&mut self, error: NativeGlobalQueryError) {
        if let Some(provider) = self.native_globals_provider { provider.global_query_error(self, error); }
    }

    fn native_globals_demand(
        &mut self,
        demand: NativeGlobalsQuery,
        compute: impl FnOnce(&mut Self, &'p dyn NativeGlobalsProvider) -> Result<NativeGlobalsValue, NativeGlobalQueryError>,
    ) -> Result<NativeGlobalsValue, NativeGlobalQueryError> {
        if let Some(result) = self.native_globals_cache.values.get(&demand) { return result.clone(); }
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
        let query = Query::NativeGlobals(demand);
        if let Some(raw) = self.provisional(query) { return Ok(NativeGlobalsValue::decode(raw)); }
        if !self.enter(query) {
            let error = if self.found_cycle { NativeGlobalQueryError::CircularQuery(demand) } else { NativeGlobalQueryError::TooDeep(demand) };
            self.report_native_global(error.clone());
            return Err(error);
        }
        let result = compute(self, provider);
        let left = self.leave(query);
        if self.left_a_cycle {
            let error = NativeGlobalQueryError::CircularQuery(demand);
            self.report_native_global(error.clone());
            self.native_globals_cache.store(demand, Err(error.clone()), self.cycle_result());
            return Err(error);
        }
        match left {
            Ok(stored) => self.native_globals_cache.store(demand, result.clone(), stored),
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
                Ok(provider.value_declaration_contract(checker, declaration)
                    .map_or(NativeGlobalsValue::NoContract, NativeGlobalsValue::Type))
            })
        }).map(|value| match value { NativeGlobalsValue::Type(ty) => Some(ty), NativeGlobalsValue::NoContract => None })
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
