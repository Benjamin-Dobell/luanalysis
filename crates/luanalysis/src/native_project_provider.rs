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
    NativeBindingKey, NativeCheckMode, NativeExpressionKey, NativeFunctionKey, NativeSignatureKey, SignatureHeader,
    NativeTypeDeclarationKey, NativeTypeMemberKey, NativeTypeParameterKey, Parameter, Type,
    TypeDeclarationHeader, TypeDeclarationOwner, TypeMemberHeader, TypeMemberOwner, TypeQueries,
    ValuePack,
};
use luanalysis_engine::{
    ModuleSourceRole, NativeLoaderPolicy, NativeModuleHeader, NativeModuleKey, NativeProjectError,
};
use luanalysis_project::{Dependency, DependencyOutcome, FileId, SourceRole};
use luanalysis_syntax::File;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub(crate) struct RegistryInput {
    pub id: FileId,
    pub role: SourceRole,
    pub standard_library: Option<StandardLibrarySource>,
    pub syntax: File,
    pub bound: BoundFile,
}

#[derive(Default)]
pub(crate) struct ProjectNamedIndex {
    names: HashMap<String, Vec<NativeTypeDeclarationKey>>,
}
impl ProjectNamedIndex {
    pub(crate) fn candidates(&self, name: &str) -> &[NativeTypeDeclarationKey] {
        self.names.get(name).map_or(&[], Vec::as_slice)
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
    names: Rc<ProjectNamedIndex>,
    globals: Rc<super::globals::ProjectGlobalIndex>,
    standard_environment: super::standard::ProjectStandardEnvironment,
    modules: Rc<ProjectModuleIndex>,
}

impl ProjectRegistry {
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

    pub(crate) fn new_with_environment(
        inputs: Vec<RegistryInput>,
        dependencies: Vec<Dependency>,
        policy: NativeLoaderPolicy,
        environment_profile: StandardEnvironmentProfile,
    ) -> Self {
        Self::new_with_environment_and_overloads(inputs, dependencies, policy, environment_profile, crate::OverloadMode::default())
    }

