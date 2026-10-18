//! Original-Lua declarations supplied to the native checker's lazy signatures.
//!
//! The immutable registry owns one parsed source snapshot. Provider callbacks
//! create independent Lua body checkers with the native query capability that
//! requested the body. They never retain a mutable report borrow across a query.

use super::native_expressions::NativeExpressions;
use super::native_flow::{FlowAdapter, FlowValueQueries, NativeFlowSources, ValueRecipe};
use super::native_named::{NativeNamed, NativeTypeContext};
use super::native_packs::{AnnotationPack, InitializerContext, NativePacks};
use super::native_types::{NativeTypes, TypeNameResolution, TypeParameterRecipe, TypeScopeRecipe};
use super::{Binding, CheckReport, Checker, Diagnostic, bind};
use bind::{BindingId, BoundFile};
use luanalysis_engine::{
    CapturedFunctionContext, ContextualFunctionContext, NativeBindingKey, NativeCallEffects, NativeCallerFrame, NativeCheckMode,
    NativeExpressionKey, NativeFlowError, NativeFlowKey, NativeFlowResult, NativeFunctionCheckState, NativeFunctionKey,
    NativeTypeParameterKey, NativeTypeProvider, Parameter, Type, TypeQueries, ValuePack,
};
use luanalysis_syntax::{Expr, ExprKind, File, Function, Name, Span, Stmt, StmtKind, TypeExpr};
use std::cell::{Cell, RefCell};
use crate::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU64;

#[path = "native_named_provider.rs"]
mod named_provider;
#[path = "native_shape_attachment.rs"]
mod shape_attachment;
#[path = "native_class_definitions.rs"]
mod class_definitions;
#[path = "native_ambient_locals.rs"]
mod ambient_locals;
#[path = "native_assignment_context.rs"]
mod assignment_context;
#[path = "native_methods.rs"]
mod methods;
#[path = "native_metatables.rs"]
mod metatables;
#[path = "native_alias_facts.rs"]
mod alias_facts;
#[path = "native_project_provider.rs"]
pub(crate) mod project_provider;
#[path = "native_globals.rs"]
pub(crate) mod globals;
#[path = "native_modules.rs"]
pub(crate) mod modules;
#[path = "native_environment_paths.rs"]
mod environment_paths;
#[path = "native_alias_contracts.rs"]
mod alias_contracts;
#[path = "native_effect_contracts.rs"]
mod effect_contracts;

static NEXT_SNAPSHOT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
enum DeclarationSource {
    Local {
        annotation: Option<TypeExpr>,
        initializer_assertion: Option<(luanalysis_syntax::TypeAnnotation, usize)>,
        owner: Option<u32>,
        automatic: bool,
    },
    Function(u32),
    Parameter {
        function: u32,
        index: usize,
    },
    NumericLoop,
    GenericLoop,
}

pub(super) struct NativeRegistry {
    snapshot: u64,
    source_role: luanalysis_project::SourceRole,
    overload_mode: crate::OverloadMode,
    no_implicit_any: bool,
    /// Report inspections (`ProjectCheckOptions::inspections`).
    inspections: bool,
    legacy_builtin_policy: super::builtins::LegacyBuiltinPolicy,
    file: std::sync::Arc<File>,
    bound: std::sync::Arc<BoundFile>,
    functions: std::sync::Arc<Vec<Function>>,
    function_names: std::sync::Arc<Vec<Option<Name>>>,
    declaration_functions: std::sync::Arc<HashSet<u32>>,
    function_spans: std::sync::Arc<HashMap<(usize, usize), u32>>,
    declarations: std::sync::Arc<HashMap<BindingId, DeclarationSource>>,
    pub(super) types: NativeTypes,
    named: std::sync::Arc<NativeNamed>,
    header_metadata: RefCell<named_provider::SourceHeaderMetadata>,
    pub(super) members: super::native_members::NativeMembers,
    pub(super) doc_members: super::native_doc_members::NativeDocMembers,
    pub(super) runtime_tables: super::native_runtime_tables::NativeRuntimeTables,
    /// The runtime-table constructor each original store's receiver names, as a
    /// binder sees it (`runtime_store_owner`). Computed once per registry.
    pub(super) runtime_store_owners: std::cell::OnceCell<Vec<Option<NativeExpressionKey>>>,
    /// The `---@type` of each value assigned to a local by an annotated assignment statement,
    /// by the value's span. Computed once per registry.
    assignment_annotations: std::cell::OnceCell<HashMap<(usize, usize), luanalysis_syntax::TypeAnnotation>>,
    class_definitions: class_definitions::NativeClassDefinitions,
    methods: methods::NativeMethods,
    metatables: metatables::NativeMetatables,
    project_names: Option<std::rc::Rc<project_provider::ProjectNamedIndex>>,
    globals: globals::NativeGlobals,
    project_globals: Option<std::rc::Rc<globals::ProjectGlobalIndex>>,
    project_modules: Option<std::rc::Rc<modules::ProjectModuleIndex>>,
    pub(super) signatures: super::native_signatures::NativeSignatures,
    expressions: NativeExpressions,
    packs: NativePacks,
    flow: NativeFlowSources,
    completed_bindings: RefCell<HashMap<BindingId, Binding>>,
    /// Only completion of body diagnostics is cached here. Return-type cache,
    /// query recursion and cycle decisions belong entirely to the native core.
    checked_bodies: RefCell<HashSet<NativeFunctionKey>>,
    checked_contextual_bodies: RefCell<HashSet<(NativeFunctionKey, ContextualFunctionContext, Option<CapturedFunctionContext>)>>,
    checked_captured_bodies: RefCell<HashSet<(NativeFunctionKey, CapturedFunctionContext)>>,
    /// Functions whose body diagnostics one complete check has reported, in any frame. As in
    /// TypeScript, a body is checked once: further contextual or captured instances of the same
    /// function are not checked again.
    bodies_checked: RefCell<HashSet<NativeFunctionKey>>,
    /// The frame of each function's single body check: the frame of its value as created in its
    /// lexical parent's own canonical frame (declaration space outside any function). Captures
    /// and contextual instances are interned by content, so this does not depend on order.
    canonical_frames: RefCell<HashMap<NativeFunctionKey, (Option<ContextualFunctionContext>, Option<CapturedFunctionContext>)>>,
    /// A language-service request's expression and the types it was checked to have.
    completion_probe: RefCell<Option<(Span, Vec<Type>)>>,
    pending_contextual_owners: RefCell<HashSet<(NativeFunctionKey, NativeFlowKey)>>,
    pending_owner_observations: Cell<u64>,
    pending_owner_bodies: RefCell<HashSet<(NativeFunctionKey, Option<ContextualFunctionContext>, Option<CapturedFunctionContext>)>>,
    report: RefCell<CheckReport>,
    provisional_reports: RefCell<Vec<CheckReport>>,
    /// Written generic references whose constraints are still to be checked, as TypeScript's
    /// deferred `checkTypeReferenceNode`: once per reference, at top level, never inside the
    /// resolution that lowered it.
    deferred_constraints: RefCell<Vec<(Span, Vec<Span>, luanalysis_engine::NativeTypeDeclarationKey, Vec<Type>)>>,
    deferred_constraint_spans: RefCell<HashSet<(usize, usize)>>,
}

/// What a registry builds from one source revision alone: its functions and declarations, named
/// declarations, expressions and flow recipes. They hold no type and no session state, so every
/// session over the same revision and identity shares them (`ProjectWorkspace`).
#[derive(Clone)]
pub(crate) struct SourceRecipes {
    snapshot: u64,
    functions: std::sync::Arc<Vec<Function>>,
    function_names: std::sync::Arc<Vec<Option<Name>>>,
    declaration_functions: std::sync::Arc<HashSet<u32>>,
    function_spans: std::sync::Arc<HashMap<(usize, usize), u32>>,
    declarations: std::sync::Arc<HashMap<BindingId, DeclarationSource>>,
    named: std::sync::Arc<NativeNamed>,
    expressions: std::sync::Arc<super::native_expressions::ExpressionRecipes>,
    flow: std::sync::Arc<super::native_flow::NativeFlowRecipes>,
}

impl SourceRecipes {
    fn new(file: &File, bound: &BoundFile, identity: project_provider::SourceRecipeIdentity) -> Self {
        let snapshot = identity.source;
        let mut index = RegistryIndex {
            bound,
            assertions: file.expression_assertions.iter().filter(|assertion| assertion.kind == luanalysis_syntax::ExpressionAssertionKind::Type)
                .map(|assertion| ((assertion.expression.start, assertion.expression.end), &assertion.annotation)).collect(),
            functions: Vec::new(),
            function_names: Vec::new(),
            declaration_functions: HashSet::default(),
            function_spans: HashMap::default(),
            declarations: HashMap::default(),
        };
        index.statements(&file.statements, None);
        let RegistryIndex {
            functions,
            function_names,
            declaration_functions,
            function_spans,
            mut declarations,
            ..
        } = index;
        let source_function_keys = function_spans
            .iter()
            .map(|(&span, &index)| (span, NativeFunctionKey { snapshot, index }))
            .collect();
        let named = NativeNamed::new(snapshot, file, bound, &source_function_keys)
            .expect("original named declaration metadata");
        for (index, function) in functions.iter().enumerate() {
            if let Some(receiver) = bound.implicit_receiver(function.span) {
                declarations.insert(receiver, DeclarationSource::Parameter { function: index as u32, index: 0 });
                for (parameter_index, parameter) in function.params.iter().enumerate() {
                    let binding = bound.binding_at(parameter.span.start).expect("original written method parameter");
                    declarations.insert(binding, DeclarationSource::Parameter { function: index as u32, index: parameter_index + 1 });
                }
            }
        }
        let expressions = super::native_expressions::ExpressionRecipes::new(snapshot, file, &function_spans);
        let flow = NativeFlowSources::new_with_snapshot(file, bound, snapshot, &function_spans, identity.flow).shared_recipes();
        Self {
            snapshot,
            functions: std::sync::Arc::new(functions),
            function_names: std::sync::Arc::new(function_names),
            declaration_functions: std::sync::Arc::new(declaration_functions),
            function_spans: std::sync::Arc::new(function_spans),
            declarations: std::sync::Arc::new(declarations),
            named: std::sync::Arc::new(named),
            expressions: std::sync::Arc::new(expressions),
            flow,
        }
    }

    #[cfg(test)]
    pub(crate) fn flow(&self) -> &std::sync::Arc<super::native_flow::NativeFlowRecipes> {
        &self.flow
    }
}

#[path = "native_standard_environment.rs"]
pub(crate) mod standard;

impl NativeRegistry {
    /// Deferred validation uses the assignment's native lifetime, including its
    /// lexical parent. Current provisional frames remain available to inference.
    pub(super) fn body_frame_check_state(
        &self,
        contextual: Option<&ContextualFunctionContext>,
        captured: Option<&CapturedFunctionContext>,
        queries: &mut dyn TypeQueries,
    ) -> NativeFunctionCheckState {
        let context = contextual.map_or(NativeFunctionCheckState::Final, |context| queries.contextual_function_check_state(context));
        let capture = captured.map_or(NativeFunctionCheckState::Final, |capture| {
            let value = queries.captured_function_value(capture);
            queries.function_check_state(&value)
        });
        match (context, capture) {
            (NativeFunctionCheckState::Expired, _) | (_, NativeFunctionCheckState::Expired) => NativeFunctionCheckState::Expired,
            (NativeFunctionCheckState::Provisional, _) | (_, NativeFunctionCheckState::Provisional) => NativeFunctionCheckState::Provisional,
            _ => NativeFunctionCheckState::Final,
        }
    }

    pub(super) fn invalidate_header_metadata(&self) {
        self.header_metadata.borrow_mut().invalidate();
    }
    #[cfg(test)]
    pub(super) fn header_metadata_revision(&self) -> (u64, u64) {
        (self.types.own_metadata_revision(), self.header_metadata.borrow().generation())
    }

    pub(super) fn new(file: File, bound: BoundFile) -> Self {
        Self::new_with_identity(std::sync::Arc::new(file), std::sync::Arc::new(bound), project_provider::SourceRecipeIdentity::new())
    }

    fn new_with_identity(file: std::sync::Arc<File>, bound: std::sync::Arc<BoundFile>, identity: project_provider::SourceRecipeIdentity) -> Self {
        Self::new_with_identity_and_flow(file, bound, identity, None)
    }

    fn new_with_identity_and_flow(
        file: std::sync::Arc<File>,
        bound: std::sync::Arc<BoundFile>,
        identity: project_provider::SourceRecipeIdentity,
        prepared: Option<SourceRecipes>,
    ) -> Self {
        let snapshot = identity.source;
        let source = prepared.unwrap_or_else(|| SourceRecipes::new(&file, &bound, identity));
        assert_eq!(source.snapshot, snapshot, "prepared recipes belong to their original source identity");
        let SourceRecipes { functions, function_names, declaration_functions, function_spans, declarations, named, expressions, flow, .. } = source;
        let class_definitions = class_definitions::NativeClassDefinitions::new(snapshot, &named);
        let signatures=super::native_signatures::NativeSignatures::new(snapshot,&file,&functions,&named);
        let mut types = NativeTypes::new_with_named(
            snapshot,
            type_scope_recipes(snapshot, &functions, &bound),
            named
                .declarations()
                .iter()
                .map(|source| source.generic_scope.clone())
                .chain(signatures.scopes(&file))
                .collect(),
        );
        for (function,signature) in signatures.implementation_scopes(&functions) {
            types.attach_signature_scope(function,signature);
        }
        // Semantic self identities are allocated only after all original
        // written generic keys. They are not class type-argument formals.
        for source in named.declarations() {
            if matches!(source.syntax.kind, luanalysis_syntax::NamedTypeDeclarationKind::Record {
                flavor: luanalysis_syntax::NamedRecordKind::Class | luanalysis_syntax::NamedRecordKind::Shape, ..
            }) {
                types.register_self_parameter(source.key, source.syntax.name.span);
            }
        }
        let expressions = NativeExpressions::from_recipes(expressions);
        let metatables = metatables::NativeMetatables::new(&expressions, &functions, &bound, &named);
        let methods = methods::NativeMethods::new(snapshot, &file, &functions, &function_spans, &bound, &named, &types, &metatables);
        for method in methods.sources() {
            if method.contextual_owner.is_none() && method.owner_application.is_none()
                && let Some(owner) = method.class_owner { types.attach_method_scope(method.function, owner); }
        }
        let packs = NativePacks::new(snapshot, &file, &function_spans, &|expression| {
            expressions.key_for_span(expression.span)
        });
        let flow = NativeFlowSources::from_shared_recipes(flow);
        assert_eq!(flow.program.snapshot, identity.flow, "prepared flow belongs to its original source recipe");
        let runtime_tables=super::native_runtime_tables::NativeRuntimeTables::new(&file,&expressions,&flow);
        let globals = globals::NativeGlobals::new(snapshot, &file, &bound, &function_spans, &expressions, named.next_member_index());
        let members = super::native_members::NativeMembers::new(snapshot, &named, Vec::new(), globals.member_namespace_end(named.next_member_index()));
        let doc_members = super::native_doc_members::NativeDocMembers::new(snapshot,&file,&named,members.namespace_end(),|name|named.candidates(name).iter().map(|key|luanalysis_engine::NativeTypeDeclarationKey {snapshot:key.snapshot,index:key.index}).collect());
        let mut report = CheckReport::default();
        report.diagnostics.extend(file.diagnostics.iter().map(|diagnostic| Diagnostic {
            code: "LUA1001",
            span: diagnostic.span,
            message: diagnostic.message.clone(),
        }));
        report
            .diagnostics
            .extend(types.diagnostics().iter().cloned());
        report.diagnostics.extend(signatures.diagnostics.iter().cloned());
        let mut registry = Self {
            snapshot,
            source_role: luanalysis_project::SourceRole::Implementation,
            overload_mode: crate::OverloadMode::default(),
            no_implicit_any: false,
            inspections: false,
            legacy_builtin_policy: super::builtins::LegacyBuiltinPolicy::Disabled,
            file,
            bound,
            functions,
            function_names,
            declaration_functions,
            function_spans,
            declarations,
            types,
            named,
            header_metadata: RefCell::new(named_provider::SourceHeaderMetadata::default()),
            members,
            doc_members,
            runtime_tables,
            runtime_store_owners: Default::default(),
            assignment_annotations: Default::default(),
            class_definitions,
            methods,
            metatables,
            project_names: None,
            globals,
            project_globals: None,
            project_modules: None,
            signatures,
            expressions,
            packs,
            flow,
            completed_bindings: RefCell::new(HashMap::default()),
            checked_bodies: RefCell::new(HashSet::default()),
            checked_contextual_bodies: RefCell::new(HashSet::default()),
            checked_captured_bodies: RefCell::new(HashSet::default()),
            bodies_checked: RefCell::new(HashSet::default()),
            canonical_frames: RefCell::new(HashMap::default()),
            completion_probe: RefCell::new(None),
            pending_contextual_owners: RefCell::new(HashSet::default()),
            pending_owner_observations: Cell::new(0),
            pending_owner_bodies: RefCell::new(HashSet::default()),
            report: RefCell::new(report),
            provisional_reports: RefCell::new(Vec::new()),
            deferred_constraints: RefCell::new(Vec::new()),
            deferred_constraint_spans: RefCell::new(HashSet::default()),
        };
        registry.rebuild_original_members();
        registry.attach_documentation_signature_owners();
        registry.project_globals = Some(std::rc::Rc::new(globals::ProjectGlobalIndex::new(
            luanalysis_engine::NativeEnvironmentKey { snapshot, index: 0 }, &[&registry],
        )));
        registry
    }

