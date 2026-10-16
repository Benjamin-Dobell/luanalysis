//! Original-source checking for the first native Lua subset.
//!
//! Lua syntax and straight-line bindings are owned here. Type construction,
//! literal widening, member lookup, freshness and assignability use the pinned
//! Bun core. Native generic calls share its inference worker. Branch flow,
//! contextual expression demands and project services are integration gates.

pub mod bind;
mod builtins;
mod diagnostic_types;
mod module_requests;
pub mod flow;
pub mod incremental;
pub mod project_check;
mod project_cost;
pub mod standard_environment;
mod functions;
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
mod native_types;
mod operators;

use bind::{BindingId, BoundFile};
use luanalysis_engine::{
    CapturedFunctionContext, ContextualFunctionContext, Engine, LuaKey, NativeCheckMode,
    NativeEnvironmentKey, NativeFlowKey, NativeFunctionKey, TableKey, Type, TypeQueries,
};
use luanalysis_syntax::{
    Expr, ExprKind, Name, Span, Stmt, StmtKind, TypeAnnotation, TypeExpr, TypeExprKind, TypeLiteral,
};
use native_functions::NativeRegistry;
use std::collections::{HashMap, HashSet};

/// Public exposure of an overloaded function's implementation contract.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverloadMode {
    #[default]
    V2,
    LegacyExposeImplementation,
}

