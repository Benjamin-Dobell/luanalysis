//! Original-source checking for the first native Lua subset.
//!
//! Lua syntax and straight-line bindings are owned here. Type construction,
//! literal widening, member lookup, freshness and assignability use the pinned
//! Bun core. Native generic calls share its inference worker. Branch flow,
//! contextual expression demands and project services are integration gates.

pub mod bind;
mod builtins;
mod module_requests;
pub mod flow;
pub mod project_check;
mod functions;
mod native_expressions;
mod native_flow;
mod native_assertions;
mod native_iterators;
mod native_functions;
mod native_named;
mod native_signatures;
mod native_packs;
mod native_tables;
mod native_types;
mod operators;

use bind::{BindingId, BoundFile};
use luanalysis_engine::{
    CapturedFunctionContext, ContextualFunctionContext, Engine, LuaKey, NativeCheckMode,
    NativeFlowKey, NativeFunctionKey, TableKey, Type, TypeQueries,
};
use luanalysis_syntax::{
    Expr, ExprKind, Name, Span, Stmt, StmtKind, TypeAnnotation, TypeExpr, TypeExprKind, TypeLiteral,
};
use native_functions::NativeRegistry;
use std::collections::{HashMap, HashSet};

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
}

impl CheckReport {
    fn append(&mut self, mut other: Self) {
        self.diagnostics.append(&mut other.diagnostics);
        self.bindings.append(&mut other.bindings);
        self.types.append(&mut other.types);
        self.pending_bindings.append(&mut other.pending_bindings);
        self.pending_types.append(&mut other.pending_types);
    }

