//! Original-Lua declarations supplied to the native checker's lazy signatures.
//!
//! The immutable registry owns one parsed source snapshot. Provider callbacks
//! create independent Lua body checkers with the native query capability that
//! requested the body. They never retain a mutable report borrow across a query.

use super::native_types::{NativeTypes, TypeParameterRecipe, TypeScopeRecipe};
use super::{Binding, CheckReport, Checker, Diagnostic, Reference, bind};
use bind::{BindingId, BoundFile};
use luanalysis_engine::{
    NativeFunctionKey, NativeTypeParameterKey, NativeTypeProvider, Parameter, Type, TypeQueries,
};
use luanalysis_syntax::{
    Expr, ExprKind, File, Function, Name, Span, Stmt, StmtKind, TypeAnnotation, TypeExpr,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SNAPSHOT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
enum DeclarationSource {
    Local {
        annotation: Option<TypeExpr>,
        owner: Option<u32>,
        endpoint: usize,
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
    completed_bindings: RefCell<HashMap<BindingId, Binding>>,
    /// Only completion of body diagnostics is cached here. Return-type cache,
    /// query recursion and cycle decisions belong entirely to the native core.
    checked_bodies: RefCell<HashSet<NativeFunctionKey>>,
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
        let types = NativeTypes::new(snapshot, type_scope_recipes(snapshot, &functions, &bound));
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
            completed_bindings: RefCell::new(HashMap::new()),
            checked_bodies: RefCell::new(HashSet::new()),
            report: RefCell::new(report),
        }
    }

    pub(super) fn bound(&self) -> &BoundFile {
        &self.bound
    }

    pub(super) fn statements(&self) -> &[Stmt] {
        &self.file.statements
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

    fn function(&self, key: NativeFunctionKey) -> &Function {
        assert!(
            self.contains_function(key),
            "foreign or missing native Lua function identity"
        );
        &self.functions[key.index as usize]
    }

    fn checker<'a>(&'a self, engine: &'a mut dyn TypeQueries) -> Checker<'a> {
        Checker {
            engine,
            bound: &self.bound,
            native: Some(self),
            locals: HashMap::new(),
            assigned_fields: HashMap::new(),
            report: CheckReport::default(),
            return_type: None,
            return_values: Vec::new(),
            type_context: None,
        }
    }

    fn merge_report(&self, report: CheckReport) {
        self.report.borrow_mut().append(report);
    }

    pub(super) fn take_report(&self) -> CheckReport {
        std::mem::take(&mut *self.report.borrow_mut())
    }

    pub(super) fn publish_binding(&self, binding: BindingId, value: &Binding) {
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

    fn metadata(
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

    /// Resolves a captured declaration's *constraint*, rather than importing
    /// the transient narrowing present when a closure happened to be created.
    /// Binding identities already encode Lua activation and lexical capture.
    pub(super) fn resolve_binding(
        &self,
        binding: BindingId,
        queries: &mut dyn TypeQueries,
    ) -> Option<Binding> {
        if let Some(value) = self.completed_bindings.borrow().get(&binding).cloned() {
            return Some(value);
        }
        let source = self.declarations.get(&binding)?.clone();
        let mut checker = self.checker(queries);
        let (constraint, evolves_from_nil) = match source {
            DeclarationSource::Local {
                annotation,
                owner,
                endpoint,
            } => {
                checker.type_context = owner.map(|owner| self.key(owner));
                if let Some(annotation) = annotation {
                    // The declaration already supplies the captured type.
                    // Its file/body check still validates the initializer.
                    (checker.annotation(&annotation), false)
                } else {
                    // Resolve the initializer at its original lexical/flow
                    // point, rather than replaying it against broad captures.
                    if let Some(owner) = owner {
                        let (parameters, _) = self.metadata(self.key(owner), checker.engine);
                        self.seed_parameters(
                            &mut checker,
                            &self.functions[owner as usize],
                            &parameters,
                        );
                    }
                    let statements = owner.map_or(self.file.statements.as_slice(), |owner| {
                        self.functions[owner as usize].body.as_slice()
                    });
                    if let Some(span) = control_span(statements) {
                        self.unsupported_flow(span);
                        return Some(Binding {
                            constraint: Type::Unknown,
                            value: Type::Unknown,
                            evolves_from_nil: false,
                        });
                    }
                    self.run_prefix(
                        &mut checker,
                        statements,
                        endpoint,
                        vec![Reference {
                            binding,
                            path: Vec::new(),
                        }],
                    );
                    let resolved = checker
                        .locals
                        .get(&binding)
                        .cloned()
                        .expect("the declaration's causal prefix includes its initializer");
                    self.merge_report(checker.report);
                    self.publish_binding(binding, &resolved);
                    return Some(Binding {
                        value: resolved.constraint.clone(),
                        ..resolved
                    });
                }
            }
            DeclarationSource::Function(index) => {
                checker.type_context = Some(self.key(index));
                let function = &self.functions[index as usize];
                let constraint = function
                    .annotations
                    .type_annotation
                    .as_ref()
                    .and_then(|annotation| annotation.types.first())
                    .map_or(Type::NativeFunction(self.key(index)), |ty| {
                        checker.annotation(ty)
                    });
                (constraint, false)
            }
            DeclarationSource::Parameter { function, index } => {
                let (parameters, _) = self.metadata(self.key(function), checker.engine);
                let parameter = parameters
                    .get(index)
                    .expect("a bound Lua parameter has metadata");
                let ty = if parameter.optional {
                    Type::Union(vec![parameter.ty.clone(), Type::Nil])
                } else {
                    parameter.ty.clone()
                };
                (ty, false)
            }
            DeclarationSource::NumericLoop => (Type::Number, false),
            DeclarationSource::GenericLoop => (Type::Unknown, false),
        };
        self.merge_report(checker.report);
        Some(Binding {
            value: constraint.clone(),
            constraint,
            evolves_from_nil,
        })
    }

    fn seed_parameters(
        &self,
        checker: &mut Checker<'_>,
        function: &Function,
        parameters: &[Parameter],
    ) {
        for (name, parameter) in function.params.iter().zip(parameters) {
            let binding = self
                .bound
                .binding_at(name.span.start)
                .expect("bound Lua function parameter");
            let ty = if parameter.optional {
                Type::Union(vec![parameter.ty.clone(), Type::Nil])
            } else {
                parameter.ty.clone()
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

    fn source_reference(&self, expression: &Expr) -> Option<Reference> {
        match &expression.kind {
            ExprKind::Name(name) => {
                self.bound
                    .binding_at(name.span.start)
                    .map(|binding| Reference {
                        binding,
                        path: Vec::new(),
                    })
            }
            ExprKind::Parenthesized(inner) => self.source_reference(inner),
            ExprKind::Field { base, name } => {
                let mut reference = self.source_reference(base)?;
                reference.path.push(name.text.clone());
                Some(reference)
            }
            ExprKind::Index { base, index } => {
                let ExprKind::String(key) = &index.kind else {
                    return None;
                };
                let mut reference = self.source_reference(base)?;
                reference
                    .path
                    .push(std::str::from_utf8(key).ok()?.to_owned());
                Some(reference)
            }
            _ => None,
        }
    }

    fn expression_dependencies(&self, expression: &Expr, dependencies: &mut HashSet<Reference>) {
        if let Some(reference) = self.source_reference(expression) {
            dependencies.insert(reference);
            return;
        }
        match &expression.kind {
            ExprKind::Parenthesized(inner) => self.expression_dependencies(inner, dependencies),
            ExprKind::Field { base, .. } => self.expression_dependencies(base, dependencies),
            ExprKind::Index { base, index } => {
                self.expression_dependencies(base, dependencies);
                self.expression_dependencies(index, dependencies);
            }
            ExprKind::Unary { value, .. } => self.expression_dependencies(value, dependencies),
            ExprKind::Binary { left, right, .. } => {
                self.expression_dependencies(left, dependencies);
                self.expression_dependencies(right, dependencies);
            }
            ExprKind::Table(fields) => {
                for field in fields {
                    self.expression_dependencies(&field.value, dependencies);
                }
            }
            ExprKind::Call { callee, args } => {
                self.expression_dependencies(callee, dependencies);
                for argument in args {
                    self.expression_dependencies(argument, dependencies);
                }
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                self.expression_dependencies(receiver, dependencies);
                for argument in args {
                    self.expression_dependencies(argument, dependencies);
                }
            }
            // A returned closure is a lazy native source identity. Its capture
            // declarations are demanded at their original source points when
            // the inner signature's own return query needs them.
            ExprKind::Function(_)
            | ExprKind::Nil
            | ExprKind::Boolean(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Name(_)
            | ExprKind::Error => {}
        }
    }

    /// Check only declaration/assignment facts causally required by a type
    /// demand. Omitted calls and initializers still get full diagnostics after
    /// the native return cache has resolved, so they cannot create artificial
    /// return cycles merely because they execute before a literal return.
    fn run_prefix(
        &self,
        checker: &mut Checker<'_>,
        statements: &[Stmt],
        endpoint: usize,
        dependencies: Vec<Reference>,
    ) {
        let mut flattened = Vec::new();
        flatten_statements(statements, &mut flattened);
        let mut dependencies: HashSet<_> = dependencies.into_iter().collect();
        let mut selected = Vec::new();
        for statement in flattened
            .into_iter()
            .rev()
            .filter(|statement| statement.span.end <= endpoint)
        {
            match &statement.kind {
                StmtKind::Local { names, values, .. } => {
                    let indices: Vec<_> = names
                        .iter()
                        .enumerate()
                        .filter_map(|(index, name)| {
                            let binding = self.bound.binding_at(name.span.start)?;
                            dependencies
                                .iter()
                                .any(|dependency| dependency.binding == binding)
                                .then_some(index)
                        })
                        .collect();
                    for &index in &indices {
                        if let Some(value) = values.get(index) {
                            self.expression_dependencies(value, &mut dependencies);
                        }
                    }
                    if !indices.is_empty() {
                        selected.push(PrefixStatement::Local(statement, indices));
                    }
                }
                StmtKind::Assign {
                    targets, values, ..
                } => {
                    let indices: Vec<_> = targets
                        .iter()
                        .enumerate()
                        .filter_map(|(index, target)| {
                            let reference = self.source_reference(target)?;
                            dependencies
                                .iter()
                                .any(|dependency| {
                                    dependency.binding == reference.binding
                                        && dependency.path.starts_with(&reference.path)
                                })
                                .then_some(index)
                        })
                        .collect();
                    for &index in &indices {
                        if let Some(value) = values.get(index) {
                            self.expression_dependencies(value, &mut dependencies);
                        }
                        if let ExprKind::Field { base, .. } = &targets[index].kind {
                            self.expression_dependencies(base, &mut dependencies);
                        }
                        if let ExprKind::Index { base, index } = &targets[index].kind {
                            self.expression_dependencies(base, &mut dependencies);
                            self.expression_dependencies(index, &mut dependencies);
                        }
                    }
                    if !indices.is_empty() {
                        selected.push(PrefixStatement::Assign(statement, indices));
                    }
                }
                StmtKind::LocalFunction { name, .. } => {
                    let binding = self
                        .bound
                        .binding_at(name.span.start)
                        .expect("bound local function");
                    if dependencies
                        .iter()
                        .any(|dependency| dependency.binding == binding)
                    {
                        selected.push(PrefixStatement::Function(statement));
                    }
                }
                StmtKind::Return(_) | StmtKind::Call(_) => {}
                StmtKind::If { .. }
                | StmtKind::While { .. }
                | StmtKind::Repeat { .. }
                | StmtKind::NumericFor { .. }
                | StmtKind::GenericFor { .. }
                | StmtKind::Break => {
                    self.unsupported_flow(statement.span);
                }
                StmtKind::Block { .. } => {
                    unreachable!("blocks are flattened without changing bound identities")
                }
            }
        }
        for statement in selected.into_iter().rev() {
            match statement {
                PrefixStatement::Local(statement, indices) => {
                    let StmtKind::Local {
                        names,
                        values,
                        annotation,
                    } = &statement.kind
                    else {
                        unreachable!()
                    };
                    let selected_names: Vec<_> =
                        indices.iter().map(|&index| names[index].clone()).collect();
                    let selected_values: Vec<_> = indices
                        .iter()
                        .map(|&index| {
                            values.get(index).cloned().unwrap_or(Expr {
                                kind: ExprKind::Nil,
                                span: names[index].span,
                            })
                        })
                        .collect();
                    let selected_annotation =
                        annotation.as_ref().map(|annotation| TypeAnnotation {
                            types: indices
                                .iter()
                                .filter_map(|&index| annotation.types.get(index).cloned())
                                .collect(),
                            span: annotation.span,
                        });
                    checker.local(
                        &selected_names,
                        &selected_values,
                        selected_annotation.as_ref(),
                    );
                }
                PrefixStatement::Assign(statement, indices) => {
                    let StmtKind::Assign {
                        targets, values, ..
                    } = &statement.kind
                    else {
                        unreachable!()
                    };
                    let selected_targets: Vec<_> = indices
                        .iter()
                        .map(|&index| targets[index].clone())
                        .collect();
                    let selected_values: Vec<_> = indices
                        .iter()
                        .map(|&index| {
                            values.get(index).cloned().unwrap_or(Expr {
                                kind: ExprKind::Nil,
                                span: targets[index].span,
                            })
                        })
                        .collect();
                    checker.assign(&selected_targets, &selected_values, None);
                }
                PrefixStatement::Function(statement) => {
                    let StmtKind::LocalFunction { name, function } = &statement.kind else {
                        unreachable!()
                    };
                    checker.local_function(name, function);
                }
            }
        }
    }

    fn inferred_return(&self, key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Type {
        let function = self.function(key);
        if let Some(span) = control_span(&function.body) {
            self.unsupported_flow(span);
            return Type::Unknown;
        }
        let (parameters, context) = self.metadata(key, queries);
        let mut flattened = Vec::new();
        flatten_statements(&function.body, &mut flattened);
        for statement in flattened {
            if let StmtKind::Return(values) = &statement.kind {
                let mut dependencies = HashSet::new();
                for value in values {
                    self.expression_dependencies(value, &mut dependencies);
                }
                let mut checker = self.checker(queries);
                checker.type_context = Some(key);
                self.seed_parameters(&mut checker, function, &parameters);
                self.run_prefix(
                    &mut checker,
                    &function.body,
                    statement.span.start,
                    dependencies.into_iter().collect(),
                );
                checker.return_statement(values, statement.span);
                let returned = checker.engine.aggregate_returns(
                    &checker.return_values,
                    false,
                    context.as_ref(),
                );
                self.merge_report(checker.report);
                return returned;
            }
            if let StmtKind::Call(expression) = &statement.kind {
                let mut dependencies = HashSet::new();
                match &expression.kind {
                    ExprKind::Call { callee, .. } => {
                        self.expression_dependencies(callee, &mut dependencies)
                    }
                    ExprKind::MethodCall { receiver, .. } => {
                        self.expression_dependencies(receiver, &mut dependencies)
                    }
                    _ => continue,
                }
                let mut checker = self.checker(queries);
                checker.type_context = Some(key);
                self.seed_parameters(&mut checker, function, &parameters);
                self.run_prefix(
                    &mut checker,
                    &function.body,
                    statement.span.start,
                    dependencies.into_iter().collect(),
                );
                let callee_type = match &expression.kind {
                    ExprKind::Call { callee, .. } => checker.expression(callee),
                    ExprKind::MethodCall { receiver, name, .. } => {
                        let receiver_type = checker.expression(receiver);
                        checker
                            .engine
                            .property_type(&receiver_type, &name.text)
                            .unwrap_or(Type::Any)
                    }
                    _ => unreachable!(),
                };
                let declared_return = match callee_type {
                    Type::Function(signature) => Some(*signature.returns),
                    Type::NativeFunction(callee_key) => self.metadata(callee_key, checker.engine).1,
                    _ => None,
                };
                if let Some(Type::Never) = declared_return {
                    let returned =
                        checker
                            .engine
                            .aggregate_returns(&[Type::Never], false, context.as_ref());
                    self.merge_report(checker.report);
                    return returned;
                }
                self.merge_report(checker.report);
            }
        }
        queries.aggregate_returns(&[], true, context.as_ref())
    }

    fn check_body(&self, key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Type {
        let function = self.function(key);
        let (parameters, declared_return) = self.metadata(key, queries);
        let mut checker = self.checker(queries);
        checker.type_context = Some(key);
        checker.return_type = declared_return.clone();
        self.seed_parameters(&mut checker, function, &parameters);
        let terminated = checker.statements(&function.body);
        if let Some(expected) = &declared_return {
            if !terminated && !checker.engine.allows_implicit_return(expected) {
                let origin = function
                    .annotations
                    .returns
                    .as_ref()
                    .map_or(function.span, |annotation| annotation.span);
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
        self.merge_report(checker.report);
        self.checked_bodies.borrow_mut().insert(key);
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
            // Enter the native QueryReturn guard/cache for inferred returns.
            // An annotated return needs no body query; validate it below.
            queries.native_function_signature(key);
            let checked = self.checked_bodies.borrow().contains(&key);
            if !checked {
                self.check_body(key, queries);
            }
        }
    }

    fn unsupported_flow(&self, span: Span) {
        self.report.borrow_mut().diagnostics.push(Diagnostic {
            code: "LUA2020",
            span,
            message: "Lua control-flow type checking requires the native flow integration"
                .to_owned(),
        });
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

impl NativeTypeProvider for NativeRegistry {
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
        checker.type_context = Some(source.owner);
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

enum PrefixStatement<'a> {
    Local(&'a Stmt, Vec<usize>),
    Assign(&'a Stmt, Vec<usize>),
    Function(&'a Stmt),
}

fn flatten_statements<'a>(statements: &'a [Stmt], flattened: &mut Vec<&'a Stmt>) {
    for statement in statements {
        if let StmtKind::Block { body, .. } = &statement.kind {
            flatten_statements(body, flattened);
        } else {
            flattened.push(statement);
        }
    }
}

fn control_span(statements: &[Stmt]) -> Option<Span> {
    statements
        .iter()
        .find_map(|statement| match &statement.kind {
            StmtKind::Block { body, .. } => control_span(body),
            StmtKind::If { .. }
            | StmtKind::While { .. }
            | StmtKind::Repeat { .. }
            | StmtKind::NumericFor { .. }
            | StmtKind::GenericFor { .. }
            | StmtKind::Break => Some(statement.span),
            _ => None,
        })
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
                                endpoint: statement.span.end,
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
