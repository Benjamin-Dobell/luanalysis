//! Original-Lua declarations supplied to the native checker's lazy signatures.
//!
//! The immutable registry owns one parsed source snapshot. Provider callbacks
//! create independent Lua body checkers with the native query capability that
//! requested the body. They never retain a mutable report borrow across a query.

use super::native_expressions::NativeExpressions;
use super::native_flow::{FlowAdapter, FlowValueQueries, NativeFlowSources, ValueRecipe};
use super::native_named::{NativeNamed, NativeTypeContext};
use super::native_packs::{AnnotationPack, InitializerContext, NativePacks, only_empty_results};
use super::native_types::{NativeTypes, TypeNameResolution, TypeParameterRecipe, TypeScopeRecipe};
use super::{Binding, CheckReport, Checker, Diagnostic, bind};
use bind::{BindingId, BoundFile};
use luanalysis_engine::{
    CapturedFunctionContext, ContextualFunctionContext, NativeBindingKey, NativeCallEffects,
    NativeExpressionKey, NativeFlowError, NativeFlowKey, NativeFlowResult, NativeFunctionKey,
    NativeTypeParameterKey, NativeTypeProvider, Parameter, Type, TypeQueries, ValuePack,
};
use luanalysis_syntax::{Expr, ExprKind, File, Function, Name, Span, Stmt, StmtKind, TypeExpr};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SNAPSHOT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
enum DeclarationSource {
    Local {
        annotation: Option<TypeExpr>,
        owner: Option<u32>,
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
    file: File,
    bound: BoundFile,
    functions: Vec<Function>,
    function_names: Vec<Option<Name>>,
    function_spans: HashMap<(usize, usize), u32>,
    declarations: HashMap<BindingId, DeclarationSource>,
    types: NativeTypes,
    named: NativeNamed,
    expressions: NativeExpressions,
    packs: NativePacks,
    flow: NativeFlowSources,
    completed_bindings: RefCell<HashMap<BindingId, Binding>>,
    /// Only completion of body diagnostics is cached here. Return-type cache,
    /// query recursion and cycle decisions belong entirely to the native core.
    checked_bodies: RefCell<HashSet<NativeFunctionKey>>,
    checked_contextual_bodies: RefCell<HashSet<(NativeFunctionKey, ContextualFunctionContext)>>,
    checked_captured_bodies: RefCell<HashSet<(NativeFunctionKey, CapturedFunctionContext)>>,
    report: RefCell<CheckReport>,
}

impl NativeRegistry {
    pub(super) fn new(file: File, bound: BoundFile) -> Self {
        let snapshot = NEXT_SNAPSHOT.fetch_add(1, Ordering::Relaxed);
        assert_ne!(snapshot, u64::MAX, "source snapshot identities exhausted");
        let mut index = RegistryIndex {
            bound: &bound,
            functions: Vec::new(),
            function_names: Vec::new(),
            function_spans: HashMap::new(),
            declarations: HashMap::new(),
        };
        index.statements(&file.statements, None);
        let RegistryIndex {
            functions,
            function_names,
            function_spans,
            declarations,
            ..
        } = index;
        let source_function_keys = function_spans
            .iter()
            .map(|(&span, &index)| (span, NativeFunctionKey { snapshot, index }))
            .collect();
        let named = NativeNamed::new(snapshot, &file, &bound, &source_function_keys)
            .expect("original named declaration metadata");
        let types = NativeTypes::new_with_named(
            snapshot,
            type_scope_recipes(snapshot, &functions, &bound),
            named
                .declarations()
                .iter()
                .map(|source| source.generic_scope.clone())
                .collect(),
        );
        let expressions = NativeExpressions::new(snapshot, &file, &function_spans);
        let packs = NativePacks::new(snapshot, &file, &function_spans, &|expression| {
            expressions.key_for_span(expression.span)
        });
        let flow = NativeFlowSources::new(&file, &bound, snapshot, &function_spans);
        let mut report = CheckReport::default();
        report
            .diagnostics
            .extend(types.diagnostics().iter().cloned());
        Self {
            snapshot,
            file,
            bound,
            functions,
            function_names,
            function_spans,
            declarations,
            types,
            named,
            expressions,
            packs,
            flow,
            completed_bindings: RefCell::new(HashMap::new()),
            checked_bodies: RefCell::new(HashSet::new()),
            checked_contextual_bodies: RefCell::new(HashSet::new()),
            checked_captured_bodies: RefCell::new(HashSet::new()),
            report: RefCell::new(report),
        }
    }

    pub(super) fn bound(&self) -> &BoundFile {
        &self.bound
    }

