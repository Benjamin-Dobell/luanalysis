//! Original selected standard-value capabilities. This records declaration
//! ownership only; no inferred types, member unions or result caches live here.
use super::NativeRegistry;
use crate::standard_environment::{StandardEnvironmentProfile, StandardLibrarySource};
use luanalysis_engine::{
    NativeGlobalBindingKey, NativeSignatureKey, NativeTypeProvider, NativeValueDeclarationKey,
};
use luanalysis_project::SourceRole;

#[derive(Clone, Copy)]
pub(crate) struct OriginalStandardFunction {
    pub binding: NativeGlobalBindingKey,
    pub declaration: NativeValueDeclarationKey,
    pub signature: NativeSignatureKey,
}

pub(crate) struct ProjectStandardEnvironment {
    pub assume_unknown_effects: bool,
    pub string_library: Option<NativeGlobalBindingKey>,
    pub type_operation: Option<OriginalStandardFunction>,
    pub assert_operations: Vec<OriginalStandardFunction>,
    pub require_operation: Option<OriginalStandardFunction>,
    pub setmetatable_operation: Option<OriginalStandardFunction>,
    pub rejected_assert_declarations: Vec<NativeValueDeclarationKey>,
}
impl ProjectStandardEnvironment {
    pub(super) fn new<'a>(
        profile: StandardEnvironmentProfile,
        files: impl Iterator<Item = (StandardLibrarySource, &'a NativeRegistry)>,
    ) -> Self {
        let mut result = Self {
            assume_unknown_effects: profile.is_selected_standard(),
            string_library: None,
            type_operation: None,
            assert_operations: Vec::new(),
            require_operation: None,
            setmetatable_operation: None,
            rejected_assert_declarations: Vec::new(),
        };
        if !profile.is_selected_standard() {
            return result;
        }
        for (kind, source) in files {
            if source.source_role() != SourceRole::Declaration {
                continue;
            }
            match profile {
                StandardEnvironmentProfile::LegacyMinimal if kind != StandardLibrarySource::LegacyRequire => continue,
                _ if profile.is_full_lua() && kind == StandardLibrarySource::LegacyRequire => continue,
                _ => {},
            }
            if let Some((capabilities, other_owners)) = source.original_standard_function("require") {
                if other_owners.is_empty() && capabilities.len() == 1 { result.require_operation = capabilities.first().copied(); }
            }
            if kind == StandardLibrarySource::String {
                let mut originals = source.globals.declarations.iter().filter(|declaration| {
                    declaration.namespace.is_none() && declaration.name.text == "string"
                });
                if originals.next().is_some() && originals.next().is_none() {
                    result.string_library = source.global_binding("string");
                }
            }
            if kind == StandardLibrarySource::Base {
                if let Some((capabilities, other_owners)) = source.original_standard_function("setmetatable") {
                    if other_owners.is_empty() && capabilities.len() == 1 {
                        result.setmetatable_operation = capabilities.first().copied();
                    }
                }
                if let Some((capabilities, other_owners)) = source.original_standard_function("type")
                {
                    if other_owners.is_empty() && capabilities.len() == 1 {
                        result.type_operation = capabilities.first().copied();
                    }
                }
                if let Some((capabilities, other_owners)) =
                    source.original_standard_function("assert")
                {
                    if other_owners.is_empty() {
                        result.assert_operations = capabilities;
                    }
                    result.rejected_assert_declarations.extend(other_owners);
                }
            }
        }
        result
    }
}

impl NativeRegistry {
    fn original_standard_function(
        &self,
        name: &str,
    ) -> Option<(Vec<OriginalStandardFunction>, Vec<NativeValueDeclarationKey>)> {
        let mut originals =
            self.globals.declarations.iter().filter(|declaration| {
                declaration.namespace.is_none() && declaration.name.text == name
            });
        let declaration = originals.next()?;
        if originals.next().is_some() {
            return None;
        }
        let function = declaration.function?;
        let binding = self.global_binding(name)?;
        let globals = self.project_globals.as_ref()?;
        let owners = globals.declarations(binding);
        let other_owners = owners
            .into_iter()
            .filter(|key| *key != declaration.key && globals.is_declaration_owner(*key))
            .collect();
        let mut heads = self.function_overload_signatures(function);
        if heads.is_empty() {
            heads.push(self.implementation_annotation_signature(function)?);
        }
        Some((
            heads.into_iter().map(|signature| OriginalStandardFunction {
                binding,
                declaration: declaration.key,
                signature,
            }).collect(),
            other_owners,
        ))
    }
    pub(crate) fn set_original_type_operation(&mut self, capability: OriginalStandardFunction) {
        debug_assert_eq!(
            capability.binding.environment,
            self.project_globals
                .as_ref()
                .expect("selected project environment")
                .environment
        );
        debug_assert_eq!(
            capability.declaration.snapshot,
            capability.signature.snapshot
        );
        self.legacy_builtin_policy = crate::builtins::LegacyBuiltinPolicy::Disabled;
        self.flow.set_runtime_type_intrinsic_enabled(true);
    }
    pub(crate) fn reject_untrusted_standard_assert(&self, key: NativeValueDeclarationKey) {
        if self.source_role() != SourceRole::Declaration { return; }
        if let Some(source) = self
            .globals
            .declarations
            .iter()
            .find(|source| source.key == key)
        {
            self.report.borrow_mut().diagnostics.push(crate::Diagnostic {
                code:"LUA2039",span:source.name.span,
                message:"This assert declaration cannot attest the selected original standard value; use an explicit custom environment for its contract.".into(),
            });
        }
    }
}
