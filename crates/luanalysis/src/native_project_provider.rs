//! Aggregate source recipe transport. Keys stay in each original file snapshot;
//! all type queries and recursion guards remain in the one native checker.
use super::modules::{ImportOutcome, ImportRecipe, ProjectModuleIndex, classify_unresolved};
use super::{NativeRegistry, NativeTypeProvider};
use crate::bind::BoundFile;
use crate::project_check::FileCheckReport;
use crate::{CheckReport, Diagnostic};
use crate::standard_environment::{StandardEnvironmentProfile, StandardLibrarySource};
use luanalysis_engine::{
    CapturedFunctionContext, ContextualFunctionContext, NamedTypeError, NamedTypeQuery,
    NativeBindingKey, NativeCheckMode, NativeExpressionKey, NativeFunctionKey, NativeOpaqueRuntimeKind, NativeSignatureKey, SignatureHeader,
    NativeTypeDeclarationKey, NativeTypeMemberKey, NativeTypeParameterKey, Parameter, Type,
    TypeDeclarationHeader, TypeDeclarationOwner, TypeMemberHeader, TypeMemberOwner, TypeQueries,
    ValuePack,
};
use luanalysis_engine::{
    ModuleSourceRole, NativeLoaderPolicy, NativeModuleHeader, NativeModuleKey, NativeProjectError,
};
use luanalysis_project::{Dependency, DependencyOutcome, FileId, SourceRole};
use luanalysis_syntax::File;
use std::cell::RefCell;
use crate::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

#[cfg(test)]
#[path = "native_flow_recipe_sharing.rs"]
mod flow_recipe_sharing_tests;

#[path = "native_project_flow.rs"]
mod flow_router;

#[derive(Clone)]
pub(crate) struct RegistryInput {
    pub id: FileId,
    pub role: SourceRole,
    pub standard_library: Option<StandardLibrarySource>,
    pub syntax: Arc<File>,
    pub bound: Arc<BoundFile>,
}

/// Original recipe identities survive task-local provider construction. They
/// contain no inferred types or contextual checker capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SourceRecipeIdentity {
    pub(super) source: u64,
    pub(super) flow: u64,
}

impl SourceRecipeIdentity {
    pub(crate) fn new() -> Self {
        let source = super::NEXT_SNAPSHOT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert_ne!(source, u64::MAX, "source snapshot identities exhausted");
        Self { source, flow: crate::flow::allocate_snapshot() }
    }
}

pub(crate) struct ProjectRegistrySnapshot {
    inputs: Vec<RegistryInput>,
    identities: Vec<SourceRecipeIdentity>,
    dependencies: Vec<Dependency>,
    policy: NativeLoaderPolicy,
    environment_profile: StandardEnvironmentProfile,
    overload_mode: crate::OverloadMode,
    no_implicit_any: bool,
    environment: luanalysis_engine::NativeEnvironmentKey,
    prepared_flows: PreparedSources,
}

/// The recipes built for each source of a project (`SourceRecipes`), and the global names their
/// flow recipes were seeded with. Recipes hold no type, so a later session over the same source
/// revisions and global names reuses them.
#[derive(Clone, Default)]
pub(crate) struct PreparedSources {
    /// Empty, or one per source: the recipes to use instead of building them.
    pub(crate) recipes: Vec<Option<super::SourceRecipes>>,
    pub(crate) names: Vec<Vec<u8>>,
}

impl ProjectRegistrySnapshot {
    #[cfg(test)]
    pub(crate) fn new(
        inputs: Vec<RegistryInput>,
        dependencies: Vec<Dependency>,
        policy: NativeLoaderPolicy,
        environment_profile: StandardEnvironmentProfile,
        overload_mode: crate::OverloadMode,
    ) -> Self {
        let identities = inputs.iter().map(|_| SourceRecipeIdentity::new()).collect();
        Self::with_identities(inputs, identities, dependencies, policy, environment_profile, overload_mode)
    }

    pub(crate) fn with_identities(
        inputs: Vec<RegistryInput>,
        identities: Vec<SourceRecipeIdentity>,
        dependencies: Vec<Dependency>,
        policy: NativeLoaderPolicy,
        environment_profile: StandardEnvironmentProfile,
        overload_mode: crate::OverloadMode,
    ) -> Self {
        assert_eq!(inputs.len(), identities.len(), "one identity per original source recipe");
        let environment = luanalysis_engine::NativeEnvironmentKey {
            snapshot: identities.first().map_or_else(|| SourceRecipeIdentity::new().source, |identity| identity.source),
            index: 0,
        };
        Self { inputs, identities, dependencies, policy, environment_profile, overload_mode, no_implicit_any: false, environment, prepared_flows: PreparedSources::default() }
    }

    pub(crate) fn with_no_implicit_any(mut self, enabled: bool) -> Self {
        self.no_implicit_any = enabled;
        self
    }

    /// Prepare source-only flow graphs, references and write indexes once. The
    /// ordinary constructor establishes canonical keys, globals and profile flags;
    /// no native query or function body is evaluated during this preparation.
    pub(crate) fn prepare_shared_flows(&mut self) {
        // Recipes from a previous session are reused unless they were seeded with other
        // global names, which only building a registry reveals.
        self.prepared_flows = self.worker_registry().source_recipes();
    }

    /// Recipes a previous session built for these sources, under the same identities.
    pub(crate) fn with_prepared_sources(mut self, prepared: PreparedSources) -> Self {
        assert!(prepared.recipes.is_empty() || prepared.recipes.len() == self.inputs.len(), "one prepared flow per original source");
        self.prepared_flows = prepared;
        self
    }

    pub(crate) fn prepared_sources(&self) -> &PreparedSources {
        &self.prepared_flows
    }

    /// Mutable scopes, report queues, body facets and session handles belong to
    /// this task. Parsing and lexical binding have already completed.
    pub(crate) fn worker_registry(&self) -> ProjectRegistry {
        ProjectRegistry::new_with_recipe_identities(
            self.inputs.clone(), &self.identities, self.dependencies.clone(), self.policy,
            self.environment_profile, self.overload_mode, self.no_implicit_any,
            self.environment, &self.prepared_flows,
        )
    }
}

#[derive(Default)]
pub(crate) struct ProjectNamedIndex {
    names: HashMap<String, Vec<NativeTypeDeclarationKey>>,
    /// Classes declared more than once are one class, as LuaLS reads repeated
    /// `---@class X` and TypeScript merges interface declarations: each later
    /// declaration is merged into the first.
    merged: HashMap<NativeTypeDeclarationKey, NativeTypeDeclarationKey>,
    merges: HashMap<NativeTypeDeclarationKey, Vec<NativeTypeDeclarationKey>>,
}
impl ProjectNamedIndex {
    pub(crate) fn candidates(&self, name: &str) -> &[NativeTypeDeclarationKey] {
        self.names.get(name).map_or(&[], Vec::as_slice)
    }
    /// The declaration that `key` is merged into, or `key` itself.
    pub(crate) fn canonical(&self, key: NativeTypeDeclarationKey) -> NativeTypeDeclarationKey {
        self.merged.get(&key).copied().unwrap_or(key)
    }
    /// The later declarations merged into `key`.
    pub(crate) fn merged_into(&self, key: NativeTypeDeclarationKey) -> &[NativeTypeDeclarationKey] {
        self.merges.get(&key).map_or(&[], Vec::as_slice)
    }
    pub(crate) fn is_merged(&self, key: NativeTypeDeclarationKey) -> bool {
        self.merged.contains_key(&key) || self.merges.contains_key(&key)
    }
    /// A class declared in a declaration file, such as the standard library's `string`, is the
    /// one its other declarations extend.
    fn merge_classes(&mut self, classes: &HashSet<NativeTypeDeclarationKey>, declared: &HashSet<NativeTypeDeclarationKey>) {
        for keys in self.names.values_mut() {
            if keys.len() < 2 || !keys.iter().all(|key| classes.contains(key)) { continue }
            let canonical = keys.iter().copied().find(|key| declared.contains(key)).unwrap_or(keys[0]);
            for &key in keys.iter().filter(|&&key| key != canonical) {
                self.merged.insert(key, canonical);
                self.merges.entry(canonical).or_default().push(key);
            }
            *keys = vec![canonical];
        }
    }
}

struct FileRegistry {
    id: FileId,
    role: SourceRole,
    standard_library: Option<StandardLibrarySource>,
    source: NativeRegistry,
}

pub(crate) struct ProjectRegistry {
    files: Vec<FileRegistry>,
    snapshots: HashMap<u64, usize>,
    flow_snapshots: HashMap<u64, usize>,
    names: Rc<ProjectNamedIndex>,
    globals: Rc<super::globals::ProjectGlobalIndex>,
    standard_environment: super::standard::ProjectStandardEnvironment,
    modules: Rc<ProjectModuleIndex>,
    header_metadata: RefCell<super::named_provider::SourceHeaderMetadata>,
    /// Whether reports describe binding and expression types. Diagnostics never
    /// depend on it; only callers that print or query types need them.
    describe_types: bool,
    /// The global names each source's flow recipes were seeded with.
    global_names: Vec<Vec<u8>>,
    /// Stops a check in progress: files not yet begun are skipped.
    cancellation: crate::project_check::Cancellation,
}

impl ProjectRegistry {
    /// Each source's flow recipes, for a later session over the same revisions.
    pub(crate) fn source_recipes(&self) -> PreparedSources {
        PreparedSources {
            recipes: self.files.iter().map(|file| Some(file.source.source_recipes())).collect(),
            names: self.global_names.clone(),
        }
    }

    pub(crate) fn set_describe_types(&mut self, describe: bool) { self.describe_types = describe; }
    pub(crate) fn set_inspections(&mut self, enabled: bool) {
        for file in &mut self.files { file.source.set_inspections(enabled); }
    }
    pub(crate) fn set_cancellation(&mut self, cancellation: &crate::project_check::Cancellation) {
        self.cancellation = cancellation.clone();
        for file in &mut self.files { file.source.set_cancellation(cancellation); }
    }
    pub(crate) fn set_standard_environment_assumptions(&mut self, enabled: bool) {
        self.standard_environment.assume_unknown_effects &= enabled;
        self.standard_environment.strict_effects = !enabled;
    }
    #[cfg(test)]
    pub(crate) fn new(inputs: Vec<RegistryInput>) -> Self {
        Self::new_with_modules(inputs, Vec::new(), NativeLoaderPolicy::Lua54StandardFile)
    }

