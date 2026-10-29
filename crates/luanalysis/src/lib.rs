//! Original-source checking for the first native Lua subset.
//!
//! Lua syntax and straight-line bindings are owned here. Type construction,
//! literal widening, member lookup, freshness and assignability use the pinned
//! Bun core. Native generic calls share its inference worker. Branch flow,
//! contextual expression demands and project services are integration gates.

/// Host tables use the checker's deterministic Fx hashing: keys are original
/// source identities, never attacker-chosen input requiring DoS resistance.
/// The version `luanalysis --version`, the language server and the C library report: the
/// package's, with the commit it was built from (`0.1.0+b2cff754f241`, `.dirty` when built from
/// changed sources).
pub const VERSION: &str = env!("LUANALYSIS_VERSION");

/// `VERSION`, NUL-terminated for C.
pub const VERSION_C: &std::ffi::CStr = match std::ffi::CStr::from_bytes_with_nul(concat!(env!("LUANALYSIS_VERSION"), "\0").as_bytes()) {
    Ok(version) => version,
    Err(_) => panic!("the version contains no NUL"),
};

pub(crate) mod collections {
    pub(crate) type HashMap<K, V> = std::collections::HashMap<K, V, luanalysis_engine::FxBuildHasher>;
    pub(crate) type HashSet<T> = std::collections::HashSet<T, luanalysis_engine::FxBuildHasher>;
}
pub mod bind;
mod builtins;
mod diagnostic_types;
mod module_map;
mod module_requests;
pub mod flow;
pub mod formatting;
pub mod cli;
pub mod incremental;
mod affected;
mod json;
mod language_version;
mod luarc;
pub mod language_service;
pub mod lsp;
mod file_operations;
pub mod semantic_actions;
pub mod report;
mod rename_failure;
pub mod project_check;
mod project_cost;
pub mod standard_environment;
pub mod symbols;
mod functions;
mod inspections;
mod native_expressions;
mod native_flow;
mod native_assertions;
mod native_iterators;
mod native_functions;
mod native_named;
mod native_members;
mod native_runtime_tables;
mod native_runtime_provider;
mod native_doc_members;
mod native_signatures;
mod native_packs;
mod native_return_annotations;
mod native_tables;
mod native_table_escapes;
mod native_type_operators;
mod native_types;
mod operators;
mod pattern_captures;

use bind::{BindingId, BoundFile};
use luanalysis_engine::{
    CapturedFunctionContext, ContextualFunctionContext, Engine, LuaKey, NativeCheckMode,
    NativeEnvironmentKey, NativeFlowKey, NativeFunctionKey, TableKey, Type, TypeQueries,
};
use luanalysis_syntax::{
    Expr, ExprKind, Name, Span, Stmt, StmtKind, TypeAnnotation, TypeExpr, TypeExprKind, TypeLiteral,
};
use native_functions::NativeRegistry;
use crate::collections::{HashMap, HashSet};

/// Public exposure of an overloaded function's implementation contract.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverloadMode {
    #[default]
    V2,
    LegacyExposeImplementation,
}

