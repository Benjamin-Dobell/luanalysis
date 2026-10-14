//! Original-source checking for the first native Lua subset.
//!
//! Lua syntax and straight-line bindings are owned here. Type construction,
//! literal widening, member lookup, freshness and assignability use the pinned
//! Bun core. Native generic calls share its inference worker. Branch flow,
//! contextual expression demands and project services are integration gates.

pub mod bind;
pub mod flow;
mod functions;
mod native_expressions;
mod native_flow;
mod native_functions;
mod native_types;
mod operators;

use bind::{BindingId, BoundFile};
use luanalysis_engine::{
    CapturedFunctionContext, ContextualFunctionContext, Engine, NativeCheckMode, NativeFlowKey,
    NativeFunctionKey, Property, Type, TypeQueries,
};
use luanalysis_syntax::{
    Expr, ExprKind, Name, Span, Stmt, StmtKind, TypeAnnotation, TypeExpr, TypeExprKind, TypeLiteral,
};
use native_functions::NativeRegistry;
use std::collections::HashMap;

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
    value: Type,
    evolves_from_nil: bool,
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
    contextual_function: Option<ContextualFunctionContext>,
    captured_function: Option<CapturedFunctionContext>,
    incomplete_flow: bool,
    annotation_depth: usize,
    check_mode: NativeCheckMode,
    flow_at: Option<NativeFlowKey>,
    locals: HashMap<BindingId, Binding>,
    assigned_fields: HashMap<Reference, Type>,
    report: CheckReport,
    return_type: Option<Type>,
    return_values: Vec<Type>,
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
            let mut report = {
                let mut checker = Checker {
                    engine,
                    bound: registry.bound(),
                    native: Some(registry),
                    type_context: None,
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
                    self.locals.insert(
                        binding,
                        Binding {
                            constraint: ty.clone(),
                            value: ty,
                            evolves_from_nil: false,
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
                let value = if let Some(context) = context {
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
                    start,
                    limit,
                    step,
                    body,
                    ..
                } => {
                    self.error(
                        "LUA2023",
                        statement.span,
                        "numeric iteration requires the native Lua iterator integration",
                    );
                    self.expression(start);
                    self.expression(limit);
                    if let Some(step) = step {
                        self.expression(step);
                    }
                    self.statements(body);
                }
                StmtKind::GenericFor { values, body, .. } => {
                    self.error(
                        "LUA2023",
                        statement.span,
                        "generic iteration requires the native Lua iterator integration",
                    );
                    for value in values {
                        self.expression(value);
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
        match &ty.kind {
            TypeExprKind::Named(name) => match name.as_str() {
                "any" => Type::Any,
                "unknown" => Type::Unknown,
                "never" => Type::Never,
                "nil" => Type::Nil,
                "boolean" => Type::Boolean,
                "number" => Type::Number,
                "string" => Type::String,
                "void" => Type::Void,
                _ => {
                    if let Some(ty) = self
                        .native
                        .and_then(|native| native.resolve_type_name(self.type_context, name))
                    {
                        return ty;
                    }
                    self.error(
                        "LUA2001",
                        ty.span,
                        format!("type {name:?} is not supported in this slice"),
                    );
                    Type::Any
                }
            },
            TypeExprKind::Union(types) => {
                Type::Union(types.iter().map(|ty| self.annotation(ty)).collect())
            }
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
            TypeExprKind::Record(fields) => Type::Shape(
                fields
                    .iter()
                    .map(|field| Property::new(field.name.text.clone(), self.annotation(&field.ty)))
                    .collect(),
            ),
            TypeExprKind::Parenthesized(ty) => self.annotation(ty),
            TypeExprKind::Function(function) => self.function_annotation(function),
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
        let ty = self.expression_at_current_flow(expr, expected);
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
                self.error(
                    "LUA2016",
                    expr.span,
                    "vararg values require the native Lua result-pack integration",
                );
                Type::Unknown
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
                            &binding.constraint,
                            self.engine,
                        );
                        self.incomplete_flow |= result.incomplete;
                        result.ty
                    } else {
                        binding.value
                    }
                }
                None => {
                    self.error(
                        "TS2304",
                        name.span,
                        format!("Cannot find name '{}'.", name.text),
                    );
                    Type::Any
                }
            },
            ExprKind::Parenthesized(value) => self.expression_with_expected(value, expected),
            ExprKind::Table(fields) => {
                let mut properties: Vec<Property> = Vec::new();
                let mut duplicate = false;
                for field in fields {
                    let field_context = expected
                        .and_then(|expected| self.engine.property_type(expected, &field.name.text));
                    let value = self.expression_with_expected(&field.value, field_context.as_ref());
                    let property =
                        Property::new(field.name.text.clone(), self.engine.widen(&value));
                    if properties.iter().any(|item| item.name == property.name) {
                        self.error("LUA2007", field.name.span, format!("duplicate table field '{}' has undefined assignment order in Lua 5.4 and is not supported", field.name.text));
                        duplicate = true;
                    } else {
                        properties.push(property);
                    }
                }
                if duplicate {
                    Type::Any
                } else {
                    Type::FreshShape(properties)
                }
            }
            ExprKind::Field { base, name } => {
                let receiver = self.expression(base);
                if matches!(receiver, Type::Any) {
                    return Type::Any;
                }
                if let Some(ty) = self.engine.property_type(&receiver, &name.text) {
                    if let (Some(native), Some(at)) = (self.native, self.flow_at) {
                        let result = native.flow_reference_type(expr, at, &ty, &ty, self.engine);
                        self.incomplete_flow |= result.incomplete;
                        result.ty
                    } else {
                        self.reference(expr)
                            .and_then(|reference| self.assigned_fields.get(&reference).cloned())
                            .unwrap_or(ty)
                    }
                } else {
                    self.error(
                        "TS2339",
                        name.span,
                        format!(
                            "Property '{}' does not exist on type '{}'.",
                            name.text,
                            describe(&receiver)
                        ),
                    );
                    Type::Any
                }
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
            ExprKind::Index { base, index } => self.index_expression(expr, base, index),
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
        if let Some(annotation) = annotation.filter(|annotation| annotation.packs.is_some()) {
            self.error("LUA2016", annotation.span, "variadic and alternative type packs require the native Lua result-pack integration");
        }
        let annotations: Vec<_> = annotation.map_or_else(Vec::new, |annotation| {
            annotation
                .types
                .iter()
                .map(|ty| self.annotation(ty))
                .collect()
        });
        // Type annotations provide call context without introducing the new
        // bindings. Every initializer still sees the preceding lexical scope.
        let values: Vec<_> = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                (
                    self.expression_with_expected(value, annotations.get(index)),
                    value.span,
                )
            })
            .collect();
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
            let constraint = annotations
                .get(index)
                .cloned()
                .unwrap_or_else(|| widened.clone());
            self.compatible(&value, &constraint, origin);
            let narrowed = self.engine.assignment_value(&constraint, &value);
            let id = self
                .bound
                .binding_at(name.span.start)
                .expect("bound Lua declaration");
            let displayed = constraint.clone();
            self.locals.insert(
                id,
                Binding {
                    constraint,
                    value: narrowed,
                    evolves_from_nil: annotations.get(index).is_none()
                        && matches!(widened, Type::Nil | Type::Undefined),
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
        let values: Vec<_> = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                (
                    self.expression_with_expected(
                        value,
                        expected.get(index).and_then(Option::as_ref),
                    ),
                    value.span,
                )
            })
            .collect();
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
                    if binding.evolves_from_nil && !matches!(value, Type::Nil | Type::Undefined) {
                        self.error("LUA2005", origin, "inference for a nil-initialized local requires the pending flow integration");
                    } else if self.compatible(&value, &constraint, origin) {
                        writes.push(Write::Local(
                            id,
                            self.engine.assignment_value(&constraint, &value),
                        ));
                    }
                }
                ExprKind::Field { .. } | ExprKind::Index { .. } => {
                    let (base, key, key_span) = match &target.kind {
                        ExprKind::Field { base, name } => {
                            (base.as_ref(), name.text.clone(), name.span)
                        }
                        ExprKind::Index { base, index } => {
                            self.expression(index);
                            let Some(key) = operators::literal_string_key(index)
                                .and_then(|key| std::str::from_utf8(key).ok())
                            else {
                                self.error("LUA2019", index.span, "typed numeric and dynamic table indices require the table/indexer integration");
                                continue;
                            };
                            (base.as_ref(), key.to_owned(), index.span)
                        }
                        _ => unreachable!(),
                    };
                    let receiver = self.expression(base);
                    if matches!(receiver, Type::Any) {
                        continue;
                    }
                    let Some(constraint) = self.engine.property_type(&receiver, &key) else {
                        self.error(
                            "TS2339",
                            key_span,
                            format!(
                                "Property '{}' does not exist on type '{}'.",
                                key,
                                describe(&receiver)
                            ),
                        );
                        continue;
                    };
                    if self.compatible(&value, &constraint, origin) {
                        if let Some(reference) = self.reference(target) {
                            writes.push(Write::Field(
                                reference,
                                self.engine.assignment_value(&constraint, &value),
                            ));
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
        Type::NativeFunction(_) => "function".into(),
        Type::NativeTypeParameter(_) => "type parameter".into(),
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
            let text = if &inspected == ty {
                engine.format(ty)
            } else {
                describe_native(&inspected, engine, stack)
            };
            stack.pop();
            text
        }
        Type::NativeTypeParameter(_) => engine.format(ty),
        Type::NativeFunction(key) => {
            if stack.contains(ty) {
                return "<recursive function>".into();
            }
            if stack.len() >= 64 {
                return "...".into();
            }
            stack.push(ty.clone());
            let signature = engine.native_function_signature(*key);
            let text = describe_native(&Type::Function(signature), engine, stack);
            stack.pop();
            text
        }
        Type::Function(function) => {
            let parameters: Vec<_> = function
                .params
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
            format!(
                "fun({}): {}",
                parameters.join(", "),
                describe_native(&function.returns, engine, stack)
            )
        }
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