    #[cfg(test)]
    pub(crate) fn new_with_modules(
        inputs: Vec<RegistryInput>,
        dependencies: Vec<Dependency>,
        policy: NativeLoaderPolicy,
    ) -> Self {
        Self::new_with_environment(inputs, dependencies, policy, StandardEnvironmentProfile::LegacyMinimal)
    }

    #[cfg(test)]
    pub(crate) fn new_with_environment(
        inputs: Vec<RegistryInput>,
        dependencies: Vec<Dependency>,
        policy: NativeLoaderPolicy,
        environment_profile: StandardEnvironmentProfile,
    ) -> Self {
        Self::new_with_environment_and_overloads(inputs, dependencies, policy, environment_profile, crate::OverloadMode::default())
    }

    #[cfg(test)]
    pub(crate) fn new_with_environment_and_overloads(
        inputs: Vec<RegistryInput>,
        dependencies: Vec<Dependency>,
        policy: NativeLoaderPolicy,
        environment_profile: StandardEnvironmentProfile,
        overload_mode: crate::OverloadMode,
    ) -> Self {
        ProjectRegistrySnapshot::new(inputs, dependencies, policy, environment_profile, overload_mode)
            .worker_registry()
    }

    fn new_with_recipe_identities(
        inputs: Vec<RegistryInput>,
        identities: &[SourceRecipeIdentity],
        dependencies: Vec<Dependency>,
        policy: NativeLoaderPolicy,
        environment_profile: StandardEnvironmentProfile,
        overload_mode: crate::OverloadMode,
        no_implicit_any: bool,
        environment: luanalysis_engine::NativeEnvironmentKey,
        prepared_flows: &PreparedSources,
    ) -> Self {
        assert!(prepared_flows.recipes.is_empty() || prepared_flows.recipes.len() == inputs.len(), "one prepared flow per original source");
        let prepared = |index: usize| prepared_flows.recipes.get(index).cloned().flatten();
        // Recipes seeded with other global names are rebuilt; keep the inputs to do so.
        let rebuild = prepared_flows.recipes.iter().any(Option::is_some).then(|| inputs.clone());
        let mut snapshots = HashMap::default();
        let mut flow_snapshots = HashMap::default();
        let mut names = ProjectNamedIndex::default();
        let mut classes = HashSet::default();
        let mut declared = HashSet::default();
        let mut files: Vec<_> = inputs
            .into_iter()
            .enumerate()
            .map(|(index, input)| {
                let mut source = NativeRegistry::new_with_identity_and_flow(input.syntax, input.bound, identities[index], prepared(index));
                source.set_overload_mode(overload_mode);
                source.set_no_implicit_any(no_implicit_any);
                source.set_source_role(input.role);
                assert!(snapshots.insert(source.snapshot, index).is_none());
                assert!(flow_snapshots.insert(source.iterator_sources().program.snapshot, index).is_none());
                for declaration in source.named.declarations() {
                    let key = NativeTypeDeclarationKey { snapshot: declaration.key.snapshot, index: declaration.key.index };
                    names.names.entry(declaration.syntax.name.text.clone()).or_default().push(key);
                    if matches!(declaration.syntax.kind, luanalysis_syntax::NamedTypeDeclarationKind::Record { flavor: luanalysis_syntax::NamedRecordKind::Class, .. }) {
                        classes.insert(key);
                        if input.role == SourceRole::Declaration { declared.insert(key); }
                    }
                }
                FileRegistry {
                    id: input.id,
                    role: input.role,
                    standard_library: input.standard_library,
                    source,
                }
            })
            .collect();
        names.merge_classes(&classes, &declared);
        let names = Rc::new(names);
        // Every source sees every other's scopes, merged once for all of them.
        let original_scopes: Vec<_> = files.iter().map(|file| file.source.types.original_scopes()).collect();
        let shared = Rc::new(crate::native_types::OriginalTypeScopes::merge(&original_scopes));
        for file in &mut files { file.source.types.share_original_scopes(Rc::clone(&shared)); }
        for file in &mut files {
            file.source.set_project_names(Rc::clone(&names));
            file.source.attach_documentation_signature_owners();
            file.source.rebuild_documentation_members();
        }
        let globals = Rc::new(super::globals::ProjectGlobalIndex::new(environment, &files.iter().map(|file| &file.source).collect::<Vec<_>>()));
        let original_names: Vec<_> = globals.bindings().into_iter().map(|(name, _)| name).collect();
        if let Some(inputs) = rebuild
            && prepared_flows.names != original_names
        {
            return Self::new_with_recipe_identities(
                inputs, identities, dependencies, policy, environment_profile, overload_mode,
                no_implicit_any, environment, &PreparedSources::default(),
            );
        }
        for (index, file) in files.iter_mut().enumerate() {
            if prepared(index).is_none() { file.source.seed_original_global_reference_keys(&original_names); }
        }
        let modules_by_file: HashMap<_, _> = files
            .iter()
            .map(|file| (file.id, file.source.module_key()))
            .collect();
        let mut imports = HashMap::default();
        for file in &files {
            if file.role == SourceRole::Declaration {
                continue;
            }
            for call in file.source.module_calls() {
                let dependency = dependencies.iter().find(|edge| {
                    edge.from == file.id
                        && edge.request.span.start == call.argument_span.start
                        && edge.request.span.end == call.argument_span.end
                });
                let outcome = if let Some(outcome) = classify_unresolved(&call) {
                    outcome
                } else {
                    match dependency.map(|edge|&edge.outcome) {
                        Some(DependencyOutcome::Resolved(target))=>modules_by_file.get(target).copied().map_or(ImportOutcome::InvalidSource,ImportOutcome::Module),
                        Some(DependencyOutcome::Unresolved(error))=>ImportOutcome::Missing(error.to_string()),
                        Some(DependencyOutcome::Unreadable(error))=>ImportOutcome::Missing(error.to_string()),
                        Some(DependencyOutcome::InvalidSpan)=>ImportOutcome::Missing("Module request has an invalid original source span.".into()),
                        None=>ImportOutcome::Missing("Static require request was not resolved by this project's source host.".into()),
                    }
                };
                // Source-host requests are candidates. Publish a missing module
                // only after the original callee retains require provenance.
                imports.insert(
                    (file.source.snapshot, call.span.start, call.span.end),
                    ImportRecipe {
                        argument_span: call.argument_span,
                        target: dependency.and_then(|edge| match edge.outcome {
                            DependencyOutcome::Resolved(target) => Some(target),
                            _ => None,
                        }),
                        outcome,
                    },
                );
            }
        }
        let modules = Rc::new(ProjectModuleIndex {
            policy,
            imports,
            required_files: Default::default(),
        });
        // Keep the historical type fallback separate from selected standard
        // capabilities. The minimal project loader has its own declaration;
        // that source does not grant type or other standard operations.
        let legacy_type_replaced = environment_profile == StandardEnvironmentProfile::LegacyMinimal && files.iter().any(|file| file.role == SourceRole::Implementation && file.source.bound.has_modified_type_intrinsic());
        let legacy_builtins = if environment_profile != StandardEnvironmentProfile::LegacyMinimal || legacy_type_replaced {
            crate::builtins::LegacyBuiltinPolicy::Disabled
        } else {
            crate::builtins::LegacyBuiltinPolicy::MinimalTypeCompatibility
        };
        // Every original registry receives the shared selected namespace before
        // immutable capability ownership is inspected.
        for file in &mut files { file.source.project_globals = Some(Rc::clone(&globals)); }
        let standard_environment = super::standard::ProjectStandardEnvironment::new(
            environment_profile,
            files.iter().filter_map(|file| file.standard_library.map(|kind| (kind, &file.source))),
        );
        for file in &mut files {
            file.source.flow.set_original_require_capability(standard_environment.require_operation.is_some());
            file.source.flow.set_require_needs_declaration(environment_profile.is_full_lua());
            for kind in [NativeOpaqueRuntimeKind::Userdata, NativeOpaqueRuntimeKind::Thread] {
                file.source.flow.set_opaque_runtime_type_enabled(
                    kind, standard_environment.runtime_type_declaration(kind).is_some(),
                );
            }
            file.source.set_project_names(Rc::clone(&names));
            file.source.project_modules = Some(Rc::clone(&modules));
            file.source.project_globals = Some(Rc::clone(&globals));
            if let Some(capability) = standard_environment.type_operation {
                file.source.set_original_type_operation(capability);
            } else {
                file.source.set_legacy_builtin_policy(legacy_builtins);
            }
            if environment_profile.is_full_lua() {
                for &declaration in &standard_environment.rejected_assert_declarations {
                    if declaration.snapshot == file.source.snapshot {
                        file.source.reject_untrusted_standard_assert(declaration);
                    }
                }
            }
            for declaration in file.source.named.declarations() {
                if names.candidates(&declaration.syntax.name.text).len() > 1 {
                    file.source
                        .report
                        .borrow_mut()
                        .diagnostics
                        .push(Diagnostic {
                            code: "TS2300",
                            span: declaration.syntax.name.span,
                            message: format!(
                                "Duplicate identifier '{}'.",
                                declaration.syntax.name.text
                            ),
                        });
                }
            }
        }
        Self {
            files,
            snapshots,
            flow_snapshots,
            names,
            globals,
            standard_environment,
            modules,
            header_metadata: RefCell::new(super::named_provider::SourceHeaderMetadata::default()),
            describe_types: true,
            global_names: original_names,
            cancellation: crate::project_check::Cancellation::default(),
        }
    }

    fn header_metadata_revisions(&self) -> [u64; 1] {
        // Every imported original scope belongs to one of the fixed source
        // owners. Their shared scope epochs and local documentation preparation
        // generations all advance the thread's metadata clock, which therefore
        // covers the complete merged metadata view.
        [crate::native_types::metadata_clock()]
    }

    fn declaration_header_metadata(&self, key: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        let owner = self.owner(key.snapshot);
        let named = crate::native_named::NativeNamedKey { snapshot: key.snapshot, index: key.index };
        if owner.named_metadata().declaration(named).is_none() { return owner.type_declaration_header(key); }
        let revisions = self.header_metadata_revisions();
        if let Some(header) = self.header_metadata.borrow_mut().get(&revisions, key) { return header; }
        let header = self.declaration_header_metadata_uncached(key);
        assert_eq!(self.header_metadata_revisions(), revisions,
            "merged header discovery only reads original project metadata");
        self.header_metadata.borrow_mut().insert(key, header.clone());
        header
    }