/// Source declaration policy independent of overload visibility and type relations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckOptions {
    pub overload_mode: OverloadMode,
    /// Require written or contextual parameter types, including runtime varargs.
    /// Ordinary Luanalysis analysis permits missing contracts as in v1.
    pub no_implicit_any: bool,
    /// Report member access, indexing and calls of `any` (`ProjectCheckOptions::no_unsafe_any`).
    pub no_unsafe_any: bool,
    /// Read EmmyLua Analyzer Rust's own annotation syntax (`ProjectCheckOptions::emmylua_syntax`).
    pub emmylua_syntax: bool,
    /// Read every optional as `nil | T`, and absence as nil (`ProjectCheckOptions::decay_optionals`).
    pub decay_optionals: bool,
    /// Require nil to appear in assignment contracts. False selects v1's
    /// permissive nil assignments without erasing physical runtime nil types.
    pub strict_nil: bool,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self { overload_mode: OverloadMode::default(), no_implicit_any: false, no_unsafe_any: false, emmylua_syntax: false, decay_optionals: false, strict_nil: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub message: String,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticCategory {
    Error,
    Warning,
    Suggestion,
}

impl Diagnostic {
    /// As a TypeScript diagnostic message carries its category. Only inspections, which an
    /// editor asks for, are warnings or suggestions.
    pub fn category(&self) -> DiagnosticCategory {
        match self.code {
            "TS2367" | "LUA2872" | "LUA2873" | "LUA4001" | "LUA4005" => DiagnosticCategory::Warning,
            "TS6133" | "TS7027" | "TS7044" | "TS7047" | "LUA4002" | "LUA4003" | "LUA4004" => DiagnosticCategory::Suggestion,
            _ => DiagnosticCategory::Error,
        }
    }

    /// Code that has no effect, unused or unreachable, which editors show faded.
    pub fn is_unnecessary(&self) -> bool {
        matches!(self.code, "TS6133" | "TS7027")
    }
}

/// Each key's last entry, in the order of those last entries.
fn last_of_each<K: Copy + Eq + std::hash::Hash, V>(entries: Vec<(K, V)>) -> Vec<(K, V)> {
    let mut seen = HashSet::with_capacity_and_hasher(entries.len(), Default::default());
    let mut kept: Vec<_> = entries.into_iter().rev().filter(|(key, _)| seen.insert(*key)).collect();
    kept.reverse();
    kept
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingInfo {
    pub binding: BindingId,
    pub name: String,
    pub span: Span,
    pub ty: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeInfo {
    pub span: Span,
    pub ty: String,
}

#[derive(Clone, Debug, Default)]
pub struct CheckReport {
    pub diagnostics: Vec<Diagnostic>,
    pub bindings: Vec<BindingInfo>,
    pub types: Vec<TypeInfo>,
    /// The module names of the `require` calls checked as the standard loader's: a call to a
    /// replacement is an ordinary call and names no module.
    pub imports: Vec<Span>,
    pub bound: Option<std::sync::Arc<BoundFile>>,
    pending_bindings: Vec<(BindingId, Type)>,
    pending_types: Vec<(Span, Type)>,
    pending_diagnostics: Vec<diagnostic_types::PendingDiagnostic>,
}

impl CheckReport {
    fn append(&mut self, mut other: Self) {
        let diagnostic_offset = self.diagnostics.len();
        for pending in &mut other.pending_diagnostics {
            pending.index += diagnostic_offset;
        }
        self.diagnostics.append(&mut other.diagnostics);
        self.bindings.append(&mut other.bindings);
        self.types.append(&mut other.types);
        self.imports.append(&mut other.imports);
        self.pending_bindings.append(&mut other.pending_bindings);
        self.pending_types.append(&mut other.pending_types);
        self.pending_diagnostics.append(&mut other.pending_diagnostics);
    }

    /// Drops recorded binding and expression types when nothing will print or
    /// query them (as Bun prints only diagnostics). Diagnostics are kept.
    pub(crate) fn discard_types(&mut self) {
        self.bindings.clear();
        self.types.clear();
        self.pending_bindings.clear();
        self.pending_types.clear();
    }

    fn render(&mut self, engine: &mut dyn TypeQueries) {
        self.render_until(engine, &|| false);
    }

    /// Describes each recorded binding and expression once, with the type it was recorded with
    /// last (a body checked again on a later pass records it again), until `cancelled`: a
    /// cancelled check's report is discarded, so what is left undescribed is dropped.
    fn render_until(&mut self, engine: &mut dyn TypeQueries, cancelled: &dyn Fn() -> bool) {
        let mut bindings = HashMap::with_capacity_and_hasher(self.pending_bindings.len(), Default::default());
        for (binding, ty) in last_of_each(std::mem::take(&mut self.pending_bindings)) {
            if cancelled() { break; }
            bindings.insert(binding, describe_native(&ty, engine, &mut Vec::new()));
        }
        for info in &mut self.bindings {
            if let Some(displayed) = bindings.get(&info.binding) {
                info.ty.clone_from(displayed);
            }
        }
        let mut types = HashMap::with_capacity_and_hasher(self.pending_types.len(), Default::default());
        let pending = std::mem::take(&mut self.pending_types).into_iter().map(|(span, ty)| ((span.start, span.end), ty)).collect();
        for (span, ty) in last_of_each(pending) {
            if cancelled() { break; }
            types.insert(span, describe_native(&ty, engine, &mut Vec::new()));
        }
        for info in &mut self.types {
            if let Some(displayed) = types.get(&(info.span.start, info.span.end)) {
                info.ty.clone_from(displayed);
            }
        }
        if cancelled() {
            self.pending_diagnostics.clear();
        } else {
            self.render_diagnostics(engine);
        }
    }

    /// Returns an owned report's displayed type at an original UTF-8 byte offset.
    pub fn type_at(&self, byte_offset: usize) -> Option<&str> {
        if let Some(occurrence) = self
            .bound
            .as_ref()
            .and_then(|bound| bound.occurrence_at(byte_offset))
        {
            if matches!(
                occurrence.kind,
                bind::OccurrenceKind::Declaration | bind::OccurrenceKind::Annotation | bind::OccurrenceKind::TypeQuery
            ) {
                let binding = occurrence.binding?;
                return self
                    .bindings
                    .iter()
                    .find(|info| info.binding == binding)
                    .map(|info| info.ty.as_str());
            }
        }
        if let Some(info) = self
            .types
            .iter()
            .filter(|info| info.span.start <= byte_offset && byte_offset < info.span.end)
            .min_by_key(|info| info.span.end - info.span.start)
        {
            return Some(&info.ty);
        }
        let binding = self.bound.as_ref()?.binding_at(byte_offset)?;
        self.bindings
            .iter()
            .find(|info| info.binding == binding)
            .map(|info| info.ty.as_str())
    }
}

/// V1 broadens a single primitive literal at an inferred local declaration,
/// including a regular literal read from a field. Literal unions and written
/// contracts are separate cases and retain their existing native behavior.
fn inferred_local_constraint(value: &Type, engine: &mut dyn TypeQueries) -> Type {
    let value = if engine.is_primitive_literal(value) {
        engine.base_of_literal(value)
    } else {
        value.clone()
    };
    engine.widen(&value)
}

#[derive(Clone)]
struct Binding {
    constraint: Type,
    /// Immutable graph-entry type; assignments only replace `value`.
    initial: Type,
    value: Type,
    evolves_from_nil: bool,
}

fn automatic_nil_initializer(values: &[Expr], index: usize) -> bool {
    let Some(mut value) = values.get(index) else {
        return !values.last().is_some_and(|value| {
            matches!(value.kind, ExprKind::Call { .. } | ExprKind::MethodCall { .. } | ExprKind::Vararg)
        });
    };
    while let ExprKind::Parenthesized(inner) = &value.kind {
        value = inner;
    }
    matches!(value.kind, ExprKind::Nil)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Reference {
    binding: BindingId,
    path: Vec<String>,
}

impl Reference {
    fn is_strict_ancestor_of(&self, other: &Self) -> bool {
        self.binding == other.binding
            && self.path.len() < other.path.len()
            && other.path.starts_with(&self.path)
    }
}

enum Write {
    Local(BindingId, Type),
    Field(Reference, Type),
}

/// One evaluation's static view and physical environment origin. Assertions
/// replace only the view; this transient result is never cached on the checker.
struct EvaluatedExpression {
    ty: Type,
    environment: Option<NativeEnvironmentKey>,
}

struct Checker<'a> {
    engine: &'a mut dyn TypeQueries,
    bound: &'a BoundFile,
    native: Option<&'a NativeRegistry>,
    type_context: Option<NativeFunctionKey>,
    named_type_context: Option<native_named::NativeNamedKey>,
    signature_type_context: Option<luanalysis_engine::NativeSignatureKey>,
    contextual_function: Option<ContextualFunctionContext>,
    captured_function: Option<CapturedFunctionContext>,
    incomplete_flow: bool,
    /// A value capability demand must retain possible runtime alternatives even
    /// when their callable shapes would reduce to the same structural type.
    runtime_value: bool,
    annotation_depth: usize,
    active_assertions: HashSet<(usize,usize)>,
    check_mode: NativeCheckMode,
    flow_at: Option<NativeFlowKey>,
    /// Selected predicate from this checker's most recent original call.
    call_predicate: Option<(Span, luanalysis_engine::TypePredicate)>,
    locals: HashMap<BindingId, Binding>,
    assigned_fields: HashMap<Reference, Type>,
    report: CheckReport,
    return_type: Option<Type>,
    return_values: Vec<Type>,
    return_pack: Option<luanalysis_engine::ValuePack>,
    return_packs: Vec<luanalysis_engine::ValuePack>,
}

/// Checks the documented native subset without generating TypeScript source.
pub fn check(source: &str) -> CheckReport {
    check_with_options(source, CheckOptions::default())
}

/// Checks one source with an explicit overload compatibility policy.
/// Both modes validate the same private implementation and function body.
pub fn check_with_overload_mode(source: &str, overload_mode: OverloadMode) -> CheckReport {
    check_with_options(source, CheckOptions { overload_mode, ..CheckOptions::default() })
}

/// Checks original Lua with explicit source declaration policies. Missing
/// contracts keep their inferred/contextual type regardless of strict reporting.
pub fn check_with_options(source: &str, options: CheckOptions) -> CheckReport {
    let file = luanalysis_syntax::parse_with_options(source, luanalysis_syntax::ParseOptions { emmylua: options.emmylua_syntax, decay_optionals: options.decay_optionals }, || {});
    if file.has_lua_syntax_errors() {
        return CheckReport {
            diagnostics: file
                .diagnostics
                .into_iter()
                .map(|error| Diagnostic {
                    code: "LUA1001",
                    message: error.message,
                    span: error.span,
                })
                .collect(),
            bindings: Vec::new(),
            types: Vec::new(),
            imports: Vec::new(),
            bound: None,
            pending_bindings: Vec::new(),
            pending_types: Vec::new(),
            pending_diagnostics: Vec::new(),
        };
    }
    let bound = bind::bind(&file);
    Engine::new().with_strict_nil(options.strict_nil).with_decayed_optionals(options.decay_optionals).with_native_provider(
        move || {
            let mut registry = NativeRegistry::new(file, bound);
            registry.set_overload_mode(options.overload_mode);
            registry.set_no_implicit_any(options.no_implicit_any);
            registry.set_no_unsafe_any(options.no_unsafe_any);
            registry.set_legacy_builtin_policy(builtins::LegacyBuiltinPolicy::MinimalTypeCompatibility);
            registry
        },
        |registry, engine| {
            registry.check_named_declarations(engine);
            registry.check_global_declarations(engine);
            let mut report = {
                let mut checker = Checker {
                    active_assertions: HashSet::default(),
                    engine,
                    bound: registry.bound(),
                    native: Some(registry),
                    type_context: None,
                    named_type_context: None,
                    signature_type_context: None,
                    contextual_function: None,
                    captured_function: None,
                    incomplete_flow: false,
                    runtime_value: false,
                    annotation_depth: 0,
                    check_mode: NativeCheckMode::empty(),
                    flow_at: Some(registry.flow_entry(None, None)),
                    call_predicate: None,
                    locals: HashMap::default(),
                    assigned_fields: HashMap::default(),
                    report: CheckReport::default(),
                    return_type: None,
                    return_values: Vec::new(),
                    return_pack: None,
                    return_packs: Vec::new(),
                };
                checker.statements(registry.statements());
                checker.report
            };
            registry.check_all_bodies(engine);
            while registry.retry_contextual_owner_validations(engine) {}
            registry.check_unread_annotation_pieces(engine);
            registry.check_deferred_constraints(engine);
            report.append(registry.take_report());
            report.render(engine);
            while registry.retry_contextual_owner_validations(engine) {}
            registry.report_unresolved_contextual_owners();
            // Explicit display demands can complete a previously unqueried signature.
            report.append(registry.take_report());
            report.render_diagnostics(engine);
            let syntax = registry.syntax();
            report.diagnostics.retain(|diagnostic| !syntax.suppresses(diagnostic.code, diagnostic.span.start));
            report.diagnostics.sort_by(|a, b| {
                (a.span.start, a.span.end, a.code, &a.message).cmp(&(
                    b.span.start,
                    b.span.end,
                    b.code,
                    &b.message,
                ))
            });
            report.diagnostics.dedup();
            report.bindings.sort_by_key(|binding| binding.span.start);
            report.bindings.dedup();
            report
                .types
                .sort_by_key(|info| (info.span.start, info.span.end));
            report.types.dedup();
            report.imports.sort_by_key(|span| (span.start, span.end));
            report.imports.dedup();
            report.bound = Some(registry.bound_arc());
            report
        },
    )
}

impl Checker<'_> {
    fn resolve_local(&mut self, binding: BindingId) -> Option<&Binding> {
        if let Some(native) = self.native {
            if let Some(context) = self
                .captured_function
                .or_else(|| self.engine.current_captured_function())
            {
                let self_value = native
                    .binding_function_key(binding)
                    .filter(|key| Some(*key) == self.type_context)
                    .and_then(|key| {
                        let value = self.engine.captured_function_value(&context);
                        (self.engine.native_function_key(&value) == Some(key)).then_some(value)
                    });
                if let Some(ty) = self_value.or_else(|| {
                    self.engine
                        .captured_binding(&context, native.source_binding_key(binding))
                }) {
                    let evolves_from_nil = self.engine.is_auto_variable_constraint(&ty);
                    self.locals.insert(
                        binding,
                        Binding {
                            constraint: ty.clone(),
                            initial: if evolves_from_nil { Type::Nil } else { ty.clone() },
                            value: ty,
                            evolves_from_nil,
                        },
                    );
                }
            }
        }
        if !self.locals.contains_key(&binding) {
            if let Some(native) = self.native {
                let context = self
                    .contextual_function
                    .or_else(|| self.engine.current_contextual_function());
                let value = if let Some((value, incomplete)) = native.iterator_binding_in_current_context(binding, self.engine) {
                    self.incomplete_flow |= incomplete;
                    Some(value)
                } else if let Some(context) = context {
                    native.resolve_binding_in_context(binding, &context, self.engine)
                } else {
                    native.resolve_binding(binding, self.engine)
                };
                if let Some(value) = value {
                    self.locals.insert(binding, value);
                }
            }
        }
        self.locals.get(&binding)
    }

    fn record_binding(&mut self, name: &Name, binding: BindingId, ty: &Type) {
        self.record_binding_at(&name.text, name.span, binding, ty);
    }

    fn record_semantic_binding(&mut self, binding: BindingId, ty: &Type) {
        let declaration = self.bound.declaration(binding);
        let name = declaration.name.clone();
        let span = declaration.span;
        self.record_binding_at(&name, span, binding, ty);
    }

    fn record_binding_at(&mut self, name: &str, span: Span, binding: BindingId, ty: &Type) {
        if let Some(native) = self.native.filter(|_| {
            self.contextual_function.is_none()
                && self.captured_function.is_none()
                && !self.incomplete_flow
                && native_expressions::publishes(self.check_mode)
        }) {
            if let Some(value) = self.locals.get(&binding) {
                native.publish_binding(binding, value);
            }
        }
        self.report.bindings.push(BindingInfo {
            binding,
            name: name.to_owned(),
            span,
            ty: describe(ty),
        });
        self.report.pending_bindings.push((binding, ty.clone()));
    }

    fn record_expression_type(&mut self, span: Span, ty: &Type) {
        if !self.incomplete_flow && let Some(native) = self.native {
            native.record_probed_type(span, ty);
        }
        self.report.types.push(TypeInfo {
            span,
            ty: describe(ty),
        });
        self.report.pending_types.push((span, ty.clone()));
    }
    /// Returns true when this straight-line block terminates with a return or
    /// an explicitly declared nonreturning call.
    fn statements(&mut self, statements: &[Stmt]) -> bool {
        let mut unreachable_reported = false;
        for (index, statement) in statements.iter().enumerate() {
            crate::project_check::unwind_if_cancelled();
            let limits = self.engine.unplaced_limits();
            let rest = &statements[index + 1..];
            match &statement.kind {
                StmtKind::Local {
                    names,
                    values,
                    annotation,
                    ..
                } => {
                    self.local(names, values, annotation.as_ref());
                }
                StmtKind::Assign {
                    targets,
                    values,
                    annotation,
                } => {
                    self.assign(targets, values, annotation.as_ref());
                }
                StmtKind::Block { body, .. } => {
                    if self.statements(body) {
                        if !unreachable_reported { self.report_unreachable(rest); }
                        if !rest.iter().any(|statement| matches!(statement.kind, StmtKind::Label(_))) { return true; }
                        unreachable_reported = true;
                    }
                }
                StmtKind::LocalFunction { name, function } => self.local_function(name, function),
                StmtKind::FunctionDeclaration {
                    target,
                    function,
                    method_span,
                } => {
                    self.named_function(target, function, *method_span);
                }
                StmtKind::Return(values) => {
                    self.return_statement(values, statement.span);
                    self.report_unplaced_limit(limits, statement.span);
                    return true;
                }
                StmtKind::Call(expr) => {
                    let result = match &expr.kind {
                        ExprKind::Call { callee, args } => {
                            self.call_expression(callee, args, expr.span, true)
                        }
                        ExprKind::MethodCall {
                            receiver,
                            name,
                            args,
                        } => self.method_call_expression(receiver, name, args, expr.span, true),
                        _ => Type::Any,
                    };
                    let terminates = match (self.native, self.flow_at) {
                        (Some(native), Some(at)) => native.call_statement_terminates(expr, at, self.engine),
                        _ => matches!(result, Type::Never),
                    };
                    if terminates {
                        if !unreachable_reported { self.report_unreachable(rest); }
                        // Statements after a later label are reached by the `goto`s naming it.
                        if !rest.iter().any(|statement| matches!(statement.kind, StmtKind::Label(_))) { return true; }
                        unreachable_reported = true;
                    }
                }
                StmtKind::If {
                    branches,
                    else_branch,
                } => {
                    let mut every_branch_terminates = else_branch.is_some();
                    for branch in branches {
                        let condition = self.expression(&branch.condition);
                        self.inspect_condition(&branch.condition, &condition);
                        every_branch_terminates &= self.statements(&branch.body);
                    }
                    if let Some(branch) = else_branch {
                        every_branch_terminates &= self.statements(&branch.body);
                    }
                    // The rest is still checked, as before; it is only reported unreachable.
                    if every_branch_terminates && !unreachable_reported {
                        unreachable_reported = true;
                        self.report_unreachable(rest);
                    }
                }
                StmtKind::While {
                    condition, body, ..
                } => {
                    let ty = self.expression(condition);
                    self.inspect_condition(condition, &ty);
                    self.statements(body);
                }
                StmtKind::Repeat {
                    body, condition, ..
                } => {
                    self.statements(body);
                    let ty = self.expression(condition);
                    self.inspect_condition(condition, &ty);
                }
                StmtKind::NumericFor {
                    name,
                    name_type,
                    start,
                    limit,
                    step,
                    body,
                    ..
                } => {
                    self.numeric_for_header(start);
                    self.numeric_for_header(limit);
                    if let Some(step) = step {
                        let step_type = self.numeric_for_header(step);
                        if flow::numeric_literal(step).is_some_and(|value| match value {
                            flow::NumericLiteral::Integer(value) => value == 0,
                            flow::NumericLiteral::Float(value) => value == 0.0,
                        }) || matches!(self.engine.inspect_type(&step_type), Type::NumberLiteral(value) if value == 0.0)
                        {
                            self.error("LUA2030", step.span, "numeric for step must be nonzero");
                        }
                    }
                    let binding = self
                        .bound
                        .binding_at(name.span.start)
                        .expect("bound Lua numeric loop variable");
                    match name_type {
                        // A written type is the variable's contract, as a typed local's is. It
                        // may refine the loop's number (`integer`), but must admit some number.
                        Some(written) => {
                            let constraint = self.annotation(written);
                            if !self.engine.is_assignable(&constraint, &Type::Number) {
                                self.compatible(&Type::Number, &constraint, name.span);
                            }
                            self.locals.insert(binding, Binding {
                                initial: constraint.clone(), value: constraint.clone(), constraint: constraint.clone(), evolves_from_nil: false,
                            });
                            self.record_binding(name, binding, &constraint);
                        }
                        None => self.record_binding(name, binding, &Type::Number),
                    }
                    self.statements(body);
                }
                StmtKind::GenericFor { names, name_types, body, .. } => {
                    if let (Some(native), Some(at)) = (self.native, self.flow_at) {
                        for (name, written) in names.iter().zip(name_types) {
                            let id = self
                                .bound
                                .binding_at(name.span.start)
                                .expect("bound Lua generic loop variable");
                            let (binding, incomplete) = native
                                .iterator_body_binding(id, at, self.engine)
                                .expect("an original generic loop has a body binding recipe");
                            self.incomplete_flow |= incomplete;
                            if let Some(written) = written {
                                // A written type is the variable's contract, as a typed local's
                                // is: each value the iterator gives it is checked against it.
                                let constraint = self.annotation(written);
                                self.compatible(&binding.value, &constraint, name.span);
                                let value = self.engine.assignment_value(&constraint, &binding.value);
                                self.locals.insert(id, Binding {
                                    initial: constraint.clone(), constraint: constraint.clone(), value, evolves_from_nil: false,
                                });
                                self.record_binding(name, id, &constraint);
                                continue;
                            }
                            let constraint = binding.constraint.clone();
                            self.locals.insert(id, binding);
                            self.record_binding(name, id, &constraint);
                        }
                    } else {
                        self.error(
                            "LUA2023",
                            statement.span,
                            "generic iteration requires an original native flow context",
                        );
                    }
                    self.statements(body);
                }
                StmtKind::Break | StmtKind::Goto(_) => {
                    if !unreachable_reported {
                        unreachable_reported = true;
                        self.report_unreachable(rest);
                    }
                }
                // A label is reached by the `goto`s naming it.
                StmtKind::Label(_) => unreachable_reported = false,
            }
            self.report_unplaced_limit(limits, statement.span);
        }
        false
    }

    /// A limit (instantiation depth, union size, tuple size) that the native checker hit while
    /// checking what is at `span`, since it had hit `since`, and could not report there itself:
    /// tsgo reports it at the node it is checking. Not where a conditional type's branch is
    /// lowered while another type is evaluated: that is still the other's check.
    /// A check that does not publish its diagnostics claims the place of one it saw first, for
    /// the check around it that publishes.
    pub(crate) fn report_unplaced_limit(&mut self, since: u64, span: Span) {
        let Some(native) = self.native else { return };
        if native.types.lowering_annotation_piece() { return }
        let hits = self.engine.unplaced_limits();
        if hits <= since { return }
        if !native_expressions::publishes(self.check_mode) {
            native.types.claim_unplaced_limit(since, hits, span);
            return;
        }
        let Some(code) = self.engine.take_unplaced_limit_since(since) else { return };
        let span = native.types.claimed_unplaced_limit(since).unwrap_or(span);
        let (code, message) = match code {
            2590 => ("TS2590", "Expression produces a union type that is too complex to represent."),
            2799 => ("TS2799", "Type produces a tuple type that is too large to represent."),
            2800 => ("TS2800", "Expression produces a tuple type that is too large to represent."),
            _ => ("TS2589", "Type instantiation is excessively deep and possibly infinite."),
        };
        self.error(code, span, message);
    }

    fn error(&mut self, code: &'static str, span: Span, message: impl Into<String>) {
        self.report.diagnostics.push(Diagnostic {
            code,
            span,
            message: message.into(),
        });
    }

    fn numeric_for_header(&mut self, expression: &Expr) -> Type {
        let ty = self.expression(expression);
        self.compatible(&ty, &Type::Number, expression.span);
        ty
    }

    fn annotation(&mut self, ty: &TypeExpr) -> Type {
        let depth = self.annotation_depth;
        self.annotation_depth = depth + 1;
        let limits = self.engine.unplaced_limits();
        let resolved = self.annotation_node(ty);
        let resolved = self.sequence_annotation(ty, resolved);
        let resolved = self.conditional_flow_type(ty, resolved);
        self.report_unplaced_limit(limits, ty.span);
        self.annotation_depth = depth;
        if depth == 0 {
            if let Some(context) = self.captured_function {
                return self.engine.instantiate_in_capture(&context, &resolved);
            }
        }
        resolved
    }

    fn annotation_node(&mut self, ty: &TypeExpr) -> Type {
        if let Some(resolved)=self.native_anonymous_annotation(ty){return resolved;}
        match &ty.kind {
            TypeExprKind::Named(name) => match name.as_str() {
                "any" => Type::Any,
                "unknown" => Type::Unknown,
                "never" => Type::Never,
                "nil" => Type::Nil,
                "boolean" => Type::Boolean,
                // LuaLS's `integer` is a number; Lua's number domain is not split here.
                "number" | "integer" => Type::Number,
                "string" => Type::String,
                "table" => Type::RuntimeTable,
                "function" => Type::RuntimeFunction,
                "void" => Type::Void,
                _ => {
                    // An `infer` type parameter is seen where it is written, whichever context
                    // lowers what has the reference.
                    if let Some(key)=self.native.and_then(|native| native.types.visible_annotation_parameter(name, ty.span)) {
                        return Type::NativeTypeParameter(key);
                    }
                    if let Some(native)=self.native {
                        let context=self.signature_type_context.map(native_named::NativeTypeContext::Signature)
                            .or_else(||self.named_type_context.map(native_named::NativeTypeContext::NamedType))
                            .or_else(||self.type_context.map(native_named::NativeTypeContext::Function));
                        let resolution = native.annotation_parameter_resolution(context,name);
                        let contextual_self = name == "self" && match &resolution {
                            native_types::TypeNameResolution::Parameter(key) => native.types.is_semantic_self_parameter(*key),
                            native_types::TypeNameResolution::Missing => true,
                            native_types::TypeNameResolution::Ambiguous(_) => false,
                        };
                        if contextual_self
                            && let Some(function) = context.and_then(|context| native.metatable_annotation_function(context))
                            && native.is_string_method(function, self.engine)
                        { return Type::String; }
                        if contextual_self
                            && let Some(function) = context.and_then(|context| native.metatable_annotation_function(context))
                            && let Some(receiver) = native.applied_method_self_type(function, self.engine)
                        { return receiver; }
                        if contextual_self
                            && let Some(function) = context.and_then(|context| native.metatable_annotation_function(context))
                            && native.method_owner(function).is_none()
                        {
                            match native.contextual_self_type(function, self.engine) {
                                luanalysis_engine::ContextualTypeOwner::Owner(ty) => return ty,
                                luanalysis_engine::ContextualTypeOwner::Pending => {
                                    self.incomplete_flow = true;
                                    return self.engine.error_type();
                                }
                                luanalysis_engine::ContextualTypeOwner::Absent => {},
                            }
                        }
                        if contextual_self
                            && let Some(function) = context.and_then(|context| native.metatable_annotation_function(context))
                            && let Some(owner) = native.original_literal_self_type(function, self.engine)
                        { return owner; }
                        match resolution {
                            native_types::TypeNameResolution::Parameter(key)=>return Type::NativeTypeParameter(key),
                            native_types::TypeNameResolution::Ambiguous(_)=>{
                                self.error("TS2300",ty.span,format!("Ambiguous generic parameter '{name}'."));
                                return self.engine.error_type();
                            },
                            native_types::TypeNameResolution::Missing=>{},
                        }
                    }
                    // self is contextual syntax in v1. The standard library's
                    // documentation placeholder is never a global type owner. Outside any
                    // member it names nothing, which TypeScript reports for `this` (TS2526).
                    if name == "self" {
                        if let Some(native) = self.native {
                            let context = self.signature_type_context.map(native_named::NativeTypeContext::Signature)
                                .or_else(|| self.named_type_context.map(native_named::NativeTypeContext::NamedType))
                                .or_else(|| self.type_context.map(native_named::NativeTypeContext::Function));
                            if !native.annotation_has_member_self(context, ty.span) {
                                self.error("TS2526", ty.span, "A 'self' type is available only in a member of a class or table.");
                                return self.engine.error_type();
                            }
                        }
                        return Type::Unknown;
                    }
                    if let Some(resolved)=self.native_named_reference(name,&[],ty){return resolved;}
                    if name == luanalysis_syntax::DECLARED_TYPES {
                        let fields = self.declared_types().into_iter()
                            .map(|(name, ty)| luanalysis_engine::TableField::new(luanalysis_engine::LuaKey::String(name.into_bytes()), ty))
                            .collect();
                        let table = luanalysis_engine::TableType { fields, indexers: Vec::new(), sequence_tails: Vec::new(), collision_candidates: false, fresh: false };
                        return self.engine.table_type(&table).unwrap_or_else(|_| self.engine.error_type());
                    }
                    self.error(
                        "TS2304",
                        ty.span,
                        format!("Cannot find name '{name}'.{}", emmylua_builtin_hint(name)),
                    );
                    self.engine.error_type()
                }
            },
            TypeExprKind::Union(types) => {
                let lowered: Vec<_> = types.iter().map(|ty| self.annotation(ty)).collect();
                if types.iter().any(TypeExpr::is_completion_only) {
                    self.engine.union_reduced(&lowered)
                } else {
                    Type::Union(lowered)
                }
            }
            TypeExprKind::Intersection(types) => Type::Intersection(types.iter().map(|ty| self.annotation(ty)).collect()),
            TypeExprKind::CompletionHint(_) => Type::Never,
            TypeExprKind::Conditional { .. } => self.conditional_annotation(ty),
            TypeExprKind::Mapped(mapped) => self.mapped_annotation(mapped, ty.span),
            TypeExprKind::Infer { name, .. } => self.infer_annotation(name, ty.span),
            TypeExprKind::TemplateLiteral { texts, types } => {
                // As TypeScript requires, each placeholder is of a type that can be written as text.
                let placeholder = Type::Union(vec![Type::String, Type::Number, Type::Boolean, Type::Nil]);
                let types: Vec<_> = types.iter().map(|syntax| {
                    let ty = self.annotation(syntax);
                    if !self.engine.is_assignable(&ty, &placeholder) {
                        self.error_types("TS2322", syntax.span, "Type '{}' is not assignable to type '{}'.", &[&ty, &placeholder]);
                        return self.engine.error_type();
                    }
                    ty
                }).collect();
                self.engine.template_literal_type(texts, &types)
            }
            TypeExprKind::TypePredicate { asserts: None, .. } => Type::Boolean,
            // An assertion function returns nothing when it returns.
            TypeExprKind::TypePredicate { asserts: Some(_), .. } => Type::Void,
            TypeExprKind::KeyOf(operand) => {
                let operand = self.annotation(operand);
                self.engine.key_type(&operand)
            }
            TypeExprKind::IndexedAccess { object, index } => self.indexed_access_annotation(object, index),
            TypeExprKind::TypeQuery(path) => self.type_query_annotation(path),
            TypeExprKind::Literal(literal) => {
                let value = match literal {
                    TypeLiteral::Boolean(value) => Type::BooleanLiteral(*value),
                    TypeLiteral::String(value) => Type::StringLiteral(value.clone()),
                    TypeLiteral::Number(spelling) => number(spelling)
                        .map(Type::NumberLiteral)
                        .unwrap_or_else(|| {
                            self.error("LUA2002", ty.span, "invalid numeric literal type");
                            self.engine.error_type()
                        }),
                };
                self.engine.regular_type(&value)
            },
            TypeExprKind::Record(fields) => self.native_table_annotation(fields, ty.span),
            TypeExprKind::Parenthesized(ty) => self.annotation(ty),
            TypeExprKind::Function(function) => self.function_annotation(function),
            TypeExprKind::Array { element, .. } => self.native_array_annotation(element),
            TypeExprKind::Generic { base, args, .. } if base.text == "table" => {
                self.native_dictionary_annotation(args, ty.span)
            }
            TypeExprKind::Generic { base, args, .. } => {
                let arguments:Vec<_>=args.iter().map(|arg|self.annotation(arg)).collect();
                if base.text == "self" || self.native.and_then(|native| native.types.visible_annotation_parameter(&base.text, base.span)).is_some() {
                    self.error("TS2315", base.span, format!("Type '{}' is not generic.", base.text));
                    return self.engine.error_type();
                }
                if let Some(resolved)=self.native_named_reference(&base.text,&arguments,ty){return resolved;}
                // TypeScript's intrinsic string types, where no declaration has the name.
                if let Some(mapping)=luanalysis_engine::StringMapping::named(&base.text) {
                    let [argument]=arguments.as_slice() else {
                        self.error("TS2314", ty.span, format!("Generic type '{}' requires 1 type argument(s).", base.text));
                        return self.engine.error_type();
                    };
                    if !self.engine.is_assignable(argument, &Type::String) {
                        self.error_types("TS2344", args[0].span, "Type '{}' does not satisfy the constraint '{}'.", &[argument, &Type::String]);
                        return self.engine.error_type();
                    }
                    return self.engine.string_mapping_type(mapping, argument);
                }
                // So is `NoInfer<T>`.
                if base.text == "NoInfer" {
                    let [argument]=arguments.as_slice() else {
                        self.error("TS2314", ty.span, "Generic type 'NoInfer' requires 1 type argument(s).");
                        return self.engine.error_type();
                    };
                    return self.engine.no_infer_type(argument);
                }
                if base.text == luanalysis_syntax::DECLARED_TYPE_NAMES {
                    let names: Vec<_> = self.declared_types().into_iter()
                        .filter(|(_, ty)| arguments.first().is_none_or(|constraint| self.engine.is_assignable(ty, constraint)))
                        .map(|(name, _)| Type::StringLiteral(name.into_bytes()))
                        .collect();
                    return if names.is_empty() { Type::Never } else { Type::Union(names) };
                }
                let parameter=self.native.map(|native| {
                    let context=self.signature_type_context.map(native_named::NativeTypeContext::Signature)
                        .or_else(||self.named_type_context.map(native_named::NativeTypeContext::NamedType))
                        .or_else(||self.type_context.map(native_named::NativeTypeContext::Function));
                    native.annotation_parameter_resolution(context,&base.text)
                });
                let (code,message)=match parameter {
                    Some(native_types::TypeNameResolution::Ambiguous(_))=>("TS2300",format!("Ambiguous generic parameter '{}'.",base.text)),
                    Some(native_types::TypeNameResolution::Parameter(_))=>("TS2315",format!("Type '{}' is not generic.",base.text)),
                    _ if matches!(base.text.as_str(),"any"|"unknown"|"never"|"nil"|"boolean"|"number"|"string"|"void")=>("TS2315",format!("Type '{}' is not generic.",base.text)),
                    // Elsewhere than a function's result, EmmyLua Analyzer's `TypeGuard<T>` is a boolean.
                    _ if base.text == "TypeGuard" && self.native.is_some_and(|native| native.syntax().options.emmylua) => return Type::Boolean,
                    _=>("TS2304",format!("Cannot find name '{}'.{}",base.text,emmylua_builtin_hint(&base.text))),
                };
                self.error(code,base.span,message);
                self.engine.error_type()
            }
            TypeExprKind::Error => self.engine.error_type(),
        }
    }

    fn expression(&mut self, expr: &Expr) -> Type {
        self.expression_with_expected(expr, None)
    }

    fn expression_with_expected(&mut self, expr: &Expr, expected: Option<&Type>) -> Type {
        self.expression_value(expr, expected, false).ty
    }

    fn receiver_expression(&mut self, expr: &Expr) -> EvaluatedExpression {
        // The normal read already establishes the runtime table's origin.
        // Retain that completed fact at its original input so a later guard
        // does not need to repeat the receiver's flow traversal.
        let observed = self.native.zip(self.flow_at)
            .and_then(|(native, at)| native.environment_receiver_read_key(expr, at))
            .and_then(|(value, input)| self.engine.begin_environment_read(value, input));
        let value = self.expression_value(expr, None, true);
        if let Some(observed) = observed {
            self.engine.end_environment_read(observed, value.environment, !self.incomplete_flow);
        }
        value
    }

    fn expression_value(&mut self, expr: &Expr, expected: Option<&Type>, capture_origin: bool) -> EvaluatedExpression {
        let limits = self.engine.unplaced_limits();
        let previous = self.flow_at;
        if let (Some(native), Some(at)) = (self.native, previous) {
            self.flow_at = Some(native.flow_input_for_expression(expr, at));
        }
        let value = if let Some(value) = self.asserted_expression_value(expr, capture_origin) { value }
            else { self.expression_value_at_current_flow(expr, expected, capture_origin) };
        self.flow_at = previous;
        self.report_unplaced_limit(limits, expr.span);
        value
    }

    fn expression_at_current_flow(&mut self, expr: &Expr, expected: Option<&Type>) -> Type {
        self.expression_value_at_current_flow(expr, expected, false).ty
    }

    fn expression_value_at_current_flow(&mut self, expr: &Expr, expected: Option<&Type>, capture_origin: bool) -> EvaluatedExpression {
        let mut environment = None;
        let ty = match &expr.kind {
            ExprKind::Nil => Type::Nil,
            ExprKind::Boolean(value) => Type::BooleanLiteral(*value),
            ExprKind::Number(spelling) => match number(spelling) {
                Some(value) => Type::NumberLiteral(value),
                None => {
                    self.error("LUA2002", expr.span, "invalid numeric literal");
                    Type::Any
                }
            },
            ExprKind::String(value) => Type::StringLiteral(value.clone()),
            ExprKind::Vararg => {
                let pack = self.expression_pack(expr, None);
                self.engine.pack_slot(&pack, 0)
            }
            ExprKind::Name(name) => match self
                .bound
                .binding_at(name.span.start)
                .and_then(|id| self.resolve_local(id).cloned())
            {
                Some(binding) => {
                    if let (Some(native), Some(at)) = (self.native, self.flow_at) {
                        // A table that stores in several functions describe enters another
                        // function as its schema, as a JavaScript object with expando
                        // properties has all of them inside a function, wherever it was created.
                        let id = self.bound.binding_at(name.span.start).unwrap();
                        let schema = (self.type_context.is_some() && native.binding_function_owner(id) != self.type_context)
                            .then(|| self.engine.runtime_table_schema(&binding.constraint)).flatten();
                        let declared = schema.clone().unwrap_or_else(|| binding.constraint.clone());
                        let initial = schema.unwrap_or_else(|| binding.initial.clone());
                        let result = native.flow_reference_type(expr, at, &declared, &initial, self.engine);
                        self.incomplete_flow |= result.incomplete;
                        // As in TypeScript, an automatic local whose type cannot be determined
                        // is reported only under `--no-implicit-any`; otherwise it is `any`.
                        if result.unresolved_auto && !result.incomplete && native.no_implicit_any()
                            && native_expressions::publishes(self.check_mode)
                        {
                            let declaration = self.bound.declaration(self.bound.binding_at(name.span.start).unwrap());
                            let declaration_span = declaration.span;
                            let declaration_name = declaration.name.clone();
                            self.error("TS7034", declaration_span, format!("Variable '{declaration_name}' implicitly has type 'any' in some locations where its type cannot be determined."));
                            self.error("TS7005", expr.span, format!("Variable '{declaration_name}' implicitly has an 'any' type."));
                        }
                        if !self.runtime_value && result.ty == Type::Unknown
                            && native.has_standard_callable_initializer(self.bound.binding_at(name.span.start).unwrap())
                            && let Some(written) = self.standard_callable_read(expr, at)
                        { written } else { result.ty }
                    } else {
                        binding.value
                    }
                }
                None => {
                    if self.bound.binding_at(name.span.start).is_none() {
                        if let Some(ty) = self.global_read(name) {
                            self.record_expression_type(expr.span, &ty);
                            let environment = if capture_origin { self.engine.global_environment_origin(&ty) } else { None };
                            return EvaluatedExpression { ty, environment };
                        }
                        if let Some(ty) = self.native.and_then(|native| native.legacy_builtin_value(name)) {
                            self.record_expression_type(expr.span, &ty);
                            let environment = if capture_origin { self.engine.global_environment_origin(&ty) } else { None };
                            return EvaluatedExpression { ty, environment };
                        }
                    }
                    self.error(
                        "TS2304",
                        name.span,
                        format!("Cannot find name '{}'.", name.text),
                    );
                    // As TypeScript recovers from an unresolved name, its value has the error
                    // type, so its uses do not report it again.
                    self.engine.error_type()
                }
            },
            ExprKind::Parenthesized(value) => {
                let value = self.expression_value(value, expected, capture_origin);
                environment = value.environment;
                value.ty
            },
            ExprKind::Table(fields) => self.native_table_expression(fields, expr.span, expected),
            ExprKind::Field { base, name } => {
                let receiver = self.receiver_expression(base);
                let environment = receiver.environment;
                let receiver = self.runtime_member_contract(base,receiver.ty);
                let ty = self.native_named_property_read(&receiver, name, base.span);
                self.member_accessibility(&receiver, name);
                // A reported access keeps its error type, rather than the flow's `any`.
                if self.keeps_error_type(&receiver, &ty) { ty } else { self.native_member_flow_type(expr, ty, environment) }
            }
            ExprKind::Function(function) => {
                if let (Some(native), Some(expected)) = (self.native, expected) {
                    native.source_check_expression(
                        native.expression_key(expr),
                        Some(expected),
                        self.check_mode,
                        self.engine,
                    )
                } else {
                    self.function_expression(function)
                }
            }
            ExprKind::Call { callee, args } => {
                self.call_expression_with_expected(callee, args, expr.span, false, expected)
            }
            ExprKind::Unary { op, op_span, value } => self.unary_expression(*op, *op_span, value),
            ExprKind::Binary {
                op,
                op_span,
                left,
                right,
            } => self.binary_expression(*op, *op_span, left, right),
            ExprKind::Index { base, index } => {
                // Lua freezes the receiver before the key expression executes.
                let receiver = self.receiver_expression(base);
                let environment = receiver.environment;
                let receiver = self.runtime_member_contract(base,receiver.ty);
                let key = self.native_table_key(index);
                let ty = self.native_table_read(&receiver, &key, index.span);
                if self.keeps_error_type(&receiver, &ty) { ty } else { self.native_member_flow_type(expr, ty, environment) }
            }
            ExprKind::MethodCall {
                receiver,
                name,
                args,
            } => self.method_call_expression_with_expected(
                receiver, name, args, expr.span, false, expected,
            ),
            ExprKind::Error => Type::Any,
        };
        let ty = self.finish_runtime_table_initializer(expr,expected,ty);
        let ty = self.finish_class_definition_initializer(expr.span, ty);
        self.record_expression_type(expr.span, &ty);
        if capture_origin && !matches!(expr.kind, ExprKind::Parenthesized(_)) {
            environment = self.engine.global_environment_origin(&ty);
        }
        EvaluatedExpression { ty, environment }
    }

    fn compatible(&mut self, value: &Type, target: &Type, origin: Span) -> bool {
        let assignable = self.engine.is_assignable(value, target);
        if !assignable {
            self.error_types(
                "TS2322",
                origin,
                "Type '{}' is not assignable to type '{}'.",
                &[value, target],
            );
        }
        assignable
    }

    fn local(&mut self, names: &[Name], values: &[Expr], annotation: Option<&TypeAnnotation>) {
        let has_initializer = !values.is_empty();
        let mut annotations: Vec<_> = annotation.map_or_else(Vec::new, |annotation| {
            annotation
                .types
                .iter()
                .map(|ty| self.annotation(ty))
                .collect()
        });
        let attached = names.first()
            .and_then(|name| self.bound.binding_at(name.span.start))
            .and_then(|binding| self.shape_binding_constraint(
                binding, annotations.first(), annotation.map(|annotation| annotation.span),
            ));
        if let Some(constraint) = &attached {
            if annotations.is_empty() {
                annotations.push(constraint.clone());
            } else {
                annotations[0] = constraint.clone();
            }
        }
        let expected = attached.map(luanalysis_engine::ValuePack::single).or_else(|| annotation.map(|annotation| {
            native_packs::AnnotationPack::annotation(annotation)
                .lower(&mut |ty| self.annotation(ty))
        })).or_else(|| names.first()
            .and_then(|name| self.bound.binding_at(name.span.start))
            .and_then(|binding| self.class_definition_initializer_context(binding))
            .map(luanalysis_engine::ValuePack::single));
        let automatic: Vec<_> = names.iter().enumerate()
            .map(|(index, _)| annotations.get(index).is_none() && automatic_nil_initializer(values, index))
            .collect();
        let values = self.adjusted_list_values(
            values,
            names.iter().map(|name| name.span),
            expected.as_ref(),
            names.first().map_or(Span::default(), |name| name.span),
        );
        if annotations.len() > names.len() {
            self.error(
                "LUA2003",
                annotation.unwrap().span,
                "more annotated types than declared names",
            );
        }
        for (index, name) in names.iter().enumerate() {
            let (value, origin) = values.get(index).cloned().unwrap_or((Type::Nil, name.span));
            let id = self.bound.binding_at(name.span.start).expect("bound Lua declaration");
            let written = annotations.get(index).is_some()
                || self.native.is_some_and(|native| native.binding_has_written_contract(id));
            let widened = if written { self.engine.widen(&value) }
                else { inferred_local_constraint(&value, self.engine) };
            let evolves_from_nil = automatic[index];
            let constraint = if evolves_from_nil {
                self.engine.auto_variable_constraint()
            } else {
                annotations.get(index).cloned().unwrap_or_else(|| widened.clone())
            };
            // A written contract without an RHS is a declaration. An explicit
            // initializer, including a short list padded with nil, is checked.
            if has_initializer || annotations.get(index).is_none() {
                self.compatible(&value, &constraint, origin);
            }
            let narrowed = self.engine.assignment_value(&constraint, &value);
            let displayed = if evolves_from_nil { Type::Nil } else { constraint.clone() };
            self.locals.insert(
                id,
                Binding {
                    initial: if evolves_from_nil { Type::Nil } else { constraint.clone() },
                    constraint,
                    value: narrowed,
                    evolves_from_nil,
                },
            );
            self.record_binding(name, id, &displayed);
        }
    }

    fn assign(&mut self, targets: &[Expr], values: &[Expr], annotation: Option<&TypeAnnotation>) {
        for target in targets {
            if let ExprKind::Name(name) = &target.kind
                && let Some(binding) = self.bound.binding_at(name.span.start)
                && self.bound.declaration(binding).constant
            {
                self.error("TS2588", name.span, format!("Cannot assign to '{}' because it is a constant.", name.text));
            }
        }
        if let Some(annotation) = annotation
            && !targets.iter().all(|target| self.is_global_annotation_target(target)
                || self.native.is_some_and(|native| native.is_original_assignment_field(target.span)
                    || native.accepts_assignment_annotation(target)))
        {
            self.error(
                "LUA2004",
                annotation.span,
                "@type casts on assignments are not implemented yet",
            );
        }
        // Original local and global contracts contextualize RHS demands before
        // any assignment destination is committed.
        let expected: Vec<_> = targets
            .iter()
            .map(|target| self.assignment_target_context(target))
            .collect();
        let expected_pack =
            luanalysis_engine::ValuePack::Alternatives(vec![luanalysis_engine::PackAlternative {
                fixed: expected
                    .iter()
                    .map(|expected| {
                        luanalysis_engine::PackSlot::required(expected.clone().unwrap_or(Type::Any))
                    })
                    .collect(),
                tail: None,
            }]);
        let mut adjusted = self.adjusted_list_values(
            values,
            targets.iter().map(|target| target.span),
            Some(&expected_pack),
            targets
                .first()
                .map_or(Span::default(), |target| target.span),
        );
        // A class's table stored into one of its fields (`mt.__index = mt`) is the class,
        // as its inferred member is.
        for (index, (value, _)) in adjusted.iter_mut().enumerate() {
            if let (Some(native), Some(expression)) = (self.native, values.get(index))
                && matches!(targets.get(index).map(|target| &target.kind), Some(ExprKind::Field { .. } | ExprKind::Index { .. }))
                && let Some(class) = native.own_definition_value(native.expression_key(expression), self.engine)
            {
                *value = class;
            }
        }
        self.assign_values(targets, adjusted);
    }

    fn assign_values(&mut self, targets: &[Expr], values: Vec<(Type, Span)>) {
        // Validate all destinations against the state before committing writes.
        let mut writes = Vec::new();
        for (index, target) in targets.iter().enumerate() {
            let (value, origin) = values
                .get(index)
                .cloned()
                .unwrap_or((Type::Nil, target.span));
            match &target.kind {
                ExprKind::Name(name) => {
                    let Some(id) = self.bound.binding_at(name.span.start) else {
                        // The chunk's implicit environment is a real Lua
                        // upvalue. Its initial selected object is not a written
                        // shape constraint on later environment replacement.
                        if name.text == "_ENV" { continue; }
                        if let Some(constraint) = self.global_read(name) {
                            // As in TypeScript, a declaration's value is checked against its
                            // own type: one of another type than the global's first is
                            // reported once, as a subsequent declaration (TS2403).
                            let constraint = self.own_global_contract(name).unwrap_or(constraint);
                            self.compatible(&value, &constraint, origin);
                            continue;
                        }
                        self.error(
                            "TS2304",
                            name.span,
                            format!("Cannot find name '{}'.", name.text),
                        );
                        continue;
                    };
                    let Some(binding) = self.resolve_local(id) else {
                        continue;
                    };
                    let constraint = binding.constraint.clone();
                    if self.compatible(&value, &constraint, origin) {
                        writes.push(Write::Local(
                            id,
                            self.engine.assignment_value(&constraint, &value),
                        ));
                    }
                }
                ExprKind::Field { .. } | ExprKind::Index { .. } => {
                    if let Some(native) = self.native
                        && let Some(written) = native.original_assignment_annotation(target.span, self.engine)
                        && !self.compatible(&value, &written, origin)
                    { continue; }
                    let base = match &target.kind {
                        ExprKind::Field { base, .. } | ExprKind::Index { base, .. } => {
                            base.as_ref()
                        }
                        _ => unreachable!(),
                    };
                    let receiver = self.expression(base);
                    if let ExprKind::Field { name, .. } = &target.kind {
                        self.member_accessibility(&receiver, name);
                    }
                    if self.readonly_local(base) {
                        let field = match &target.kind {
                            ExprKind::Field { name, .. } => Some(name.text.clone()),
                            ExprKind::Index { index, .. } => operators::literal_string_key(index)
                                .and_then(|key| std::str::from_utf8(key).ok()).map(str::to_owned),
                            _ => None,
                        };
                        match field {
                            Some(field) => self.error("TS2540", target.span, format!("Cannot assign to '{field}' because it is a read-only property.")),
                            None => self.error_types("TS2542", target.span, "Index signature in type '{}' only permits reading.", &[&receiver]),
                        }
                        continue;
                    }
                    let (key, key_span) = match &target.kind {
                        ExprKind::Field { name, .. } => (
                            TableKey::Literal(LuaKey::String(name.text.as_bytes().to_vec())),
                            name.span,
                        ),
                        ExprKind::Index { index, .. } => (self.native_table_key(index), index.span),
                        _ => unreachable!(),
                    };
                    let checked = self.field_widened_value(target, &value);
                    if self.native_table_write(&receiver, &key, &checked, target.span, key_span, origin) {
                        if let Some(reference) = self.reference(target) {
                            let constraint = self.engine.table_value_context(&receiver, &key);
                            let assigned = constraint.as_ref().map_or_else(
                                || value.clone(),
                                |constraint| self.engine.assignment_value(constraint, &value),
                            );
                            writes.push(Write::Field(reference, assigned));
                        }
                    }
                }
                _ => self.error("LUA2006", target.span, "unsupported assignment destination"),
            }
        }
        // Every field destination captured its old receiver before any stores.
        // A root/member replacement anywhere in this assignment makes a fact
        // about that old table unusable through its former source path. This
        // holds even when the replacement is stored before the field write.
        let replaced: Vec<_> = writes
            .iter()
            .filter_map(|write| match write {
                Write::Local(id, _) => self.locals.get(id).map(|_| Reference {
                    binding: *id,
                    path: Vec::new(),
                }),
                Write::Field(reference, _) => Some(reference.clone()),
            })
            .collect();
        // Match the initial dialect's Lua 5.4 implementation: stores unwind
        // right-to-left, so a repeated destination retains its leftmost value.
        for write in writes.into_iter().rev() {
            match write {
                Write::Local(id, value) => {
                    if let Some(binding) = self.locals.get_mut(&id) {
                        self.assigned_fields
                            .retain(|reference, _| reference.binding != id);
                        binding.value = value;
                    }
                }
                Write::Field(reference, value) => {
                    self.assigned_fields.retain(|previous, _| {
                        previous.binding != reference.binding
                            || !previous.path.starts_with(&reference.path)
                    });
                    if !replaced
                        .iter()
                        .any(|ancestor| ancestor.is_strict_ancestor_of(&reference))
                    {
                        self.assigned_fields.insert(reference, value);
                    }
                }
            }
        }
    }

    /// Whether `expr` names a local that EmmyLua Analyzer's `---@readonly` declares, whose
    /// table's fields are not assigned.
    fn readonly_local(&self, expr: &Expr) -> bool {
        let ExprKind::Name(name) = &expr.kind else { return false };
        let (Some(native), Some(binding)) = (self.native, self.bound.binding_at(name.span.start)) else { return false };
        let declaration = self.bound.declarations[binding.0].span;
        native.syntax().readonly_locals.contains(&declaration)
    }

    fn reference(&self, expr: &Expr) -> Option<Reference> {
        match &expr.kind {
            ExprKind::Name(name) => self.bound.binding_at(name.span.start).map(|id| Reference {
                binding: id,
                path: Vec::new(),
            }),
            ExprKind::Parenthesized(inner) => self.reference(inner),
            ExprKind::Field { base, name } => {
                let mut reference = self.reference(base)?;
                reference.path.push(name.text.clone());
                Some(reference)
            }
            ExprKind::Index { base, index } => {
                let key = std::str::from_utf8(operators::literal_string_key(index)?)
                    .ok()?
                    .to_owned();
                let mut reference = self.reference(base)?;
                reference.path.push(key);
                Some(reference)
            }
            _ => None,
        }
    }
}

fn number(spelling: &str) -> Option<f64> {
    let (sign, unsigned) = spelling
        .strip_prefix('-')
        .map_or((1.0, spelling), |unsigned| (-1.0, unsigned));
    let Some(hex) = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))
    else {
        return spelling.parse().ok();
    };
    let (mantissa, exponent) = hex
        .split_once(['p', 'P'])
        .map_or((hex, 0), |(mantissa, exponent)| {
            (mantissa, exponent.parse::<i32>().unwrap_or(i32::MAX))
        });
    if exponent == i32::MAX {
        return None;
    }
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    let mut value = 0.0;
    for digit in whole.chars() {
        value = value * 16.0 + digit.to_digit(16)? as f64;
    }
    let mut place = 1.0 / 16.0;
    for digit in fraction.chars() {
        value += digit.to_digit(16)? as f64 * place;
        place /= 16.0;
    }
    Some(sign * value * 2.0_f64.powi(exponent))
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TypePrecedence {
    Function,
    Union,
    Intersection,
    Postfix,
    Atom,
}

fn parenthesize_type(text: String, precedence: TypePrecedence, minimum: TypePrecedence) -> String {
    if precedence < minimum {
        format!("({text})")
    } else {
        text
    }
}

fn describe_at_precedence(ty: &Type, minimum: TypePrecedence) -> String {
    let precedence = match ty {
        Type::Function(_) => TypePrecedence::Function,
        Type::Union(_) => TypePrecedence::Union,
        Type::Intersection(_) => TypePrecedence::Intersection,
        _ => TypePrecedence::Atom,
    };
    parenthesize_type(describe(ty), precedence, minimum)
}

/// Lua-oriented presentation of resolved type descriptions.
pub fn describe(ty: &Type) -> String {
    match ty {
        Type::RuntimeTable => "table".to_owned(),
        Type::RuntimeFunction => "function".to_owned(),
        Type::Any => "any".into(),
        Type::Unknown => "unknown".into(),
        Type::Never => "never".into(),
        Type::Nil | Type::Undefined => "nil".into(),
        Type::Void => "void".into(),
        Type::Boolean => "boolean".into(),
        Type::Number => "number".into(),
        Type::String => "string".into(),
        Type::BooleanLiteral(value) => value.to_string(),
        Type::NumberLiteral(value) => value.to_string(),
        Type::StringLiteral(value) => {
            let mut result = String::from("\"");
            for byte in value {
                match byte {
                    b'"' => result.push_str("\\\""),
                    b'\\' => result.push_str("\\\\"),
                    32..=126 => result.push(*byte as char),
                    _ => result.push_str(&format!("\\x{byte:02x}")),
                }
            }
            result.push('"');
            result
        }
        Type::Union(types) => types
            .iter()
            .map(|ty| describe_at_precedence(ty, TypePrecedence::Union))
            .collect::<Vec<_>>()
            .join(" | "),
        Type::Intersection(types) => types
            .iter()
            .map(|ty| describe_at_precedence(ty, TypePrecedence::Intersection))
            .collect::<Vec<_>>()
            .join(" & "),
        Type::Table(table) => native_tables::describe_table(table, |ty| {
            describe_at_precedence(ty, TypePrecedence::Union)
        }),
        Type::Shape(properties) | Type::FreshShape(properties) => format!(
            "{{ {} }}",
            properties
                .iter()
                .map(|property| {
                    format!(
                        "{}{}: {}",
                        property.name,
                        if property.optional { "?" } else { "" },
                        describe_at_precedence(&property.ty, TypePrecedence::Union)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Type::Function(function) => format!(
            "fun({}): {}",
            function
                .params
                .iter()
                .map(|parameter| {
                    format!(
                        "{}{}: {}",
                        parameter.name,
                        if parameter.optional { "?" } else { "" },
                        describe_at_precedence(&parameter.ty, TypePrecedence::Union)
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
            describe(&function.returns)
        ),
        // Native declarations remain references during checking. Expanding
        // their signatures here would create artificial return-demand cycles.
        Type::NativeFunction(_) | Type::NativeSignature(_) => "function".into(),
        Type::NativeTypeParameter(_) => "type parameter".into(),
        Type::NativeNamedReference { .. } => "named type".into(),
        Type::SessionType(_) => "contextual type".into(),
    }
}

fn describe_native(ty: &Type, engine: &mut dyn TypeQueries, stack: &mut Vec<Type>) -> String {
    describe_native_mode(ty, engine, stack, false)
}

pub(crate) fn describe_diagnostic_type(ty: &Type, engine: &mut dyn TypeQueries) -> String {
    describe_native_mode(ty, engine, &mut Vec::new(), true)
}

fn describe_native_mode(
    ty: &Type,
    engine: &mut dyn TypeQueries,
    stack: &mut Vec<Type>,
    diagnostic: bool,
) -> String {
    describe_native_at_precedence(ty, engine, stack, diagnostic, TypePrecedence::Function)
}

fn describe_native_at_precedence(
    ty: &Type,
    engine: &mut dyn TypeQueries,
    stack: &mut Vec<Type>,
    diagnostic: bool,
    minimum: TypePrecedence,
) -> String {
    match ty {
        Type::SessionType(_) => {
            // A named object already has its original printable identity.
            // Inspecting its call signatures would unnecessarily traverse its
            // members, potentially demanding an unrelated inferred body.
            if diagnostic && engine.named_object_declaration(ty).is_some() {
                return engine.format(ty);
            }
            if stack.contains(ty) {
                return "<recursive type>".into();
            }
            if stack.len() >= 64 {
                return "...".into();
            }
            stack.push(ty.clone());
            let inspected = engine.inspect_type(ty);
            let text = if let Type::Function(function) = &inspected {
                parenthesize_type(
                    describe_native_function(function, Some(ty), engine, stack, diagnostic),
                    TypePrecedence::Function,
                    minimum,
                )
            } else if &inspected == ty {
                engine.format(ty)
            } else {
                describe_native_at_precedence(&inspected, engine, stack, diagnostic, minimum)
            };
            stack.pop();
            text
        }
        Type::NativeTypeParameter(_) | Type::NativeNamedReference { .. } => engine.format(ty),
        Type::Table(table) => native_tables::describe_table(table, |ty| {
            describe_native_at_precedence(ty, engine, stack, diagnostic, TypePrecedence::Union)
        }),
        Type::NativeFunction(_) | Type::NativeSignature(_) => {
            if stack.contains(ty) {
                return "<recursive function>".into();
            }
            if stack.len() >= 64 {
                return "...".into();
            }
            stack.push(ty.clone());
            // Public heads can differ from the private runtime body signature.
            // Each native handle retains its exact capture/context mapper.
            let heads = engine.call_signature_types(ty);
            let text = match heads.as_slice() {
                [] => "function".into(),
                [single] => {
                    describe_native_at_precedence(single, engine, stack, diagnostic, minimum)
                }
                _ => format!(
                    "overload {{ {} }}",
                    heads
                        .iter()
                        .map(|head| describe_native_mode(head, engine, stack, diagnostic))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            };
            stack.pop();
            text
        }
        Type::Function(function) => parenthesize_type(
            describe_native_function(function, None, engine, stack, diagnostic),
            TypePrecedence::Function,
            minimum,
        ),
        Type::Union(types) => parenthesize_type(
            types
                .iter()
                .map(|ty| {
                    describe_native_at_precedence(
                        ty,
                        engine,
                        stack,
                        diagnostic,
                        TypePrecedence::Union,
                    )
                })
                .collect::<Vec<_>>()
                .join(" | "),
            TypePrecedence::Union,
            minimum,
        ),
        Type::Intersection(types) => parenthesize_type(
            types
                .iter()
                .map(|ty| {
                    describe_native_at_precedence(
                        ty,
                        engine,
                        stack,
                        diagnostic,
                        TypePrecedence::Intersection,
                    )
                })
                .collect::<Vec<_>>()
                .join(" & "),
            TypePrecedence::Intersection,
            minimum,
        ),
        Type::Shape(properties) | Type::FreshShape(properties) => format!(
            "{{ {} }}",
            properties
                .iter()
                .map(|property| format!(
                    "{}{}: {}",
                    property.name,
                    if property.optional { "?" } else { "" },
                    describe_native_at_precedence(
                        &property.ty,
                        engine,
                        stack,
                        diagnostic,
                        TypePrecedence::Union
                    )
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => describe(ty),
    }
}

fn describe_native_function(
    function: &luanalysis_engine::FunctionType,
    original: Option<&Type>,
    engine: &mut dyn TypeQueries,
    stack: &mut Vec<Type>,
    diagnostic: bool,
) -> String {
    let (parameters, returns) = describe_native_function_parts(function, original, engine, stack, diagnostic);
    format!("fun({}): {}", parameters.join(", "), returns)
}

/// A signature's parameters (`name: type`, `name?: type`, `...: type`) and its results, as a
/// `fun(...)` type prints them, for signature help.
pub(crate) fn describe_signature(head: &Type, engine: &mut dyn TypeQueries) -> Option<(Vec<String>, String)> {
    let mut stack = Vec::new();
    let mut ty = head.clone();
    // As `describe_native_at_precedence` prints a callable: a native handle through its one
    // call signature, a session type through its inspected function.
    for _ in 0..8 {
        match &ty {
            Type::Function(function) => return Some(describe_native_function_parts(function, None, engine, &mut stack, false)),
            Type::SessionType(_) => {
                let inspected = engine.inspect_type(&ty);
                if let Type::Function(function) = &inspected {
                    return Some(describe_native_function_parts(function, Some(&ty), engine, &mut stack, false));
                }
                if inspected == ty { return None; }
                ty = inspected;
            }
            Type::NativeFunction(_) | Type::NativeSignature(_) => match engine.call_signature_types(&ty).as_slice() {
                [single] if *single != ty => ty = single.clone(),
                _ => return None,
            },
            _ => return None,
        }
    }
    None
}

fn describe_native_function_parts(
    function: &luanalysis_engine::FunctionType,
    original: Option<&Type>,
    engine: &mut dyn TypeQueries,
    stack: &mut Vec<Type>,
    diagnostic: bool,
) -> (Vec<String>, String) {
    let rest = original.and_then(|ty| engine.signature_rest_pack(ty));
    let fixed = function
        .params
        .len()
        .saturating_sub(usize::from(rest.is_some()));
    let mut parameters: Vec<_> = function.params[..fixed]
        .iter()
        .map(|parameter| {
            format!(
                "{}{}: {}",
                parameter.name,
                if parameter.optional { "?" } else { "" },
                describe_native_at_precedence(
                    &parameter.ty,
                    engine,
                    stack,
                    diagnostic,
                    TypePrecedence::Union
                )
            )
        })
        .collect();
    // Varargs that are an empty sequence, `bind()`'s, take no values: `fun()`.
    let rest = rest.filter(|rest| !matches!(engine.inspect_pack(rest).as_deref(), Some([row]) if row.fixed.is_empty() && row.tail.is_none()));
    if let Some(rest) = rest {
        let text = match engine.inspect_pack(&rest).as_deref() {
            Some([row]) if row.fixed.is_empty() && row.tail.is_some() => {
                describe_native_at_precedence(
                    row.tail.as_ref().unwrap(),
                    engine,
                    stack,
                    diagnostic,
                    TypePrecedence::Union,
                )
            }
            // Varargs that are a sequence still to be inferred, as written: `...: T...`.
            None => match engine.inspect_pack_view(&rest) {
                luanalysis_engine::PackInspection::Symbolic(ty) => {
                    format!("{}...", describe_native_at_precedence(&ty, engine, stack, diagnostic, TypePrecedence::Union))
                }
                _ => describe_native_pack(&rest, engine, stack, diagnostic),
            },
            _ => describe_native_pack(&rest, engine, stack, diagnostic),
        };
        parameters.push(format!("...: {text}"));
    }
    let returns = original
        .and_then(|ty| engine.function_result_pack(ty))
        .map(|pack| describe_native_pack(&pack, engine, stack, diagnostic))
        .unwrap_or_else(|| describe_native_mode(&function.returns, engine, stack, diagnostic));
    (parameters, returns)
}

/// A result pack as a diagnostic prints it.
pub(crate) fn describe_result_pack(pack: &luanalysis_engine::ValuePack, engine: &mut dyn TypeQueries) -> String {
    describe_native_pack(pack, engine, &mut Vec::new(), true)
}

fn describe_native_pack(
    pack: &luanalysis_engine::ValuePack,
    engine: &mut dyn TypeQueries,
    stack: &mut Vec<Type>,
    diagnostic: bool,
) -> String {
    use luanalysis_engine::PackInspection;
    let alternatives = match engine.inspect_pack_view(pack) {
        PackInspection::Alternatives(rows) => rows,
        PackInspection::Whole(ty) => {
            return format!("{} result pack", describe_native_mode(&ty, engine, stack, diagnostic));
        }
        PackInspection::Symbolic(_) => return describe_symbolic_pack(pack, engine, stack, diagnostic),
    };
    if alternatives.is_empty() {
        return "never".to_owned();
    }
    let alternatives = absent_result_rows(alternatives);
    alternatives
        .iter()
        .map(|(row, present)| {
            let tuple_slot =
                row.fixed.len() > 1 || row.tail.is_some() || alternatives.len() > 1;
            let mut slots: Vec<_> = row
                .fixed
                .iter()
                .enumerate()
                .map(|(index, slot)| {
                    let absent = slot.optional || index >= *present;
                    let minimum = if absent {
                        TypePrecedence::Postfix
                    } else if tuple_slot {
                        TypePrecedence::Union
                    } else {
                        TypePrecedence::Function
                    };
                    let ty = describe_native_at_precedence(
                        &slot.ty, engine, stack, diagnostic, minimum,
                    );
                    if absent { format!("{ty}?") } else { ty }
                })
                .collect();
            if let Some(tail) = &row.tail {
                slots.push(format!(
                    "{}...",
                    describe_native_at_precedence(
                        tail,
                        engine,
                        stack,
                        diagnostic,
                        TypePrecedence::Postfix
                    )
                ));
            }
            match slots.len() {
                0 => "void".to_owned(),
                1 if row.tail.is_none() => slots.remove(0),
                _ => format!("({})", slots.join(", ")),
            }
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// A pack that names a sequence parameter, with the sequence written as varargs write it, `T...`,
/// apart from one value of `T`: `(true, T...) | (false, string)`.
fn describe_symbolic_pack(
    pack: &luanalysis_engine::ValuePack,
    engine: &mut dyn TypeQueries,
    stack: &mut Vec<Type>,
    diagnostic: bool,
) -> String {
    use luanalysis_engine::PackElement;
    let rows = engine.symbolic_pack_rows(pack);
    let alternatives = rows.len();
    rows.iter()
        .map(|row| {
            let tuple_slot = row.len() > 1 || alternatives > 1;
            let value = if tuple_slot { TypePrecedence::Union } else { TypePrecedence::Function };
            let mut slots: Vec<_> = row
                .iter()
                .map(|element| match element {
                    PackElement::Required(ty) => {
                        describe_native_at_precedence(ty, engine, stack, diagnostic, value)
                    }
                    PackElement::Optional(ty) => format!(
                        "{}?",
                        describe_native_at_precedence(ty, engine, stack, diagnostic, TypePrecedence::Postfix)
                    ),
                    PackElement::Rest(ty) | PackElement::Spread(ty) => format!(
                        "{}...",
                        describe_native_at_precedence(ty, engine, stack, diagnostic, TypePrecedence::Postfix)
                    ),
                })
                .collect();
            match row.as_slice() {
                [] => "void".to_owned(),
                [PackElement::Required(_)] => slots.remove(0),
                _ => format!("({})", slots.join(", ")),
            }
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Rows each one result shorter than the next are one row whose last results may be absent,
/// as `---@return A, B?` writes them: `(A, B?)`. Each distinct row, in order, with how many of
/// its results are always present.
fn absent_result_rows(rows: Vec<luanalysis_engine::PackAlternative>) -> Vec<(luanalysis_engine::PackAlternative, usize)> {
    let mut distinct: Vec<luanalysis_engine::PackAlternative> = Vec::with_capacity(rows.len());
    for row in rows {
        if !distinct.contains(&row) { distinct.push(row); }
    }
    let rows = distinct;
    let mut present: Vec<Option<usize>> = rows.iter().map(|row| Some(row.fixed.len())).collect();
    let mut longest: Vec<usize> = (0..rows.len()).collect();
    longest.sort_by_key(|&index| std::cmp::Reverse(rows[index].fixed.len()));
    for index in longest {
        let Some(mut count) = present[index] else { continue };
        while count > 0 {
            let prefix = &rows[index].fixed[..count - 1];
            let Some(shorter) = (0..rows.len()).find(|&other| {
                other != index && present[other].is_some() && rows[other].tail.is_none() && rows[other].fixed == prefix
            }) else { break };
            present[shorter] = None;
            count -= 1;
        }
        present[index] = Some(count);
    }
    rows.into_iter().zip(present).filter_map(|(row, present)| Some((row, present?))).collect()
}

/// The starts of a source's lines, to place many spans in it without rescanning it for each.
pub(crate) struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub(crate) fn new(source: &str) -> Self {
        let bytes = source.as_bytes();
        let mut starts = vec![0];
        for (index, &byte) in bytes.iter().enumerate() {
            match byte {
                b'\r' if bytes.get(index + 1) != Some(&b'\n') => starts.push(index + 1),
                b'\n' => starts.push(index + 1),
                _ => {}
            }
        }
        Self { starts }
    }

    /// `location`, by a binary search of the line starts.
    pub(crate) fn location(&self, source: &str, span: Span) -> (usize, usize) {
        let mut offset = span.start.min(source.len());
        while !source.is_char_boundary(offset) {
            offset -= 1;
        }
        let bytes = source.as_bytes();
        let line = self.starts.partition_point(|&start| start <= offset);
        // Between the halves of a CRLF, the CR has ended the line.
        if offset > 0 && bytes[offset - 1] == b'\r' && bytes.get(offset) == Some(&b'\n') {
            return (line + 1, 1);
        }
        let start = self.starts[line - 1];
        (line, source[start..offset].chars().count() + 1)
    }
}

#[cfg(test)]
#[test]
fn a_line_index_places_every_offset_as_location_does() {
    let source = "a\r\nbé\rc\n\nd\r\n\r\nλx";
    let lines = LineIndex::new(source);
    for start in 0..=source.len() + 2 {
        let span = Span { start, end: start };
        assert_eq!(lines.location(source, span), location(source, span), "offset {start}");
    }
}

/// One-based original-source line and Unicode scalar column for CLI output.
pub fn location(source: &str, span: Span) -> (usize, usize) {
    let mut offset = span.start.min(source.len());
    while !source.is_char_boundary(offset) {
        offset -= 1;
    }
    let mut line = 1;
    let mut column = 1;
    let mut chars = source[..offset].chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                line += 1;
                column = 1;
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            '\n' => {
                line += 1;
                column = 1;
            }
            _ => column += 1,
        }
    }
    (line, column)
}

/// A note for an unresolved name that is EmmyLua Analyzer's builtin type, which only
/// `--emmylua-syntax` reads.
fn emmylua_builtin_hint(name: &str) -> &'static str {
    if luanalysis_syntax::emmylua_builtin_type(name) { " It is EmmyLua Analyzer's builtin type (--emmylua-syntax)." } else { "" }
}

impl Checker<'_> {
    /// The types the project declares by name, for `---@param name \`T\`` captures: each name
    /// that declares one type and needs no type arguments.
    fn declared_types(&mut self) -> Vec<(String, Type)> {
        let Some(native) = self.native else { return Vec::new() };
        native.declared_types().into_iter()
            .filter_map(|(name, key)| Some((name, self.engine.named_type_reference(key, &[]).ok()?)))
            .collect()
    }
}

impl Checker<'_> {
    /// What a store to `target` must be of its field's type: `value` without what EmmyLua
    /// Analyzer's `---@cast obj.value +T` before it, in its block, lets the field also hold.
    fn field_widened_value(&mut self, target: &Expr, value: &Type) -> Type {
        let Some(native) = self.native else { return value.clone() };
        if native.syntax().field_widenings.is_empty() { return value.clone() }
        let mut path = Vec::new();
        let mut expression = target;
        loop {
            match &expression.kind {
                ExprKind::Name(name) => { path.push(name.text.clone()); break }
                ExprKind::Field { base, name } => { path.push(name.text.clone()); expression = base; }
                _ => return value.clone(),
            }
        }
        path.reverse();
        let widenings: Vec<_> = native.syntax().field_widenings.iter()
            .filter(|widening| widening.path == path && (widening.scope.start..widening.scope.end).contains(&target.span.start))
            .map(|widening| widening.ty.clone())
            .collect();
        let mut value = value.clone();
        for annotation in widenings {
            let pack = native_packs::AnnotationPack::annotation(&annotation).lower(&mut |ty| self.annotation(ty));
            let widened = self.engine.pack_slot(&pack, 0);
            value = self.engine.exclude_type(&value, &widened);
        }
        value
    }
}