    /// Whether an expression's type cannot depend on its expected type,
    /// inference or contextual instantiation: reads of locals that are not
    /// function declarations (named functions receive contextual instances),
    /// static member reads of those, and operators over them. Literals, calls,
    /// constructors, function expressions, varargs and globals are excluded.
    pub(super) fn expression_is_context_free(&self, expression: &Expr) -> bool {
        match &expression.kind {
            ExprKind::Parenthesized(inner) => self.expression_is_context_free(inner),
            ExprKind::Name(name) => self.bound.binding_at(name.span.start).is_some_and(|binding| {
                !matches!(self.declarations.get(&binding), Some(DeclarationSource::Function(_)))
            }),
            ExprKind::Field { base, .. } => self.expression_is_context_free(base),
            ExprKind::Index { base, index } => crate::native_tables::literal_key(index).is_some()
                && self.expression_is_context_free(base),
            ExprKind::Unary { value, .. } => self.expression_is_context_free(value),
            ExprKind::Binary { left, right, .. } => self.expression_is_context_free(left) && self.expression_is_context_free(right),
            _ => false,
        }
    }

    /// The declared receiver expression (`X` in `function X:m()`) of the method
    /// whose implicit `self` is `binding`.
    pub(super) fn method_receiver_for(&self, binding: BindingId) -> Option<&Expr> {
        self.methods.sources().find(|method| method.implicit_receiver == Some(binding)).map(|method| &method.receiver)
    }

    pub(super) fn bound(&self) -> &BoundFile {
        &self.bound
    }
    pub(super) fn bound_arc(&self) -> std::sync::Arc<BoundFile> {
        std::sync::Arc::clone(&self.bound)
    }
    /// This registry's source recipes, for a later session over the same revision.
    pub(super) fn source_recipes(&self) -> SourceRecipes {
        SourceRecipes {
            snapshot: self.snapshot,
            functions: std::sync::Arc::clone(&self.functions),
            function_names: std::sync::Arc::clone(&self.function_names),
            declaration_functions: std::sync::Arc::clone(&self.declaration_functions),
            function_spans: std::sync::Arc::clone(&self.function_spans),
            declarations: std::sync::Arc::clone(&self.declarations),
            named: std::sync::Arc::clone(&self.named),
            expressions: self.expressions.recipes(),
            flow: self.flow.shared_recipes(),
        }
    }
    pub(crate) fn binding_function_owner(&self,binding:BindingId)->Option<NativeFunctionKey> {self.flow.binding_function_owner(binding)}

    /// Original annotation metadata only. Inferred runtime values retain their
    /// own continuation/capture path instead of being read as written bounds.
    pub(crate) fn binding_has_written_contract(&self, binding: BindingId) -> bool {
        match self.declarations.get(&binding) {
            Some(DeclarationSource::Local { annotation, initializer_assertion, .. }) => {
                annotation.is_some() || initializer_assertion.is_some()
            }
            Some(DeclarationSource::Parameter { function, index }) => {
                let function = &self.functions[*function as usize];
                if function.annotations.type_annotation.is_some() { return true; }
                let offset = usize::from(self.bound.implicit_receiver(function.span).is_some());
                let name = if *index < offset { Some("self") }
                    else { function.params.get(*index - offset).map(|parameter| parameter.text.as_str()) };
                name.is_some_and(|name| function.annotations.params.iter().any(|tag| tag.name.text == name))
            }
            _ => false,
        }
    }

    pub(crate) fn source_role(&self) -> luanalysis_project::SourceRole { self.source_role }
    pub(crate) fn no_implicit_any(&self) -> bool { self.no_implicit_any }
    pub(crate) fn set_no_implicit_any(&mut self, enabled: bool) { self.no_implicit_any = enabled; }
    pub(crate) fn inspections(&self) -> bool { self.inspections }
    pub(crate) fn set_inspections(&mut self, enabled: bool) { self.inspections = enabled; }
    pub(crate) fn set_overload_mode(&mut self, mode: crate::OverloadMode) {
        self.overload_mode = mode;
    }
    pub(crate) fn set_source_role(&mut self, role: luanalysis_project::SourceRole) {
        if self.source_role != role {
            self.source_role = role;
            if role == luanalysis_project::SourceRole::Declaration {
                for method in self.methods.sources() {
                    if let Some(owner) = method.declaration_owner {
                        self.types.attach_method_scope(method.function, owner);
                    }
                }
            }
            self.rebuild_original_members();
        }
    }

    /// The declaration a repeated `---@class` is merged into, or `key` itself.
    pub(crate) fn canonical_declaration(&self, key: luanalysis_engine::NativeTypeDeclarationKey) -> luanalysis_engine::NativeTypeDeclarationKey {
        self.project_names.as_ref().map_or(key, |names| names.canonical(key))
    }
    pub(crate) fn is_merged_declaration(&self, key: luanalysis_engine::NativeTypeDeclarationKey) -> bool {
        self.project_names.as_ref().is_some_and(|names| names.is_merged(key))
    }
    pub(super) fn set_project_names(&mut self, names: std::rc::Rc<project_provider::ProjectNamedIndex>) {
        self.header_metadata.get_mut().invalidate();
        self.project_names = Some(names);
    }

    // Setup only: project roles are assigned before the provider is installed
    // into a native session, so no member query can retain the previous IDs.
    pub(super) fn rebuild_original_members(&mut self) {
        self.header_metadata.get_mut().invalidate();
        let mut inputs = self.original_constructor_member_inputs();
        inputs.extend(self.methods.original_members());
        if self.source_role() == luanalysis_project::SourceRole::Implementation {
            inputs.extend(self.methods.original_assignment_members(&self.expressions));
        }
        self.members = super::native_members::NativeMembers::new(self.snapshot, &self.named, inputs, self.globals.member_namespace_end(self.named.next_member_index()));
        self.rebuild_documentation_members();
        for (context, owner) in self.original_member_owner_scopes() {
            self.types.attach_owner_scope(context, owner);
        }
    }
    pub(super) fn rebuild_documentation_members(&mut self) {
        self.header_metadata.get_mut().invalidate();
        self.doc_members=super::native_doc_members::NativeDocMembers::new(self.snapshot,&self.file,&self.named,self.members.namespace_end(),|name|self.named_candidates(name));
        self.runtime_tables.register_keys(self.snapshot,(self.named.declarations().len()+self.named.anonymous_heads().len()).try_into().expect("original declaration namespace fits u32"),self.doc_members.namespace_end());
    }

    pub(crate) fn set_legacy_builtin_policy(&mut self, policy: super::builtins::LegacyBuiltinPolicy) {
        let enabled = policy == super::builtins::LegacyBuiltinPolicy::MinimalTypeCompatibility
            && self.global_binding("type").is_none()
            && !self.bound.has_modified_type_intrinsic();
        self.legacy_builtin_policy = if enabled { policy } else { super::builtins::LegacyBuiltinPolicy::Disabled };
        self.flow.set_runtime_type_intrinsic_enabled(enabled);
    }

    pub(crate) fn legacy_builtin_value(&self, name: &Name) -> Option<Type> {
        if self.legacy_builtin_policy != super::builtins::LegacyBuiltinPolicy::MinimalTypeCompatibility
            || self.bound.intrinsic_at(name.span.start) != Some(crate::bind::BuiltinIntrinsic::LuaType)
        { return None; }
        super::builtins::value(&name.text)
    }

    pub(super) fn statements(&self) -> &[Stmt] {
        &self.file.statements
    }

    pub(super) fn expressions(&self) -> &NativeExpressions {
        &self.expressions
    }

    /// The `---@type` a value assigned to a local has: `---@type T` above `x = value`, as LuaLS
    /// reads it. Unlike an inline cast, the value is checked against it.
    pub(super) fn assignment_annotation(&self, expression: Span) -> Option<&luanalysis_syntax::TypeAnnotation> {
        self.assignment_annotations.get_or_init(|| {
            let mut statements = Vec::new();
            super::native_runtime_tables::collect_statements(&self.file.statements, &mut statements);
            for (_, source) in self.expressions().sources() {
                if let ExprKind::Function(function) = &source.expression.kind {
                    super::native_runtime_tables::collect_statements(&function.body, &mut statements);
                }
            }
            let mut annotations = HashMap::default();
            for statement in statements {
                let StmtKind::Assign { targets, values, annotation: Some(annotation) } = &statement.kind else { continue };
                for (slot, (target, value)) in targets.iter().zip(values).enumerate() {
                    let (ExprKind::Name(name), Some(ty)) = (&target.kind, annotation.types.get(slot)) else { continue };
                    if self.bound.binding_at(name.span.start).is_none() { continue; }
                    annotations.insert((value.span.start, value.span.end), luanalysis_syntax::TypeAnnotation {
                        types: vec![ty.clone()],
                        packs: None,
                        span: annotation.span,
                    });
                }
            }
            annotations
        }).get(&(expression.start, expression.end))
    }

    /// Whether an annotated assignment's `---@type` applies to the store or local it assigns.
    pub(crate) fn accepts_assignment_annotation(&self, target: &Expr) -> bool {
        match &target.kind {
            ExprKind::Name(name) => self.bound.binding_at(name.span.start).is_some(),
            _ => self.runtime_tables.store_for_target(target.span).is_some_and(|store| store.annotation.is_some()),
        }
    }

    pub(super) fn expression_assertions(&self, expression: Span) -> Vec<luanalysis_syntax::ExpressionAssertion> {
        self.file.expression_assertions.iter().filter(|assertion| assertion.expression == expression).cloned().collect()
    }

    pub(super) fn packs(&self) -> &NativePacks {
        &self.packs
    }


    /// The native call scheduler requests the original final expression under
    /// its active inference/context frame. Cardinality and row relations stay
    /// in the native pack worker.
    pub(super) fn source_check_expression_pack(
        &self,
        key: NativeExpressionKey,
        expected: Option<&ValuePack>,
        mode: luanalysis_engine::NativeCheckMode,
        queries: &mut dyn luanalysis_engine::TypeQueries,
    ) -> ValuePack {
        let expression = &self.expressions().get(key).expression;
        let mut checker = self.checker_for_expression(key, mode, queries);
        checker.check_mode = mode;
        let pack = checker.expression_pack(expression, expected);
        if super::native_expressions::publishes(mode) && !checker.incomplete_flow
            && self.body_frame_check_state(checker.contextual_function.as_ref(), checker.captured_function.as_ref(), checker.engine) == NativeFunctionCheckState::Final
        {
            self.merge_report(checker.report);
        }
        pack
    }