    pub(crate) fn new_with_environment_and_overloads(
        inputs: Vec<RegistryInput>,
        dependencies: Vec<Dependency>,
        policy: NativeLoaderPolicy,
        environment_profile: StandardEnvironmentProfile,
        overload_mode: crate::OverloadMode,
    ) -> Self {
        let mut snapshots = HashMap::new();
        let mut names = ProjectNamedIndex::default();
        let mut files: Vec<_> = inputs
            .into_iter()
            .enumerate()
            .map(|(index, input)| {
                let mut source = NativeRegistry::new(input.syntax, input.bound);
                source.set_overload_mode(overload_mode);
                source.set_source_role(input.role);
                assert!(snapshots.insert(source.snapshot, index).is_none());
                for declaration in source.named.declarations() {
                    names
                        .names
                        .entry(declaration.syntax.name.text.clone())
                        .or_default()
                        .push(NativeTypeDeclarationKey {
                            snapshot: declaration.key.snapshot,
                            index: declaration.key.index,
                        });
                }
                FileRegistry {
                    id: input.id,
                    role: input.role,
                    standard_library: input.standard_library,
                    source,
                }
            })
            .collect();
        let names = Rc::new(names);
        let original_scopes:Vec<_>=files.iter().map(|file|file.source.types.original_scopes()).collect();
        for file in &mut files {
            for scopes in &original_scopes { file.source.types.import_original_scopes(scopes); }
            file.source.project_names=Some(Rc::clone(&names));
            file.source.attach_documentation_signature_owners();
            file.source.rebuild_documentation_members();
        }
        let environment = luanalysis_engine::NativeEnvironmentKey {
            snapshot: files.first().map_or_else(|| super::NEXT_SNAPSHOT.fetch_add(1, std::sync::atomic::Ordering::Relaxed), |file| file.source.snapshot),
            index: 0,
        };
        let globals = Rc::new(super::globals::ProjectGlobalIndex::new(environment, &files.iter().map(|file| &file.source).collect::<Vec<_>>()));
        let modules_by_file: HashMap<_, _> = files
            .iter()
            .map(|file| (file.id, file.source.module_key()))
            .collect();
        let mut imports = HashMap::new();
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
        // The sparse legacy compatibility API has no original standard value
        // declarations. Keep its conservative historical policy isolated from
        // the selected native standard environment and its per-reference flow.
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
            file.source.project_names = Some(Rc::clone(&names));
            file.source.project_modules = Some(Rc::clone(&modules));
            file.source.project_globals = Some(Rc::clone(&globals));
            file.source.set_legacy_builtin_policy(legacy_builtins);
            if let Some(capability) = standard_environment.type_operation {
                file.source.set_original_type_operation(capability);
            }
            if environment_profile == StandardEnvironmentProfile::Lua54 {
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
            names,
            globals,
            standard_environment,
            modules,
        }
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

    pub(crate) fn required_files(&self) -> HashSet<FileId> {
        self.modules.required_files.borrow().clone()
    }

    pub(crate) fn check_roots(&self, queries: &mut dyn TypeQueries, roots: &[FileId]) -> Vec<FileCheckReport> {
        // Canonical scope identities must exist for every sibling extension
        // before any alias head can publish a class's captured outer mapper.
        for file in &self.files { file.source.prepare_documentation_scope_keys(queries); }
        for file in &self.files { file.source.validate_documentation_scope_owners(queries); }
        // Candidate registration establishes every identity/name, but only
        // explicit roots and proven original require targets schedule checks.
        self.modules.required_files.borrow_mut().extend(roots.iter().copied());
        let mut checked = HashSet::new();
        let mut reports: Vec<_> = self.files.iter().map(|file| FileCheckReport {
                file: file.id,
                report: CheckReport::default(),
            }).collect();
        // Rendering can complete a deferred signature in any owner file. Drain
        // those source report queues before the arena ends; this schedules
        // report publication only and never iterates or joins inferred types.
        loop {
            for (index, file) in self.files.iter().enumerate() {
                if !self.modules.required_files.borrow().contains(&file.id) || !checked.insert(index) {
                    continue;
                }
                file.source.check_named_declarations(queries);
                file.source.check_global_declarations(queries);
                file.source.check_module_requests(queries);
                if file.role == SourceRole::Implementation {
                    let mut checker = file.source.checker(queries);
                    checker.flow_at = Some(file.source.flow_entry(None, None));
                    checker.statements(file.source.statements());
                    reports[index].report.append(checker.report);
                    file.source.check_all_bodies(queries);
                }
            }
            let mut published = false;
            for (index, (file, report)) in self.files.iter().zip(&mut reports).enumerate() {
                if !checked.contains(&index) { continue; }
                let pending = file.source.take_report();
                published |= !pending.diagnostics.is_empty()
                    || !pending.bindings.is_empty()
                    || !pending.types.is_empty()
                    || !pending.pending_bindings.is_empty()
                    || !pending.pending_types.is_empty();
                report.report.append(pending);
                published |= !report.report.pending_bindings.is_empty()
                    || !report.report.pending_types.is_empty();
                report.report.render(queries);
            }
            let unchecked = self.files.iter().enumerate().any(|(index, file)| {
                !checked.contains(&index) && self.modules.required_files.borrow().contains(&file.id)
            });
            if !published && !unchecked {
                break;
            }
        }
        for (index, (file, report)) in self.files.iter().zip(&mut reports).enumerate() {
            if !checked.contains(&index) { continue; }
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
            report.report.bound = Some(file.source.bound().clone());
        }
        reports.retain(|report| self.modules.required_files.borrow().contains(&report.file));
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
        self.globals.declarations(binding).iter().all(|owner| {
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
        if let TypeMemberOwner::Reference { name, candidates } = &mut header.owner {
            *candidates = self.names.candidates(name).to_vec();
        }
        header
    }
    fn type_member_signatures(&self,key:NativeTypeMemberKey)->Option<Vec<NativeSignatureKey>> {
        self.owner(key.snapshot).type_member_signatures(key)
    }
    fn type_declaration_header(&self,key:NativeTypeDeclarationKey)->TypeDeclarationHeader {
        let owner=self.owner(key.snapshot);
        let mut header=owner.type_declaration_header(key);
        let named=crate::native_named::NativeNamedKey {snapshot:key.snapshot,index:key.index};
        if owner.named_metadata().declaration(named).is_none() {return header;}
        let mut used=std::collections::HashSet::new();
        for file in &self.files {
            file.source.doc_members.dependencies(named,&mut used,|id|file.source.doc_signature_key(id),|signature|file.source.signature_outer_keys(signature));
        }
        header.outer_parameters.extend(owner.types.outer_parameters_in_context(crate::native_named::NativeTypeContext::NamedType(named),&used));
        header.outer_parameters.sort_by_key(|key|(key.snapshot,key.index));
        header.outer_parameters.dedup();
        header
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
    fn type_declaration_members(&self, key: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        let mut members = self.owner(key.snapshot).type_declaration_members(key);
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
            NamedTypeError::UnknownOwner { member, .. }
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
                | NamedTypeQuery::ObjectMembers(key) => key.snapshot,
                NamedTypeQuery::DefinitionContext(key) => key.snapshot,
                NamedTypeQuery::Member(key) | NamedTypeQuery::MemberContext(key) | NamedTypeQuery::Indexer(key) => key.snapshot,
            },
        };
        self.owner(snapshot).type_declaration_error(error);
    }

    dispatch! {
        fn type_declaration_self_parameter(key: NativeTypeDeclarationKey) -> Option<NativeTypeParameterKey>;
        fn signature_header(key: NativeSignatureKey) -> SignatureHeader;
        fn function_overload_signatures(key: NativeFunctionKey) -> Vec<NativeSignatureKey>;
        fn function_exposes_implementation(key: NativeFunctionKey) -> bool;
        fn signature_type_parameters(key: NativeSignatureKey) -> Vec<NativeTypeParameterKey>;
        fn signature_outer_type_parameters(key: NativeSignatureKey) -> Vec<NativeTypeParameterKey>;
        fn signature_parameters(key: NativeSignatureKey, queries: &mut dyn TypeQueries) -> Vec<Parameter>;
        fn signature_rest_pack(key: NativeSignatureKey, queries: &mut dyn TypeQueries) -> Option<ValuePack>;
        fn signature_declared_return_pack(key: NativeSignatureKey, queries: &mut dyn TypeQueries) -> ValuePack;
        fn signature_return_cycle(key: NativeSignatureKey) -> ();
        fn signature_return_type_too_deep(key: NativeSignatureKey) -> ();
        fn type_declaration_call_signatures(key: NativeTypeDeclarationKey) -> Vec<NativeSignatureKey>;
        fn function_declared_scalar_contract(key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Option<ValuePack>;
        fn function_declared_scalar_contract_in_context(key: NativeFunctionKey, context: &ContextualFunctionContext, queries: &mut dyn TypeQueries) -> Option<ValuePack>;
        fn function_declared_scalar_contract_in_capture(key: NativeFunctionKey, context: &CapturedFunctionContext, queries: &mut dyn TypeQueries) -> Option<ValuePack>;
        fn type_alias_target(key: NativeTypeDeclarationKey, queries: &mut dyn TypeQueries) -> Type;
        fn type_member_type(key: NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> Type;
        fn type_indexer_key(key: NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> Type;
        fn type_declaration_bases(key: NativeTypeDeclarationKey, queries: &mut dyn TypeQueries) -> Vec<Type>;
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
            syntax,
            bound,
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
}