/// Source declaration policy independent of overload visibility and type relations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CheckOptions {
    pub overload_mode: OverloadMode,
    /// Require written or contextual parameter types, including runtime varargs.
    /// Ordinary Luanalysis analysis permits missing contracts as in v1.
    pub no_implicit_any: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub message: String,
    pub span: Span,
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
    pub bound: Option<BoundFile>,
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
        self.pending_bindings.append(&mut other.pending_bindings);
        self.pending_types.append(&mut other.pending_types);
        self.pending_diagnostics.append(&mut other.pending_diagnostics);
    }

    fn render(&mut self, engine: &mut dyn TypeQueries) {
        let mut bindings = HashMap::with_capacity(self.pending_bindings.len());
        for (binding, ty) in std::mem::take(&mut self.pending_bindings) {
            let displayed = describe_native(&ty, engine, &mut Vec::new());
            bindings.insert(binding, displayed);
        }
        for info in &mut self.bindings {
            if let Some(displayed) = bindings.get(&info.binding) {
                info.ty.clone_from(&displayed);
            }
        }
        let mut types = HashMap::with_capacity(self.pending_types.len());
        for (span, ty) in std::mem::take(&mut self.pending_types) {
            let displayed = describe_native(&ty, engine, &mut Vec::new());
            types.insert((span.start, span.end), displayed);
        }
        for info in &mut self.types {
            if let Some(displayed) = types.get(&(info.span.start, info.span.end)) {
                info.ty.clone_from(&displayed);
            }
        }
        self.render_diagnostics(engine);
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
                bind::OccurrenceKind::Declaration | bind::OccurrenceKind::Annotation
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
    let file = luanalysis_syntax::parse(source);
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
            bound: None,
            pending_bindings: Vec::new(),
            pending_types: Vec::new(),
            pending_diagnostics: Vec::new(),
        };
    }
    let bound = bind::bind(&file);
    Engine::new().with_native_provider(
        move || {
            let mut registry = NativeRegistry::new(file, bound);
            registry.set_overload_mode(options.overload_mode);
            registry.set_no_implicit_any(options.no_implicit_any);
            registry.set_legacy_builtin_policy(builtins::LegacyBuiltinPolicy::MinimalTypeCompatibility);
            registry
        },
        |registry, engine| {
            registry.check_named_declarations(engine);
            registry.check_global_declarations(engine);
            let mut report = {
                let mut checker = Checker {
                    active_assertions: HashSet::new(),
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
                    locals: HashMap::new(),
                    assigned_fields: HashMap::new(),
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
            report.append(registry.take_report());
            report.render(engine);
            while registry.retry_contextual_owner_validations(engine) {}
            registry.report_unresolved_contextual_owners();
            // Explicit display demands can complete a previously unqueried signature.
            report.append(registry.take_report());
            report.render_diagnostics(engine);
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
            report.bound = Some(registry.bound().clone());
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
        self.report.types.push(TypeInfo {
            span,
            ty: describe(ty),
        });
        self.report.pending_types.push((span, ty.clone()));
    }
    /// Returns true when this straight-line block terminates with a return or
    /// an explicitly declared nonreturning call.
    fn statements(&mut self, statements: &[Stmt]) -> bool {
        for statement in statements {
            match &statement.kind {
                StmtKind::Local {
                    names,
                    values,
                    annotation,
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
                        return true;
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
                        return true;
                    }
                }
                StmtKind::If {
                    branches,
                    else_branch,
                } => {
                    for branch in branches {
                        self.expression(&branch.condition);
                        self.statements(&branch.body);
                    }
                    if let Some(branch) = else_branch {
                        self.statements(&branch.body);
                    }
                }
                StmtKind::While {
                    condition, body, ..
                } => {
                    self.expression(condition);
                    self.statements(body);
                }
                StmtKind::Repeat {
                    body, condition, ..
                } => {
                    self.statements(body);
                    self.expression(condition);
                }
                StmtKind::NumericFor {
                    name,
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
                    self.record_binding(name, binding, &Type::Number);
                    self.statements(body);
                }
                StmtKind::GenericFor { names, body, .. } => {
                    if let (Some(native), Some(at)) = (self.native, self.flow_at) {
                        for name in names {
                            let id = self
                                .bound
                                .binding_at(name.span.start)
                                .expect("bound Lua generic loop variable");
                            let (binding, incomplete) = native
                                .iterator_body_binding(id, at, self.engine)
                                .expect("an original generic loop has a body binding recipe");
                            self.incomplete_flow |= incomplete;
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
                StmtKind::Break => {}
            }
        }
        false
    }

    fn implicit_parameter_any(&mut self, code: &'static str, span: Span, message: impl Into<String>) {
        if self.native.is_some_and(|native| native.no_implicit_any()) {
            self.error(code, span, message);
        }
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
        let resolved = self.annotation_node(ty);
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
                "number" => Type::Number,
                "string" => Type::String,
                "table" => Type::RuntimeTable,
                "function" => Type::RuntimeFunction,
                "void" => Type::Void,
                _ => {
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
                    // documentation placeholder is never a global type owner.
                    if name == "self" { return Type::Unknown; }
                    if let Some(resolved)=self.native_named_reference(name,&[],ty){return resolved;}
                    self.error(
                        "TS2304",
                        ty.span,
                        format!("Cannot find name '{name}'."),
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
            TypeExprKind::CompletionHint(_) => Type::Never,
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
                if base.text == "self" {
                    self.error("TS2315", base.span, "Type 'self' is not generic.");
                    return self.engine.error_type();
                }
                if let Some(resolved)=self.native_named_reference(&base.text,&arguments,ty){return resolved;}
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
                    _=>("TS2304",format!("Cannot find name '{}'.",base.text)),
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
        self.expression_value(expr, None, true)
    }

    fn expression_value(&mut self, expr: &Expr, expected: Option<&Type>, capture_origin: bool) -> EvaluatedExpression {
        let previous = self.flow_at;
        if let (Some(native), Some(at)) = (self.native, previous) {
            self.flow_at = Some(native.flow_input_for_expression(expr, at));
        }
        let value = if let Some(value) = self.asserted_expression_value(expr, capture_origin) { value }
            else { self.expression_value_at_current_flow(expr, expected, capture_origin) };
        self.flow_at = previous;
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
                        let result = native.flow_reference_type(
                            expr,
                            at,
                            &binding.constraint,
                            &binding.initial,
                            self.engine,
                        );
                        self.incomplete_flow |= result.incomplete;
                        if result.unresolved_auto && !result.incomplete
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
                    Type::Any
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
                self.native_member_flow_type(expr, ty, environment)
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
                self.native_member_flow_type(expr, ty, environment)
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
            names.len(),
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
        if let Some(annotation) = annotation
            && !targets.iter().all(|target| self.is_global_annotation_target(target)
                || self.native.is_some_and(|native| native.is_original_assignment_field(target.span)))
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
        let values = self.adjusted_list_values(
            values,
            targets.len(),
            Some(&expected_pack),
            targets
                .first()
                .map_or(Span::default(), |target| target.span),
        );
        self.assign_values(targets, values);
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
                        if self.rejects_generic_global_write(name) { continue; }
                        if let Some(constraint) = self.global_read(name) {
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
                    let (key, key_span) = match &target.kind {
                        ExprKind::Field { name, .. } => (
                            TableKey::Literal(LuaKey::String(name.text.as_bytes().to_vec())),
                            name.span,
                        ),
                        ExprKind::Index { index, .. } => (self.native_table_key(index), index.span),
                        _ => unreachable!(),
                    };
                    if self.native_table_write(&receiver, &key, &value, target.span, key_span, origin) {
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

fn describe_diagnostic_type(ty: &Type, engine: &mut dyn TypeQueries) -> String {
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
            _ => describe_native_pack(&rest, engine, stack, diagnostic),
        };
        parameters.push(format!("...: {text}"));
    }
    let returns = original
        .and_then(|ty| engine.function_result_pack(ty))
        .map(|pack| describe_native_pack(&pack, engine, stack, diagnostic))
        .unwrap_or_else(|| describe_native_mode(&function.returns, engine, stack, diagnostic));
    format!("fun({}): {}", parameters.join(", "), returns)
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
        PackInspection::Symbolic(ty) => {
            return format!("result pack {}", describe_native_mode(&ty, engine, stack, diagnostic));
        }
    };
    if alternatives.is_empty() {
        return "never".to_owned();
    }
    alternatives
        .iter()
        .map(|row| {
            let tuple_slot =
                row.fixed.len() > 1 || row.tail.is_some() || alternatives.len() > 1;
            let mut slots: Vec<_> = row
                .fixed
                .iter()
                .map(|slot| {
                    let minimum = if slot.optional {
                        TypePrecedence::Postfix
                    } else if tuple_slot {
                        TypePrecedence::Union
                    } else {
                        TypePrecedence::Function
                    };
                    let ty = describe_native_at_precedence(
                        &slot.ty, engine, stack, diagnostic, minimum,
                    );
                    if slot.optional { format!("{ty}?") } else { ty }
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