    fn declared_pack_recipe(&self, key: NativeFunctionKey) -> Option<AnnotationPack<'_>> {
        let function = self.function(key);
        Some(if let Some(annotation) = &function.annotations.returns {
            AnnotationPack::annotation(annotation)
        } else {
            let annotation = function.annotations.type_annotation.as_ref()?;
            let mut ty = match annotation.types.as_slice() {
                [ty] => ty,
                _ => return None,
            };
            ty = ty.value_syntax();
            while let luanalysis_syntax::TypeExprKind::Parenthesized(inner) = &ty.kind {
                ty = inner.value_syntax();
            }
            let luanalysis_syntax::TypeExprKind::Function(signature) = &ty.kind else {
                return None;
            };
            // A function type with no result syntax constrains parameters only.
            // Its runtime implementation still infers its own physical returns.
            if signature.returns_span.is_none() { return None; }
            AnnotationPack::new(
                &signature.returns,
                signature.packs.as_deref(),
                signature.span,
            )
        })
    }

    pub(super) fn declared_pack_contract(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        let recipe = self.declared_pack_recipe(key)?;
        let mut checker = self.checker(queries);
        checker.type_context = Some(key);
        checker.signature_type_context=self.signature_key_for(recipe.span);
        checker.check_mode = checker
            .engine
            .current_contextual_function()
            .map_or_else(luanalysis_engine::NativeCheckMode::empty, |context| {
                context.mode
            });
        let pack = recipe.lower(&mut |ty| checker.annotation(ty));
        if super::native_expressions::publishes(checker.check_mode) {
            self.merge_report(checker.report);
        }
        Some(pack)
    }

    pub(super) fn source_declared_return_pack(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
        contextual: Option<ContextualFunctionContext>,
        captured: Option<CapturedFunctionContext>,
    ) -> Option<ValuePack> {
        let raw = self.declared_pack_contract(key, queries)?;
        let declared = queries.declared_result_pack(&raw);
        // A written fixed physical row already establishes the public result.
        // Inferring it again would make recursive calls depend on callback bodies
        // that the contract was meant to separate. check_all_bodies independently
        // validates the implementation, including its return expressions.
        if queries.return_pack_has_fixed_cardinality(&declared) {
            return Some(declared);
        }
        let actual = self.inferred_return_pack_with_context(key, queries, contextual, captured);
        Some(queries.return_pack_with_cardinality(&declared, &actual))
    }

    pub(super) fn source_rest_pack(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        let function = self.function(key);
        if function.vararg.is_none() {
            return None;
        }
        let annotation = function.annotations.vararg.as_deref().or_else(|| {
            let annotation = function.annotations.type_annotation.as_ref()?;
            let mut ty = match annotation.types.as_slice() {
                [ty] => ty,
                _ => return None,
            };
            ty = ty.value_syntax();
            while let luanalysis_syntax::TypeExprKind::Parenthesized(inner) = &ty.kind {
                ty = inner.value_syntax();
            }
            let luanalysis_syntax::TypeExprKind::Function(signature) = &ty.kind else {
                return None;
            };
            signature.vararg.as_deref()
        });
        let mut checker = self.checker(queries);
        checker.type_context = Some(key);
        if function.annotations.vararg.is_none(){checker.signature_type_context=self.implementation_annotation_signature(key);}
        checker.check_mode = checker
            .engine
            .current_contextual_function()
            .map_or_else(luanalysis_engine::NativeCheckMode::empty, |context| {
                context.mode
            });
        let ty = match annotation {
            None if self.source_role() == luanalysis_project::SourceRole::Declaration => Type::Unknown,
            None => {
                checker.implicit_parameter_any(
                    "TS7019",
                    function.vararg.unwrap(),
                    "...",
                    "Rest parameter '...' implicitly has an 'any[]' type.",
                );
                Type::Any
            }
            Some(annotation) => checker.annotation(&annotation.ty),
        };
        if super::native_expressions::publishes(checker.check_mode) {
            self.merge_report(checker.report);
        }
        Some(ValuePack::variadic(ty))
    }

    pub(super) fn checker_for_expression<'a>(
        &'a self,
        key: NativeExpressionKey,
        mode: luanalysis_engine::NativeCheckMode,
        queries: &'a mut dyn TypeQueries,
    ) -> Checker<'a> {
        self.expression_checker(key, mode, queries, true)
    }
    pub(super) fn checker_for_declaration_expression<'a>(
        &'a self,
        key: NativeExpressionKey,
        queries: &'a mut dyn TypeQueries,
    ) -> Checker<'a> {
        self.expression_checker(key, luanalysis_engine::NativeCheckMode::empty(), queries, false)
    }
    fn expression_checker<'a>(
        &'a self,
        key: NativeExpressionKey,
        mode: luanalysis_engine::NativeCheckMode,
        queries: &'a mut dyn TypeQueries,
        inherit_signature_context: bool,
    ) -> Checker<'a> {
        let record = self.expressions.get(key);
        let contextual = queries
            .current_contextual_function()
            .filter(|_| inherit_signature_context)
            .filter(|context| self.expressions.contains(context.expression)
                && self.expressions.function_for(context.expression) == record.owner);
        let mut checker = self.checker(queries);
        checker.check_mode = mode;
        checker.type_context = record.owner;
        checker.contextual_function = contextual;
        let captured = checker.engine.current_captured_function().filter(|_| inherit_signature_context);
        checker.captured_function = captured;
        if let Some(owner) = record.owner {
            let (parameters, _) = if let Some(context) = contextual {
                self.metadata_in_context(owner, &context, checker.engine)
            } else if let Some(captured) = captured {
                self.metadata_in_capture(owner, &captured, checker.engine)
            } else {
                self.metadata(owner, checker.engine)
            };
            self.seed_parameters(&mut checker, self.function(owner), &parameters);
        }
        let graph = record.owner.map_or_else(
            || self.flow.graph_for_region(0, contextual.as_ref()),
            |owner| {
                self.flow
                    .graph_for_function(owner, contextual.as_ref())
                    .unwrap()
            },
        );
        let graph =
            self.flow
                .graph_with_contexts(graph.region, contextual.as_ref(), captured.as_ref());
        checker.flow_at = self
            .flow
            .evaluation_for_expression(&record.expression, graph)
            .map(|evaluation| self.flow.flow_key(evaluation.input, graph.context));
        checker
    }

    pub(super) fn key_for(&self, function: &Function) -> NativeFunctionKey {
        let index = *self
            .function_spans
            .get(&(function.span.start, function.span.end))
            .expect("the function belongs to this original Lua source snapshot");
        self.key(index)
    }
    pub(super) fn named_metadata(&self) -> &NativeNamed { &self.named }
    pub(crate) fn documentation_function(&self,id:luanalysis_syntax::DocFunctionDeclarationId)->Option<&luanalysis_syntax::DocFunctionDeclaration> { self.file.function_declarations.iter().find(|source|source.id==id) }

    fn key(&self, index: u32) -> NativeFunctionKey {
        NativeFunctionKey {
            snapshot: self.snapshot,
            index,
        }
    }

    pub(super) fn function(&self, key: NativeFunctionKey) -> &Function {
        assert!(
            self.contains_function(key),
            "foreign or missing native Lua function identity"
        );
        &self.functions[key.index as usize]
    }

    pub(super) fn checker<'a>(&'a self, engine: &'a mut dyn TypeQueries) -> Checker<'a> {
        Checker {
            active_assertions: HashSet::default(),
            engine,
            bound: &self.bound,
            native: Some(self),
            locals: HashMap::default(),
            assigned_fields: HashMap::default(),
            report: CheckReport::default(),
            return_type: None,
            return_values: Vec::new(),
            return_pack: None,
            return_packs: Vec::new(),
            type_context: None,
            named_type_context: None,
            signature_type_context: None,
            contextual_function: None,
            captured_function: None,
            check_mode: luanalysis_engine::NativeCheckMode::empty(),
            flow_at: None,
            incomplete_flow: false,
            runtime_value: false,
            annotation_depth: 0,
        }
    }

    pub(super) fn flow_input_for_expression(
        &self,
        expression: &Expr,
        at: NativeFlowKey,
    ) -> NativeFlowKey {
        self.flow
            .evaluation_for_expression(expression, at.graph)
            .map_or(at, |evaluation| {
                self.flow.flow_key(evaluation.input, at.graph.context)
            })
    }

    pub(super) fn call_statement_terminates(&self, expression: &Expr, at: NativeFlowKey, queries: &mut dyn TypeQueries) -> bool {
        let Some(evaluation) = self.flow.evaluation_for_expression(expression, at.graph) else { return false };
        let after = self.flow.flow_key(evaluation.completed, at.graph.context);
        queries.flow_definitely_unreachable(&FlowAdapter { sources: &self.flow, values: self }, after)
    }

    pub(super) fn flow_entry(
        &self,
        function: Option<NativeFunctionKey>,
        contextual: Option<&ContextualFunctionContext>,
    ) -> NativeFlowKey {
        let graph = function.map_or_else(
            || self.flow.graph_for_region(0, contextual),
            |function| self.flow.graph_for_function(function, contextual).unwrap(),
        );
        self.flow.flow_key(
            self.flow.program.graphs[graph.region as usize].entry,
            graph.context,
        )
    }

    pub(super) fn flow_reference_type(
        &self,
        expression: &Expr,
        at: NativeFlowKey,
        declared: &Type,
        initial: &Type,
        queries: &mut dyn TypeQueries,
    ) -> NativeFlowResult {
        let Some(reference) = self.flow.expression_reference(expression, at.graph) else {
            return NativeFlowResult {
                ty: initial.clone(),
                incomplete: false,
                unresolved_auto: false,
            };
        };
        let ordinary = queries.flow_type(
            &FlowAdapter {
                sources: &self.flow,
                values: self,
            },
            at,
            reference,
            declared,
            initial,
        );
        if let Some(projection) = self.flow.pack_projection(reference) {
            let mut narrowed = queries.flow_pack_projection(&FlowAdapter { sources: &self.flow, values: self }, at, projection, &ordinary.ty);
            narrowed.incomplete |= ordinary.incomplete;
            narrowed.unresolved_auto |= ordinary.unresolved_auto;
            narrowed
        } else if let Some(projection) = self.flow.iterator_projection(reference) {
            let mut narrowed = queries.flow_iterator_projection(&FlowAdapter { sources: &self.flow, values: self }, at, projection, &ordinary.ty);
            narrowed.incomplete |= ordinary.incomplete;
            narrowed.unresolved_auto |= ordinary.unresolved_auto;
            narrowed
        } else {
            ordinary
        }
    }
    pub(super) fn flow_environment_member_type(&self, expression: &Expr, at: NativeFlowKey, initial: &Type, queries: &mut dyn TypeQueries) -> Option<NativeFlowResult> {
        let reference = self.flow.environment_member_reference(at.graph, expression)?;
        let key = self.flow.source_reference(reference)?.path.last();
        let capability = matches!(key, Some(super::flow::Key::String(name)) if matches!(name.as_slice(), b"_G" | b"require" | b"type" | b"assert" | b"setmetatable"));
        let declared = if capability { Type::Unknown } else { initial.clone() };
        Some(queries.flow_type(&FlowAdapter { sources: &self.flow, values: self }, at, reference, &declared, initial))
    }

    pub(super) fn checker_at_flow<'a>(
        &'a self,
        at: NativeFlowKey,
        queries: &'a mut dyn TypeQueries,
    ) -> Checker<'a> {
        let contextual = self.flow.contextual(at.graph);
        let captured = self.flow.captured(at.graph);
        let owner = self.flow.function_for_graph(at.graph);
        let mut checker = self.checker(queries);
        checker.check_mode = checker
            .engine
            .current_contextual_function()
            .map_or_else(luanalysis_engine::NativeCheckMode::empty, |context| {
                context.mode
            });
        checker.type_context = owner;
        checker.contextual_function = contextual;
        checker.captured_function = captured;
        checker.flow_at = Some(at);
        if let Some(context) = contextual {
            checker.check_mode = context.mode;
        }
        if let Some(owner) = owner {
            let (parameters, _) = if let Some(context) = contextual {
                self.metadata_in_context(owner, &context, checker.engine)
            } else if let Some(captured) = captured {
                self.metadata_in_capture(owner, &captured, checker.engine)
            } else {
                self.metadata(owner, checker.engine)
            };
            self.seed_parameters(&mut checker, self.function(owner), &parameters);
        }
        checker
    }

    pub(super) fn merge_flow_report(
        &self,
        at: NativeFlowKey,
        mode: luanalysis_engine::NativeCheckMode,
        report: CheckReport,
        queries: &mut dyn TypeQueries,
    ) {
        if !super::native_expressions::publishes(mode) {
            return;
        }
        let contextual = self.flow.contextual(at.graph);
        let captured = self.flow.captured(at.graph);
        if self.body_frame_check_state(contextual.as_ref(), captured.as_ref(), queries) != NativeFunctionCheckState::Final {
            return;
        }
        if let Some(context) = contextual {
            self.contextual_report(&context, report, queries);
        } else {
            self.merge_report(report);
        }
    }

    fn with_flow_context<R>(
        &self,
        at: NativeFlowKey,
        queries: &mut dyn TypeQueries,
        work: &mut dyn FnMut(&mut dyn TypeQueries) -> R,
    ) -> R {
        let mut result = None;
        // The frozen graph owns this callback's lexical frame. Explicit None
        // selects its declaration space; it cannot mean the requesting caller's
        // unrelated capture/context. The native scope restores both afterward.
        queries.with_native_function_frame(NativeCallerFrame {
            contextual: self.flow.contextual(at.graph),
            captured: self.flow.captured(at.graph),
        }, &mut |queries| { result = Some(work(queries)); });
        result.expect("the native flow capability scope executes its callback")
    }

    fn flow_declaration_binding(
        &self,
        binding: BindingId,
        contextual: Option<&ContextualFunctionContext>,
        queries: &mut dyn TypeQueries,
    ) -> Option<Binding> {
        let write = self.flow.declaration_write(binding)?;
        let super::flow::NodeKind::Write { address, value, .. } = &write.kind else {
            unreachable!();
        };
        let captured = queries.current_captured_function();
        let graph = self
            .flow
            .graph_with_contexts(write.id.graph, contextual, captured.as_ref());
        let input = match value {
            super::flow::ValueDemand::Evaluation(evaluation)
            | super::flow::ValueDemand::PackElement { evaluation, .. } => {
                self.flow.program.evaluation(*evaluation).input
            }
            _ => address.epoch,
        };
        let at = self.flow.flow_key(input, graph.context);
        let value_key = self.flow.write_value(write)?;
        self.flow.value(value_key)?;
        // Through the engine's flow value cache: every reference to an
        // unannotated local re-evaluated its initializer here (35% of
        // Prototype Inertia time). Bun computes a local's type once.
        let value = queries.flow_value(&FlowAdapter { sources: &self.flow, values: self }, value_key, at);
        let evolves_from_nil = matches!(self.declarations.get(&binding), Some(DeclarationSource::Local { automatic: true, .. }));
        let constraint = if evolves_from_nil { queries.auto_variable_constraint() }
            else { super::inferred_local_constraint(&value, queries) };
        Some(Binding {
            initial: if evolves_from_nil { Type::Nil } else { constraint.clone() },
            value: constraint.clone(),
            evolves_from_nil,
            constraint,
        })
    }

    pub(super) fn merge_report(&self, report: CheckReport) {
        let mut provisional = self.provisional_reports.borrow_mut();
        if let Some(current) = provisional.last_mut() {
            current.append(report);
            return;
        }
        drop(provisional);
        self.report.borrow_mut().append(report);
    }

    fn with_body_report<R>(&self, work: impl FnOnce() -> (R, bool)) -> R {
        self.provisional_reports.borrow_mut().push(CheckReport::default());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
        let report = self.provisional_reports.borrow_mut().pop().expect("original body report scope");
        match result {
            Ok((result, complete)) => {
                // Completed nested bodies own checked-body cache entries. Their
                // diagnostics must survive even if an enclosing body remains
                // provisional; never merge them into its staging buffer.
                if complete { self.report.borrow_mut().append(report); }
                result
            }
            Err(error) => std::panic::resume_unwind(error),
        }
    }

    /// Records `value`'s frame as `function`'s canonical body frame when the value is created, in a
    /// publishing mode, inside its lexical parent's canonical frame.
    pub(super) fn retain_canonical_frame(&self, function: NativeFunctionKey, mode: NativeCheckMode, value: &Type, queries: &mut dyn TypeQueries) {
        if !super::native_expressions::publishes(mode) || self.canonical_frames.borrow().contains_key(&function) { return; }
        let parent = self.flow.lexical_parent(function)
            .and_then(|parent| self.canonical_frames.borrow().get(&parent).copied())
            .unwrap_or((None, None));
        if (queries.current_contextual_function(), queries.current_captured_function()) != parent { return; }
        let frame = if matches!(value, Type::NativeFunction(key) if *key == function) {
            (None, None)
        } else {
            let captured = queries.function_capture_context(value);
            (captured.as_ref().and_then(|captured| queries.captured_function_context(captured)), captured)
        };
        self.canonical_frames.borrow_mut().insert(function, frame);
    }

    pub(super) fn body_checked_once(&self, key: NativeFunctionKey) -> bool {
        self.bodies_checked.borrow().contains(&key)
    }

    /// Collects the types checking gives the expression at `span`, for the language service.
    pub(crate) fn probe_expression(&self, span: Span) {
        *self.completion_probe.borrow_mut() = Some((span, Vec::new()));
    }

    pub(crate) fn probed_types(&self) -> Vec<Type> {
        self.completion_probe.borrow_mut().take().map(|(_, types)| types).unwrap_or_default()
    }

    pub(super) fn record_probed_type(&self, span: Span, ty: &Type) {
        if let Some((probe, types)) = self.completion_probe.borrow_mut().as_mut()
            && *probe == span
            && !types.contains(ty)
        {
            types.push(ty.clone());
        }
    }

    /// The global environment of the project this source belongs to.
    pub(crate) fn global_environment(&self) -> Option<luanalysis_engine::NativeEnvironmentKey> {
        self.project_globals.as_ref().map(|globals| globals.environment)
    }

    /// The original syntax this registry checks.
    pub(crate) fn syntax(&self) -> &File {
        &self.file
    }

    /// Records a written generic reference for `check_deferred_constraints`.
    pub(super) fn defer_constraint_check(&self, span: Span, argument_spans: Vec<Span>, declaration: luanalysis_engine::NativeTypeDeclarationKey, arguments: &[Type]) {
        if self.deferred_constraint_spans.borrow_mut().insert((span.start, span.end)) {
            self.deferred_constraints.borrow_mut().push((span, argument_spans, declaration, arguments.to_vec()));
        }
    }

    /// Reports TS2344 for every recorded reference whose arguments do not satisfy their
    /// constraints, at the offending argument. Run at top level, before the file's report is
    /// taken.
    pub(crate) fn check_deferred_constraints(&self, queries: &mut dyn TypeQueries) {
        loop {
            let pending = std::mem::take(&mut *self.deferred_constraints.borrow_mut());
            if pending.is_empty() { return; }
            for (span, argument_spans, declaration, arguments) in pending {
                if let Some(error) = queries.named_type_constraint_error(declaration, &arguments) {
                    let span = match &error {
                        luanalysis_engine::NamedTypeError::TypeArgumentConstraint { index, .. } => {
                            argument_spans.get(*index).copied().unwrap_or(span)
                        }
                        _ => span,
                    };
                    self.record_named_application_error(error, span, queries);
                }
            }
        }
    }

    pub(super) fn take_report(&self) -> CheckReport {
        std::mem::take(&mut *self.report.borrow_mut())
    }

    pub(super) fn iterator_sources(&self) -> &NativeFlowSources { &self.flow }
    pub(crate) fn has_shape_attachment(&self, binding: BindingId) -> bool {
        self.shape_for_binding(binding).is_some()
    }
    pub(super) fn seed_original_global_reference_keys(&mut self, names: &[Vec<u8>]) {
        self.flow.seed_original_global_reference_keys(names);
    }

    pub(super) fn flow_sources(&self) -> &NativeFlowSources { &self.flow }

    pub(super) fn publish_binding(&self, binding: BindingId, value: &Binding) {
        if matches!(self.declarations.get(&binding), Some(DeclarationSource::GenericLoop)) { return; }
        let mut captured = value.clone();
        captured.value = captured.constraint.clone();
        self.completed_bindings
            .borrow_mut()
            .insert(binding, captured);
    }

    /// An automatic local is declared nil, unless a nested function assigns it. Its declaring
    /// function cannot see that write, so a read there that no assignment of its own reaches
    /// cannot be determined, and is `any` as in TypeScript, reported under `--no-implicit-any`.
    fn automatic_declaration_value(&self, binding: BindingId, queries: &mut dyn TypeQueries) -> Type {
        if self.binding_is_automatic(binding) && self.bound.written_in_nested_function(binding) {
            queries.auto_variable_constraint()
        } else {
            Type::Nil
        }
    }

    pub(super) fn binding_is_automatic(&self, binding: BindingId) -> bool {
        matches!(self.declarations.get(&binding), Some(DeclarationSource::Local { automatic: true, .. }))
    }

    pub(super) fn automatic_capture_type(
        &self,
        function: NativeFunctionKey,
        binding: BindingId,
        declared: &Type,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        let contextual = queries.current_contextual_function();
        let captured = queries.current_captured_function();
        let base = self.flow.graph_for_function(function, None).unwrap();
        let graph = self.flow.graph_with_contexts(base.region, contextual.as_ref(), captured.as_ref());
        let reference = self.flow.reference_key(graph, &super::flow::Reference {
            root: super::flow::ReferenceRoot::Binding(binding),
            path: Vec::new(),
        }).expect("a lexical capture has an original function reference");
        let at = self.flow.flow_key(self.flow.program.graphs[graph.region as usize].entry, graph.context);
        queries.flow_capture_type(&FlowAdapter { sources: &self.flow, values: self }, at, reference, declared, &Type::Nil).ty
    }

    pub(super) fn resolve_type_name(
        &self,
        context: Option<NativeFunctionKey>,
        name: &str,
    ) -> Option<Type> {
        self.types.resolve_name(context, name)
    }

    pub(super) fn resolve_named_type_parameter(
        &self,
        context: super::native_named::NativeNamedKey,
        name: &str,
    ) -> Option<Type> {
        match self
            .types
            .resolve_name_in_context(Some(NativeTypeContext::NamedType(context)), name)
        {
            TypeNameResolution::Parameter(key) => Some(Type::NativeTypeParameter(key)),
            TypeNameResolution::Missing | TypeNameResolution::Ambiguous(_) => None,
        }
    }

    pub(super) fn metadata(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> (Vec<Parameter>, Option<Type>) {
        let mut checker = self.checker(queries);
        checker.type_context = Some(key);
        let parts = checker.signature_parts(self.function(key));
        self.merge_report(checker.report);
        parts
    }

    /// An ambient enclosing capture cannot supply this function's formals.
    fn body_parameter_type(
        &self,
        function: NativeFunctionKey,
        index: usize,
        captured: Option<CapturedFunctionContext>,
        contextual: Option<ContextualFunctionContext>,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        let own_capture = captured.filter(|capture| {
            let value = queries.captured_function_value(capture);
            queries.native_function_key(&value) == Some(function)
        });
        own_capture
            .and_then(|capture| queries.captured_parameter_type(&capture, index))
            .or_else(|| {
                contextual.and_then(|context| queries.contextual_parameter_type(&context, index))
            })
    }

    /// Resolves a captured declaration's constraint, rather than importing
    /// transient narrowing from the time the closure happened to be created.
    pub(super) fn resolve_binding(
        &self,
        binding: BindingId,
        queries: &mut dyn TypeQueries,
    ) -> Option<Binding> {
        if let Some(key) = self.binding_function_key(binding) {
            if let Some(context) = queries.current_captured_function() {
                let value = queries.captured_function_value(&context);
                if queries.native_function_key(&value) == Some(key) {
                    return Some(Binding {
                        constraint: value.clone(),
                        initial: value.clone(),
                        value,
                        evolves_from_nil: false,
                    });
                }
            }
        }
        let function_in_facet = matches!(
            self.declarations.get(&binding),
            Some(DeclarationSource::Function(_))
        ) && (queries.current_contextual_function().is_some()
            || queries.current_captured_function().is_some());
        if !function_in_facet {
            if let Some(value) = self.completed_bindings.borrow().get(&binding).cloned() {
                return Some(value);
            }
        }
        let source = self.declarations.get(&binding)?.clone();
        let mut checker = self.checker(queries);
        checker.captured_function = checker.engine.current_captured_function();
        let (constraint, evolves_from_nil) = match source {
            DeclarationSource::Local { annotation, initializer_assertion, owner, .. } => {
                checker.type_context = owner.map(|owner| self.key(owner));
                if let Some(annotation) = annotation {
                    // The declaration already supplies the captured type.
                    // Its file/body check still validates the initializer.
                    let explicit = checker.annotation(&annotation);
                    let constraint = checker
                        .shape_binding_constraint(binding, Some(&explicit), Some(annotation.span))
                        .unwrap_or(explicit);
                    (constraint, false)
                } else if let Some((annotation, slot)) = initializer_assertion {
                    // Reading a declaration's written initializer type does not
                    // execute or revalidate that initializer. Its original
                    // expression still checks the assertion and all effects.
                    let pack = AnnotationPack::annotation(&annotation).lower(&mut |ty| checker.annotation(ty));
                    let value = checker.engine.pack_slot(&pack, slot);
                    (checker.engine.widen(&value), false)
                } else if let Some(constraint) = checker.shape_binding_constraint(binding, None, None) {
                    (constraint, false)
                } else {
                    let resolved = self.flow_declaration_binding(binding, None, checker.engine)?;
                    // A demand can run inside an active native loop query.
                    // Only completed declaration checking publishes immutable
                    // constraints; callback demands never publish provisional
                    // native flow values into a second cache.
                    return Some(Binding {
                        value: resolved.constraint.clone(),
                        ..resolved
                    });
                }
            }
            DeclarationSource::Function(index) => {
                checker.type_context = Some(self.key(index));
                let function = &self.functions[index as usize];
                let annotation = function
                    .annotations
                    .type_annotation
                    .as_ref()
                    .and_then(|annotation| annotation.types.first());
                let constraint = match annotation {
                    Some(ty) => checker.annotation(ty),
                    None => {
                        self.source_function_value(function, checker.check_mode, checker.engine)
                    }
                };
                (constraint, false)
            }
            DeclarationSource::Parameter { function, index } => {
                let (parameters, _) = self.metadata(self.key(function), checker.engine);
                let parameter = parameters
                    .get(index)
                    .expect("a bound Lua parameter has metadata");
                let parameter_type = self
                    .body_parameter_type(
                        self.key(function),
                        index,
                        checker.captured_function,
                        None,
                        checker.engine,
                    )
                    .unwrap_or_else(|| parameter.ty.clone());
                let ty = if parameter.optional && !crate::functions::untyped_parameters(&self.functions[function as usize]) {
                    Type::Union(vec![parameter_type, Type::Nil])
                } else {
                    parameter_type
                };
                (ty, false)
            }
            DeclarationSource::NumericLoop => (Type::Number, false),
            DeclarationSource::GenericLoop => {
                return self.iterator_binding_in_current_context(binding, checker.engine).map(|(binding, _)| binding);
            },
        };
        self.merge_report(checker.report);
        Some(Binding {
            initial: constraint.clone(),
            value: constraint.clone(),
            constraint,
            evolves_from_nil,
        })
    }

    pub(super) fn binding_function_key(&self, binding: BindingId) -> Option<NativeFunctionKey> {
        match self.declarations.get(&binding) {
            Some(DeclarationSource::Function(index)) => Some(self.key(*index)),
            _ => None,
        }
    }

    pub(super) fn resolve_binding_in_context(
        &self,
        binding: BindingId,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<Binding> {
        if !self.expressions.contains(context.expression) {
            return self.resolve_binding(binding, queries);
        }
        let owner = self.expressions.function_for(context.expression);
        let source = self.declarations.get(&binding)?.clone();
        match source {
            DeclarationSource::Parameter { function, index }
                if Some(self.key(function)) == owner =>
            {
                let (parameters, _) =
                    self.metadata_in_context(self.key(function), context, queries);
                let parameter = parameters.get(index)?;
                let captured = queries.current_captured_function();
                let parameter_type = self
                    .body_parameter_type(
                        self.key(function),
                        index,
                        captured,
                        Some(*context),
                        queries,
                    )
                    .unwrap_or_else(|| parameter.ty.clone());
                let ty = if parameter.optional && !crate::functions::untyped_parameters(&self.functions[function as usize]) {
                    Type::Union(vec![parameter_type, Type::Nil])
                } else {
                    parameter_type
                };
                Some(Binding {
                    constraint: ty.clone(),
                    initial: ty.clone(),
                    value: ty,
                    evolves_from_nil: false,
                })
            }
            DeclarationSource::Local {
                annotation,
                initializer_assertion,
                owner: Some(function),
                ..
            } if Some(self.key(function)) == owner => {
                let key = self.key(function);
                let mut checker = self.checker(queries);
                checker.type_context = Some(key);
                checker.contextual_function = Some(*context);
                checker.captured_function = checker.engine.current_captured_function();
                checker.check_mode = context.mode;
                if let Some(annotation) = annotation {
                    let explicit = checker.annotation(&annotation);
                    let constraint = checker
                        .shape_binding_constraint(binding, Some(&explicit), Some(annotation.span))
                        .unwrap_or(explicit);
                    self.contextual_report(context, checker.report, checker.engine);
                    return Some(Binding {
                        initial: constraint.clone(),
                        value: constraint.clone(),
                        constraint,
                        evolves_from_nil: false,
                    });
                }
                if let Some((annotation, slot)) = initializer_assertion {
                    let pack = AnnotationPack::annotation(&annotation).lower(&mut |ty| checker.annotation(ty));
                    let value = checker.engine.pack_slot(&pack, slot);
                    let constraint = checker.engine.widen(&value);
                    self.contextual_report(context, checker.report, checker.engine);
                    return Some(Binding {
                        initial: constraint.clone(), value: constraint.clone(), constraint, evolves_from_nil: false,
                    });
                }
                if let Some(constraint) = checker.shape_binding_constraint(binding, None, None) {
                    self.contextual_report(context, checker.report, checker.engine);
                    return Some(Binding {
                        initial: constraint.clone(),
                        value: constraint.clone(),
                        constraint,
                        evolves_from_nil: false,
                    });
                }
                self.flow_declaration_binding(binding, Some(context), checker.engine)
            }
            _ => self.resolve_binding(binding, queries),
        }
    }

    pub(super) fn seed_parameters(
        &self,
        checker: &mut Checker<'_>,
        function: &Function,
        parameters: &[Parameter],
    ) {
        let receiver = self.bound.implicit_receiver(function.span);
        let offset = usize::from(receiver.is_some());
        if let Some(binding) = receiver {
            let parameter = parameters.first().expect("real Lua receiver parameter");
            // A concrete member-assignment facet describes the public value,
            // while this body still owns the original polymorphic self. Keep
            // an explicit @param self contract ahead of either default. Apply
            // the same lexical capture mapper as the written return contract.
            let written_receiver = !function.params.iter().any(|parameter| parameter.text == "self")
                && function.annotations.params.iter().any(|parameter| parameter.name.text == "self");
            let parameter_type = (!written_receiver && self.has_polymorphic_method_owner(self.key_for(function)))
                .then(|| self.default_receiver_type(function, checker.engine).map(|ty| {
                    checker.captured_function.map_or(ty.clone(), |capture| checker.engine.instantiate_in_capture(&capture, &ty))
                })).flatten()
                .or_else(|| self.body_parameter_type(self.key_for(function), 0, checker.captured_function, checker.contextual_function, checker.engine))
                .unwrap_or_else(|| parameter.ty.clone());
            // A method is called after the table is built, so its `self` is the table with every
            // stored field, as a table another function's stores describe enters a function.
            let parameter_type = if written_receiver { parameter_type }
                else { checker.engine.runtime_table_schema(&parameter_type).unwrap_or(parameter_type) };
            let ty = if parameter.optional { Type::Union(vec![parameter_type, Type::Nil]) } else { parameter_type };
            checker.locals.insert(binding, Binding { constraint:ty.clone(), initial:ty.clone(), value:ty.clone(), evolves_from_nil:false });
            checker.record_semantic_binding(binding, &ty);
        }
        for (written_index, (name, parameter)) in function.params.iter().zip(parameters.iter().skip(offset)).enumerate() {
            let index = written_index + offset;
            let binding = self
                .bound
                .binding_at(name.span.start)
                .expect("bound Lua function parameter");
            let parameter_type = self
                .body_parameter_type(
                    self.key_for(function),
                    index,
                    checker.captured_function,
                    checker.contextual_function,
                    checker.engine,
                )
                .unwrap_or_else(|| parameter.ty.clone());
            let ty = if parameter.optional && !crate::functions::untyped_parameters(function) {
                Type::Union(vec![parameter_type, Type::Nil])
            } else {
                parameter_type
            };
            checker.locals.insert(
                binding,
                Binding {
                    constraint: ty.clone(),
                    initial: ty.clone(),
                    value: ty.clone(),
                    evolves_from_nil: false,
                },
            );
            checker.record_binding(name, binding, &ty);
        }
    }

    fn inferred_return(&self, key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Type {
        self.inferred_return_with_context(key, queries, None, None)
    }

    fn has_empty_declaration_result(&self, key: NativeFunctionKey) -> bool {
        if !self.declaration_functions.contains(&key.index) || self.declared_pack_recipe(key).is_some() { return false }
        let graph = self.flow.graph_for_function(key, None).expect("original declaration owns a function graph");
        self.flow.program.graphs[graph.region as usize].returns.iter().all(|&at| {
            matches!(&self.flow.program.node(at).kind, super::flow::NodeKind::Return { values } if values.is_empty())
        })
    }

    fn inferred_return_pack_with_context(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
        contextual: Option<ContextualFunctionContext>,
        captured: Option<CapturedFunctionContext>,
    ) -> ValuePack {
        // Like TS declarations, an unannotated declaration with no valued
        // return cannot infer never, independently of end reachability. Lua
        // represents that result as zero physical values. Its body is still
        // checked separately by check_all_bodies in the selected frame.
        if self.has_empty_declaration_result(key) { return ValuePack::empty() }
        let base = self
            .flow
            .graph_for_function(key, contextual.as_ref())
            .unwrap();
        let graph =
            self.flow
                .graph_with_contexts(base.region, contextual.as_ref(), captured.as_ref());
        let source = &self.flow.program.graphs[graph.region as usize];
        let context = contextual
            .as_ref()
            .map(|context| queries.contextual_return_pack(context));
        let adapter = FlowAdapter {
            sources: &self.flow,
            values: self,
        };
        let mut returned = Vec::new();
        for return_id in &source.returns {
            let statement = self.flow.program.node(*return_id);
            let recipe = self
                .packs()
                .list_at(statement.span)
                .expect("original return owns an immutable pack recipe");
            assert_eq!(
                recipe.owner,
                Some(key),
                "return pack belongs to this original function"
            );
            let at = self.flow.flow_key(statement.antecedents[0], graph.context);
            if !queries.flow_reachable(&adapter, at) {
                continue;
            }
            let super::flow::NodeKind::Return { values } = &statement.kind else {
                unreachable!();
            };
            let expressions: Vec<_> = values
                .iter()
                .map(|value| {
                    let evaluation = self.flow.program.evaluation(*value);
                    self.flow
                        .program
                        .expression(evaluation.expression)
                        .expression
                        .clone()
                })
                .collect();
            let mut checker = self.checker_at_flow(at, queries);
            returned.push(checker.return_expression_pack(&expressions, statement.span, context.as_ref()));
            if !checker.incomplete_flow {
                self.merge_flow_report(at, checker.check_mode, checker.report, checker.engine);
            }
        }
        let has_fallthrough =
            queries.flow_reachable(&adapter, self.flow.flow_key(source.exit, graph.context));
        queries.aggregate_result_packs(&returned, has_fallthrough, context.as_ref())
    }

    pub(super) fn inferred_return_in_context(
        &self,
        key: NativeFunctionKey,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        let captured = queries.current_captured_function();
        self.inferred_return_with_context(key, queries, Some(*context), captured)
    }

    pub(super) fn source_inferred_return_in_capture(
        &self,
        key: NativeFunctionKey,
        context: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        self.inferred_return_with_context(key, queries, None, Some(*context))
    }

    fn inferred_return_with_context(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
        contextual: Option<ContextualFunctionContext>,
        captured: Option<CapturedFunctionContext>,
    ) -> Type {
        if self.has_empty_declaration_result(key) { return Type::Void }
        let graph = self
            .flow
            .graph_for_function(key, contextual.as_ref())
            .expect("every original Lua function has a detached source graph");
        let graph =
            self.flow
                .graph_with_contexts(graph.region, contextual.as_ref(), captured.as_ref());
        let source = &self.flow.program.graphs[graph.region as usize];
        let (_, mut context) = if let Some(contextual) = contextual {
            self.metadata_in_context(key, &contextual, queries)
        } else if let Some(captured) = captured {
            self.metadata_in_capture(key, &captured, queries)
        } else {
            self.metadata(key, queries)
        };
        if let (Some(captured), Some(expected)) = (captured, context.as_ref()) {
            context = Some(queries.instantiate_in_capture(&captured, expected));
        }
        if context.is_none() {
            if let Some(contextual) = contextual {
                context = Some(queries.contextual_return_type(&contextual));
            }
        }
        let adapter = FlowAdapter {
            sources: &self.flow,
            values: self,
        };
        let mut returned = Vec::new();
        for return_id in &source.returns {
            let statement = self.flow.program.node(*return_id);
            let predecessor = statement.antecedents[0];
            let at = self.flow.flow_key(predecessor, graph.context);
            if !queries.flow_reachable(&adapter, at) {
                continue;
            }
            let super::flow::NodeKind::Return { values } = &statement.kind else {
                unreachable!();
            };
            let mut checker = self.checker_at_flow(at, queries);
            checker.return_type = context.clone();
            let expressions: Vec<_> = values
                .iter()
                .map(|value| {
                    let evaluation = self.flow.program.evaluation(*value);
                    self.flow
                        .program
                        .expression(evaluation.expression)
                        .expression
                        .clone()
                })
                .collect();
            checker.return_statement(&expressions, statement.span);
            returned.extend(checker.return_values.drain(..));
            self.merge_flow_report(at, checker.check_mode, checker.report, checker.engine);
        }
        let has_fallthrough =
            queries.flow_reachable(&adapter, self.flow.flow_key(source.exit, graph.context));
        if returned.is_empty() && !has_fallthrough {
            returned.push(Type::Never);
        }
        queries.aggregate_returns(&returned, has_fallthrough, context.as_ref())
    }

    fn check_body(&self, key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Type {
        self.check_body_with_context(key, queries, None, None)
    }

    pub(super) fn check_body_in_context(
        &self,
        key: NativeFunctionKey,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        let mut result = None;
        queries.with_contextual_function(context, &mut |queries| {
            let captured = queries.current_captured_function();
            result = Some(self.check_body_with_context(key, queries, Some(*context), captured));
        });
        result.expect("the native contextual body frame executes its callback")
    }

    pub(super) fn check_body_in_capture(
        &self,
        key: NativeFunctionKey,
        context: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        let mut result = None;
        queries.with_captured_function(context, &mut |queries| {
            result = Some(self.check_body_with_context(key, queries, None, Some(*context)));
        });
        result.expect("the native captured body frame executes its callback")
    }

    fn check_body_with_context(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
        contextual: Option<ContextualFunctionContext>,
        captured: Option<CapturedFunctionContext>,
    ) -> Type {
        let mark = queries.argument_results_mark();
        let (ty, complete) = self.with_body_report(|| {
            let (ty, complete) = self.check_body_report(key, queries, contextual, captured);
            ((ty, complete), complete)
        });
        // The discarded report held what those arguments reported.
        if !complete { queries.discard_argument_results_since(mark); }
        ty
    }

    fn check_body_report(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
        contextual: Option<ContextualFunctionContext>,
        captured: Option<CapturedFunctionContext>,
    ) -> (Type, bool) {
        let pending_before = self.pending_owner_observations.get();
        let function = self.function(key);
        let (parameters, mut declared_return) = if let Some(context) = contextual {
            self.metadata_in_context(key, &context, queries)
        } else if let Some(captured) = captured {
            self.metadata_in_capture(key, &captured, queries)
        } else {
            self.metadata(key, queries)
        };
        if let (Some(captured), Some(expected)) = (captured, declared_return.as_ref()) {
            declared_return = Some(queries.instantiate_in_capture(&captured, expected));
        }
        let mut checker = self.checker(queries);
        checker.type_context = Some(key);
        checker.contextual_function = contextual;
        checker.captured_function = captured;
        if let Some(context) = contextual {
            checker.check_mode = context.mode;
        }
        checker.return_type = declared_return.clone();
        checker.return_pack = self
            .declared_pack_contract(key, checker.engine)
            .map(|mut pack| {
                pack = checker.engine.declared_result_pack(&pack);
                pack = checker.engine.pack_with_implicit_empty(&pack);
                if let Some(captured) = captured {
                    pack = checker.engine.instantiate_pack_in_capture(&captured, &pack);
                }
                pack
            });
        checker.flow_at = Some(self.flow_entry(Some(key), contextual.as_ref()));
        let at = checker.flow_at.unwrap();
        checker.flow_at = Some(NativeFlowKey {
            graph: self.flow.graph_with_contexts(
                at.graph.region,
                contextual.as_ref(),
                captured.as_ref(),
            ),
            node: at.node,
        });
        self.seed_parameters(&mut checker, function, &parameters);
        checker.statements(&function.body);
        let graph = checker.flow_at.unwrap().graph;
        let terminated = !checker.engine.flow_reachable(
            &FlowAdapter {
                sources: &self.flow,
                values: self,
            },
            self.flow.flow_key(
                self.flow.program.graphs[graph.region as usize].exit,
                graph.context,
            ),
        );
        if let Some(expected) = &declared_return {
            let accepts_empty = checker
                .return_pack
                .as_ref()
                .is_some_and(|pack| checker.engine.pack_assignable(&ValuePack::empty(), pack));
            // In a LuaLS `---@meta` file, a function with an empty body only declares its signature.
            let declared_only = function.body.is_empty() && self.file.meta().is_some();
            if !terminated && !checker.engine.allows_implicit_return(expected) && !accepts_empty && !declared_only {
                let origin = self
                    .declared_pack_recipe(key)
                    .map_or(function.span, |recipe| recipe.span);
                let (code, message) = if matches!(expected, Type::Never) {
                    (
                        "TS2534",
                        "A function returning never cannot have a reachable end point.",
                    )
                } else {
                    (
                        "TS2355",
                        "A function whose declared type is neither nil, void, nor any must return a value.",
                    )
                };
                checker.error(code, origin, message);
            }
        }
        let result = checker.engine.aggregate_returns(
            &checker.return_values,
            !terminated,
            declared_return.as_ref(),
        );
        // A contextual owner can depend on an open native flow query. The
        // provisional body is neither a diagnostic result nor a completed
        // validation; a later demand must check it in the completed context.
        if checker.incomplete_flow || self.pending_owner_observations.get() != pending_before {
            self.pending_owner_bodies.borrow_mut().insert((key, contextual, captured));
            return (result, false);
        }
        self.pending_owner_bodies.borrow_mut().remove(&(key, contextual, captured));
        if let Some(context) = contextual {
            if super::native_expressions::publishes(context.mode)
                && checker.engine.contextual_function_check_state(&context) == NativeFunctionCheckState::Final
            {
                self.bodies_checked.borrow_mut().insert(key);
            }
            self.contextual_report(&context, checker.report, checker.engine);
            self.checked_contextual_bodies
                .borrow_mut()
                .insert((key, context, captured));
        } else {
            self.merge_report(checker.report);
            self.bodies_checked.borrow_mut().insert(key);
            if let Some(captured) = captured {
                self.checked_captured_bodies
                    .borrow_mut()
                    .insert((key, captured));
            } else {
                self.checked_bodies.borrow_mut().insert(key);
            }
        }
        (result, true)
    }

    /// Demand every body for semantic diagnostics, including unused annotated
    /// declarations whose return type required no native inference query.
    pub(super) fn check_all_bodies(&self, queries: &mut dyn TypeQueries) {
        self.check_all_signatures(queries);
        self.check_function_annotations(queries);
        self.check_bodies(queries, false);
    }

    /// Parameter-tag diagnostics belong to the function's declaration, as in
    /// TypeScript's signature declaration check: the file's own check
    /// publishes them once. Otherwise they appeared only if some lowering whose
    /// report is published (rendering a type, not a body check that stays
    /// provisional) happened to request the function's metadata.
    fn check_function_annotations(&self, queries: &mut dyn TypeQueries) {
        for index in 0..self.functions.len() {
            let key = self.key(index as u32);
            let mut checker = self.checker(queries);
            checker.type_context = Some(key);
            checker.check_parameter_annotations(self.function(key));
            self.report.borrow_mut().append(checker.report);
        }
    }

    /// Selected foreign callbacks retain task-local inference and capture
    /// frames. Validate those frames without starting unrelated source bodies.
    pub(super) fn check_selected_bodies(&self, queries: &mut dyn TypeQueries) {
        self.check_bodies(queries, true);
    }

    fn check_bodies(&self, queries: &mut dyn TypeQueries, selected_only: bool) {
        for index in 0..self.functions.len() {
            let key = self.key(index as u32);
            if selected_only && self.expressions.final_facets(key).is_empty() { continue; }
            for parameter in self.types.own_parameters(key) {
                queries.type_parameter_constraint(parameter);
            }
            if self.bodies_checked.borrow().contains(&key) { continue; }
            // As TypeScript, the body is checked once, in the frame its value has where it is
            // written (`canonical_frames`); other instances of the same function are not checked.
            let canonical = self.canonical_frames.borrow().get(&key).copied();
            if let Some((contextual, captured)) = canonical
                && (contextual.is_some() || captured.is_some())
            {
                if self.body_frame_check_state(contextual.as_ref(), captured.as_ref(), queries) != NativeFunctionCheckState::Final { continue; }
                match (contextual, captured) {
                    (Some(context), Some(captured)) => queries.with_captured_function(&captured, &mut |queries| {
                        self.check_body_in_context(key, &context, queries);
                    }),
                    (Some(context), None) => { self.check_body_in_context(key, &context, queries); }
                    (None, Some(captured)) => { self.check_body_in_capture(key, &captured, queries); }
                    (None, None) => unreachable!(),
                }
                continue;
            }
            let facets = self.expressions.final_facets(key);
            if !facets.is_empty() {
                // The selected session capability owns this callback's
                // parameter/return caches. Demanding its base signature would
                // invent an unrelated untyped declaration and bogus TS7006.
                let mut final_facet = false;
                for facet in facets {
                    if queries.function_check_state(&facet) != NativeFunctionCheckState::Final { continue; }
                    final_facet = true;
                    // Retain the native implementation frame directly. A
                    // printer demand can re-run still-provisional metadata and
                    // publish its annotation errors on every report-drain pass.
                    self.retain_selected_body_frame(key, &facet, queries);
                }
                // An expired inference assignment is not an uncontextual
                // declaration. A later committed source demand supplies its
                // own facet; never invent a raw callback check here.
                if !final_facet { continue; }
                let mut final_body_frame = false;
                for context in self.expressions.final_contexts(key) {
                    let captures = self.expressions.captures_for_context(key, &context);
                    let facets = if captures.is_empty() { vec![None] } else { captures.into_iter().map(Some).collect() };
                    for captured in facets {
                        if self.body_frame_check_state(Some(&context), captured.as_ref(), queries) != NativeFunctionCheckState::Final { continue; }
                        final_body_frame = true;
                        if self.checked_contextual_bodies.borrow().contains(&(key, context, captured)) { continue; }
                        if self.bodies_checked.borrow().contains(&key) { break; }
                        if let Some(captured) = captured {
                            queries.with_captured_function(&captured, &mut |queries| {
                                self.check_body_in_context(key, &context, queries);
                            });
                        } else {
                            self.check_body_in_context(key, &context, queries);
                        }
                    }
                }
                // A function whose only frames are captures (a closure created once per outer
                // instance) is checked once in declaration space below, as TypeScript checks a
                // body once: its upvalues resolve through the lexical owner chain to their
                // declared types, whichever instances exist.
                if final_body_frame {
                    continue;
                }
                // A selected raw declaration, or an unapplied contextual
                // candidate, still owns a private implementation body. Written
                // public overload heads alone cannot establish its diagnostics.
            }
            // Enter the native QueryReturn guard/cache for inferred returns.
            // An annotated return needs no body query; validate it below.
            queries.native_function_signature(key);
            let checked = self.checked_bodies.borrow().contains(&key);
            if !checked {
                self.check_body(key, queries);
            }
        }
    }

    pub(super) fn function_type_dependencies(
        &self,
        key: NativeFunctionKey,
        used: &mut HashSet<NativeTypeParameterKey>,
        functions: &mut HashSet<NativeFunctionKey>,
        bindings: &mut HashSet<BindingId>,
    ) {
        if !functions.insert(key) {
            return;
        }
        if let Some(method) = self.methods.get(key) {
            if method.implicit_receiver.is_some() {
                if method.contextual_owner.is_some() && let Some(parameter) = self.contextual_self_dependency(key) { used.insert(parameter); }
                if let Some(owner) = self.method_owner(key) {
                    if let Some(parameter) = self.types.self_parameter(owner) { used.insert(parameter); }
                    used.extend(self.types.own_parameters_in_context(NativeTypeContext::NamedType(owner)));
                }
            }
        }
        let function = self.function(key);
        for parameter in &function.annotations.generics {
            if let Some(constraint) = &parameter.constraint {
                self.function_annotation_parameter_dependencies(Some(key), constraint, used);
            }
        }
        for parameter in &function.annotations.params {
            self.function_annotation_parameter_dependencies(Some(key), &parameter.ty, used);
        }
        if let Some(vararg) = &function.annotations.vararg {
            self.function_annotation_parameter_dependencies(Some(key), &vararg.ty, used);
        }
        if let Some(annotation) = &function.annotations.type_annotation {
            for ty in &annotation.types {
                self.function_annotation_parameter_dependencies(Some(key), ty, used);
            }
        }
        if let Some(annotation) = &function.annotations.returns {
            for ty in &annotation.types {
                self.function_annotation_parameter_dependencies(Some(key), ty, used);
            }
        }
        let mut statements = Vec::new();
        source_statements(&function.body, &mut statements);
        for statement in statements {
            if let StmtKind::Return(values) = &statement.kind {
                if let Some(annotation) = self.packs().return_annotation(statement.span) {
                    for ty in &annotation.types {
                        self.function_annotation_parameter_dependencies(Some(key), ty, used);
                    }
                }
                if function.annotations.returns.is_none() {
                    for value in values {
                        self.value_type_dependencies(value, used, functions, bindings);
                    }
                }
            }
        }
    }

    fn binding_type_dependencies(
        &self,
        binding: BindingId,
        used: &mut HashSet<NativeTypeParameterKey>,
        functions: &mut HashSet<NativeFunctionKey>,
        bindings: &mut HashSet<BindingId>,
    ) {
        if !bindings.insert(binding) {
            return;
        }
        let Some(source) = self.declarations.get(&binding) else {
            return;
        };
        match source {
            DeclarationSource::Local {
                annotation: Some(annotation),
                owner,
                ..
            } => self.function_annotation_parameter_dependencies(
                owner.map(|owner| self.key(owner)),
                annotation,
                used,
            ),
            DeclarationSource::Local {
                annotation: None,
                owner,
                ..
            } => {
                let body = owner.map_or(self.file.statements.as_slice(), |owner| {
                    self.functions[owner as usize].body.as_slice()
                });
                let mut statements = Vec::new();
                source_statements(body, &mut statements);
                for statement in statements {
                    if let StmtKind::Local { names, values, .. } = &statement.kind {
                        if let Some(index) = names.iter().position(|name| {
                            self.bound.binding_at(name.span.start) == Some(binding)
                        }) {
                            if let Some(value) = values.get(index) {
                                self.value_type_dependencies(value, used, functions, bindings);
                            }
                            break;
                        }
                    }
                }
            }
            DeclarationSource::Function(index) => {
                self.function_type_dependencies(self.key(*index), used, functions, bindings)
            }
            DeclarationSource::Parameter { function, index } => {
                let owner = self.key(*function);
                let function = self.function(owner);
                let offset = usize::from(self.bound.implicit_receiver(function.span).is_some());
                if *index < offset {
                    let tag = (!function.params.iter().any(|parameter| parameter.text == "self"))
                        .then(|| function.annotations.params.iter().find(|tag| tag.name.text == "self")).flatten();
                    if let Some(tag) = tag {
                        self.function_annotation_parameter_dependencies(Some(owner), &tag.ty, used);
                    } else if let Some(method) = self.methods.get(owner) {
                        if let Some(class) = self.method_owner(owner) {
                            if let Some(parameter) = self.types.self_parameter(class) { used.insert(parameter); }
                            used.extend(self.types.own_parameters_in_context(NativeTypeContext::NamedType(class)));
                        } else {
                            self.value_type_dependencies(&method.receiver, used, functions, bindings);
                        }
                    }
                    return;
                }
                let name = &function.params[*index - offset];
                if let Some(tag) = function
                    .annotations
                    .params
                    .iter()
                    .find(|tag| tag.name.text == name.text)
                {
                    self.function_annotation_parameter_dependencies(Some(owner), &tag.ty, used);
                } else if let Some(context) = &function.annotations.type_annotation {
                    for ty in &context.types {
                        self.function_annotation_parameter_dependencies(Some(owner), ty, used);
                    }
                }
            }
            DeclarationSource::NumericLoop | DeclarationSource::GenericLoop => {}
        }
    }

    pub(super) fn value_type_dependencies(
        &self,
        expression: &Expr,
        used: &mut HashSet<NativeTypeParameterKey>,
        functions: &mut HashSet<NativeFunctionKey>,
        bindings: &mut HashSet<BindingId>,
    ) {
        let owner = self.expressions.get(self.expression_key(expression)).owner;
        for assertion in self.expression_assertions(expression.span) {
            for ty in &assertion.annotation.types {
                self.function_annotation_parameter_dependencies(owner, ty, used);
            }
        }
        match &expression.kind {
            ExprKind::Name(name) => {
                if let Some(binding) = self.bound.binding_at(name.span.start) {
                    self.binding_type_dependencies(binding, used, functions, bindings);
                }
            }
            ExprKind::Function(function) => {
                self.function_type_dependencies(self.key_for(function), used, functions, bindings)
            }
            ExprKind::Parenthesized(inner) | ExprKind::Unary { value: inner, .. } => {
                self.value_type_dependencies(inner, used, functions, bindings)
            }
            ExprKind::Field { base, .. } => {
                self.value_type_dependencies(base, used, functions, bindings)
            }
            ExprKind::Index { base, index }
            | ExprKind::Binary {
                left: base,
                right: index,
                ..
            } => {
                self.value_type_dependencies(base, used, functions, bindings);
                self.value_type_dependencies(index, used, functions, bindings);
            }
            ExprKind::Table(fields) => {
                for field in fields {
                    if let Some(annotation) = &field.annotation {
                        for ty in &annotation.types {
                            self.function_annotation_parameter_dependencies(owner, ty, used);
                        }
                    }
                    if let luanalysis_syntax::TableFieldKind::Keyed(key) = &field.kind {
                        self.value_type_dependencies(key, used, functions, bindings);
                    }
                    self.value_type_dependencies(&field.value, used, functions, bindings);
                }
            }
            ExprKind::Call { callee, args } => {
                self.value_type_dependencies(callee, used, functions, bindings);
                for argument in args {
                    self.value_type_dependencies(argument, used, functions, bindings);
                }
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                self.value_type_dependencies(receiver, used, functions, bindings);
                for argument in args {
                    self.value_type_dependencies(argument, used, functions, bindings);
                }
            }
            ExprKind::Nil
            | ExprKind::Boolean(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Vararg
            | ExprKind::Error => {}
        }
    }

    fn cycle_diagnostic(&self, key: NativeFunctionKey, too_deep: bool) {
        // As in TypeScript, a return type inferred from itself is `any`, which only
        // `--no-implicit-any` reports.
        if !too_deep && !self.no_implicit_any {
            return;
        }
        let function = self.function(key);
        let name = self.function_names[key.index as usize].as_ref();
        let (code, message) = if too_deep {
            (
                "TS2589",
                "Type instantiation is excessively deep and possibly infinite.".to_owned(),
            )
        } else if let Some(name) = name {
            (
                "TS7023",
                format!(
                    "Function '{}' implicitly has return type 'any' because it is referenced directly or indirectly in one of its return expressions.",
                    name.text
                ),
            )
        } else {
            ("TS7024", "Function implicitly has return type 'any' because it does not have a return type annotation and is referenced directly or indirectly in one of its return expressions.".to_owned())
        };
        self.report.borrow_mut().diagnostics.push(Diagnostic {
            code,
            message,
            span: name.map_or(function.span, |name| name.span),
        });
    }
}

impl NativeRegistry {
    /// Restore the original declaration/assignment context without changing the
    /// source graph or publishing a mutable last-call context. Native pack
    /// filtering owns the remaining-row guidance for an expanded final RHS.
    fn flow_initializer_context(
        &self,
        expression: &Expr,
        expanded: bool,
        checker: &mut Checker<'_>,
    ) -> Option<ValuePack> {
        let key = self.expression_key(expression);
        let (list, source, index) = self.packs.initializer_for_expression(key)?;
        debug_assert_eq!(list.owner, checker.type_context, "initializer retains its original type scope");
        let expected = match source {
            InitializerContext::Local(annotation) => {
                let expected = AnnotationPack::annotation(annotation)
                    .lower(&mut |ty| checker.annotation(ty));
                if let Some(directive) = self.shape_for_initializer(expression.span) {
                    let explicit = checker.engine.pack_slot(&expected, 0);
                    ValuePack::single(checker
                        .shape_binding_constraint(directive.binding, Some(&explicit), Some(annotation.span))
                        .unwrap_or(explicit))
                } else {
                    expected
                }
            }
            InitializerContext::LocalDeclarations(names) => {
                let binding = names.first()
                    .and_then(|name| checker.bound.binding_at(name.span.start))?;
                let context = checker.shape_binding_constraint(binding, None, None)
                    .or_else(|| checker.class_definition_initializer_context(binding))?;
                ValuePack::single(context)
            }
            InitializerContext::Assignment(targets) => {
                let fixed = targets.iter().map(|target| {
                    let ty = checker.assignment_target_context(target);
                    luanalysis_engine::PackSlot::required(ty.unwrap_or(Type::Any))
                }).collect();
                ValuePack::Alternatives(vec![luanalysis_engine::PackAlternative { fixed, tail: None }])
            }
        };
        let slot_context = checker.engine.pack_contextual_slot(&expected, index)?;
        if !expanded {
            return Some(ValuePack::single(slot_context));
        }
        let mut prefix = Vec::with_capacity(index);
        for (slot, expression) in list.expressions.iter().take(index).enumerate() {
            let context = checker.engine.pack_contextual_slot(&expected, slot);
            let expression = &self.expressions().get(expression.expression).expression;
            prefix.push(checker.expression_with_expected(expression, context.as_ref()));
        }
        checker.engine.pack_contextual_tail(&expected, &prefix)
    }

    fn flow_value_type_in_context(
        &self,
        sources: &NativeFlowSources,
        value: &ValueRecipe,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
        mode: NativeCheckMode,
    ) -> Type {
        match value {
            ValueRecipe::Nil => Type::Nil,
            ValueRecipe::DeclarationNil(binding) => self.automatic_declaration_value(*binding, queries),
            ValueRecipe::PackElement { evaluation, index } => {
                let expression = &sources
                    .program
                    .expression(sources.program.evaluation(*evaluation).expression)
                    .expression;
                let mut checker = self.checker_at_flow(input, queries);
                checker.check_mode |= mode;
                checker.runtime_value = true;
                let expected = self.flow_initializer_context(expression, true, &mut checker);
                let pack = checker.expression_pack(expression, expected.as_ref());
                let value = checker.engine.pack_slot(&pack, *index);
                if !checker.incomplete_flow {
                    self.merge_flow_report(input, checker.check_mode, checker.report, checker.engine);
                }
                value
            }
            ValueRecipe::Function(region) => {
                let key = sources
                    .function_for_graph(sources.graph(*region, 0))
                    .expect("an original function graph has a source declaration");
                let mode = mode | sources
                    .contextual(input.graph)
                    .map_or_else(luanalysis_engine::NativeCheckMode::empty, |context| {
                        context.mode
                    });
                self.source_function_value(self.function(key), mode, queries)
            }
            ValueRecipe::NumericIteration => Type::Number,
            ValueRecipe::IteratorHeader(iterator) => {
                let pack = self.source_iterator_header(*iterator, input, queries);
                queries.pack_slot(&pack, 0)
            }
            ValueRecipe::IteratorResult { iterator, index } => self.source_iterator_result(*iterator, *index, input, queries).ty,
            ValueRecipe::Evaluation(evaluation) => {
                let evaluation = sources.program.evaluation(*evaluation);
                let expression = &sources.program.expression(evaluation.expression).expression;
                let mut checker = self.checker_at_flow(input, queries);
                checker.check_mode |= mode;
                checker.runtime_value = true;
                let expected = self.flow_initializer_context(expression, false, &mut checker);
                let expected = expected.as_ref().map(|pack| checker.engine.pack_slot(pack, 0));
                let result = checker.expression_with_expected(expression, expected.as_ref());
                if !checker.incomplete_flow {
                    self.merge_flow_report(input, checker.check_mode, checker.report, checker.engine);
                }
                result
            }
        }
    }

    fn flow_call_effects_in_context(
        &self,
        sources: &NativeFlowSources,
        value: &ValueRecipe,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> NativeCallEffects {
        let ValueRecipe::Evaluation(evaluation) = value else {
            return NativeCallEffects::None;
        };
        let evaluation = sources.program.evaluation(*evaluation);
        let expression = &sources.program.expression(evaluation.expression).expression;
        // The import query owns its result. Reachability must not ask a
        // builtin's source spelling for a function signature or infer a
        // module's result merely to classify an unused call.
        if self.module_import(expression.span).is_some() {
            return NativeCallEffects::None;
        }
        if let ExprKind::Call { .. } = &expression.kind
            && let Some(callee) = evaluation.operands.first().copied()
        {
            let callee = sources.program.evaluation(callee);
            match queries.flow_standard_operation(
                &FlowAdapter { sources, values: self },
                sources.evaluation_value(callee.id),
                sources.flow_key(callee.input, input.graph.context),
            ) {
                luanalysis_engine::NativeStandardOperation::Standard(site) => return match site.kind {
                    luanalysis_engine::NativeIntrinsicKind::Assert => sources.assertion_call_effect(evaluation, input),
                    _ => NativeCallEffects::None,
                },
                // The native alias query taints its dependent memo scopes;
                // this provisional value cannot publish a completed effect.
                luanalysis_engine::NativeStandardOperation::Pending => return NativeCallEffects::None,
                luanalysis_engine::NativeStandardOperation::Ordinary => {},
            }
        }
        // As in Bun (`getEffectsSignature` with `getTypeOfDottedName`), a call's
        // effect comes only from the explicit, written contract of its target.
        // A target without one has no effect; its expression is never
        // evaluated here (that evaluation was 63% of Prototype Inertia time).
        let Some(callee) = self.original_effect_call_contract(expression, queries) else {
            return NativeCallEffects::None;
        };
        // An authoritative written overload set can have no single scalar
        // result without lacking a written effect contract. Only the native
        // all-never result terminates this continuation. Any Pending native
        // dependency still taints the enclosing query.
        if matches!(queries.function_declared_scalar_return(&callee), Some(Type::Never)) {
            NativeCallEffects::Never
        } else {
            NativeCallEffects::None
        }
    }
}

impl FlowValueQueries for NativeRegistry {
    fn runtime_table_store_in_scope(&self, declaration: luanalysis_engine::NativeTypeDeclarationKey, at: NativeFlowKey) -> bool {
        self.runtime_store_in_scope(declaration, at)
    }
    fn binding_has_written_contract(&self, binding: BindingId) -> bool {
        NativeRegistry::binding_has_written_contract(self, binding)
    }
    fn alias_value_type(&self, sources:&NativeFlowSources, value:&ValueRecipe, input:NativeFlowKey, queries:&mut dyn TypeQueries)->Type {
        self.with_flow_context(input, queries, &mut |queries| {
            self.flow_value_type_in_context(sources,value,input,queries,NativeCheckMode::TYPE_ONLY)
        })
    }
    fn written_binding_contract(&self, binding: BindingId, queries: &mut dyn TypeQueries) -> Option<Type> {
        self.original_written_binding_contract(binding, queries)
    }
    fn original_alias_initial(&self, reference: &super::flow::Reference) -> luanalysis_engine::NativeAliasInitial {
        self.source_alias_initial(reference)
    }
    fn loader_never_mutated(&self) -> bool {
        self.project_globals.as_ref().is_some_and(|globals| !globals.loader_mutated())
    }
    fn lexical_standard(&self, sources: &NativeFlowSources, evaluation: super::flow::EvaluationId, graph: luanalysis_engine::NativeGraphKey) -> luanalysis_engine::NativeLexicalStandard {
        let Some(globals) = &self.project_globals else { return luanalysis_engine::NativeLexicalStandard::Unknown; };
        sources.lexical_standard(&self.bound, globals.replaced_standard_names(), evaluation, graph)
    }
    fn original_alias_execution(&self, function: NativeFunctionKey, creation: NativeFlowKey) -> Option<luanalysis_engine::NativeAliasExecution> {
        self.source_alias_execution(function, creation)
    }
    fn environment_access_path(&self, expression: &Expr, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> Option<Vec<super::flow::Key>> { self.frozen_environment_path(expression, input, queries) }
    fn original_type_operation(&self, value: &ValueRecipe, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> bool {
        if self.legacy_builtin_policy == super::builtins::LegacyBuiltinPolicy::MinimalTypeCompatibility {
            let ValueRecipe::Evaluation(value) = value else { return false; };
            let expression = &self.flow.program.expression(self.flow.program.evaluation(*value).expression).expression;
            let mut expression = expression;
            while let ExprKind::Parenthesized(inner) = &expression.kind { expression = inner; }
            return matches!(expression.kind, ExprKind::Name(_)) && self.bound.standard_candidate_at(expression.span.start) == Some(super::bind::BuiltinIntrinsic::LuaType);
        }
        let ValueRecipe::Evaluation(evaluation) = value else { return false; };
        // The static callable view can be an asserted ordinary function shape.
        // Runtime identity comes from its original value and frozen input,
        // including saved aliases and intervening replacements.
        self.with_flow_context(input, queries, &mut |queries| {
            matches!(queries.flow_standard_operation(
                &FlowAdapter { sources: &self.flow, values: self },
                self.flow.evaluation_value(*evaluation),
                input,
            ), luanalysis_engine::NativeStandardOperation::Standard(site)
                if site.kind == luanalysis_engine::NativeIntrinsicKind::Type)
        })
    }
    fn environment_value(&self, binding: Option<BindingId>, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> Type {
        let mut checker = self.checker_at_flow(input, queries);
        let offset = binding.map_or_else(|| self.flow.source_node(input).map_or(self.file.span.start, |node| node.span.start), |binding| self.bound.declaration(binding).activation);
        // Use the original lexical binding supplied by the source address;
        // offsets at a store's epoch can precede its declaration activation.
        if let Some(binding) = binding {
            let Some(value) = checker.resolve_local(binding).cloned() else { return Type::Unknown; };
            let Some(reference) = self.flow.environment_reference(input.graph, Some(binding)) else { return value.initial; };
            return checker.engine.flow_type(&FlowAdapter { sources: &self.flow, values: self }, input, reference, &value.constraint, &value.initial).ty;
        }
        checker.environment_value(offset)
    }
    fn report_iterator_error(
        &self,
        at: NativeFlowKey,
        stage: luanalysis_engine::NativeIteratorStage,
        callee: &Type,
        error: luanalysis_engine::CallError,
        queries: &mut dyn TypeQueries,
    ) {
        let Some(node) = self.flow.source_node(at) else { return; };
        let super::flow::NodeKind::Iteration { iterator, .. } = &node.kind else { return; };
        let recipe = self.flow.program.iterator(*iterator);
        let expressions: Vec<_> = recipe.values.iter().map(|value| {
            &self.flow.program.expression(self.flow.program.evaluation(*value).expression).expression
        }).collect();
        let origin = |slot: usize| {
            let Some((last, prefix)) = expressions.split_last() else { return recipe.span; };
            if let Some(expression) = prefix.get(slot) { return expression.span; }
            if slot == prefix.len() || super::native_packs::adjustment(last, true) == super::native_packs::Adjustment::Expand { last.span } else { recipe.span }
        };
        let callee_origin = origin(0);
        let arguments = if stage == luanalysis_engine::NativeIteratorStage::First {
            vec![origin(1), origin(2)]
        } else {
            // The invariant state is the same frozen header argument on every
            // call. Only the control comes from the previous iterator result.
            vec![origin(1), callee_origin]
        };
        let feedback = stage == luanalysis_engine::NativeIteratorStage::Next && matches!(&error, luanalysis_engine::CallError::Argument { index: 1, .. });
        let mut checker = self.checker_at_flow(at, queries);
        let first_diagnostic = checker.report.diagnostics.len();
        checker.call_result(luanalysis_engine::CallResult { return_type: Type::Any, errors: vec![error] }, callee, &arguments, callee_origin, callee_origin);
        if feedback {
            for index in first_diagnostic..checker.report.diagnostics.len() {
                checker.report.append_diagnostic_note(index, " The previous first iterator result supplies the next control argument.");
            }
        }
        self.merge_flow_report(at, checker.check_mode, checker.report, checker.engine);
    }

    fn flow_value_pack(&self, sources: &NativeFlowSources, value: &ValueRecipe, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> ValuePack {
        self.with_flow_context(input, queries, &mut |queries| {
            if let ValueRecipe::IteratorHeader(iterator) = value {
                return self.source_iterator_header(*iterator, input, queries);
            }

            let ValueRecipe::Evaluation(evaluation) = value else {
                return ValuePack::single(self.flow_value_type_in_context(sources, value, input, queries, NativeCheckMode::empty()));
            };
            let evaluation = sources.program.evaluation(*evaluation);
            let expression = &sources.program.expression(evaluation.expression).expression;
            let mut checker = self.checker_at_flow(input, queries);
            let expected = self.flow_initializer_context(expression, true, &mut checker);
            let pack = checker.expression_pack(expression, expected.as_ref());
            if !checker.incomplete_flow { self.merge_flow_report(input, checker.check_mode, checker.report, checker.engine); }
            pack
        })
    }
    fn flow_value_type(
        &self,
        sources: &NativeFlowSources,
        value: &ValueRecipe,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        self.with_flow_context(input, queries, &mut |queries| {
            self.flow_value_type_in_context(sources, value, input, queries, NativeCheckMode::empty())
        })
    }
    fn flow_call_effects(
        &self,
        sources: &NativeFlowSources,
        value: &ValueRecipe,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> NativeCallEffects {
        self.with_flow_context(input, queries, &mut |queries| {
            self.flow_call_effects_in_context(sources, value, input, queries)
        })
    }
    fn report_flow_error(&self, span: Span, error: NativeFlowError) {
        let (code, message) = match error {
            NativeFlowError::Unsupported(reason) => ("LUA2020", reason.to_owned()),
            NativeFlowError::TooDeep(_) | NativeFlowError::StepLimit(_) => (
                "LUA2019",
                "native Lua flow query exceeded its supported complexity".to_owned(),
            ),
            _ => panic!("invalid original Lua flow provider identity: {error:?}"),
        };
        self.report.borrow_mut().diagnostics.push(Diagnostic {
            code,
            span,
            message,
        });
    }
}

impl NativeTypeProvider for NativeRegistry {
    fn contextual_type_owner(&self, scope: NativeFunctionKey, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> luanalysis_engine::ContextualTypeOwner {
        self.source_contextual_type_owner(scope, input, queries)
    }
    fn type_member_activation(&self, member: luanalysis_engine::NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> luanalysis_engine::TypeMemberActivation {
        self.source_member_activation(member, queries)
    }
    fn global_environment_binding(&self, environment: luanalysis_engine::NativeEnvironmentKey, name: &[u8]) -> Option<luanalysis_engine::NativeGlobalBindingKey> {
        let globals = self.project_globals.as_ref()?;
        if environment != globals.environment { return None; }
        globals.binding(std::str::from_utf8(name).ok()?)
    }
    fn global_environment_bindings(&self, environment: luanalysis_engine::NativeEnvironmentKey) -> Vec<(Vec<u8>, luanalysis_engine::NativeGlobalBindingKey)> {
        self.project_globals.as_ref().filter(|globals| globals.environment == environment)
            .map_or_else(Vec::new, |globals| globals.bindings())
    }
    fn global_environment_self_binding(&self, _: luanalysis_engine::NativeEnvironmentKey) -> Option<luanalysis_engine::NativeGlobalBindingKey> {
        // A standalone source registry owns no selected standard-library
        // declaration provenance. A user value named `_G` is an ordinary value.
        None
    }
    fn contains_global_environment(&self, key: luanalysis_engine::NativeEnvironmentKey) -> bool {
        self.project_globals.as_ref().is_some_and(|globals| globals.environment == key)
    }
    fn contains_global_binding(&self, key: luanalysis_engine::NativeGlobalBindingKey) -> bool {
        self.project_globals.as_ref().is_some_and(|globals| globals.contains(key))
    }
    fn global_binding_declarations(&self, key: luanalysis_engine::NativeGlobalBindingKey) -> Vec<luanalysis_engine::NativeValueDeclarationKey> {
        self.project_globals.as_ref().map_or_else(Vec::new, |globals| globals.declarations(key))
    }
    fn contains_value_declaration(&self, key: luanalysis_engine::NativeValueDeclarationKey) -> bool {
        self.contains_global_declaration(key)
    }
    fn value_declaration_header(&self, key: luanalysis_engine::NativeValueDeclarationKey) -> luanalysis_engine::ValueDeclarationHeader {
        self.global_header(key)
    }
    fn value_declaration_contract(&self, key: luanalysis_engine::NativeValueDeclarationKey, queries: &mut dyn TypeQueries) -> Option<Type> {
        self.global_contract(key, queries)
    }
    fn value_declaration_signatures(&self,key:luanalysis_engine::NativeValueDeclarationKey)->Option<Vec<luanalysis_engine::NativeSignatureKey>> { self.global_declaration_signatures(key) }
    fn value_declaration_function_signatures(&self,key:luanalysis_engine::NativeValueDeclarationKey)->Option<Vec<luanalysis_engine::NativeSignatureKey>> { self.global_function_signatures(key) }
    fn value_declaration_initializer(&self, key: luanalysis_engine::NativeValueDeclarationKey, queries: &mut dyn TypeQueries) -> Type {
        self.global_initializer(key, queries)
    }
    fn global_query_error(&self, error: luanalysis_engine::GlobalQueryError, _: &mut dyn TypeQueries) {
        let mut declarations = globals::query_declarations(&error);
        if let luanalysis_engine::GlobalQueryError::CircularQuery(luanalysis_engine::GlobalQuery::GlobalBinding(binding))
        | luanalysis_engine::GlobalQueryError::TooDeep(luanalysis_engine::GlobalQuery::GlobalBinding(binding))
        | luanalysis_engine::GlobalQueryError::UnresolvedQuery(luanalysis_engine::GlobalQuery::GlobalBinding(binding)) = &error {
            declarations.extend(self.global_binding_declarations(*binding));
        }
        for key in declarations { self.global_error(key, &error); }
    }
    fn contains_class_definition(&self, key: luanalysis_engine::NativeClassDefinitionKey) -> bool {
        self.class_definition_source(key).is_some_and(|source| {
            self.class_definition_directive(source).ok().flatten().is_some()
        })
    }
    fn class_definition_header(&self, key: luanalysis_engine::NativeClassDefinitionKey) -> luanalysis_engine::ClassDefinitionHeader {
        let source = self.class_definition_source(key).expect("original class definition key");
        let directive = self.class_definition_directive(source).expect("supported class definition").expect("attached class declaration");
        luanalysis_engine::ClassDefinitionHeader { declaration: directive.declaration }
    }
    fn function_is_method(&self,key:NativeFunctionKey)->bool{self.methods.get(key).is_some_and(|method|method.declaration)}
    fn signature_is_method(&self,key:luanalysis_engine::NativeSignatureKey)->bool{self.signature_has_method_origin(key)}
    fn type_member_owner_application(&self,key:luanalysis_engine::NativeTypeMemberKey,queries:&mut dyn TypeQueries)->Option<luanalysis_engine::MemberOwnerApplication>{self.original_member_owner_application(key,queries)}
    fn contains_signature(&self,key:luanalysis_engine::NativeSignatureKey)->bool{self.signature_available(key)}
    fn signature_header(&self,key:luanalysis_engine::NativeSignatureKey)->luanalysis_engine::SignatureHeader{self.signatures.get(key).header}
    fn signature_has_unconstrained_return(&self, key: luanalysis_engine::NativeSignatureKey) -> bool {
        self.signature_has_omitted_result(key)
    }
    fn function_overload_signatures(&self,key:NativeFunctionKey)->Vec<luanalysis_engine::NativeSignatureKey>{
        if self.source_role() == luanalysis_project::SourceRole::Declaration {
            self.signatures.ambient_function_heads(key, self.overload_mode == crate::OverloadMode::LegacyExposeImplementation)
        } else { self.signatures.function_heads(key) }
    }
    fn function_exposes_implementation(&self, _: NativeFunctionKey) -> bool {
        self.source_role() == luanalysis_project::SourceRole::Implementation
            && self.overload_mode == crate::OverloadMode::LegacyExposeImplementation
    }
    fn signature_type_parameters(&self,key:luanalysis_engine::NativeSignatureKey)->Vec<NativeTypeParameterKey>{self.signature_parameters_keys(key)}
    fn signature_outer_type_parameters(&self,key:luanalysis_engine::NativeSignatureKey)->Vec<NativeTypeParameterKey>{self.signature_outer_keys(key)}
    fn signature_outer_type_parameter_instances(&self,key:luanalysis_engine::NativeSignatureKey,queries:&mut dyn TypeQueries)->Vec<Type>{
        self.signature_applied_owner_function(key).and_then(|function|self.applied_method_self_type(function,queries)).into_iter().collect()
    }
    fn signature_parameters(&self,key:luanalysis_engine::NativeSignatureKey,queries:&mut dyn TypeQueries)->Vec<Parameter>{self.source_signature_parameters(key,queries)}
    fn signature_rest_pack(&self,key:luanalysis_engine::NativeSignatureKey,queries:&mut dyn TypeQueries)->Option<ValuePack>{self.source_signature_rest(key,queries)}
    fn signature_declared_return_pack(&self,key:luanalysis_engine::NativeSignatureKey,queries:&mut dyn TypeQueries)->ValuePack{self.source_signature_return(key,queries)}
    fn signature_return_cycle(&self,key:luanalysis_engine::NativeSignatureKey) {
        self.merge_report(CheckReport { diagnostics:vec![Diagnostic {code:"TS2577",span:self.signatures.get(key).span,message:"Return type annotation circularly references itself.".into()}],..Default::default() });
    }
    fn signature_return_type_too_deep(&self,key:luanalysis_engine::NativeSignatureKey) {
        self.merge_report(CheckReport { diagnostics:vec![Diagnostic {code:"TS2589",span:self.signatures.get(key).span,message:"Type instantiation is excessively deep and possibly infinite.".into()}],..Default::default() });
    }
    fn type_declaration_call_signatures(&self,key:luanalysis_engine::NativeTypeDeclarationKey)->Vec<luanalysis_engine::NativeSignatureKey>{
        self.signatures.named_heads(super::native_named::NativeNamedKey{snapshot:key.snapshot,index:key.index})
    }
    fn type_declaration_operator_signatures(&self,key:luanalysis_engine::NativeTypeDeclarationKey,operator:luanalysis_engine::NativeOperator)->Vec<luanalysis_engine::NativeSignatureKey>{
        self.signatures.operator_heads(super::native_named::NativeNamedKey{snapshot:key.snapshot,index:key.index},operator)
    }
    fn contains_type_declaration(&self, key: luanalysis_engine::NativeTypeDeclarationKey) -> bool {
        self.named_contains_declaration(key)
    }
    fn type_declaration_self_parameter(&self, key: luanalysis_engine::NativeTypeDeclarationKey) -> Option<NativeTypeParameterKey> {
        self.types.self_parameter(crate::native_named::NativeNamedKey { snapshot: key.snapshot, index: key.index })
    }
    fn type_declaration_header(&self, key: luanalysis_engine::NativeTypeDeclarationKey) -> luanalysis_engine::TypeDeclarationHeader {
        self.named_declaration_header(key)
    }
    fn header_revision(&self) -> Option<u64> {
        Some(crate::native_types::metadata_clock())
    }
    fn runtime_table_constructor(&self,key:luanalysis_engine::NativeTypeDeclarationKey)->Option<NativeExpressionKey> {self.runtime_schema_constructor(key)}
    fn runtime_table_constructor_owner(&self,key:luanalysis_engine::NativeTypeDeclarationKey)->Option<NativeFunctionKey> {self.runtime_tables.schema(key)?.owner}
    fn runtime_table_seed(&self,key:luanalysis_engine::NativeTypeDeclarationKey,queries:&mut dyn TypeQueries)->Option<Type> {self.runtime_schema_seed(key,queries)}
    fn runtime_store_receiver(&self,key:NativeFlowKey,queries:&mut dyn TypeQueries)->Option<Type> {self.runtime_store_receiver_type(key,queries)}
    fn type_declaration_owner_scope(&self, key: luanalysis_engine::NativeTypeDeclarationKey) -> luanalysis_engine::OwnerScopeRecipe {
        self.named_owner_scope_recipe(key)
    }
    fn contains_type_member(&self, key: luanalysis_engine::NativeTypeMemberKey) -> bool {
        self.named_contains_member(key)
    }
    fn type_member_header(&self, key: luanalysis_engine::NativeTypeMemberKey) -> luanalysis_engine::TypeMemberHeader {
        self.named_member_header(key)
    }
    fn type_member_signatures(&self,key:luanalysis_engine::NativeTypeMemberKey)->Option<Vec<luanalysis_engine::NativeSignatureKey>> {
        self.doc_members.signature_keys(key,|id|self.doc_signature_key(id))
    }
    fn type_member_origin(&self, key: luanalysis_engine::NativeTypeMemberKey) -> luanalysis_engine::TableFieldOrigin {
        if let Some(source)=self.runtime_tables.member(key) {return if source.annotation.is_some() {luanalysis_engine::TableFieldOrigin::Declared} else {luanalysis_engine::TableFieldOrigin::Inferred};}
        self.original_member_origin(key).unwrap_or(luanalysis_engine::TableFieldOrigin::Declared)
    }
    fn type_member_effective_metadata(&self, key: luanalysis_engine::NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> luanalysis_engine::TypeMemberEffectiveMetadata {
        self.original_member_effective_metadata(key, queries).unwrap_or(luanalysis_engine::TypeMemberEffectiveMetadata {
            origin: luanalysis_engine::TableFieldOrigin::Declared, is_assignment: false,
        })
    }
    fn type_member_is_assignment(&self, key: luanalysis_engine::NativeTypeMemberKey) -> bool {
        self.members.get(key).is_some_and(|source| matches!(source.candidates[0].origin, crate::native_members::OriginalMemberOrigin::AssignmentField(_)))
    }
    fn type_declaration_members(&self, key: luanalysis_engine::NativeTypeDeclarationKey) -> Vec<luanalysis_engine::NativeTypeMemberKey> {
        self.named_declaration_members(key)
    }
    fn type_alias_target(&self, key: luanalysis_engine::NativeTypeDeclarationKey, queries: &mut dyn TypeQueries) -> Type {
        self.named_alias_target(key, queries)
    }
    fn type_member_type(&self, key: luanalysis_engine::NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> Type {
        self.named_member_type(key, queries)
    }
    fn type_indexer_key(&self, key: luanalysis_engine::NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> Type {
        self.named_indexer_type(key, queries)
    }
    fn type_declaration_bases(&self, key: luanalysis_engine::NativeTypeDeclarationKey, queries: &mut dyn TypeQueries) -> Vec<Type> {
        self.named_bases(key, queries)
    }
    fn type_parameter_owner(&self, key: NativeTypeParameterKey) -> Option<luanalysis_engine::TypeDeclarationOwner> {
        self.named_parameter_owner(key)
    }
    fn type_declaration_error(&self, error: luanalysis_engine::NamedTypeError) {
        self.named_declaration_error(error)
    }
    fn function_has_result_pack(&self, key: NativeFunctionKey) -> bool {
        self.contains_function(key)
    }
    fn function_inferred_return_pack(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        self.inferred_return_pack_with_context(key, queries, None, None)
    }
    fn function_inferred_return_pack_in_context(
        &self,
        key: NativeFunctionKey,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        let captured = queries.current_captured_function();
        self.inferred_return_pack_with_context(key, queries, Some(*context), captured)
    }
    fn function_inferred_return_pack_in_capture(
        &self,
        key: NativeFunctionKey,
        context: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        self.inferred_return_pack_with_context(key, queries, None, Some(*context))
    }
    fn function_declared_scalar_contract(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.declared_pack_contract(key, queries)
    }
    fn function_declared_scalar_contract_in_context(
        &self,
        key: NativeFunctionKey,
        _: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.declared_pack_contract(key, queries)
    }
    fn function_declared_scalar_contract_in_capture(
        &self,
        key: NativeFunctionKey,
        _: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.declared_pack_contract(key, queries)
    }
    fn function_declared_return_pack(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.source_declared_return_pack(key, queries, None, None)
    }
    fn function_declared_return_pack_in_context(
        &self,
        key: NativeFunctionKey,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        let captured = queries.current_captured_function();
        self.source_declared_return_pack(key, queries, Some(*context), captured)
    }
    fn function_declared_return_pack_in_capture(
        &self,
        key: NativeFunctionKey,
        context: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.source_declared_return_pack(key, queries, None, Some(*context))
    }
    fn function_rest_pack(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.source_rest_pack(key, queries)
    }
    fn function_rest_pack_in_capture(
        &self,
        key: NativeFunctionKey,
        _: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.source_rest_pack(key, queries)
    }
    fn contextual_rest_parameter(&self, key: NativeFunctionKey) -> Option<usize> {
        let function = self.function(key);
        (function.vararg.is_some()
            && function.annotations.vararg.is_none()
            && function.annotations.type_annotation.is_none())
        .then_some(function.params.len() + usize::from(self.implicit_receiver(function).is_some()))
    }
    fn function_rest_pack_in_context(
        &self,
        key: NativeFunctionKey,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        if let Some(count) = self.contextual_rest_parameter(key) {
            Some(queries.contextual_rest_pack(context, count))
        } else {
            self.source_rest_pack(key, queries)
        }
    }
    fn contains_binding(&self, key: NativeBindingKey) -> bool {
        key.snapshot == self.snapshot && (key.index as usize) < self.bound.declarations.len()
    }
    fn function_parameters_in_capture(
        &self,
        key: NativeFunctionKey,
        context: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        self.source_parameters_in_capture(key, context, queries)
    }
    fn function_declared_return_in_capture(
        &self,
        key: NativeFunctionKey,
        context: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        self.source_declared_return_in_capture(key, context, queries)
    }
    fn function_inferred_return_in_capture(
        &self,
        key: NativeFunctionKey,
        context: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        self.source_inferred_return_in_capture(key, context, queries)
    }
    fn contains_expression(&self, key: NativeExpressionKey) -> bool {
        self.expressions.contains(key)
    }
    fn is_context_sensitive(&self, key: NativeExpressionKey) -> bool {
        self.expressions.context_sensitive(key)
    }
    fn has_context_sensitive_parameters(&self, key: NativeExpressionKey) -> bool {
        self.expressions.has_context_sensitive_parameters(key)
    }
    fn is_expression_descendant(
        &self,
        expression: NativeExpressionKey,
        ancestor: NativeExpressionKey,
    ) -> bool {
        self.expressions.descendant(expression, ancestor)
    }
    fn check_expression(
        &self,
        key: NativeExpressionKey,
        expected: Option<&Type>,
        mode: luanalysis_engine::NativeCheckMode,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        self.source_check_expression(key, expected, mode, queries)
    }
    fn check_reusable_expression(
        &self,
        key: NativeExpressionKey,
        expected: Option<&Type>,
        mode: luanalysis_engine::NativeCheckMode,
        queries: &mut dyn TypeQueries,
    ) -> (Type, bool) {
        self.source_check_reusable_expression(key, expected, mode, queries)
    }
    fn check_expression_pack(
        &self,
        key: NativeExpressionKey,
        expected: Option<&ValuePack>,
        mode: luanalysis_engine::NativeCheckMode,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        self.source_check_expression_pack(key, expected, mode, queries)
    }

    fn contextual_parameter_indices(&self, key: NativeFunctionKey) -> Vec<usize> {
        let function = self.function(key);
        let offset = usize::from(self.implicit_receiver(function).is_some());
        let mut indices: Vec<_> = NativeExpressions::contextual_indices(function).into_iter().map(|index| index + offset).collect();
        if offset != 0 && function.annotations.type_annotation.is_none()
            && !self.methods.get(key).is_some_and(|method| method.owner_application.is_some())
            && (!function.annotations.params.iter().any(|parameter| parameter.name.text=="self")
                || function.params.iter().any(|parameter| parameter.text=="self")) {
            indices.insert(0, 0);
        }
        indices
    }
    fn function_parameters_in_context(
        &self,
        key: NativeFunctionKey,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        self.source_parameters_in_context(key, context, queries)
    }
    fn function_declared_return_in_context(
        &self,
        key: NativeFunctionKey,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        self.source_declared_return_in_context(key, context, queries)
    }
    fn function_inferred_return_in_context(
        &self,
        key: NativeFunctionKey,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        self.inferred_return_in_context(key, context, queries)
    }
    fn function_lexical_parent(&self, key: NativeFunctionKey) -> Option<NativeFunctionKey> {
        self.flow.lexical_parent(key)
    }
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        key.snapshot == self.snapshot && (key.index as usize) < self.functions.len()
    }
    fn function_calling_convention(&self, key: NativeFunctionKey) -> luanalysis_engine::FunctionCallingConvention {
        let _ = self.function(key);
        luanalysis_engine::FunctionCallingConvention::Lua
    }

    fn function_parameters(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        self.metadata(key, queries).0
    }

    fn function_declared_return(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        self.metadata(key, queries).1
    }

    fn function_inferred_return(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        self.inferred_return(key, queries)
    }

    fn return_cycle(&self, key: NativeFunctionKey) {
        self.cycle_diagnostic(key, false);
    }

    fn return_type_too_deep(&self, key: NativeFunctionKey) {
        self.cycle_diagnostic(key, true);
    }

    fn function_type_parameters(
        &self,
        key: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<NativeTypeParameterKey> {
        self.function(key);
        self.implementation_type_parameters(key)
    }

    fn function_outer_type_parameters(
        &self,
        key: NativeFunctionKey,
    ) -> Vec<NativeTypeParameterKey> {
        let mut used = HashSet::default();
        self.function_type_dependencies(key, &mut used, &mut HashSet::default(), &mut HashSet::default());
        let mut outer = self.types.outer_parameters(key, &used);
        self.extend_contextual_self_dependencies(NativeTypeContext::Function(key), &used, &mut outer);
        outer
    }
    fn function_outer_type_parameter_instances(&self,key:NativeFunctionKey,queries:&mut dyn TypeQueries)->Vec<Type>{
        self.applied_method_self_type(key,queries).into_iter().collect()
    }

    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        self.types.contains(key)
    }

    fn type_parameter_name(&self, key: NativeTypeParameterKey) -> String {
        self.types.parameter(key).name.text.clone()
    }

    fn has_type_parameter_constraint(&self, key: NativeTypeParameterKey) -> bool {
        let source = self.types.parameter(key);
        source.constraint.is_some() || source.semantic_self.is_some()
    }

    fn type_parameter_constraint(
        &self,
        key: NativeTypeParameterKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        let source = self.types.parameter(key);
        if let Some(owner) = source.semantic_self {
            return Some(queries.named_declared_type(luanalysis_engine::NativeTypeDeclarationKey {
                snapshot: owner.snapshot, index: owner.index,
            }).unwrap_or_else(|_| queries.error_type()));
        }
        let recipe = source.constraint.as_ref()?;
        let mut checker = self.checker(queries);
        match source.owner {
            NativeTypeContext::Function(owner) => checker.type_context = Some(owner),
            NativeTypeContext::NamedType(owner) => checker.named_type_context = Some(owner),
            NativeTypeContext::Signature(owner) => checker.signature_type_context = Some(owner),
        }
        let constraint = checker.annotation(recipe);
        self.merge_report(checker.report);
        Some(constraint)
    }

    fn type_parameter_order(&self, key: NativeTypeParameterKey) -> u32 {
        self.types.parameter(key).order
    }

    fn type_parameter_siblings(&self, key: NativeTypeParameterKey) -> Vec<NativeTypeParameterKey> {
        self.types.siblings(key)
    }

    fn circular_type_parameter_constraint(&self, key: NativeTypeParameterKey) {
        let source = self.types.parameter(key);
        self.report.borrow_mut().diagnostics.push(Diagnostic {
            code: "TS2313",
            span: source.name.span,
            message: format!(
                "Type parameter '{}' has a circular constraint.",
                source.name.text
            ),
        });
    }
}

fn source_statements<'a>(statements: &'a [Stmt], all: &mut Vec<&'a Stmt>) {
    for statement in statements {
        all.push(statement);
        match &statement.kind {
            StmtKind::Block { body, .. }
            | StmtKind::While { body, .. }
            | StmtKind::Repeat { body, .. }
            | StmtKind::NumericFor { body, .. }
            | StmtKind::GenericFor { body, .. } => source_statements(body, all),
            StmtKind::If {
                branches,
                else_branch,
            } => {
                for branch in branches {
                    source_statements(&branch.body, all);
                }
                if let Some(branch) = else_branch {
                    source_statements(&branch.body, all);
                }
            }
            _ => {}
        }
    }
}

fn type_scope_recipes(
    snapshot: u64,
    functions: &[Function],
    bound: &BoundFile,
) -> Vec<TypeScopeRecipe> {
    let scope_by_span: HashMap<_, _> = bound
        .scopes
        .iter()
        .filter(|scope| scope.kind == bind::ScopeKind::Function)
        .map(|scope| ((scope.span.start, scope.span.end), scope.id))
        .collect();
    let function_by_scope: HashMap<_, _> = functions
        .iter()
        .enumerate()
        .map(|(index, function)| {
            (
                scope_by_span[&(function.span.start, function.span.end)],
                NativeFunctionKey {
                    snapshot,
                    index: index as u32,
                },
            )
        })
        .collect();
    functions
        .iter()
        .enumerate()
        .map(|(index, function)| {
            let scope = scope_by_span[&(function.span.start, function.span.end)];
            let mut enclosing = bound.scope(scope).parent;
            let mut parent = None;
            while let Some(scope) = enclosing {
                if let Some(owner) = function_by_scope.get(&scope) {
                    parent = Some(*owner);
                    break;
                }
                enclosing = bound.scope(scope).parent;
            }
            TypeScopeRecipe {
                owner: NativeFunctionKey {
                    snapshot,
                    index: index as u32,
                },
                parent,
                parameters: function
                    .annotations
                    .generics
                    .iter()
                    .map(|parameter| TypeParameterRecipe {
                        name: parameter.name.clone(),
                        constraint: parameter.constraint.clone(),
                    })
                    .collect(),
            }
        })
        .collect()
}

struct RegistryIndex<'a> {
    bound: &'a BoundFile,
    assertions: HashMap<(usize, usize), &'a luanalysis_syntax::TypeAnnotation>,
    functions: Vec<Function>,
    function_names: Vec<Option<Name>>,
    declaration_functions: HashSet<u32>,
    function_spans: HashMap<(usize, usize), u32>,
    declarations: HashMap<BindingId, DeclarationSource>,
}

impl RegistryIndex<'_> {
    fn initializer_assertion(&self, values: &[Expr], destination: usize) -> Option<(luanalysis_syntax::TypeAnnotation, usize)> {
        // Preserve existing automatic-nil declarations. Scalar RHS values
        // contribute one slot; only the final Lua call/vararg can expand.
        if super::automatic_nil_initializer(values, destination) { return None; }
        let (expression, slot) = if let Some(expression) = values.get(destination) {
            (expression, 0)
        } else {
            let expression = values.last()?;
            if super::native_packs::adjustment(expression, true) != super::native_packs::Adjustment::Expand { return None; }
            (expression, destination - (values.len() - 1))
        };
        self.assertions.get(&(expression.span.start, expression.span.end)).map(|annotation| ((*annotation).clone(), slot))
    }
    fn statements(&mut self, statements: &[Stmt], owner: Option<u32>) {
        for statement in statements {
            match &statement.kind {
                StmtKind::Local {
                    names,
                    values,
                    annotation,
                    ..
                } => {
                    for (index, name) in names.iter().enumerate() {
                        let binding = self
                            .bound
                            .binding_at(name.span.start)
                            .expect("bound local declaration");
                        self.declarations.insert(
                            binding,
                            DeclarationSource::Local {
                                annotation: annotation
                                    .as_ref()
                                    .and_then(|annotation| annotation.types.get(index))
                                    .cloned(),
                                initializer_assertion: self.initializer_assertion(values, index),
                                owner,
                                automatic: annotation.as_ref().and_then(|annotation| annotation.types.get(index)).is_none()
                                    && super::automatic_nil_initializer(values, index),
                            },
                        );
                    }
                    for (index, value) in values.iter().enumerate() {
                        self.expression(value, names.get(index));
                    }
                }
                StmtKind::Assign {
                    targets, values, ..
                } => {
                    for target in targets {
                        self.expression(target, None);
                    }
                    for value in values {
                        self.expression(value, None);
                    }
                }
                StmtKind::Block { body, .. } => self.statements(body, owner),
                StmtKind::LocalFunction { name, function } => {
                    let index = self.function(function, Some(name));
                    self.declaration_functions.insert(index);
                    let binding = self
                        .bound
                        .binding_at(name.span.start)
                        .expect("bound local function");
                    self.declarations
                        .insert(binding, DeclarationSource::Function(index));
                }
                StmtKind::FunctionDeclaration {
                    target, function, ..
                } => {
                    self.expression(target, None);
                    let name = match &target.kind {
                        ExprKind::Name(name) | ExprKind::Field { name, .. } => Some(name),
                        _ => None,
                    };
                    let index = self.function(function, name);
                    self.declaration_functions.insert(index);
                }
                StmtKind::Return(values) => {
                    for value in values {
                        self.expression(value, None);
                    }
                }
                StmtKind::Call(expression) => self.expression(expression, None),
                StmtKind::If {
                    branches,
                    else_branch,
                } => {
                    for branch in branches {
                        self.expression(&branch.condition, None);
                        self.statements(&branch.body, owner);
                    }
                    if let Some(branch) = else_branch {
                        self.statements(&branch.body, owner);
                    }
                }
                StmtKind::While {
                    condition, body, ..
                }
                | StmtKind::Repeat {
                    condition, body, ..
                } => {
                    self.expression(condition, None);
                    self.statements(body, owner);
                }
                StmtKind::NumericFor {
                    name,
                    start,
                    limit,
                    step,
                    body,
                    ..
                } => {
                    self.expression(start, None);
                    self.expression(limit, None);
                    if let Some(step) = step {
                        self.expression(step, None);
                    }
                    let binding = self
                        .bound
                        .binding_at(name.span.start)
                        .expect("bound numeric loop variable");
                    self.declarations
                        .insert(binding, DeclarationSource::NumericLoop);
                    self.statements(body, owner);
                }
                StmtKind::GenericFor {
                    names,
                    values,
                    body,
                    ..
                } => {
                    for value in values {
                        self.expression(value, None);
                    }
                    for name in names {
                        let binding = self
                            .bound
                            .binding_at(name.span.start)
                            .expect("bound generic loop variable");
                        self.declarations
                            .insert(binding, DeclarationSource::GenericLoop);
                    }
                    self.statements(body, owner);
                }
                StmtKind::Break | StmtKind::Goto(_) | StmtKind::Label(_) => {}
            }
        }
    }

    fn expression(&mut self, expression: &Expr, name: Option<&Name>) {
        match &expression.kind {
            ExprKind::Function(function) => {
                self.function(function, name);
            }
            ExprKind::Parenthesized(inner) => self.expression(inner, name),
            ExprKind::Field { base, .. } => self.expression(base, None),
            ExprKind::Index { base, index } => {
                self.expression(base, None);
                self.expression(index, None);
            }
            ExprKind::Unary { value, .. } => self.expression(value, None),
            ExprKind::Binary { left, right, .. } => {
                self.expression(left, None);
                self.expression(right, None);
            }
            ExprKind::Table(fields) => {
                for field in fields {
                    if let luanalysis_syntax::TableFieldKind::Keyed(key) = &field.kind {
                        self.expression(key, None);
                    }
                    self.expression(&field.value, None);
                }
            }
            ExprKind::Call { callee, args } => {
                self.expression(callee, None);
                for argument in args {
                    self.expression(argument, None);
                }
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                self.expression(receiver, None);
                for argument in args {
                    self.expression(argument, None);
                }
            }
            ExprKind::Nil
            | ExprKind::Boolean(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Name(_)
            | ExprKind::Vararg
            | ExprKind::Error => {}
        }
    }

    fn function(&mut self, function: &Function, name: Option<&Name>) -> u32 {
        let span = (function.span.start, function.span.end);
        if let Some(index) = self.function_spans.get(&span) {
            return *index;
        }
        let index =
            u32::try_from(self.functions.len()).expect("native Lua function index fits in u32");
        self.functions.push(function.clone());
        self.function_names.push(name.cloned());
        self.function_spans.insert(span, index);
        for (parameter_index, parameter) in function.params.iter().enumerate() {
            let binding = self
                .bound
                .binding_at(parameter.span.start)
                .expect("bound function parameter");
            self.declarations.insert(
                binding,
                DeclarationSource::Parameter {
                    function: index,
                    index: parameter_index,
                },
            );
        }
        self.statements(&function.body, Some(index));
        index
    }
}

#[cfg(test)]
mod body_report_tests {
    use super::*;
    use luanalysis_engine::Engine;

    #[test]
    fn a_discarded_body_report_keeps_completed_nested_body_errors_and_cache() {
        let file = luanalysis_syntax::parse("---@return number\nlocal function child() return 'wrong' end\n");
        let bound = bind::bind(&file);
        Engine::new().with_native_provider(
            move || NativeRegistry::new(file, bound),
            |registry, queries| {
                let child = registry.key(0);
                registry.with_body_report(|| {
                    registry.merge_report(CheckReport {
                        diagnostics: vec![Diagnostic { code: "LUA2011", span: Span { start: 0, end: 1 }, message: "prospective receiver expression".into() }],
                        ..CheckReport::default()
                    });
                    registry.check_body(child, queries);
                    ((), false)
                });
                assert!(registry.checked_bodies.borrow().contains(&child));
                let report = registry.take_report();
                assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
                assert_eq!(report.diagnostics[0].code, "TS2322");
                assert!(registry.provisional_reports.borrow().is_empty());
            },
        );
    }

    #[test]
    fn provisional_parent_discards_metadata_but_retains_completed_child_diagnostics_and_cache() {
        let source = "---@return number\nlocal function child() return 'wrong' end\n";
        let file = luanalysis_syntax::parse(source);
        let bound = bind::bind(&file);
        Engine::new().with_native_provider(
            move || NativeRegistry::new(file, bound),
            |registry, queries| {
                let child = registry.key(0);
                let metadata = || CheckReport {
                    diagnostics: vec![Diagnostic { code: "LUA2011", span: Span { start: 0, end: 1 }, message: "provisional parent metadata".into() }],
                    pending_types: vec![(Span { start: 0, end: 1 }, Type::Number)],
                    ..CheckReport::default()
                };
                registry.with_body_report(|| {
                    registry.merge_report(metadata());
                    registry.check_body(child, queries);
                    registry.merge_report(metadata());
                    ((), false)
                });
                assert!(registry.checked_bodies.borrow().contains(&child));
                let committed = registry.take_report();
                assert_eq!(committed.diagnostics.iter().filter(|error| error.code == "TS2322").count(), 1, "{:#?}", committed.diagnostics);
                assert!(!committed.diagnostics.iter().any(|error| error.code == "LUA2011"));
                assert!(!committed.pending_types.iter().any(|(span, _)| span.start == 0 && span.end == 1));
                registry.with_body_report(|| {
                    registry.merge_report(metadata());
                    assert!(registry.checked_bodies.borrow().contains(&child));
                    ((), false)
                });
                let retried = registry.take_report();
                assert!(retried.diagnostics.is_empty());
                assert!(retried.pending_types.is_empty());
                registry.with_body_report(|| {
                    registry.merge_report(metadata());
                    ((), true)
                });
                assert_eq!(registry.take_report().diagnostics.len(), 1);
                assert!(registry.provisional_reports.borrow().is_empty());
            },
        );
    }
}

#[cfg(test)]
mod standalone_environment_tests {
    use super::*;
    use luanalysis_engine::Engine;

    #[test]
    fn environment_members_use_original_single_source_binding_keys_and_reject_foreign_owners() {
        let source = "answer = 42\nfunction global() return 1 end\n";
        let file = luanalysis_syntax::parse(source);
        let bound = bind::bind(&file);
        Engine::new().with_native_provider(
            move || NativeRegistry::new(file, bound),
            |registry, queries| {
                let environment = registry.project_globals.as_ref().unwrap().environment;
                let binding = registry.global_binding("answer").unwrap();
                assert_eq!(registry.global_environment_binding(environment, b"answer"), Some(binding));
                let bindings = registry.global_environment_bindings(environment);
                assert!(bindings.contains(&(b"answer".to_vec(), binding)));
                assert!(bindings.iter().all(|(_, binding)| binding.environment == environment));
                let foreign = luanalysis_engine::NativeEnvironmentKey { index: environment.index + 1, ..environment };
                assert_eq!(registry.global_environment_binding(foreign, b"answer"), None);
                assert!(registry.global_environment_bindings(foreign).is_empty());
                assert_eq!(registry.global_environment_binding(environment, &[0xff]), None);
                assert_eq!(registry.global_environment_self_binding(environment), None);
                let table = queries.global_environment_type(environment).unwrap();
                let value = queries.property_type(&table, "answer").unwrap();
                assert_eq!(queries.format(&value), "number");
                let function = queries.property_type(&table, "global").unwrap();
                let call = queries.call(&function, &[]);
                assert!(call.errors.is_empty(), "{:#?}", call.errors);
                let returned = queries.widen(&call.return_type);
                assert_eq!(queries.format(&returned), "number");
            },
        );
    }
}

#[cfg(test)]
mod generic_scope_tests {
    use super::*;
    use luanalysis_engine::Engine;

    #[test]
    fn ambient_type_visibility_does_not_create_unneeded_closure_mapper_parameters() {
        let source = "---@generic T\n---@param value T\nlocal function outer(value)\nlocal function constant() return 1 end\nlocal function captured() return value end\nreturn constant\nend";
        let file = luanalysis_syntax::parse(source);
        assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
        let bound = bind::bind(&file);
        Engine::new().with_native_provider(
            move || NativeRegistry::new(file, bound),
            |provider, queries| {
                let outer = provider.key(0);
                let constant = provider.key(1);
                let captured = provider.key(2);
                let own = provider.function_type_parameters(outer, queries);
                assert_eq!(own.len(), 1);
                assert!(provider.function_outer_type_parameters(constant).is_empty());
                assert_eq!(provider.function_outer_type_parameters(captured), own);
                assert_eq!(
                    queries.native_function_signature(constant).returns.as_ref(),
                    &Type::Number
                );
                assert_eq!(
                    queries.native_function_signature(captured).returns.as_ref(),
                    &Type::NativeTypeParameter(own[0])
                );
            },
        );
    }
}
