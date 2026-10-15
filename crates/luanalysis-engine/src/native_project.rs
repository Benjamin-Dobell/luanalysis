//! Typed original chunk identities and guarded native module/require queries.
use super::{
    Checker, NativeTypeProvider, ProviderAdapter, SessionQueries, Type, ValuePack, export_type,
};
use bun_sema::check::native_project as core;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeModuleKey {
    pub snapshot: u64,
    pub index: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleSourceRole {
    Implementation,
    Declaration,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeModuleHeader {
    pub role: ModuleSourceRole,
}
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
    ChunkResult(NativeModuleKey),
    ModuleValue {
        module: NativeModuleKey,
        policy: NativeLoaderPolicy,
    },
    RequirePack {
        module: NativeModuleKey,
        policy: NativeLoaderPolicy,
    },
}
impl NativeProjectQuery {
    pub fn module(self) -> NativeModuleKey {
        match self {
            Self::ChunkResult(module)
            | Self::ModuleValue { module, .. }
            | Self::RequirePack { module, .. } => module,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeProjectError {
    InvalidModule(NativeModuleKey),
    MissingDeclarationContract(NativeModuleKey),
    UnsupportedLoader {
        module: NativeModuleKey,
        policy: NativeLoaderPolicy,
    },
    CircularQuery(NativeProjectQuery),
    TooDeep(NativeProjectQuery),
    UnresolvedQuery(NativeProjectQuery),
}
fn core_module(key: NativeModuleKey) -> core::NativeModuleId {
    core::NativeModuleId {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn source_module(key: core::NativeModuleId) -> NativeModuleKey {
    NativeModuleKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn core_policy(policy: NativeLoaderPolicy) -> core::NativeLoaderPolicy {
    match policy {
        NativeLoaderPolicy::Lua50StandardFile => core::NativeLoaderPolicy::Lua50StandardFile,
        NativeLoaderPolicy::Lua51StandardFile => core::NativeLoaderPolicy::Lua51StandardFile,
        NativeLoaderPolicy::Lua52StandardFile => core::NativeLoaderPolicy::Lua52StandardFile,
        NativeLoaderPolicy::Lua53StandardFile => core::NativeLoaderPolicy::Lua53StandardFile,
        NativeLoaderPolicy::Lua54StandardFile => core::NativeLoaderPolicy::Lua54StandardFile,
        NativeLoaderPolicy::Unsupported => core::NativeLoaderPolicy::Unsupported,
    }
}
fn source_policy(policy: core::NativeLoaderPolicy) -> NativeLoaderPolicy {
    match policy {
        core::NativeLoaderPolicy::Lua50StandardFile => NativeLoaderPolicy::Lua50StandardFile,
        core::NativeLoaderPolicy::Lua51StandardFile => NativeLoaderPolicy::Lua51StandardFile,
        core::NativeLoaderPolicy::Lua52StandardFile => NativeLoaderPolicy::Lua52StandardFile,
        core::NativeLoaderPolicy::Lua53StandardFile => NativeLoaderPolicy::Lua53StandardFile,
        core::NativeLoaderPolicy::Lua54StandardFile => NativeLoaderPolicy::Lua54StandardFile,
        core::NativeLoaderPolicy::Unsupported => NativeLoaderPolicy::Unsupported,
    }
}
fn source_query(query: core::NativeProjectQuery) -> NativeProjectQuery {
    match query {
        core::NativeProjectQuery::ChunkResult(module) => {
            NativeProjectQuery::ChunkResult(source_module(module))
        }
        core::NativeProjectQuery::ModuleValue { module, policy } => {
            NativeProjectQuery::ModuleValue {
                module: source_module(module),
                policy: source_policy(policy),
            }
        }
        core::NativeProjectQuery::RequirePack { module, policy } => {
            NativeProjectQuery::RequirePack {
                module: source_module(module),
                policy: source_policy(policy),
            }
        }
    }
}
fn source_error(error: core::NativeProjectError) -> NativeProjectError {
    match error {
        core::NativeProjectError::InvalidModule(module) => {
            NativeProjectError::InvalidModule(source_module(module))
        }
        core::NativeProjectError::MissingDeclarationContract(module) => {
            NativeProjectError::MissingDeclarationContract(source_module(module))
        }
        core::NativeProjectError::UnsupportedLoader { module, policy } => {
            NativeProjectError::UnsupportedLoader {
                module: source_module(module),
                policy: source_policy(policy),
            }
        }
        core::NativeProjectError::CircularQuery(query) => {
            NativeProjectError::CircularQuery(source_query(query))
        }
        core::NativeProjectError::TooDeep(query) => {
            NativeProjectError::TooDeep(source_query(query))
        }
        core::NativeProjectError::UnresolvedQuery(query) => {
            NativeProjectError::UnresolvedQuery(source_query(query))
        }
    }
}
impl<P: NativeTypeProvider> core::NativeProjectProvider for ProviderAdapter<'_, P> {
    fn contains_module(&self, key: core::NativeModuleId) -> bool {
        self.provider.contains_module(source_module(key))
    }
    fn module_header(&self, key: core::NativeModuleId) -> core::NativeModuleHeader {
        let header = self.provider.module_header(source_module(key));
        core::NativeModuleHeader {
            role: match header.role {
                ModuleSourceRole::Implementation => core::NativeModuleSourceRole::Implementation,
                ModuleSourceRole::Declaration => core::NativeModuleSourceRole::Declaration,
            },
        }
    }
    fn module_inferred_result_pack(
        &self,
        checker: &mut Checker<'_, '_>,
        key: core::NativeModuleId,
    ) -> bun_sema::types::NativeResultPack {
        let pack = self.provider.module_inferred_result_pack(
            source_module(key),
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        super::native_packs::import_pack(checker, self.handles, &pack)
    }
    fn project_query_error(&self, error: core::NativeProjectError) {
        self.provider.project_query_error(source_error(error));
    }
}
impl SessionQueries<'_, '_, '_> {
    pub(super) fn module_result_pack_native(
        &mut self,
        key: NativeModuleKey,
    ) -> Result<ValuePack, NativeProjectError> {
        let pack = self
            .checker
            .native_module_result_pack(core_module(key))
            .map_err(source_error)?;
        Ok(self.handles.retain_pack(self.checker, pack))
    }
    pub(super) fn module_value_native(
        &mut self,
        key: NativeModuleKey,
        policy: NativeLoaderPolicy,
    ) -> Result<Type, NativeProjectError> {
        let ty = self
            .checker
            .native_module_value(core_module(key), core_policy(policy))
            .map_err(source_error)?;
        Ok(export_type(self.checker, self.handles, ty))
    }
    pub(super) fn require_pack_native(
        &mut self,
        key: NativeModuleKey,
        policy: NativeLoaderPolicy,
    ) -> Result<ValuePack, NativeProjectError> {
        let pack = self
            .checker
            .native_require_pack(core_module(key), core_policy(policy))
            .map_err(source_error)?;
        Ok(self.handles.retain_pack(self.checker, pack))
    }
}
