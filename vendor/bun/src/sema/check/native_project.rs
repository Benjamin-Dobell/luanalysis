//! Original Lua chunks use their own guarded query namespace. A module is not a
//! synthetic function; its provider returns the checked whole source result pack.

use super::{Checker, Query};
use super::native_flow::NativeRuntimeType;
use super::native_packs::{NativePackAlternative, NativePackSlot};
use super::task::Stored;
use crate::types::{NativeResultPack, TypeId};
use crate::util::FxHashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeModuleId {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeModuleSourceRole {
    Implementation,
    Declaration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeModuleHeader {
    pub role: NativeModuleSourceRole,
}

/// A supported policy asserts the standard file loader without package.loaded,
/// searcher or preload replacement. Dynamic/custom effects are not approximated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeLoaderPolicy {
    Lua50StandardFile,
    Lua51StandardFile,
    Lua52StandardFile,
    Lua53StandardFile,
    Lua54StandardFile,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeProjectQuery {
    ChunkResult(NativeModuleId),
    ModuleValue { module: NativeModuleId, policy: NativeLoaderPolicy },
    RequirePack { module: NativeModuleId, policy: NativeLoaderPolicy },
}

impl crate::local::MaybeLocal for NativeProjectQuery {
    fn is_local(&self) -> bool { false }
}

impl crate::types::Follow for NativeProjectQuery {
    fn visit<V: crate::types::Visitor>(&self, visitor: &mut V) {
        visitor.plain(&std::mem::discriminant(self));
        match self {
            Self::ChunkResult(module) => { visitor.plain(&module.snapshot); visitor.plain(&module.index); }
            Self::ModuleValue { module, policy } | Self::RequirePack { module, policy } => {
                visitor.plain(&module.snapshot); visitor.plain(&module.index); visitor.plain(policy);
            }
        }
    }
    fn follow(&mut self, _: &crate::types::Link) {
        match self {
            Self::ChunkResult(NativeModuleId { snapshot: _, index: _ })
            | Self::ModuleValue { module: NativeModuleId { snapshot: _, index: _ }, policy: _ }
            | Self::RequirePack { module: NativeModuleId { snapshot: _, index: _ }, policy: _ } => {}
        }
    }
}

impl NativeProjectQuery {
    pub fn module(self) -> NativeModuleId {
        match self {
            Self::ChunkResult(module) | Self::ModuleValue { module, .. }
                | Self::RequirePack { module, .. } => module,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeProjectError {
    InvalidModule(NativeModuleId),
    MissingDeclarationContract(NativeModuleId),
    UnsupportedLoader { module: NativeModuleId, policy: NativeLoaderPolicy },
    CircularQuery(NativeProjectQuery),
    TooDeep(NativeProjectQuery),
    UnresolvedQuery(NativeProjectQuery),
}

pub trait NativeProjectProvider {
    fn contains_module(&self, module: NativeModuleId) -> bool;
    fn module_header(&self, module: NativeModuleId) -> NativeModuleHeader;
    fn module_inferred_result_pack(&self, checker: &mut Checker<'_, '_>, module: NativeModuleId) -> NativeResultPack;
    fn project_query_error(&self, _: NativeProjectError) {}
}

#[derive(Default)]
pub(super) struct NativeProjectCache {
    values: FxHashMap<NativeProjectQuery, Result<TypeId, NativeProjectError>>,
}

impl NativeProjectCache {
    fn store(&mut self, key: NativeProjectQuery, value: Result<TypeId, NativeProjectError>, _: Stored) {
        self.values.insert(key, value);
    }
}

impl<'p, 's> Checker<'p, 's> {
    pub fn set_native_project_provider(&mut self, provider: &'p dyn NativeProjectProvider) {
        assert!(self.native_project_provider.is_none(), "native project provider already installed");
        self.native_project_provider = Some(provider);
    }

    fn report_native_project(&self, error: NativeProjectError) {
        if let Some(provider) = self.native_project_provider {
            provider.project_query_error(error);
        }
    }

    fn native_project_demand(
        &mut self,
        demand: NativeProjectQuery,
        compute: impl FnOnce(&mut Self, &'p dyn NativeProjectProvider) -> Result<TypeId, NativeProjectError>,
    ) -> Result<TypeId, NativeProjectError> {
        let module = demand.module();
        let Some(provider) = self.native_project_provider.filter(|provider| provider.contains_module(module)) else {
            let error = NativeProjectError::InvalidModule(module);
            self.report_native_project(error);
            return Err(error);
        };
        if let Some(known) = self.p.native_project_results.get(&self.task, &demand) { return Ok(known) }
        if let Some(&known) = self.native_project_cache.values.get(&demand) { return known }
        let query = Query::NativeProject(demand);
        if let Some(raw) = self.provisional(query) { return Ok(TypeId(raw as u32)) }
        if !self.enter(query) {
            let error = if self.found_cycle { NativeProjectError::CircularQuery(demand) }
                else { NativeProjectError::TooDeep(demand) };
            self.report_native_project(error);
            return Err(error);
        }
        // A nested native worker can refuse before another project query is
        // entered. Preserve that demand-local limit cause when it taints an
        // otherwise successful fallback; unrelated Pending stays unresolved.
        let cuts = self.cuts();
        let result = compute(self, provider);
        let left = self.leave(query);
        if self.left_a_cycle {
            let error = NativeProjectError::CircularQuery(demand);
            self.report_native_project(error);
            self.native_project_cache.store(demand, Err(error), self.cycle_result());
            return Err(error);
        }
        match left {
            Ok(stored) => {
                if let Ok(ty) = result { self.p.native_project_results.insert(&self.task, demand, ty, stored); }
                self.native_project_cache.store(demand, result, stored);
            }
            Err(open) => {
                if let Ok(ty) = result {
                    self.cache_provisionally(query, u64::from(ty.0), open);
                    let error = if self.cuts() != cuts { NativeProjectError::TooDeep(demand) }
                        else { NativeProjectError::UnresolvedQuery(demand) };
                    self.report_native_project(error);
                    return Err(error);
                }
            }
        }
        if let Err(error) = result { self.report_native_project(error) }
        result
    }

    pub fn native_module_result_pack(&mut self, module: NativeModuleId) -> Result<NativeResultPack, NativeProjectError> {
        self.native_project_demand(NativeProjectQuery::ChunkResult(module), |checker, provider| {
            if provider.module_header(module).role == NativeModuleSourceRole::Declaration {
                return Err(NativeProjectError::MissingDeclarationContract(module));
            }
            // A chunk is a lexical root. Its exports do not inherit the
            // importing function's expected signature or capture environment.
            // The active native query stack remains intact across this scope.
            let contextual = std::mem::take(&mut checker.native_function_contexts);
            let captured = std::mem::take(&mut checker.native_capture_contexts);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(||
                provider.module_inferred_result_pack(checker, module).ty));
            checker.native_function_contexts = contextual;
            checker.native_capture_contexts = captured;
            Ok(result.unwrap_or_else(|error| std::panic::resume_unwind(error)))
        }).map(|ty| NativeResultPack { ty })
    }

    pub fn native_module_value(&mut self, module: NativeModuleId, policy: NativeLoaderPolicy) -> Result<TypeId, NativeProjectError> {
        self.native_project_demand(NativeProjectQuery::ModuleValue { module, policy }, |checker, _| {
            if policy == NativeLoaderPolicy::Unsupported {
                return Err(NativeProjectError::UnsupportedLoader { module, policy });
            }
            let pack = checker.native_module_result_pack(module)?;
            let first = checker.native_pack_slot(pack, 0);
            let non_nil = checker.native_runtime_type(first, &NativeRuntimeType::Nil, false);
            let nil = checker.native_runtime_type(first, &NativeRuntimeType::Nil, true);
            Ok(if nil.is_never() { non_nil } else { checker.union(&[non_nil, TypeId::TRUE]) })
        })
    }

    pub fn native_require_pack(&mut self, module: NativeModuleId, policy: NativeLoaderPolicy) -> Result<NativeResultPack, NativeProjectError> {
        self.native_project_demand(NativeProjectQuery::RequirePack { module, policy }, |checker, _| {
            let value = checker.native_module_value(module, policy)?;
            let mut rows = vec![NativePackAlternative { fixed: vec![NativePackSlot { ty: value, optional: false }], tail: None }];
            if policy == NativeLoaderPolicy::Lua54StandardFile {
                rows.push(NativePackAlternative { fixed: vec![
                    NativePackSlot { ty: value, optional: false },
                    NativePackSlot { ty: TypeId::STRING, optional: false },
                ], tail: None });
            }
            Ok(checker.native_result_pack(&rows).ty)
        }).map(|ty| NativeResultPack { ty })
    }
}