    fn render(&mut self, engine: &mut dyn TypeQueries) {
        for (binding, ty) in std::mem::take(&mut self.pending_bindings) {
            let displayed = describe_native(&ty, engine, &mut Vec::new());
            for info in self
                .bindings
                .iter_mut()
                .filter(|info| info.binding == binding)
            {
                info.ty.clone_from(&displayed);
            }
        }
        for (span, ty) in std::mem::take(&mut self.pending_types) {
            let displayed = describe_native(&ty, engine, &mut Vec::new());
            for info in self.types.iter_mut().filter(|info| info.span == span) {
                info.ty.clone_from(&displayed);
            }
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
    let file = luanalysis_syntax::parse(source);
    if !file.diagnostics.is_empty() {
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
        };
    }
    let bound = bind::bind(&file);
    Engine::new().with_native_provider(
        move || NativeRegistry::new(file, bound),
        |registry, engine| {
            registry.check_named_declarations(engine);
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
            report.append(registry.take_report());
            report.render(engine);
            // Explicit display demands can complete a previously unqueried signature.
            report.append(registry.take_report());
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
            name: name.text.clone(),
            span: name.span,
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
                    if matches!(result, Type::Never) {
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
                        match native.annotation_parameter_resolution(context,name) {
                            native_types::TypeNameResolution::Parameter(key)=>return Type::NativeTypeParameter(key),
                            native_types::TypeNameResolution::Ambiguous(_)=>{
                                self.error("TS2300",ty.span,format!("Ambiguous generic parameter '{name}'."));
                                return Type::Any;
                            },
                            native_types::TypeNameResolution::Missing=>{},
                        }
                    }
                    if let Some(resolved)=self.native_named_reference(name,&[],ty){return resolved;}
                    self.error(
                        "TS2304",
                        ty.span,
                        format!("Cannot find name '{name}'."),
                    );
                    Type::Any
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
            TypeExprKind::Literal(literal) => match literal {
                TypeLiteral::Boolean(value) => Type::BooleanLiteral(*value),
                TypeLiteral::String(value) => Type::StringLiteral(value.clone()),
                TypeLiteral::Number(spelling) => number(spelling)
                    .map(Type::NumberLiteral)
                    .unwrap_or_else(|| {
                        self.error("LUA2002", ty.span, "invalid numeric literal type");
                        Type::Unknown
                    }),
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
                Type::Unknown
            }
            TypeExprKind::Error => Type::Any,
        }
    }

    fn expression(&mut self, expr: &Expr) -> Type {
        self.expression_with_expected(expr, None)
    }

    fn expression_with_expected(&mut self, expr: &Expr, expected: Option<&Type>) -> Type {
        let previous = self.flow_at;
        if let (Some(native), Some(at)) = (self.native, previous) {
            self.flow_at = Some(native.flow_input_for_expression(expr, at));
        }
        let ty = if let Some(ty) = self.asserted_expression_type(expr) { ty } else { self.expression_at_current_flow(expr, expected) };
        self.flow_at = previous;
        ty
    }

    fn expression_at_current_flow(&mut self, expr: &Expr, expected: Option<&Type>) -> Type {
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
                        result.ty
                    } else {
                        binding.value
                    }
                }
                None => {
                    if self.bound.binding_at(name.span.start).is_none() {
                        if let Some(ty) = builtins::value(&name.text) {
                            self.record_expression_type(expr.span, &ty);
                            return ty;
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
            ExprKind::Parenthesized(value) => self.expression_with_expected(value, expected),
            ExprKind::Table(fields) => self.native_table_expression(fields, expr.span, expected),
            ExprKind::Field { base, name } => {
                let receiver = self.expression(base);
                let ty = self.native_named_property_read(&receiver, name);
                self.native_member_flow_type(expr, ty)
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
                let receiver = self.expression(base);
                let key = self.native_table_key(index);
                let ty = self.native_table_read(&receiver, &key, index.span);
                self.native_member_flow_type(expr, ty)
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
        self.record_expression_type(expr.span, &ty);
        ty
    }

    fn compatible(&mut self, value: &Type, target: &Type, origin: Span) -> bool {
        let relation = self.engine.compare(value, target);
        if !relation.assignable {
            self.error(
                "TS2322",
                origin,
                format!(
                    "Type '{}' is not assignable to type '{}'.",
                    describe(value),
                    describe(target)
                ),
            );
        }
        relation.assignable
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
        }));
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
            let widened = self.engine.widen(&value);
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
            let id = self
                .bound
                .binding_at(name.span.start)
                .expect("bound Lua declaration");
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
        if let Some(annotation) = annotation {
            self.error(
                "LUA2004",
                annotation.span,
                "@type casts on assignments are not implemented yet",
            );
        }
        // A local destination's declaration can contextualize a generic call
        // without evaluating or storing any assignment destination early.
        let expected: Vec<_> = targets
            .iter()
            .map(|target| match &target.kind {
                ExprKind::Name(name) => self
                    .bound
                    .binding_at(name.span.start)
                    .and_then(|id| self.resolve_local(id))
                    .filter(|binding| !binding.evolves_from_nil)
                    .map(|binding| binding.constraint.clone()),
                _ => None,
            })
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
                    if self.native_table_write(&receiver, &key, &value, key_span, origin) {
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
        Type::Union(types) => types.iter().map(describe).collect::<Vec<_>>().join(" | "),
        Type::Intersection(types) => types.iter().map(describe).collect::<Vec<_>>().join(" & "),
        Type::Table(table) => native_tables::describe_table(table, describe),
        Type::Shape(properties) | Type::FreshShape(properties) => format!(
            "{{ {} }}",
            properties
                .iter()
                .map(|property| {
                    format!(
                        "{}{}: {}",
                        property.name,
                        if property.optional { "?" } else { "" },
                        describe(&property.ty)
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
                        describe(&parameter.ty)
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
    match ty {
        Type::SessionType(_) => {
            if stack.contains(ty) {
                return "<recursive type>".into();
            }
            if stack.len() >= 64 {
                return "...".into();
            }
            stack.push(ty.clone());
            let inspected = engine.inspect_type(ty);
            let text = if let Type::Function(function) = &inspected {
                describe_native_function(function, Some(ty), engine, stack)
            } else if &inspected == ty {
                engine.format(ty)
            } else {
                describe_native(&inspected, engine, stack)
            };
            stack.pop();
            text
        }
        Type::NativeTypeParameter(_) => engine.format(ty),
        Type::Table(table) => {
            native_tables::describe_table(table, |ty| describe_native(ty, engine, stack))
        }
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
            let texts: Vec<_> = heads.iter()
                .map(|head|describe_native(head,engine,stack)).collect();
            let text=match texts.as_slice() {
                []=>"function".into(),
                [single]=>single.clone(),
                _=>format!("overload {{ {} }}",texts.join("; ")),
            };
            stack.pop();
            text
        }
        Type::Function(function) => describe_native_function(function, None, engine, stack),
        Type::Union(types) => types
            .iter()
            .map(|ty| describe_native(ty, engine, stack))
            .collect::<Vec<_>>()
            .join(" | "),
        Type::Intersection(types) => types
            .iter()
            .map(|ty| describe_native(ty, engine, stack))
            .collect::<Vec<_>>()
            .join(" & "),
        Type::Shape(properties) | Type::FreshShape(properties) => format!(
            "{{ {} }}",
            properties
                .iter()
                .map(|property| format!(
                    "{}{}: {}",
                    property.name,
                    if property.optional { "?" } else { "" },
                    describe_native(&property.ty, engine, stack)
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
                describe_native(&parameter.ty, engine, stack)
            )
        })
        .collect();
    if let Some(rest) = rest {
        let text = match engine.inspect_pack(&rest).as_deref() {
            Some([row]) if row.fixed.is_empty() && row.tail.is_some() => {
                describe_native(row.tail.as_ref().unwrap(), engine, stack)
            }
            _ => describe_native_pack(&rest, engine, stack).unwrap_or_else(|| "...".to_owned()),
        };
        parameters.push(format!("...: {text}"));
    }
    let returns = original
        .and_then(|ty| engine.function_result_pack(ty))
        .and_then(|pack| describe_native_pack(&pack, engine, stack))
        .unwrap_or_else(|| describe_native(&function.returns, engine, stack));
    format!("fun({}): {}", parameters.join(", "), returns)
}

fn describe_native_pack(
    pack: &luanalysis_engine::ValuePack,
    engine: &mut dyn TypeQueries,
    stack: &mut Vec<Type>,
) -> Option<String> {
    let alternatives = engine.inspect_pack(pack)?;
    if alternatives.is_empty() {
        return Some("never".to_owned());
    }
    Some(
        alternatives
            .iter()
            .map(|row| {
                let mut slots: Vec<_> = row
                    .fixed
                    .iter()
                    .map(|slot| {
                        let ty = describe_native(&slot.ty, engine, stack);
                        if slot.optional { format!("{ty}?") } else { ty }
                    })
                    .collect();
                if let Some(tail) = &row.tail {
                    slots.push(format!("{}...", describe_native(tail, engine, stack)));
                }
                match slots.len() {
                    0 => "void".to_owned(),
                    1 if row.tail.is_none() => slots.remove(0),
                    _ => format!("({})", slots.join(", ")),
                }
            })
            .collect::<Vec<_>>()
            .join(" | "),
    )
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