    fn declaration_header_metadata_uncached(&self, key: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        let owner=self.owner(key.snapshot);
        let mut header=owner.type_declaration_header(key);
        let named=crate::native_named::NativeNamedKey {snapshot:key.snapshot,index:key.index};
        if owner.named_metadata().declaration(named).is_none() {return header;}
        let mut used=crate::collections::HashSet::default();
        for file in &self.files {
            file.source.doc_members.dependencies(named,&mut used,|id|file.source.doc_signature_key(id),|signature|file.source.signature_outer_keys(signature));
        }
        header.outer_parameters.extend(owner.types.outer_parameters_in_context(crate::native_named::NativeTypeContext::NamedType(named),&used));
        header.outer_parameters.sort_by_key(|key|(key.snapshot,key.index));
        header.outer_parameters.dedup();
        header
    }

    fn owner_if(&self, snapshot: u64) -> Option<&NativeRegistry> {
        self.snapshots
            .get(&snapshot)
            .map(|&index| &self.files[index].source)
    }

    fn owner(&self, snapshot: u64) -> &NativeRegistry {
        self.owner_if(snapshot)
            .expect("native source key belongs to this immutable project provider")
    }

    /// Ready documentation members (`---@function Owner.member`) for queries made outside a check,
    /// as a language service's are: checking a file's declarations prepares its own.
    pub(crate) fn prepare_documentation_members(&self, queries: &mut dyn TypeQueries) {
        for file in &self.files { file.source.prepare_documentation_scopes(queries); }
    }
    /// Where a native function's syntax is, whose documentation describes it.
    pub(crate) fn function_site(&self, key: luanalysis_engine::NativeFunctionKey) -> Option<(FileId, luanalysis_syntax::Span)> {
        let file = &self.files[*self.snapshots.get(&key.snapshot)?];
        Some((file.id, file.source.function(key).span))
    }
    /// Where a signature (`---@function`, `---@overload`) is written.
    pub(crate) fn signature_site(&self, key: luanalysis_engine::NativeSignatureKey) -> Option<(FileId, luanalysis_syntax::Span)> {
        let file = &self.files[*self.snapshots.get(&key.snapshot)?];
        Some((file.id, file.source.signatures.get(key).span))
    }
    /// Where a named type's name is declared: in its own declaration and those merged into it.
    pub(crate) fn type_declaration_sites(&self, key: NativeTypeDeclarationKey) -> Vec<(FileId, luanalysis_syntax::Span)> {
        std::iter::once(key).chain(self.names.merged_into(key).iter().copied()).filter_map(|key| {
            let file = &self.files[*self.snapshots.get(&key.snapshot)?];
            let source = file.source.named.declaration(crate::native_named::NativeNamedKey { snapshot: key.snapshot, index: key.index })?;
            Some((file.id, source.syntax.name.span))
        }).collect()
    }
    /// The globals a declaration library declares as values of a written type (`---@type T`
    /// above `name = nil` or `_G.name = nil`), which a script may implement: each one's file,
    /// name, key and declaring statement. Function declarations are the library's own.
    pub(crate) fn ambient_values(&self) -> Vec<(FileId, String, luanalysis_engine::NativeValueDeclarationKey, luanalysis_syntax::Span)> {
        self.files.iter()
            .filter(|file| file.source.source_role() == SourceRole::Declaration)
            .flat_map(|file| file.source.globals.declarations.iter()
                .filter(|source| source.annotation.is_some() && source.function.is_none() && source.documentation_function.is_none() && source.namespace.is_none())
                .map(|source| (file.id, source.name.text.clone(), source.key, source.statement)))
            .collect()
    }
    /// Where the global `name` is declared: each declaration's file, its statement or its
    /// `---@function` tag, and whether that is a tag.
    pub(crate) fn global_sites(&self, name: &str) -> Vec<(FileId, luanalysis_syntax::Span, bool)> {
        self.files.iter().flat_map(|file| file.source.globals.declarations.iter()
            .filter(|source| source.namespace.is_none() && source.name.text == name)
            .map(|source| match source.documentation_function
                .and_then(|id| file.source.syntax().function_declarations.iter().find(|declaration| declaration.id == id))
            {
                Some(declaration) => (file.id, declaration.span, true),
                None => (file.id, source.statement, false),
            }))
            .collect()
    }
    /// Where the constructor of runtime table `key` declares its field `name`.
    pub(crate) fn runtime_table_field_site(&self, key: NativeTypeDeclarationKey, name: &str) -> Option<(FileId, luanalysis_syntax::Span, bool)> {
        let file = &self.files[*self.snapshots.get(&key.snapshot)?];
        Some((file.id, file.source.runtime_table_field_site(key, name)?, false))
    }
    /// The project's globals by name, with the required parameters of each one a function
    /// declaration declares (and whether it takes more), from declarations alone: unlike the
    /// environment's members, no global's value is inferred.
    pub(crate) fn global_completions(&self, queries: &mut dyn TypeQueries) -> Vec<(String, Option<(Vec<String>, bool)>)> {
        let mut globals = Vec::new();
        for (name, binding) in self.globals.bindings() {
            let Ok(name) = String::from_utf8(name) else { continue };
            let parameters = self.globals.declarations(binding).into_iter().find_map(|key| {
                let file = &self.files[*self.snapshots.get(&key.snapshot)?];
                file.source.global_function_parameters(key, queries)
            });
            globals.push((name, parameters));
        }
        globals
    }
    /// Where a member is declared: its file, its span, and whether that is a documentation tag.
    pub(crate) fn member_site(&self, key: NativeTypeMemberKey) -> Option<(FileId, luanalysis_syntax::Span, bool)> {
        let file = &self.files[*self.snapshots.get(&key.snapshot)?];
        let (span, tag) = file.source.member_site(key)?;
        Some((file.id, span, tag))
    }
    /// The checker registry of one source, for the language service.
    pub(crate) fn source(&self, file: FileId) -> Option<&NativeRegistry> {
        self.files.iter().find(|registry| registry.id == file).map(|registry| &registry.source)
    }

    #[cfg(test)]
    pub(crate) fn required_files(&self) -> HashSet<FileId> {
        self.modules.required_files.borrow().clone()
    }

    pub(crate) fn check_roots(&self, queries: &mut dyn TypeQueries, roots: &[FileId]) -> Vec<FileCheckReport> {
        self.check_roots_mode(queries, roots, false, None)
    }

    /// A planned task validates its own roots. Sibling recipes remain available
    /// to lazy queries, and their emitted diagnostics stay with their source.
    pub(crate) fn check_owned_roots(&self, queries: &mut dyn TypeQueries, roots: &[FileId]) -> Vec<FileCheckReport> {
        self.check_roots_mode(queries, roots, true, None)
    }

    /// Explicit scheduled sources have their own tasks. This task follows only
    /// proven dependencies outside that plan, including transitive discoveries.
    #[cfg(test)]
    pub(crate) fn check_scheduled_roots(
        &self,
        queries: &mut dyn TypeQueries,
        roots: &[FileId],
        scheduled: &HashSet<FileId>,
    ) -> Vec<FileCheckReport> {
        self.check_roots_mode(queries, roots, true, Some(scheduled))
    }

    fn check_roots_mode(&self, queries: &mut dyn TypeQueries, roots: &[FileId], owned_only: bool, scheduled: Option<&HashSet<FileId>>) -> Vec<FileCheckReport> {
        // Canonical scope identities must exist for every sibling extension
        // before any alias head can publish a class's captured outer mapper.
        for file in &self.files { file.source.prepare_documentation_scope_keys(queries); }
        for file in &self.files { file.source.validate_documentation_scope_owners(queries); }
        // Setup visits original declaration candidates even when they are never
        // selected. Keep that baseline separate from later demand diagnostics.
        let mut baselines: Vec<_> = self.files.iter().map(|file| {
            if owned_only { file.source.take_report() } else { CheckReport::default() }
        }).collect();
        // Candidate registration establishes every identity/name, but only
        // explicit roots and proven original require targets schedule checks.
        self.modules.required_files.borrow_mut().extend(roots.iter().copied());
        let mut checked = HashSet::default();
        let mut emitted = HashSet::default();
        let owned: HashSet<_> = roots.iter().copied().collect();
        let selected = |file: FileId| {
            owned.contains(&file) || self.modules.required_files.borrow().contains(&file)
                && (!owned_only || scheduled.is_some_and(|scheduled| !scheduled.contains(&file)))
        };
        let mut reports: Vec<_> = self.files.iter().map(|file| FileCheckReport {
                file: file.id,
                report: CheckReport::default(),
            }).collect();
        // Rendering can complete a deferred signature in any owner file. Drain
        // those source report queues before the arena ends; this schedules
        // report publication only and never iterates or joins inferred types.
        let mut unresolved_reported = false;
        let profile = std::env::var_os("LUANALYSIS_PROFILE").is_some();
        loop {
            for (index, file) in self.files.iter().enumerate() {
                if !selected(file.id) || self.cancellation.is_cancelled() || !checked.insert(index) {
                    continue;
                }
                let started = profile.then(std::time::Instant::now);
                unresolved_reported = false;
                file.source.check_named_declarations(queries);
                file.source.check_global_declarations(queries);
                file.source.check_module_requests(queries);
                file.source.report_unused_declarations();
                if file.role == SourceRole::Implementation {
                    let mut checker = file.source.checker(queries);
                    checker.flow_at = Some(file.source.flow_entry(None, None));
                    checker.statements(file.source.statements());
                    reports[index].report.append(checker.report);
                    file.source.check_all_bodies(queries);
                }
                if let Some(started) = started {
                    eprintln!("luanalysis profile: file_id={} checked={:.6}s", file.id.index, started.elapsed().as_secs_f64());
                }
            }
            let mut published = false;
            for (index, file) in self.files.iter().enumerate() {
                // A task checks the bodies of its own sources only, as a Bun task checks its files:
                // another source's bodies are checked once, by the task that owns it.
                let started = profile.then(std::time::Instant::now);
                if owned_only && file.role == SourceRole::Implementation && selected(file.id) { file.source.check_selected_bodies(queries); }
                if owned_only || checked.contains(&index) { published |= file.source.retry_contextual_owner_validations(queries); }
                if let Some(started) = started.filter(|started| started.elapsed().as_secs_f64() >= 0.05) {
                    eprintln!("luanalysis profile: file_id={} bodies_and_retries={:.6}s", file.id.index, started.elapsed().as_secs_f64());
                }
            }
            for (index, (file, report)) in self.files.iter().zip(&mut reports).enumerate() {
                if !owned_only && !checked.contains(&index) { continue; }
                file.source.check_deferred_constraints(queries);
                let mut pending = file.source.take_report();
                if !self.describe_types { pending.discard_types(); }
                // The drain loop runs to a fixed point: only new information
                // keeps it alive. A body checked again on a later pass can
                // report the same diagnostic again; one already in this file's
                // report (same code and range) is not new. Pending renders
                // always accompany an entry in `diagnostics`.
                let known: HashSet<_> = report.report.diagnostics.iter()
                    .map(|diagnostic| (diagnostic.code, diagnostic.span.start, diagnostic.span.end))
                    .collect();
                let has_pending = pending.diagnostics.iter()
                        .any(|diagnostic| !known.contains(&(diagnostic.code, diagnostic.span.start, diagnostic.span.end)))
                    || !pending.bindings.is_empty()
                    || !pending.types.is_empty()
                    || !pending.pending_bindings.is_empty()
                    || !pending.pending_types.is_empty();
                published |= has_pending;
                if owned_only && (has_pending || checked.contains(&index)
                    || self.modules.required_files.borrow().contains(&file.id))
                    && emitted.insert(index)
                {
                    report.report.append(std::mem::take(&mut baselines[index]));
                    if !self.describe_types { report.report.discard_types(); }
                }
                if owned_only && !emitted.contains(&index) { continue; }
                report.report.append(pending);
                published |= !report.report.pending_bindings.is_empty()
                    || !report.report.pending_types.is_empty();
                report.report.render(queries);
            }
            // A cancelled check has nothing left to check: it only settles what it began.
            let unchecked = !self.cancellation.is_cancelled() && self.files.iter().enumerate().any(|(index, file)| {
                !checked.contains(&index) && selected(file.id)
            });
            if !published && !unchecked {
                if owned_only && !unresolved_reported {
                    for (index, file) in self.files.iter().enumerate() {
                        if checked.contains(&index) { file.source.report_unresolved_contextual_owners(); }
                    }
                    unresolved_reported = true;
                    continue;
                }
                break;
            }
        }
        for (index, (file, report)) in self.files.iter().zip(&mut reports).enumerate() {
            if !checked.contains(&index) && !(owned_only && emitted.contains(&index)) { continue; }
            if !owned_only {
                file.source.report_unresolved_contextual_owners();
                file.source.check_deferred_constraints(queries);
                let mut pending = file.source.take_report();
                // As in the drain loop: types are described only when printed.
                if self.describe_types {
                    pending.render(queries);
                } else {
                    pending.discard_types();
                }
                report.report.append(pending);
                report.report.render_diagnostics(queries);
            }
            report.report.diagnostics.sort_by(|left, right| {
                (left.span.start, left.span.end, left.code, &left.message).cmp(&(
                    right.span.start,
                    right.span.end,
                    right.code,
                    &right.message,
                ))
            });
            report.report.diagnostics.dedup();
            report
                .report
                .bindings
                .sort_by_key(|binding| binding.span.start);
            report.report.bindings.dedup();
            report
                .report
                .types
                .sort_by_key(|ty| (ty.span.start, ty.span.end));
            report.report.types.dedup();
            report.report.imports.sort_by_key(|span| (span.start, span.end));
            report.report.imports.dedup();
            report.report.bound = Some(file.source.bound_arc());
        }
        reports = reports.into_iter().enumerate().filter_map(|(index, report)| {
            (self.modules.required_files.borrow().contains(&report.file) || owned_only && emitted.contains(&index))
                .then_some(report)
        }).collect();
        assert!(reports.iter().all(|report| report.report.pending_bindings.is_empty()
            && report.report.pending_types.is_empty() && report.report.pending_diagnostics.is_empty()),
            "project task reports must be rendered before native capabilities expire");
        reports
    }
}