    pub(super) fn statements(&self) -> &[Stmt] {
        &self.file.statements
    }

    pub(super) fn expressions(&self) -> &NativeExpressions {
        &self.expressions
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
        if super::native_expressions::publishes(mode) && !checker.incomplete_flow {
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
            while let luanalysis_syntax::TypeExprKind::Parenthesized(inner) = &ty.kind {
                ty = inner;
            }
            let luanalysis_syntax::TypeExprKind::Function(signature) = &ty.kind else {
                return None;
            };
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
    ) -> Option<ValuePack> {
        let pack = self.declared_pack_contract(key, queries)?;
        // Written scalar nil/void/any contracts may allow a function to end.
        // Their permission cannot fabricate a value in its runtime pack.
        if only_empty_results(self.function(key)) {
            if queries.pack_allows_implicit_return(&pack) {
                return Some(ValuePack::empty());
            }
        }
        Some(pack)
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
            while let luanalysis_syntax::TypeExprKind::Parenthesized(inner) = &ty.kind {
                ty = inner;
            }
            let luanalysis_syntax::TypeExprKind::Function(signature) = &ty.kind else {
                return None;
            };
            signature.vararg.as_deref()
        });
        let mut checker = self.checker(queries);
        checker.type_context = Some(key);
        checker.check_mode = checker
            .engine
            .current_contextual_function()
            .map_or_else(luanalysis_engine::NativeCheckMode::empty, |context| {
                context.mode
            });
        let ty = match annotation {
            None => {
                checker.error(
                    "TS7019",
                    function.vararg.unwrap(),
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
        let record = self.expressions.get(key);
        let contextual = queries
            .current_contextual_function()
            .filter(|context| self.expressions.function_for(context.expression) == record.owner);
        let mut checker = self.checker(queries);
        checker.check_mode = mode;
        checker.type_context = record.owner;
        checker.contextual_function = contextual;
        let captured = checker.engine.current_captured_function();
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
            engine,
            bound: &self.bound,
            native: Some(self),
            locals: HashMap::new(),
            assigned_fields: HashMap::new(),
            report: CheckReport::default(),
            return_type: None,
            return_values: Vec::new(),
            return_pack: None,
            return_packs: Vec::new(),
            type_context: None,
            named_type_context: None,
            contextual_function: None,
            captured_function: None,
            check_mode: luanalysis_engine::NativeCheckMode::empty(),
            flow_at: None,
            incomplete_flow: false,
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
            narrowed
        } else if let Some(projection) = self.flow.iterator_projection(reference) {
            let mut narrowed = queries.flow_iterator_projection(&FlowAdapter { sources: &self.flow, values: self }, at, projection, &ordinary.ty);
            narrowed.incomplete |= ordinary.incomplete;
            narrowed
        } else {
            ordinary
        }
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
    ) {
        if !super::native_expressions::publishes(mode) {
            return;
        }
        if let Some(context) = self.flow.contextual(at.graph) {
            self.contextual_report(&context, report);
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
        let contextual = self.flow.contextual(at.graph);
        let captured = self.flow.captured(at.graph);
        let mut result = None;
        let mut contextual_work = |queries: &mut dyn TypeQueries| {
            if let Some(contextual) = contextual {
                queries.with_contextual_function(&contextual, &mut |queries| {
                    result = Some(work(queries));
                });
            } else {
                result = Some(work(queries));
            }
        };
        if let Some(captured) = captured {
            queries.with_captured_function(&captured, &mut contextual_work);
        } else {
            contextual_work(queries);
        }
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
        let recipe = self.flow.value(value_key)?;
        let value = self.flow_value_type(&self.flow, recipe, at, queries);
        let constraint = queries.widen(&value);
        Some(Binding {
            value: constraint.clone(),
            evolves_from_nil: matches!(constraint, Type::Nil | Type::Undefined),
            constraint,
        })
    }

    pub(super) fn merge_report(&self, report: CheckReport) {
        self.report.borrow_mut().append(report);
    }

    pub(super) fn take_report(&self) -> CheckReport {
        std::mem::take(&mut *self.report.borrow_mut())
    }

    pub(super) fn iterator_sources(&self) -> &NativeFlowSources { &self.flow }

    pub(super) fn publish_binding(&self, binding: BindingId, value: &Binding) {
        if matches!(self.declarations.get(&binding), Some(DeclarationSource::GenericLoop)) { return; }
        let mut captured = value.clone();
        captured.value = captured.constraint.clone();
        self.completed_bindings
            .borrow_mut()
            .insert(binding, captured);
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
            DeclarationSource::Local { annotation, owner } => {
                checker.type_context = owner.map(|owner| self.key(owner));
                if let Some(annotation) = annotation {
                    // The declaration already supplies the captured type.
                    // Its file/body check still validates the initializer.
                    (checker.annotation(&annotation), false)
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
                let ty = if parameter.optional {
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
                let ty = if parameter.optional {
                    Type::Union(vec![parameter_type, Type::Nil])
                } else {
                    parameter_type
                };
                Some(Binding {
                    constraint: ty.clone(),
                    value: ty,
                    evolves_from_nil: false,
                })
            }
            DeclarationSource::Local {
                annotation,
                owner: Some(function),
            } if Some(self.key(function)) == owner => {
                let key = self.key(function);
                let mut checker = self.checker(queries);
                checker.type_context = Some(key);
                checker.contextual_function = Some(*context);
                checker.captured_function = checker.engine.current_captured_function();
                checker.check_mode = context.mode;
                if let Some(annotation) = annotation {
                    let constraint = checker.annotation(&annotation);
                    self.contextual_report(context, checker.report);
                    return Some(Binding {
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
        for (index, (name, parameter)) in function.params.iter().zip(parameters).enumerate() {
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
            let ty = if parameter.optional {
                Type::Union(vec![parameter_type, Type::Nil])
            } else {
                parameter_type
            };
            checker.locals.insert(
                binding,
                Binding {
                    constraint: ty.clone(),
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

    fn inferred_return_pack_with_context(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
        contextual: Option<ContextualFunctionContext>,
        captured: Option<CapturedFunctionContext>,
    ) -> ValuePack {
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
            returned.push(checker.expression_list_pack(&expressions, context.as_ref()));
            if !checker.incomplete_flow {
                self.merge_flow_report(at, checker.check_mode, checker.report);
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
            self.merge_flow_report(at, checker.check_mode, checker.report);
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
                if let Some(captured) = captured {
                    pack = checker.engine.instantiate_pack_in_capture(&captured, &pack);
                }
                checker.engine.pack_with_implicit_empty(&pack)
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
            if !terminated && !checker.engine.allows_implicit_return(expected) && !accepts_empty {
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
        if let Some(context) = contextual {
            self.contextual_report(&context, checker.report);
            self.checked_contextual_bodies
                .borrow_mut()
                .insert((key, context));
        } else {
            self.merge_report(checker.report);
            if let Some(captured) = captured {
                self.checked_captured_bodies
                    .borrow_mut()
                    .insert((key, captured));
            } else {
                self.checked_bodies.borrow_mut().insert(key);
            }
        }
        result
    }

    /// Demand every body for semantic diagnostics, including unused annotated
    /// declarations whose return type required no native inference query.
    pub(super) fn check_all_bodies(&self, queries: &mut dyn TypeQueries) {
        for index in 0..self.functions.len() {
            let key = self.key(index as u32);
            for parameter in self.types.own_parameters(key) {
                queries.type_parameter_constraint(parameter);
            }
            let facets = self.expressions.final_facets(key);
            if !facets.is_empty() {
                // The selected session capability owns this callback's
                // parameter/return caches. Demanding its base signature would
                // invent an unrelated untyped declaration and bogus TS7006.
                for facet in facets {
                    queries.format(&facet);
                }
                for context in self.expressions.final_contexts(key) {
                    let checked = self
                        .checked_contextual_bodies
                        .borrow()
                        .contains(&(key, context));
                    if !checked {
                        if let Some(captured) = self.expressions.capture_for_context(key, &context)
                        {
                            queries.with_captured_function(&captured, &mut |queries| {
                                self.check_body_in_context(key, &context, queries);
                            });
                        } else {
                            self.check_body_in_context(key, &context, queries);
                        }
                    }
                }
                let contextual_captures = self.expressions.contextual_captures(key);
                for context in self.expressions.final_captures(key) {
                    if contextual_captures.contains(&context) {
                        continue;
                    }
                    let checked = self
                        .checked_captured_bodies
                        .borrow()
                        .contains(&(key, context));
                    if !checked {
                        self.check_body_in_capture(key, &context, queries);
                    }
                }
                continue;
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

    fn function_type_dependencies(
        &self,
        key: NativeFunctionKey,
        used: &mut HashSet<NativeTypeParameterKey>,
        functions: &mut HashSet<NativeFunctionKey>,
        bindings: &mut HashSet<BindingId>,
    ) {
        if !functions.insert(key) {
            return;
        }
        let function = self.function(key);
        for parameter in &function.annotations.generics {
            if let Some(constraint) = &parameter.constraint {
                self.types
                    .annotation_dependencies(Some(key), constraint, used);
            }
        }
        for parameter in &function.annotations.params {
            self.types
                .annotation_dependencies(Some(key), &parameter.ty, used);
        }
        if let Some(vararg) = &function.annotations.vararg {
            self.types
                .annotation_dependencies(Some(key), &vararg.ty, used);
        }
        if let Some(annotation) = &function.annotations.type_annotation {
            for ty in &annotation.types {
                self.types.annotation_dependencies(Some(key), ty, used);
            }
        }
        if let Some(annotation) = &function.annotations.returns {
            for ty in &annotation.types {
                self.types.annotation_dependencies(Some(key), ty, used);
            }
        } else {
            let mut statements = Vec::new();
            source_statements(&function.body, &mut statements);
            for statement in statements {
                if let StmtKind::Return(values) = &statement.kind {
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
            } => self.types.annotation_dependencies(
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
                let name = &function.params[*index];
                if let Some(tag) = function
                    .annotations
                    .params
                    .iter()
                    .find(|tag| tag.name.text == name.text)
                {
                    self.types
                        .annotation_dependencies(Some(owner), &tag.ty, used);
                } else if let Some(context) = &function.annotations.type_annotation {
                    for ty in &context.types {
                        self.types.annotation_dependencies(Some(owner), ty, used);
                    }
                }
            }
            DeclarationSource::NumericLoop | DeclarationSource::GenericLoop => {}
        }
    }

    fn value_type_dependencies(
        &self,
        expression: &Expr,
        used: &mut HashSet<NativeTypeParameterKey>,
        functions: &mut HashSet<NativeFunctionKey>,
        bindings: &mut HashSet<BindingId>,
    ) {
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
                AnnotationPack::annotation(annotation).lower(&mut |ty| checker.annotation(ty))
            }
            InitializerContext::Assignment(targets) => {
                let fixed = targets.iter().map(|target| {
                    let ty = match &target.kind {
                        ExprKind::Name(name) => checker.bound.binding_at(name.span.start)
                            .and_then(|binding| checker.resolve_local(binding))
                            .filter(|binding| !binding.evolves_from_nil)
                            .map(|binding| binding.constraint.clone()),
                        _ => None,
                    };
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
    ) -> Type {
        match value {
            ValueRecipe::Nil => Type::Nil,
            ValueRecipe::PackElement { evaluation, index } => {
                let expression = &sources
                    .program
                    .expression(sources.program.evaluation(*evaluation).expression)
                    .expression;
                let mut checker = self.checker_at_flow(input, queries);
                let expected = self.flow_initializer_context(expression, true, &mut checker);
                let pack = checker.expression_pack(expression, expected.as_ref());
                let value = checker.engine.pack_slot(&pack, *index);
                if !checker.incomplete_flow {
                    self.merge_flow_report(input, checker.check_mode, checker.report);
                }
                value
            }
            ValueRecipe::Function(region) => {
                let key = sources
                    .function_for_graph(sources.graph(*region, 0))
                    .expect("an original function graph has a source declaration");
                let mode = sources
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
                let expected = self.flow_initializer_context(expression, false, &mut checker);
                let expected = expected.as_ref().map(|pack| checker.engine.pack_slot(pack, 0));
                let result = checker.expression_with_expected(expression, expected.as_ref());
                if !checker.incomplete_flow {
                    self.merge_flow_report(input, checker.check_mode, checker.report);
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
        let mut checker = self.checker_at_flow(input, queries);
        let callee = match &expression.kind {
            ExprKind::Call { callee, .. } => checker.expression(callee),
            ExprKind::MethodCall { receiver, name, .. } => {
                let receiver = checker.expression(receiver);
                checker
                    .engine
                    .property_type(&receiver, &name.text)
                    .unwrap_or(Type::Unknown)
            }
            _ => return NativeCallEffects::None,
        };
        // Reachability must not demand the inferred result of an unused call:
        // that result can recursively depend on the current function. An
        // explicit nonreturning contract is sufficient for this flow effect.
        let declared_return = match &callee {
            Type::Function(signature) => Some((*signature.returns).clone()),
            _ => checker.engine.native_function_key(&callee).and_then(|key| {
                let function = self.function(key);
                let mut declaration = self.checker(checker.engine);
                declaration.type_context = Some(key);
                let result = if let Some(annotation) = &function.annotations.returns {
                    match annotation.types.as_slice() {
                        [ty] if annotation.packs.is_none() => Some(declaration.annotation(ty)),
                        _ => None,
                    }
                } else {
                    function
                        .annotations
                        .type_annotation
                        .as_ref()
                        .and_then(|annotation| annotation.types.first())
                        .and_then(|ty| match declaration.annotation(ty) {
                            Type::Function(signature) => Some(*signature.returns),
                            _ => None,
                        })
                };
                let result =
                    result.and_then(|ty| declaration.engine.instantiate_in_function(&callee, &ty));
                self.merge_report(declaration.report);
                result
            }),
        };
        if !checker.incomplete_flow {
            self.merge_flow_report(input, checker.check_mode, checker.report);
        }
        if matches!(declared_return, Some(Type::Never)) {
            NativeCallEffects::Never
        } else {
            NativeCallEffects::None
        }
    }
}

impl FlowValueQueries for NativeRegistry {
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
            vec![callee_origin; 2]
        };
        let feedback = stage == luanalysis_engine::NativeIteratorStage::Next && matches!(&error, luanalysis_engine::CallError::Argument { index: 1, .. });
        let mut checker = self.checker_at_flow(at, queries);
        let first_diagnostic = checker.report.diagnostics.len();
        checker.call_result(luanalysis_engine::CallResult { return_type: Type::Any, errors: vec![error] }, callee, &arguments, callee_origin, callee_origin);
        if feedback {
            for diagnostic in checker.report.diagnostics.iter_mut().skip(first_diagnostic) {
                diagnostic.message.push_str(" The previous first iterator result supplies the next control argument.");
            }
        }
        self.merge_flow_report(at, checker.check_mode, checker.report);
    }

    fn flow_value_pack(&self, sources: &NativeFlowSources, value: &ValueRecipe, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> ValuePack {
        self.with_flow_context(input, queries, &mut |queries| {
            if let ValueRecipe::IteratorHeader(iterator) = value {
                return self.source_iterator_header(*iterator, input, queries);
            }

            let ValueRecipe::Evaluation(evaluation) = value else {
                return ValuePack::single(self.flow_value_type_in_context(sources, value, input, queries));
            };
            let evaluation = sources.program.evaluation(*evaluation);
            let expression = &sources.program.expression(evaluation.expression).expression;
            let mut checker = self.checker_at_flow(input, queries);
            let expected = self.flow_initializer_context(expression, true, &mut checker);
            let pack = checker.expression_pack(expression, expected.as_ref());
            if !checker.incomplete_flow { self.merge_flow_report(input, checker.check_mode, checker.report); }
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
            self.flow_value_type_in_context(sources, value, input, queries)
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
    fn function_declared_return_pack(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.source_declared_return_pack(key, queries)
    }
    fn function_declared_return_pack_in_context(
        &self,
        key: NativeFunctionKey,
        _: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.source_declared_return_pack(key, queries)
    }
    fn function_declared_return_pack_in_capture(
        &self,
        key: NativeFunctionKey,
        _: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.source_declared_return_pack(key, queries)
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
        .then_some(function.params.len())
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
        NativeExpressions::contextual_indices(self.function(key))
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
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        key.snapshot == self.snapshot && (key.index as usize) < self.functions.len()
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
        self.types.own_parameters(key)
    }

    fn function_outer_type_parameters(
        &self,
        key: NativeFunctionKey,
    ) -> Vec<NativeTypeParameterKey> {
        let mut used = HashSet::new();
        self.function_type_dependencies(key, &mut used, &mut HashSet::new(), &mut HashSet::new());
        self.types.outer_parameters(key, &used)
    }

    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        self.types.contains(key)
    }

    fn type_parameter_name(&self, key: NativeTypeParameterKey) -> String {
        self.types.parameter(key).name.text.clone()
    }

    fn has_type_parameter_constraint(&self, key: NativeTypeParameterKey) -> bool {
        self.types.parameter(key).constraint.is_some()
    }

    fn type_parameter_constraint(
        &self,
        key: NativeTypeParameterKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        let source = self.types.parameter(key);
        let recipe = source.constraint.as_ref()?;
        let mut checker = self.checker(queries);
        match source.owner {
            NativeTypeContext::Function(owner) => checker.type_context = Some(owner),
            NativeTypeContext::NamedType(owner) => checker.named_type_context = Some(owner),
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
    functions: Vec<Function>,
    function_names: Vec<Option<Name>>,
    function_spans: HashMap<(usize, usize), u32>,
    declarations: HashMap<BindingId, DeclarationSource>,
}

impl RegistryIndex<'_> {
    fn statements(&mut self, statements: &[Stmt], owner: Option<u32>) {
        for statement in statements {
            match &statement.kind {
                StmtKind::Local {
                    names,
                    values,
                    annotation,
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
                                owner,
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
                    self.function(function, name);
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
                StmtKind::Break => {}
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