// Keeping every hook explicit preserves the file provider's contextual,
// captured, declaration-space and whole-pack behavior. Defaults must not
// silently turn a physical pack demand into a scalar one.
macro_rules! dispatch {
    ($(fn $method:ident($key:ident: $key_ty:ty $(, $arg:ident: $arg_ty:ty)*) -> $result:ty;)*) => {
        $(fn $method(&self, $key: $key_ty $(, $arg: $arg_ty)*) -> $result {
            self.owner($key.snapshot).$method($key $(, $arg)*)
        })*
    };
}

impl NativeTypeProvider for ProjectRegistry {
    fn runtime_type_declaration(&self, kind: NativeOpaqueRuntimeKind) -> Option<NativeTypeDeclarationKey> {
        self.standard_environment.runtime_type_declaration(kind)
    }
    fn assumed_standard_environment(&self) -> Option<luanalysis_engine::NativeEnvironmentKey> {
        self.standard_environment.assume_unknown_effects.then_some(self.globals.environment)
    }
    fn strict_runtime_effects(&self) -> bool { self.standard_environment.strict_effects }
    fn project_flow_provider(&self) -> Option<&dyn luanalysis_engine::NativeFlowProvider> { Some(self) }
    fn global_environment_binding(&self, environment: luanalysis_engine::NativeEnvironmentKey, name: &[u8]) -> Option<luanalysis_engine::NativeGlobalBindingKey> {
        if environment != self.globals.environment { return None; }
        self.globals.binding(std::str::from_utf8(name).ok()?)
    }
    fn global_environment_bindings(&self, environment: luanalysis_engine::NativeEnvironmentKey) -> Vec<(Vec<u8>, luanalysis_engine::NativeGlobalBindingKey)> {
        if environment != self.globals.environment { return Vec::new(); }
        self.globals.bindings()
    }
    fn global_environment_self_binding(&self, environment: luanalysis_engine::NativeEnvironmentKey) -> Option<luanalysis_engine::NativeGlobalBindingKey> {
        if environment != self.globals.environment { return None; }
        let binding = self.globals.binding("_G")?;
        // Only selected bundled declaration ownership attests the initial
        // runtime self-reference. A user-written `_G` contract is ordinary.
        let owners = self.globals.declarations(binding);
        let mut declarations = owners.iter().filter(|&&owner| self.globals.is_declaration_owner(owner));
        let first = declarations.next()?;
        std::iter::once(first).chain(declarations).all(|owner| {
            let file = &self.files[self.snapshots[&owner.snapshot]];
            file.role == SourceRole::Declaration && file.standard_library.is_some()
        }).then_some(binding)
    }
    fn signature_intrinsic(&self, signature: luanalysis_engine::NativeSignatureKey) -> Option<luanalysis_engine::NativeIntrinsicSite> {
        if let Some(original) = self.standard_environment.assert_operations.iter().find(|original| original.signature == signature) {
            return Some(luanalysis_engine::NativeIntrinsicSite { binding: original.binding, declaration: original.declaration, kind: luanalysis_engine::NativeIntrinsicKind::Assert });
        }
        for (original, kind) in [
            (self.standard_environment.type_operation, luanalysis_engine::NativeIntrinsicKind::Type),
            (self.standard_environment.require_operation, luanalysis_engine::NativeIntrinsicKind::Require),
            (self.standard_environment.setmetatable_operation, luanalysis_engine::NativeIntrinsicKind::SetMetatable),
        ] {
            if let Some(original) = original && original.signature == signature {
                return Some(luanalysis_engine::NativeIntrinsicSite { binding: original.binding, declaration: original.declaration, kind });
            }
        }
        None
    }
    // The selected initial namespace is independent of current lexical names.
    fn primitive_member_environment(&self) -> Option<luanalysis_engine::NativeEnvironmentKey> {
        self.standard_environment.string_library.map(|binding| binding.environment)
    }
    fn primitive_member_binding(&self, environment: luanalysis_engine::NativeEnvironmentKey, _: luanalysis_engine::NativePrimitiveMemberKind) -> Option<luanalysis_engine::NativeGlobalBindingKey> {
        self.standard_environment.string_library.filter(|binding| binding.environment == environment)
    }
    fn contains_global_environment(&self, key: luanalysis_engine::NativeEnvironmentKey) -> bool {
        key == self.globals.environment
    }
    fn contains_global_binding(&self, key: luanalysis_engine::NativeGlobalBindingKey) -> bool {
        self.globals.contains(key)
    }
    fn global_binding_declarations(&self, key: luanalysis_engine::NativeGlobalBindingKey) -> Vec<luanalysis_engine::NativeValueDeclarationKey> {
        self.globals.declarations(key)
    }
    fn contains_value_declaration(&self, key: luanalysis_engine::NativeValueDeclarationKey) -> bool {
        self.owner_if(key.snapshot).is_some_and(|owner| owner.contains_global_declaration(key))
    }
    fn value_declaration_signatures(&self,key:luanalysis_engine::NativeValueDeclarationKey)->Option<Vec<NativeSignatureKey>> { self.owner(key.snapshot).value_declaration_signatures(key) }
    fn value_declaration_function_signatures(&self,key:luanalysis_engine::NativeValueDeclarationKey)->Option<Vec<NativeSignatureKey>> { self.owner(key.snapshot).value_declaration_function_signatures(key) }
    fn value_declaration_header(&self, key: luanalysis_engine::NativeValueDeclarationKey) -> luanalysis_engine::ValueDeclarationHeader {
        self.owner(key.snapshot).global_header(key)
    }
    fn value_declaration_contract(&self, key: luanalysis_engine::NativeValueDeclarationKey, queries: &mut dyn TypeQueries) -> Option<Type> {
        self.owner(key.snapshot).global_contract(key, queries)
    }
    fn value_declaration_initializer(&self, key: luanalysis_engine::NativeValueDeclarationKey, queries: &mut dyn TypeQueries) -> Type {
        self.owner(key.snapshot).global_initializer(key, queries)
    }
    fn global_query_error(&self, error: luanalysis_engine::GlobalQueryError, _: &mut dyn TypeQueries) {
        let mut declarations = super::globals::query_declarations(&error);
        if let luanalysis_engine::GlobalQueryError::CircularQuery(luanalysis_engine::GlobalQuery::GlobalBinding(binding))
        | luanalysis_engine::GlobalQueryError::TooDeep(luanalysis_engine::GlobalQuery::GlobalBinding(binding))
        | luanalysis_engine::GlobalQueryError::UnresolvedQuery(luanalysis_engine::GlobalQuery::GlobalBinding(binding))
        | luanalysis_engine::GlobalQueryError::InvalidPrimitiveMemberNamespace { binding, .. } = &error {
            declarations.extend(self.globals.declarations(*binding));
        }
        for key in declarations {
            if let Some(owner) = self.owner_if(key.snapshot) { owner.global_error(key, &error); }
        }
    }
    fn contains_module(&self, key: NativeModuleKey) -> bool {
        key.index == 0 && self.owner_if(key.snapshot).is_some()
    }
    fn module_header(&self, key: NativeModuleKey) -> NativeModuleHeader {
        let file = &self.files[*self
            .snapshots
            .get(&key.snapshot)
            .expect("original module snapshot")];
        NativeModuleHeader {
            role: match file.source.source_role() {
                SourceRole::Implementation => ModuleSourceRole::Implementation,
                SourceRole::Declaration => ModuleSourceRole::Declaration,
            },
        }
    }
    fn module_inferred_result_pack(
        &self,
        key: NativeModuleKey,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        self.owner(key.snapshot).module_result_pack(queries)
    }
    fn project_query_error(&self, error: NativeProjectError) {
        let module = match error {
            NativeProjectError::InvalidModule(module)
            | NativeProjectError::MissingDeclarationContract(module)
            | NativeProjectError::UnsupportedLoader { module, .. } => module,
            NativeProjectError::CircularQuery(query)
            | NativeProjectError::TooDeep(query)
            | NativeProjectError::UnresolvedQuery(query) => query.module(),
        };
        if let Some(owner) = self.owner_if(module.snapshot) {
            owner.module_query_error(error);
        }
    }
    fn contains_class_definition(&self, key: luanalysis_engine::NativeClassDefinitionKey) -> bool {
        self.owner_if(key.snapshot).is_some_and(|source| source.contains_class_definition(key))
    }
    fn class_definition_header(&self, key: luanalysis_engine::NativeClassDefinitionKey) -> luanalysis_engine::ClassDefinitionHeader {
        self.owner(key.snapshot).class_definition_header(key)
    }
    fn contains_signature(&self,key:NativeSignatureKey)->bool {
        self.owner_if(key.snapshot).is_some_and(|owner|owner.contains_signature(key))
    }
    fn function_lexical_parent(&self, key: NativeFunctionKey) -> Option<NativeFunctionKey> {
        self.owner(key.snapshot).function_lexical_parent(key)
    }
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        self.owner_if(key.snapshot)
            .is_some_and(|owner| owner.contains_function(key))
    }
    fn function_calling_convention(&self, key: NativeFunctionKey) -> luanalysis_engine::FunctionCallingConvention {
        self.owner(key.snapshot).function_calling_convention(key)
    }
    fn contains_binding(&self, key: NativeBindingKey) -> bool {
        self.owner_if(key.snapshot)
            .is_some_and(|owner| owner.contains_binding(key))
    }
    fn contains_expression(&self, key: NativeExpressionKey) -> bool {
        self.owner_if(key.snapshot)
            .is_some_and(|owner| owner.contains_expression(key))
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        self.owner_if(key.snapshot)
            .is_some_and(|owner| owner.contains_type_parameter(key))
    }
    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool {
        self.owner_if(key.snapshot)
            .is_some_and(|owner| owner.contains_type_declaration(key))
    }
    fn contains_type_member(&self, key: NativeTypeMemberKey) -> bool {
        self.owner_if(key.snapshot)
            .is_some_and(|owner| owner.contains_type_member(key))
    }
    fn is_expression_descendant(
        &self,
        expression: NativeExpressionKey,
        ancestor: NativeExpressionKey,
    ) -> bool {
        expression.snapshot == ancestor.snapshot
            && self.owner_if(expression.snapshot).is_some_and(|owner| {
                owner.contains_expression(expression)
                    && owner.contains_expression(ancestor)
                    && owner.is_expression_descendant(expression, ancestor)
            })
    }
    fn type_member_header(&self, key: NativeTypeMemberKey) -> TypeMemberHeader {
        let mut header = self.owner(key.snapshot).type_member_header(key);
        match &mut header.owner {
            TypeMemberOwner::Reference { name, candidates } => *candidates = self.names.candidates(name).to_vec(),
            // A merged class declaration's members belong to the class it merges into.
            TypeMemberOwner::Declared(owner) => *owner = self.names.canonical(*owner),
            TypeMemberOwner::RuntimeStore(_) => {}
        }
        header
    }
    fn runtime_table_constructor(&self,key:NativeTypeDeclarationKey)->Option<NativeExpressionKey> {self.owner(key.snapshot).runtime_table_constructor(key)}
    fn runtime_table_constructor_owner(&self,key:NativeTypeDeclarationKey)->Option<NativeFunctionKey> {self.owner(key.snapshot).runtime_table_constructor_owner(key)}
    fn runtime_table_seed(&self,key:NativeTypeDeclarationKey,queries:&mut dyn TypeQueries)->Option<Type> {self.owner(key.snapshot).runtime_table_seed(key,queries)}
    fn runtime_store_receiver(&self,key:luanalysis_engine::NativeFlowKey,queries:&mut dyn TypeQueries)->Option<Type> {
        self.files.iter().find(|file|file.source.iterator_sources().program.snapshot==key.graph.snapshot)?.source.runtime_store_receiver(key,queries)
    }
    fn type_member_signatures(&self,key:NativeTypeMemberKey)->Option<Vec<NativeSignatureKey>> {
        self.owner(key.snapshot).type_member_signatures(key)
    }
    fn type_declaration_header(&self,key:NativeTypeDeclarationKey)->TypeDeclarationHeader {
        self.declaration_header_metadata(key)
    }
    fn header_revision(&self) -> Option<u64> {
        // Headers derive only from original metadata, and every change to it
        // advances the clock.
        Some(crate::native_types::metadata_clock())
    }
    fn type_declaration_owner_scope(&self, key: NativeTypeDeclarationKey) -> luanalysis_engine::OwnerScopeRecipe {
        self.owner(key.snapshot).named_owner_scope_recipe(key)
    }
    fn type_member_origin(&self, key: NativeTypeMemberKey) -> luanalysis_engine::TableFieldOrigin {
        self.owner(key.snapshot).type_member_origin(key)
    }
    fn type_member_is_assignment(&self, key: NativeTypeMemberKey) -> bool {
        self.owner(key.snapshot).type_member_is_assignment(key)
    }
    // A merged class's signatures and bases are those of all its declarations.
    fn type_declaration_call_signatures(&self, key: NativeTypeDeclarationKey) -> Vec<NativeSignatureKey> {
        let mut heads = self.owner(key.snapshot).type_declaration_call_signatures(key);
        for &merged in self.names.merged_into(key) {
            heads.extend(self.owner(merged.snapshot).type_declaration_call_signatures(merged));
        }
        heads
    }
    fn type_declaration_operator_signatures(&self, key: NativeTypeDeclarationKey, operator: luanalysis_engine::NativeOperator) -> Vec<NativeSignatureKey> {
        let mut heads = self.owner(key.snapshot).type_declaration_operator_signatures(key, operator);
        for &merged in self.names.merged_into(key) {
            heads.extend(self.owner(merged.snapshot).type_declaration_operator_signatures(merged, operator));
        }
        heads
    }
    fn type_declaration_bases(&self, key: NativeTypeDeclarationKey, queries: &mut dyn TypeQueries) -> Vec<Type> {
        let mut bases = self.owner(key.snapshot).type_declaration_bases(key, queries);
        for &merged in self.names.merged_into(key) {
            // A class declared in several files names its base in each: it is one base.
            for base in self.owner(merged.snapshot).type_declaration_bases(merged, queries) {
                if !bases.contains(&base) { bases.push(base); }
            }
        }
        bases
    }
    fn type_declaration_members(&self, key: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        let mut members = self.owner(key.snapshot).type_declaration_members(key);
        for &merged in self.names.merged_into(key) {
            members.extend(self.owner(merged.snapshot).type_declaration_members(merged));
        }
        // A runtime schema's members are the stores its own file's receivers
        // name syntactically; another file's store cannot name its constructor.
        if self.owner(key.snapshot).runtime_schema_contains(key) {
            return members;
        }
        // Applicability, aliases, specialized owners and canonical object
        // ownership are resolved by the backend, not by source name matching.
        for file in &self.files {
            if file.source.snapshot == key.snapshot {
                continue;
            }
            members.extend(
                file.source
                    .named
                    .augmentations()
                    .iter()
                    .flat_map(|augmentation| {
                        augmentation
                            .members
                            .iter()
                            .map(|member| NativeTypeMemberKey {
                                snapshot: member.snapshot,
                                index: member.index,
                            })
                    }),
            );
            members.extend(file.source.doc_members.keys());
            members.extend(file.source.members.applied_members());
        }
        members
    }
    fn type_declaration_error(&self, error: NamedTypeError) {
        let snapshot = match &error {
            NamedTypeError::InvalidReceiver { .. } | NamedTypeError::UnsupportedReceiverType { .. } => {
                unreachable!("receiver failures belong to their original expression query")
            },
            NamedTypeError::InvalidDefinition(owner)
            | NamedTypeError::DefinitionOwner { owner, .. }
            | NamedTypeError::DefinitionInitializer { owner, .. }
            | NamedTypeError::DefinitionField { owner, .. } => owner.snapshot,
            NamedTypeError::InvalidDeclaration(key)
            | NamedTypeError::CircularAlias(key)
            | NamedTypeError::CircularBase(key)
            | NamedTypeError::UnsupportedKind(key, _)
            | NamedTypeError::UnsupportedBases(key) => key.snapshot,
            NamedTypeError::InvalidMember(key)
            | NamedTypeError::InvalidMemberSignatures {member:key,..}
            | NamedTypeError::UnsupportedVisibility(key)
            | NamedTypeError::UnsupportedOwnerInstantiation(key) => key.snapshot,
            NamedTypeError::InvalidTypeParameter(key) => key.snapshot,
            NamedTypeError::PendingContext { member }
            | NamedTypeError::UnknownOwner { member, .. }
            | NamedTypeError::AmbiguousOwner { member, .. } => member.snapshot,
            NamedTypeError::TypeArgumentArity { declaration, .. }
            | NamedTypeError::TypeArgumentConstraint { declaration, .. }
            | NamedTypeError::InvalidBase { declaration, .. }
            | NamedTypeError::IncompatibleOverride { declaration, .. }
            | NamedTypeError::IncompatibleBase { declaration, .. }
            | NamedTypeError::Table { declaration, .. } => declaration.snapshot,
            NamedTypeError::TooDeep(query) => match query {
                NamedTypeQuery::OwnerScope(key) | NamedTypeQuery::Head(key)
                | NamedTypeQuery::Bases(key)
                | NamedTypeQuery::ObjectMembers(key)
                | NamedTypeQuery::ObjectValidation(key) => key.snapshot,
                NamedTypeQuery::DefinitionContext(key) => key.snapshot,
                NamedTypeQuery::Member(key) | NamedTypeQuery::MemberContext(key) | NamedTypeQuery::MemberOwnerApplication(key) | NamedTypeQuery::MemberOwnerMatch(key) | NamedTypeQuery::Indexer(key) => key.snapshot,
            },
        };
        self.owner(snapshot).type_declaration_error(error);
    }

    dispatch! {
        fn type_declaration_self_parameter(key: NativeTypeDeclarationKey) -> Option<NativeTypeParameterKey>;
        fn function_is_method(key: NativeFunctionKey) -> bool;
        fn signature_is_method(key: NativeSignatureKey) -> bool;
        fn signature_header(key: NativeSignatureKey) -> SignatureHeader;
        fn signature_has_unconstrained_return(key: NativeSignatureKey) -> bool;
        fn function_overload_signatures(key: NativeFunctionKey) -> Vec<NativeSignatureKey>;
        fn function_exposes_implementation(key: NativeFunctionKey) -> bool;
        fn signature_type_parameters(key: NativeSignatureKey) -> Vec<NativeTypeParameterKey>;
        fn signature_outer_type_parameters(key: NativeSignatureKey) -> Vec<NativeTypeParameterKey>;
        fn signature_outer_type_parameter_instances(key: NativeSignatureKey, queries: &mut dyn TypeQueries) -> Vec<Type>;
        fn signature_parameters(key: NativeSignatureKey, queries: &mut dyn TypeQueries) -> Vec<Parameter>;
        fn signature_rest_pack(key: NativeSignatureKey, queries: &mut dyn TypeQueries) -> Option<ValuePack>;
        fn signature_declared_return_pack(key: NativeSignatureKey, queries: &mut dyn TypeQueries) -> ValuePack;
        fn signature_return_cycle(key: NativeSignatureKey) -> ();
        fn signature_return_type_too_deep(key: NativeSignatureKey) -> ();

        fn function_declared_scalar_contract(key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Option<ValuePack>;
        fn function_declared_scalar_contract_in_context(key: NativeFunctionKey, context: &ContextualFunctionContext, queries: &mut dyn TypeQueries) -> Option<ValuePack>;
        fn function_declared_scalar_contract_in_capture(key: NativeFunctionKey, context: &CapturedFunctionContext, queries: &mut dyn TypeQueries) -> Option<ValuePack>;
        fn type_alias_target(key: NativeTypeDeclarationKey, queries: &mut dyn TypeQueries) -> Type;
        fn type_member_type(key: NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> Type;
        fn type_member_owner_application(key: NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> Option<luanalysis_engine::MemberOwnerApplication>;
        fn type_member_activation(key: NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> luanalysis_engine::TypeMemberActivation;
        fn type_member_effective_metadata(key: NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> luanalysis_engine::TypeMemberEffectiveMetadata;
        fn contextual_type_owner(key: NativeFunctionKey, input: luanalysis_engine::NativeFlowKey, queries: &mut dyn TypeQueries) -> luanalysis_engine::ContextualTypeOwner;
        fn type_indexer_key(key: NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> Type;
        fn type_parameter_owner(key: NativeTypeParameterKey) -> Option<TypeDeclarationOwner>;
        fn check_expression_pack(key: NativeExpressionKey, expected: Option<&ValuePack>, mode: NativeCheckMode, queries: &mut dyn TypeQueries) -> ValuePack;
        fn function_has_result_pack(key: NativeFunctionKey) -> bool;
        fn function_declared_return_pack(key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Option<ValuePack>;
        fn function_inferred_return_pack(key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> ValuePack;
        fn function_declared_return_pack_in_context(key: NativeFunctionKey, context: &ContextualFunctionContext, queries: &mut dyn TypeQueries) -> Option<ValuePack>;
        fn function_inferred_return_pack_in_context(key: NativeFunctionKey, context: &ContextualFunctionContext, queries: &mut dyn TypeQueries) -> ValuePack;
        fn function_declared_return_pack_in_capture(key: NativeFunctionKey, context: &CapturedFunctionContext, queries: &mut dyn TypeQueries) -> Option<ValuePack>;
        fn function_inferred_return_pack_in_capture(key: NativeFunctionKey, context: &CapturedFunctionContext, queries: &mut dyn TypeQueries) -> ValuePack;
        fn function_rest_pack(key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Option<ValuePack>;
        fn function_rest_pack_in_context(key: NativeFunctionKey, context: &ContextualFunctionContext, queries: &mut dyn TypeQueries) -> Option<ValuePack>;
        fn function_rest_pack_in_capture(key: NativeFunctionKey, context: &CapturedFunctionContext, queries: &mut dyn TypeQueries) -> Option<ValuePack>;
        fn contextual_rest_parameter(key: NativeFunctionKey) -> Option<usize>;
        fn function_parameters_in_capture(key: NativeFunctionKey, context: &CapturedFunctionContext, queries: &mut dyn TypeQueries) -> Vec<Parameter>;
        fn function_declared_return_in_capture(key: NativeFunctionKey, context: &CapturedFunctionContext, queries: &mut dyn TypeQueries) -> Option<Type>;
        fn function_inferred_return_in_capture(key: NativeFunctionKey, context: &CapturedFunctionContext, queries: &mut dyn TypeQueries) -> Type;
        fn is_context_sensitive(key: NativeExpressionKey) -> bool;
        fn has_context_sensitive_parameters(key: NativeExpressionKey) -> bool;
        fn check_expression(key: NativeExpressionKey, expected: Option<&Type>, mode: NativeCheckMode, queries: &mut dyn TypeQueries) -> Type;
        fn check_reusable_expression(key: NativeExpressionKey, expected: Option<&Type>, mode: NativeCheckMode, queries: &mut dyn TypeQueries) -> (Type, bool);
        fn contextual_parameter_indices(key: NativeFunctionKey) -> Vec<usize>;
        fn function_parameters_in_context(key: NativeFunctionKey, context: &ContextualFunctionContext, queries: &mut dyn TypeQueries) -> Vec<Parameter>;
        fn function_declared_return_in_context(key: NativeFunctionKey, context: &ContextualFunctionContext, queries: &mut dyn TypeQueries) -> Option<Type>;
        fn function_inferred_return_in_context(key: NativeFunctionKey, context: &ContextualFunctionContext, queries: &mut dyn TypeQueries) -> Type;
        fn function_parameters(key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Vec<Parameter>;
        fn function_declared_return(key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Option<Type>;
        fn function_inferred_return(key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Type;
        fn return_cycle(key: NativeFunctionKey) -> ();
        fn return_type_too_deep(key: NativeFunctionKey) -> ();
        fn function_type_parameters(key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Vec<NativeTypeParameterKey>;
        fn function_outer_type_parameters(key: NativeFunctionKey) -> Vec<NativeTypeParameterKey>;
        fn function_outer_type_parameter_instances(key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Vec<Type>;
        fn type_parameter_name(key: NativeTypeParameterKey) -> String;
        fn has_type_parameter_constraint(key: NativeTypeParameterKey) -> bool;
        fn type_parameter_constraint(key: NativeTypeParameterKey, queries: &mut dyn TypeQueries) -> Option<Type>;
        fn has_type_parameter_default(key: NativeTypeParameterKey) -> bool;
        fn type_parameter_default(key: NativeTypeParameterKey, queries: &mut dyn TypeQueries) -> Option<Type>;
        fn type_parameter_order(key: NativeTypeParameterKey) -> u32;
        fn type_parameter_siblings(key: NativeTypeParameterKey) -> Vec<NativeTypeParameterKey>;
        fn circular_type_parameter_constraint(key: NativeTypeParameterKey) -> ();
        fn circular_type_parameter_default(key: NativeTypeParameterKey) -> ();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luanalysis_engine::Engine;

    #[test]
    fn selected_builtin_opaque_roots_retain_original_declaration_keys() {
        for profile in [
            StandardEnvironmentProfile::Lua50,
            StandardEnvironmentProfile::Lua51,
            StandardEnvironmentProfile::Lua52,
            StandardEnvironmentProfile::Lua53,
            StandardEnvironmentProfile::Lua54,
        ] {
            let mut builtin = input(0, "---@class userdata\n\n---@class thread\n");
            builtin.role = SourceRole::Declaration;
            builtin.standard_library = Some(StandardLibrarySource::Builtin);
            let provider = ProjectRegistry::new_with_environment(
                vec![builtin], Vec::new(), NativeLoaderPolicy::Lua54StandardFile, profile,
            );
            for (kind, name) in [
                (NativeOpaqueRuntimeKind::Userdata, "userdata"),
                (NativeOpaqueRuntimeKind::Thread, "thread"),
            ] {
                assert_eq!(provider.runtime_type_declaration(kind), provider.names.candidates(name).first().copied());
                assert!(provider.runtime_type_declaration(kind).is_some());
            }
        }
    }

    #[test]
    fn opaque_runtime_roots_require_selected_builtin_zero_parameter_classes() {
        let classes = "---@class userdata\n\n---@class thread\n";
        for (profile, role, library, source) in [
            (StandardEnvironmentProfile::Lua54, SourceRole::Implementation, Some(StandardLibrarySource::Builtin), classes),
            (StandardEnvironmentProfile::Lua54, SourceRole::Declaration, None, classes),
            (StandardEnvironmentProfile::Lua54, SourceRole::Declaration, Some(StandardLibrarySource::Base), classes),
            (StandardEnvironmentProfile::None, SourceRole::Declaration, Some(StandardLibrarySource::Builtin), classes),
            (StandardEnvironmentProfile::LegacyMinimal, SourceRole::Declaration, Some(StandardLibrarySource::Builtin), classes),
            (StandardEnvironmentProfile::Lua54, SourceRole::Declaration, Some(StandardLibrarySource::Builtin), "---@alias userdata string\n\n---@class thread<T>\n"),
            (StandardEnvironmentProfile::Lua54, SourceRole::Declaration, Some(StandardLibrarySource::Builtin), "---@class userdata<T>\n\n---@alias thread string\n"),
        ] {
            let mut builtin = input(0, source);
            builtin.role = role;
            builtin.standard_library = library;
            let provider = ProjectRegistry::new_with_environment(
                vec![builtin], Vec::new(), NativeLoaderPolicy::Lua54StandardFile, profile,
            );
            assert_eq!(provider.runtime_type_declaration(NativeOpaqueRuntimeKind::Userdata), None);
            assert_eq!(provider.runtime_type_declaration(NativeOpaqueRuntimeKind::Thread), None);
        }

        let mut builtin = input(0, "---@class userdata\n\n---@class thread<T>\n");
        builtin.role = SourceRole::Declaration;
        builtin.standard_library = Some(StandardLibrarySource::Builtin);
        let provider = ProjectRegistry::new_with_environment(
            vec![builtin], Vec::new(), NativeLoaderPolicy::Lua54StandardFile,
            StandardEnvironmentProfile::Lua54,
        );
        assert!(provider.runtime_type_declaration(NativeOpaqueRuntimeKind::Userdata).is_some());
        assert_eq!(provider.runtime_type_declaration(NativeOpaqueRuntimeKind::Thread), None);
    }

    #[test]
    fn merged_header_metadata_tracks_documentation_preparation_and_recovery() {
        use crate::native_doc_members::DocOwnerScope;
        use crate::native_named::{NativeNamedKey, NativeTypeContext};
        use crate::native_types::TypeNameResolution;
        let provider = ProjectRegistry::new(vec![
            input(0, "---@generic T\nlocal function outer()\n---@class Owner\nend\n"),
            input(1, "---@function Owner.read():T\n"),
        ]);
        let owner = provider.names.candidates("Owner")[0];
        let named = NativeNamedKey { snapshot: owner.snapshot, index: owner.index };
        let source = &provider.files[1].source;
        let signature = source.doc_signature_key(source.file.function_declarations[0].id).unwrap();
        let context = Some(NativeTypeContext::Signature(signature));
        let TypeNameResolution::Parameter(parameter) = source.types.resolve_name_in_context(context, "T") else {
            panic!("the imported original owner supplies its outer T");
        };
        assert!(provider.owner(owner.snapshot).type_declaration_header(owner).outer_parameters.is_empty(),
            "only the other file's original documentation head introduces this dependency");
        let header = provider.type_declaration_header(owner);
        assert_eq!(header.outer_parameters, vec![parameter]);
        assert_eq!(provider.type_declaration_header(owner), header);
        assert_eq!(provider.declaration_header_metadata_uncached(owner), header);
        let before = source.header_metadata_revision();
        let doc = &source.doc_members.sources()[0];
        doc.scope.set(DocOwnerScope::Failed);
        source.invalidate_header_metadata();
        let after = source.header_metadata_revision();
        assert_eq!(after.0, before.0, "the original scope itself has not changed");
        assert_ne!(after.1, before.1, "documentation applicability is independent metadata");
        assert_eq!(source.types.resolve_name_in_context(context, "T"), TypeNameResolution::Parameter(parameter));
        let absent = provider.type_declaration_header(owner);
        assert!(absent.outer_parameters.is_empty());
        assert_eq!(absent, provider.declaration_header_metadata_uncached(owner));
        for state in [DocOwnerScope::Scoped(named), DocOwnerScope::Ready(named)] {
            doc.scope.set(state);
            source.invalidate_header_metadata();
            assert_eq!(provider.type_declaration_header(owner), header);
            assert_eq!(provider.declaration_header_metadata_uncached(owner), header);
        }
    }

    fn input(index: u32, source: &str) -> RegistryInput {
        let syntax = luanalysis_syntax::parse(source);
        assert!(syntax.diagnostics.is_empty(), "{:#?}", syntax.diagnostics);
        let bound = crate::bind::bind(&syntax);
        RegistryInput {
            id: FileId {
                project: 777,
                index,
            },
            role: SourceRole::Implementation,
            standard_library: None,
            syntax: syntax.into(),
            bound: bound.into(),
        }
    }

    #[test]
    fn all_native_provider_hooks_have_explicit_aggregate_transport() {
        let facade = include_str!("../../luanalysis-engine/src/lib.rs");
        let trait_source = facade
            .split("pub trait NativeTypeProvider {")
            .nth(1)
            .unwrap()
            .split("\n}\n")
            .next()
            .unwrap();
        let implementation = include_str!("native_project_provider.rs")
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap();
        for method in trait_source.split("fn ").skip(1) {
            let name = method.split('(').next().unwrap();
            assert!(
                implementation.contains(&format!("fn {name}(")),
                "native hook {name} must be explicitly transported; a default may drop context or physical result count"
            );
        }
    }

    #[test]
    fn selected_setmetatable_capability_tracks_actual_alias_values_and_shadowing() {
        let standard = "---@generic T:table\n---@param value T\n---@param meta table|nil\n---@return T\nfunction setmetatable(value,meta) end";
        let source = "local cap=setmetatable\nlocal original=cap\n---@type fun(value:table,meta:table|nil):table\nlocal ordinary\ncap=ordinary\nlocal replaced=cap\ncap=setmetatable\nlocal restored=cap\n---@param value table\n---@param meta table|nil\n---@return table\nlocal function setmetatable(value,meta) return value end\nlocal shadowed=setmetatable\n";
        for selected in [false, true] {
            Engine::new().with_native_provider(
                || {
                    let mut library = input(0, standard);
                    library.role = SourceRole::Declaration;
                    library.standard_library = selected.then_some(StandardLibrarySource::Base);
                    ProjectRegistry::new_with_environment(vec![library, input(1, source)], Vec::new(), NativeLoaderPolicy::Lua54StandardFile, StandardEnvironmentProfile::Lua54)
                },
                |provider, queries| {
                    let registry = &provider.files[1].source;
                    for (prefix, name, expected) in [
                        ("original=", "cap", selected),
                        ("replaced=", "cap", false),
                        ("restored=", "cap", selected),
                        ("shadowed=", "setmetatable", false),
                    ] {
                        let start = source.find(prefix).unwrap() + prefix.len();
                        let key = registry.expression_key_for_span(luanalysis_syntax::Span { start, end: start + name.len() });
                        let value = registry.source_check_expression(key, None, luanalysis_engine::NativeCheckMode::empty(), queries);
                        let site = queries.function_intrinsic(&value);
                        assert_eq!(site.is_some(), expected, "{prefix}{name}, selected={selected}");
                        if let Some(site) = site { assert_eq!(site.kind, luanalysis_engine::NativeIntrinsicKind::SetMetatable); }
                    }
                },
            );
        }
    }

    #[test]
    fn equal_local_function_indices_keep_distinct_capture_mappers_and_whole_result_packs() {
        let source = "---@generic T\n---@param seed T\nlocal function outer(seed) return function() return seed,seed end end";
        Engine::new().with_native_provider(
            move || ProjectRegistry::new(vec![input(0, source), input(1, source)]),
            |provider, queries| {
                let first = provider.files[0].source.key(0);
                let second = provider.files[1].source.key(0);
                assert_eq!(first.index, second.index);
                assert_ne!(first.snapshot, second.snapshot);
                assert!(provider.contains_function(first));
                assert!(provider.contains_function(second));
                let numeric = queries.call(&Type::NativeFunction(first), &[Type::Number]);
                let textual = queries.call(&Type::NativeFunction(second), &[Type::String]);
                assert!(numeric.errors.is_empty(), "{:#?}", numeric.errors);
                assert!(textual.errors.is_empty(), "{:#?}", textual.errors);
                let first_inner = queries.native_function_key(&numeric.return_type).unwrap();
                let second_inner = queries.native_function_key(&textual.return_type).unwrap();
                assert_eq!(first_inner.index, second_inner.index);
                assert_ne!(first_inner.snapshot, second_inner.snapshot);
                let numeric_pack = queries.function_result_pack(&numeric.return_type).unwrap();
                let textual_pack = queries.function_result_pack(&textual.return_type).unwrap();
                for slot in 0..2 {
                    let number = queries.pack_slot(&numeric_pack, slot);
                    let string = queries.pack_slot(&textual_pack, slot);
                    assert!(queries.is_assignable(&number, &Type::Number));
                    assert!(!queries.is_assignable(&number, &Type::String));
                    assert!(queries.is_assignable(&string, &Type::String));
                    assert!(!queries.is_assignable(&string, &Type::Number));
                }
                assert!(matches!(queries.pack_slot(&numeric_pack, 2), Type::Nil));
                let one = provider.files[0].source.types.own_parameters(first)[0];
                let two = provider.files[1].source.types.own_parameters(second)[0];
                assert_eq!(one.index, two.index);
                assert_ne!(one.snapshot, two.snapshot);
                assert_eq!(
                    provider.type_parameter_owner(one),
                    Some(TypeDeclarationOwner::Function(first))
                );
                assert_eq!(
                    provider.type_parameter_owner(two),
                    Some(TypeDeclarationOwner::Function(second))
                );
            },
        );
    }

    #[test]
    fn a_contextual_call_routes_callee_and_argument_expressions_to_different_original_files() {
        let callee = "---@generic T,R\n---@param value T\n---@param project fun(value:T):R\n---@return R\nlocal function transform(value,project) return project(value) end";
        let caller = "local result=external({id=42},function(value) return value.id end)";
        Engine::new().with_native_provider(
            move || ProjectRegistry::new(vec![input(0, callee), input(1, caller)]),
            |provider, queries| {
                let source = &provider.files[1].source;
                let luanalysis_syntax::StmtKind::Local { values, .. } =
                    &source.statements()[0].kind
                else {
                    panic!("local")
                };
                let expression = &values[0];
                let luanalysis_syntax::ExprKind::Call { args, .. } = &expression.kind else {
                    panic!("call")
                };
                let arguments: Vec<_> = args
                    .iter()
                    .map(|argument| source.expression_key(argument))
                    .collect();
                let result = queries.call_expression_native(
                    &Type::NativeFunction(provider.files[0].source.key(0)),
                    source.expression_key(expression),
                    &arguments,
                    &[],
                    None,
                );
                assert!(result.errors.is_empty(), "{:#?}", result.errors);
                assert!(queries.is_assignable(&result.return_type, &Type::Number));
                assert!(!queries.is_assignable(&result.return_type, &Type::String));
                assert!(source.take_report().diagnostics.is_empty());
                let callee_source = &provider.files[0].source;
                let luanalysis_syntax::StmtKind::Return(returned) =
                    &callee_source.functions[0].body[0].kind
                else {
                    panic!("return")
                };
                assert!(!provider.is_expression_descendant(
                    arguments[0],
                    callee_source.expression_key(&returned[0])
                ));
            },
        );
    }

    #[test]
    fn aggregate_validation_rejects_foreign_snapshots_and_local_index_overflow() {
        let provider = ProjectRegistry::new(vec![input(0, "local function first() return 1 end")]);
        let key = provider.files[0].source.key(0);
        let foreign = ProjectRegistry::new(vec![input(0, "local function second() return 1 end")]);
        assert!(!provider.contains_function(foreign.files[0].source.key(0)));
        assert!(!provider.contains_function(NativeFunctionKey {
            index: u32::MAX,
            ..key
        }));
        assert!(
            !provider.contains_type_declaration(NativeTypeDeclarationKey {
                snapshot: key.snapshot,
                index: u32::MAX
            })
        );
        assert!(!provider.contains_type_member(NativeTypeMemberKey {
            snapshot: key.snapshot,
            index: u32::MAX
        }));
        assert!(!provider.contains_binding(NativeBindingKey {
            snapshot: key.snapshot,
            index: u32::MAX
        }));
    }

    fn recipe_fingerprint(provider: &ProjectRegistry) -> Vec<String> {
        let mut keys = vec![format!("{:?}", provider.globals.bindings())];
        for file in &provider.files {
            let source = &file.source;
            keys.push(format!("{:?}:{:?}", file.id, source.module_key()));
            keys.extend(source.functions.iter().map(|function| format!("{:?}", source.key_for(function))));
            keys.extend(source.expressions.sources().map(|(key, expression)| format!("{key:?}:{:?}", expression.expression.span)));
            let declarations = source.named.declarations().iter()
                .map(|declaration| NativeTypeDeclarationKey { snapshot: declaration.key.snapshot, index: declaration.key.index })
                .chain(source.named.anonymous_heads().iter().map(|head| NativeTypeDeclarationKey { snapshot: head.key.snapshot, index: head.key.index }))
                .chain(source.runtime_tables.schemas());
            for declaration in declarations {
                keys.push(format!("{declaration:?}:{:?}", provider.type_declaration_header(declaration)));
                for member in provider.type_declaration_members(declaration) {
                    keys.push(format!("{member:?}:{:?}", provider.type_member_header(member)));
                }
            }
            for index in 0.. {
                let key = NativeSignatureKey { snapshot: source.snapshot, index };
                if !source.signatures.contains(key) { break; }
                keys.push(format!("{key:?}:{:?}:{:?}", source.signatures.get(key).header, source.signature_parameters_keys(key)));
            }
            for index in 0.. {
                let key = NativeTypeParameterKey { snapshot: source.snapshot, index };
                if !source.contains_type_parameter(key) { break; }
                keys.push(format!("{key:?}:{:?}:{:?}", source.type_parameter_name(key), source.type_parameter_siblings(key)));
            }
            for graph in &source.flow.program.graphs {
                keys.extend(graph.nodes.iter().map(|node| format!("{:?}:{:?}", node.id, node.kind)));
                let graph = source.flow.graph(graph.index, 0);
                for reference in 0.. {
                    let key = luanalysis_engine::NativeReferenceKey { graph, reference };
                    let Some(original) = source.flow.source_reference(key) else { break; };
                    keys.push(format!("{key:?}:{original:?}"));
                }
            }
        }
        keys
    }

    #[test]
    fn parallel_task_registries_keep_original_recipe_keys_and_isolate_mutable_state() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ProjectRegistrySnapshot>();
        let snapshot = ProjectRegistrySnapshot::new(vec![
            input(0, "---@class Box<T>\n---@field value T\nlocal object={}\nfunction object:read() return self.value end\n---@generic U\n---@param value U\n---@return U\nlocal function identity(value) return value end\n---@type Box<number>\nlocal result={value=identity(1)}\nresult.extra=2\nreturn result\n"),
            input(1, "---@alias Result {value:number, callback:fun<T>(value:T):T}\nlocal function another() return {value=1} end\nreturn another\n"),
        ], Vec::new(), NativeLoaderPolicy::Lua54StandardFile,
            StandardEnvironmentProfile::LegacyMinimal, crate::OverloadMode::V2);
        let expected = recipe_fingerprint(&snapshot.worker_registry());
        let barrier = std::sync::Barrier::new(4);
        std::thread::scope(|scope| {
            let tasks: Vec<_> = (0..4).map(|task| {
                let snapshot = &snapshot;
                let barrier = &barrier;
                scope.spawn(move || {
                    let registry = snapshot.worker_registry();
                    let source = &registry.files[0].source;
                    barrier.wait();
                    source.report.borrow_mut().diagnostics.push(Diagnostic {
                        code: "TASK-LOCAL", span: source.file.span, message: task.to_string(),
                    });
                    source.checked_bodies.borrow_mut().insert(source.key(0));
                    registry.modules.required_files.borrow_mut().insert(registry.files[task % 2].id);
                    barrier.wait();
                    let report = source.take_report();
                    let messages: Vec<_> = report.diagnostics.into_iter().filter(|diagnostic| diagnostic.code == "TASK-LOCAL")
                        .map(|diagnostic| diagnostic.message).collect();
                    assert_eq!(messages, [task.to_string()]);
                    assert_eq!(source.checked_bodies.borrow().len(), 1);
                    assert_eq!(registry.required_files().len(), 1);
                    recipe_fingerprint(&registry)
                })
            }).collect();
            for task in tasks { assert_eq!(task.join().unwrap(), expected); }
        });
        let clean = snapshot.worker_registry();
        assert!(clean.files[0].source.checked_bodies.borrow().is_empty());
        assert!(clean.required_files().is_empty());
    }

    #[test]
    fn recipe_identities_can_survive_a_project_epoch_without_retaining_task_state() {
        let original = SourceRecipeIdentity::new();
        let changed = SourceRecipeIdentity::new();
        assert_ne!(original, changed);
        let make = |identity| ProjectRegistrySnapshot::with_identities(
            vec![input(0, "local function identity(value) return value end")], vec![identity],
            Vec::new(), NativeLoaderPolicy::Lua54StandardFile,
            StandardEnvironmentProfile::LegacyMinimal, crate::OverloadMode::V2,
        );
        assert_eq!(recipe_fingerprint(&make(original).worker_registry()), recipe_fingerprint(&make(original).worker_registry()));
        assert_ne!(recipe_fingerprint(&make(original).worker_registry()), recipe_fingerprint(&make(changed).worker_registry()));
        let empty = ProjectRegistrySnapshot::new(Vec::new(), Vec::new(), NativeLoaderPolicy::Lua54StandardFile,
            StandardEnvironmentProfile::None, crate::OverloadMode::V2);
        assert_eq!(empty.worker_registry().globals.environment, empty.worker_registry().globals.environment);
    }

    #[test]
    fn owned_root_returns_proven_dependencies_without_checking_their_unused_bodies() {
        const ENTRY: &str = "local exported=require('dep')\nreturn exported";
        const DEPENDENCY: &str = "local function unused() local bad=missingDependencyValue return bad end\nreturn 42";
        let make = || {
            let entry = input(0, ENTRY);
            let dependency = input(1, DEPENDENCY);
            let mut library = input(2, include_str!("../stdlib/LegacyMinimal/require.def.lua"));
            library.role = SourceRole::Declaration;
            library.standard_library = Some(StandardLibrarySource::LegacyRequire);
            let start = ENTRY.find("'dep'").unwrap();
            let edge = Dependency { from: entry.id,
                request: luanalysis_project::RequireRequest::new("dep", start..start + "'dep'".len()),
                outcome: DependencyOutcome::Resolved(dependency.id),
            };
            ProjectRegistry::new_with_modules(vec![entry, dependency, library,
                input(3, "local unusedStaticCandidate=missingStaticValue")], vec![edge], NativeLoaderPolicy::Lua54StandardFile)
        };
        Engine::new().with_native_provider(make, |provider, queries| {
            let root = provider.files[0].id;
            let dependency = provider.files[1].id;
            let reports = provider.check_owned_roots(queries, &[root]);
            assert!(provider.required_files().contains(&dependency), "native loader provenance schedules the dependency");
            assert!(!provider.required_files().contains(&provider.files[3].id));
            assert!(provider.files[1].source.checked_bodies.borrow().is_empty());
            assert!(reports.iter().all(|file| file.report.diagnostics.iter()
                .all(|diagnostic| !diagnostic.message.contains("missingDependencyValue"))));
            assert!(reports.iter().all(|file| file.file != provider.files[3].id));
            assert!(reports.iter().all(|file| file.report.pending_bindings.is_empty()
                && file.report.pending_types.is_empty() && file.report.pending_diagnostics.is_empty()));
        });
        Engine::new().with_native_provider(make, |provider, queries| {
            let reports = provider.check_roots(queries, &[provider.files[0].id]);
            assert!(reports.iter().any(|file| file.file == provider.files[1].id
                && file.report.diagnostics.iter().any(|diagnostic| diagnostic.message.contains("missingDependencyValue"))));
        });
        for dependency_scheduled in [true, false] {
            Engine::new().with_native_provider(make, move |provider, queries| {
                let root = provider.files[0].id;
                let dependency = provider.files[1].id;
                let mut scheduled = HashSet::from_iter([root, provider.files[2].id]);
                if dependency_scheduled { scheduled.insert(dependency); }
                let reports = provider.check_scheduled_roots(queries, &[root], &scheduled);
                assert!(provider.required_files().contains(&dependency));
                assert_eq!(provider.files[1].source.checked_bodies.borrow().is_empty(), dependency_scheduled);
                assert_eq!(reports.iter().any(|file| file.file == dependency && file.report.diagnostics.iter()
                    .any(|diagnostic| diagnostic.message.contains("missingDependencyValue"))), !dependency_scheduled);
                assert!(!provider.required_files().contains(&provider.files[3].id));
                assert!(reports.iter().all(|file| file.file != provider.files[3].id));
            });
        }
    }

    #[test]
    fn owned_root_drains_foreign_alias_diagnostics_in_the_requesting_task() {
        Engine::new().with_native_provider(|| ProjectRegistry::new(vec![
            input(0, "---@type Imported\nlocal selected\nreturn selected"),
            input(1, "---@alias Imported MissingForeignType\nlocal function unused() return missingUnusedValue end"),
        ]), |provider, queries| {
            let reports = provider.check_owned_roots(queries, &[provider.files[0].id]);
            let foreign = reports.iter().find(|file| file.file == provider.files[1].id)
                .expect("a demanded foreign alias keeps its original diagnostic owner");
            assert!(foreign.report.diagnostics.iter().any(|diagnostic|
                diagnostic.code == "TS2304" && diagnostic.message.contains("MissingForeignType")));
            assert!(foreign.report.diagnostics.iter().all(|diagnostic| !diagnostic.message.contains("missingUnusedValue")));
            assert!(provider.files[1].source.checked_bodies.borrow().is_empty());
            assert!(foreign.report.pending_bindings.is_empty() && foreign.report.pending_types.is_empty()
                && foreign.report.pending_diagnostics.is_empty());
            assert!(!provider.required_files().contains(&provider.files[1].id),
                "query diagnostic ownership does not activate an unrequired module");
        });
    }
}
