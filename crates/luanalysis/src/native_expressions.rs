//! Immutable original-Lua expression demands and contextual signature facets.
//! Candidate collection, fixing, substitution and return caching stay native.
use crate::{CheckReport, native_functions::NativeRegistry};
use luanalysis_engine::{
    CapturedFunctionContext, ContextualFunctionContext, NativeBindingKey, NativeCheckMode,
    NativeExpressionKey, NativeFunctionKey, Parameter, Type, TypeQueries,
};
use luanalysis_syntax::{
    BinaryOperator, Expr, ExprKind, File, Function, Span, Stmt, StmtKind, TableFieldKind,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
pub(super) struct SourceExpression {
    pub expression: Expr,
    pub owner: Option<NativeFunctionKey>,
    pub parent: Option<NativeExpressionKey>,
}

/// Source recipes never contain backend IDs or a mutable "last context".
/// Final selected facets are exact session capabilities owned by the provider.
pub(super) struct NativeExpressions {
    snapshot: u64,
    expressions: Vec<SourceExpression>,
    spans: HashMap<(usize, usize), NativeExpressionKey>,
    functions: HashMap<(usize, usize), NativeFunctionKey>,
    final_facets: RefCell<HashMap<NativeFunctionKey, Vec<Type>>>,
    final_contexts: RefCell<HashMap<NativeFunctionKey, Vec<ContextualFunctionContext>>>,
    final_captures: RefCell<HashMap<NativeFunctionKey, Vec<CapturedFunctionContext>>>,
    context_captures:
        RefCell<HashMap<(NativeFunctionKey, ContextualFunctionContext), CapturedFunctionContext>>,
}
impl NativeExpressions {
    pub(super) fn new(
        snapshot: u64,
        file: &File,
        functions: &HashMap<(usize, usize), u32>,
    ) -> Self {
        let mut registry = Self {
            snapshot,
            expressions: Vec::new(),
            spans: HashMap::new(),
            functions: functions
                .iter()
                .map(|(span, index)| {
                    (
                        *span,
                        NativeFunctionKey {
                            snapshot,
                            index: *index,
                        },
                    )
                })
                .collect(),
            final_facets: RefCell::new(HashMap::new()),
            final_contexts: RefCell::new(HashMap::new()),
            final_captures: RefCell::new(HashMap::new()),
            context_captures: RefCell::new(HashMap::new()),
        };
        registry.statements(&file.statements, None, None);
        registry
    }
    pub(super) fn contains(&self, key: NativeExpressionKey) -> bool {
        key.snapshot == self.snapshot && (key.node as usize) < self.expressions.len()
    }
    pub(super) fn get(&self, key: NativeExpressionKey) -> &SourceExpression {
        assert!(self.contains(key), "foreign original Lua expression");
        &self.expressions[key.node as usize]
    }
    pub(super) fn key_for_span(&self, span: Span) -> NativeExpressionKey {
        *self
            .spans
            .get(&(span.start, span.end))
            .expect("expression belongs to original Lua snapshot")
    }
    pub(super) fn function_for(&self, key: NativeExpressionKey) -> Option<NativeFunctionKey> {
        let mut expression = &self.get(key).expression;
        while let ExprKind::Parenthesized(inner) = &expression.kind {
            expression = inner;
        }
        if let ExprKind::Function(function) = &expression.kind {
            Some(self.function_key(function))
        } else {
            None
        }
    }
    fn function_key(&self, function: &Function) -> NativeFunctionKey {
        self.functions[&(function.span.start, function.span.end)]
    }
    pub(super) fn descendant(
        &self,
        mut expression: NativeExpressionKey,
        ancestor: NativeExpressionKey,
    ) -> bool {
        if !self.contains(expression) || !self.contains(ancestor) {
            return false;
        }
        loop {
            if expression == ancestor {
                return true;
            }
            let Some(parent) = self.get(expression).parent else {
                return false;
            };
            expression = parent;
        }
    }
    pub(super) fn contextual_indices(function: &Function) -> Vec<usize> {
        function
            .params
            .iter()
            .enumerate()
            .filter_map(|(index, param)| {
                let explicit_tag = function
                    .annotations
                    .params
                    .iter()
                    .any(|tag| tag.name.text == param.text);
                // An explicit function @type supplies contextual metadata in the
                // ordinary declaration provider, independent of call inference.
                (!explicit_tag && function.annotations.type_annotation.is_none()).then_some(index)
            })
            .collect()
    }
    pub(super) fn has_context_sensitive_parameters(&self, key: NativeExpressionKey) -> bool {
        let mut expression = &self.get(key).expression;
        while let ExprKind::Parenthesized(inner) = &expression.kind {
            expression = inner;
        }
        matches!(&expression.kind,ExprKind::Function(function) if Self::has_contextual_parameters(function))
    }

    fn has_contextual_parameters(function: &Function) -> bool {
        !Self::contextual_indices(function).is_empty()
            || (function.vararg.is_some()
                && function.annotations.vararg.is_none()
                && function.annotations.type_annotation.is_none())
    }
    pub(super) fn context_sensitive(&self, key: NativeExpressionKey) -> bool {
        self.sensitive(&self.get(key).expression)
    }
    fn sensitive(&self, expression: &Expr) -> bool {
        match &expression.kind {
            ExprKind::Function(function) => {
                Self::has_contextual_parameters(function) || self.sensitive_returns(&function.body)
            }
            ExprKind::Parenthesized(inner) => self.sensitive(inner),
            ExprKind::Table(fields) => fields.iter().any(|field| {
                matches!(&field.kind, TableFieldKind::Keyed(key) if self.sensitive(key))
                    || self.sensitive(&field.value)
            }),
            ExprKind::Binary {
                op: BinaryOperator::And | BinaryOperator::Or,
                left,
                right,
                ..
            } => self.sensitive(left) || self.sensitive(right),
            _ => false,
        }
    }
    fn sensitive_returns(&self, statements: &[Stmt]) -> bool {
        statements.iter().any(|statement| match &statement.kind {
            StmtKind::Return(values) => values.iter().any(|value| self.sensitive(value)),
            StmtKind::Block { body, .. }
            | StmtKind::While { body, .. }
            | StmtKind::Repeat { body, .. }
            | StmtKind::NumericFor { body, .. }
            | StmtKind::GenericFor { body, .. } => self.sensitive_returns(body),
            StmtKind::If {
                branches,
                else_branch,
            } => {
                branches
                    .iter()
                    .any(|branch| self.sensitive_returns(&branch.body))
                    || else_branch
                        .as_ref()
                        .is_some_and(|branch| self.sensitive_returns(&branch.body))
            }
            _ => false,
        })
    }
    pub(super) fn final_facets(&self, key: NativeFunctionKey) -> Vec<Type> {
        self.final_facets
            .borrow()
            .get(&key)
            .cloned()
            .unwrap_or_default()
    }
    pub(super) fn final_contexts(&self, key: NativeFunctionKey) -> Vec<ContextualFunctionContext> {
        self.final_contexts
            .borrow()
            .get(&key)
            .cloned()
            .unwrap_or_default()
    }
    pub(super) fn final_captures(&self, key: NativeFunctionKey) -> Vec<CapturedFunctionContext> {
        self.final_captures
            .borrow()
            .get(&key)
            .cloned()
            .unwrap_or_default()
    }
    pub(super) fn capture_for_context(
        &self,
        key: NativeFunctionKey,
        context: &ContextualFunctionContext,
    ) -> Option<CapturedFunctionContext> {
        self.context_captures
            .borrow()
            .get(&(key, *context))
            .copied()
    }
    pub(super) fn contextual_captures(
        &self,
        key: NativeFunctionKey,
    ) -> HashSet<CapturedFunctionContext> {
        self.context_captures
            .borrow()
            .iter()
            .filter_map(|((function, _), capture)| (*function == key).then_some(*capture))
            .collect()
    }
    fn retain_context_capture(
        &self,
        key: NativeFunctionKey,
        context: &ContextualFunctionContext,
        captured: &CapturedFunctionContext,
    ) {
        if publishes(context.mode) {
            let mut contexts = self.context_captures.borrow_mut();
            if let Some(existing) = contexts.insert((key, *context), *captured) {
                assert_eq!(
                    existing, *captured,
                    "a selected native facet has one immutable capture environment"
                );
            }
        }
    }
    fn retain_capture(
        &self,
        key: NativeFunctionKey,
        context: &CapturedFunctionContext,
        mode: NativeCheckMode,
    ) {
        if publishes(mode) {
            let mut captures = self.final_captures.borrow_mut();
            let captures = captures.entry(key).or_default();
            if !captures.contains(context) {
                captures.push(*context);
            }
        }
    }
    fn retain_context(&self, key: NativeFunctionKey, context: &ContextualFunctionContext) {
        if publishes(context.mode) {
            let mut contexts = self.final_contexts.borrow_mut();
            let contexts = contexts.entry(key).or_default();
            if !contexts.contains(context) {
                contexts.push(*context);
            }
        }
    }
    fn retain_final(&self, key: NativeFunctionKey, mode: NativeCheckMode, ty: &Type) {
        if publishes(mode) {
            let mut facets = self.final_facets.borrow_mut();
            let facets = facets.entry(key).or_default();
            if !facets.contains(ty) {
                facets.push(ty.clone());
            }
        }
    }
    fn statements(
        &mut self,
        statements: &[Stmt],
        owner: Option<NativeFunctionKey>,
        parent: Option<NativeExpressionKey>,
    ) {
        for statement in statements {
            match &statement.kind {
                StmtKind::Local { values, .. } | StmtKind::Return(values) => {
                    for value in values {
                        self.index(value, owner, parent);
                    }
                }
                StmtKind::Assign {
                    targets, values, ..
                } => {
                    for value in targets.iter().chain(values) {
                        self.index(value, owner, parent);
                    }
                }
                StmtKind::LocalFunction { function, .. } => {
                    self.statements(&function.body, Some(self.function_key(function)), parent)
                }
                StmtKind::FunctionDeclaration {
                    target, function, ..
                } => {
                    self.index(target, owner, parent);
                    self.statements(&function.body, Some(self.function_key(function)), parent);
                }
                StmtKind::Block { body, .. } => self.statements(body, owner, parent),
                StmtKind::Call(expression) => {
                    self.index(expression, owner, parent);
                }
                StmtKind::If {
                    branches,
                    else_branch,
                } => {
                    for branch in branches {
                        self.index(&branch.condition, owner, parent);
                        self.statements(&branch.body, owner, parent);
                    }
                    if let Some(branch) = else_branch {
                        self.statements(&branch.body, owner, parent);
                    }
                }
                StmtKind::While {
                    condition, body, ..
                }
                | StmtKind::Repeat {
                    condition, body, ..
                } => {
                    self.index(condition, owner, parent);
                    self.statements(body, owner, parent);
                }
                StmtKind::NumericFor {
                    start,
                    limit,
                    step,
                    body,
                    ..
                } => {
                    self.index(start, owner, parent);
                    self.index(limit, owner, parent);
                    if let Some(step) = step {
                        self.index(step, owner, parent);
                    }
                    self.statements(body, owner, parent);
                }
                StmtKind::GenericFor { values, body, .. } => {
                    for value in values {
                        self.index(value, owner, parent);
                    }
                    self.statements(body, owner, parent);
                }
                StmtKind::Break => {}
            }
        }
    }
    fn index(
        &mut self,
        expression: &Expr,
        owner: Option<NativeFunctionKey>,
        parent: Option<NativeExpressionKey>,
    ) -> NativeExpressionKey {
        let span = (expression.span.start, expression.span.end);
        if let Some(key) = self.spans.get(&span) {
            return *key;
        }
        let key = NativeExpressionKey {
            snapshot: self.snapshot,
            node: u32::try_from(self.expressions.len()).expect("source expression index fits u32"),
        };
        self.spans.insert(span, key);
        self.expressions.push(SourceExpression {
            expression: expression.clone(),
            owner,
            parent,
        });
        let parent = Some(key);
        match &expression.kind {
            ExprKind::Function(function) => {
                self.statements(&function.body, Some(self.function_key(function)), parent)
            }
            ExprKind::Parenthesized(inner)
            | ExprKind::Unary { value: inner, .. }
            | ExprKind::Field { base: inner, .. } => {
                self.index(inner, owner, parent);
            }
            ExprKind::Index { base, index }
            | ExprKind::Binary {
                left: base,
                right: index,
                ..
            } => {
                self.index(base, owner, parent);
                self.index(index, owner, parent);
            }
            ExprKind::Table(fields) => {
                for field in fields {
                    if let TableFieldKind::Keyed(key) = &field.kind {
                        self.index(key, owner, parent);
                    }
                    self.index(&field.value, owner, parent);
                }
            }
            ExprKind::Call { callee, args } => {
                self.index(callee, owner, parent);
                for arg in args {
                    self.index(arg, owner, parent);
                }
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                self.index(receiver, owner, parent);
                for arg in args {
                    self.index(arg, owner, parent);
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
        key
    }
}

/// Trial/inference facets remain native facts, never user-visible report state.
pub(super) fn publishes(mode: NativeCheckMode) -> bool {
    !mode.intersects(
        NativeCheckMode::INFERENTIAL
            | NativeCheckMode::SKIP_CONTEXT_SENSITIVE
            | NativeCheckMode::SKIP_GENERIC_FUNCTIONS
            | NativeCheckMode::TYPE_ONLY
            | NativeCheckMode::IS_FOR_SIGNATURE_HELP,
    )
}

impl NativeRegistry {
    pub(super) fn source_binding_key(&self, binding: crate::bind::BindingId) -> NativeBindingKey {
        assert!(
            binding.0 < self.bound().declarations.len(),
            "original source binding"
        );
        NativeBindingKey {
            snapshot: self.expressions().snapshot,
            index: u32::try_from(binding.0).expect("source binding index fits u32"),
        }
    }
    pub(super) fn expression_key(&self, expression: &Expr) -> NativeExpressionKey {
        self.expressions().key_for_span(expression.span)
    }
    pub(super) fn expression_key_for_span(&self, span: Span) -> NativeExpressionKey {
        self.expressions().key_for_span(span)
    }
    pub(super) fn source_check_expression(
        &self,
        key: NativeExpressionKey,
        expected: Option<&Type>,
        mode: NativeCheckMode,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        let record = self.expressions().get(key);
        if let Some(function) = self.expressions().function_for(key) {
            if let Some(expected) = expected {
                let value = self.source_function_value(self.function(function), mode, queries);
                let ty = queries.contextual_function_value(&value, key, expected, mode);
                self.expressions().retain_final(function, mode, &ty);
                return ty;
            }
        }
        let mut checker = self.checker_for_expression(key, mode, queries);
        checker.check_mode = mode;
        let ty = checker.expression_with_expected(&record.expression, expected);
        if publishes(mode) {
            self.merge_report(checker.report);
        }
        ty
    }
    /// Captures only lexical binding-cell constraints. Expected return
    /// inference variables are not part of this immutable environment.
    pub(super) fn source_function_value(
        &self,
        function: &Function,
        mode: NativeCheckMode,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        let key = self.key_for(function);
        let contextual = queries.current_contextual_function();
        let captured = queries.current_captured_function();
        if contextual.is_none() && captured.is_none() {
            return Type::NativeFunction(key);
        }
        let bindings = self
            .bound()
            .scopes
            .iter()
            .find(|scope| {
                scope.kind == crate::bind::ScopeKind::Function && scope.span == function.span
            })
            .map(|scope| scope.captures.clone())
            .unwrap_or_default();
        if bindings.is_empty() {
            return Type::NativeFunction(key);
        }
        let mut environment = Vec::with_capacity(bindings.len());
        for binding in bindings {
            // A recursive declaration refers to this signature itself. It is
            // resolved by the native captured-function capability in the body,
            // rather than recursively constructing another frozen environment.
            if self.binding_function_key(binding) == Some(key) {
                continue;
            }
            let source = self.source_binding_key(binding);
            let value = captured.and_then(|context| queries.captured_binding(&context, source));
            let value = match value {
                Some(value) => value,
                None => {
                    let resolved = if let Some(context) = contextual {
                        self.resolve_binding_in_context(binding, &context, queries)
                    } else {
                        self.resolve_binding(binding, queries)
                    };
                    resolved
                        .expect("a lexical capture has a declaration constraint")
                        .constraint
                }
            };
            environment.push((source, value));
        }
        let value = queries.captured_function(key, &environment);
        self.expressions().retain_final(key, mode, &value);
        value
    }
    pub(super) fn source_parameters_in_capture(
        &self,
        key: NativeFunctionKey,
        context: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        self.metadata_in_capture(key, context, queries).0
    }
    pub(super) fn source_declared_return_in_capture(
        &self,
        key: NativeFunctionKey,
        context: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        self.metadata_in_capture(key, context, queries).1
    }
    pub(super) fn metadata_in_capture(
        &self,
        key: NativeFunctionKey,
        context: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> (Vec<Parameter>, Option<Type>) {
        let mode = queries
            .current_contextual_function()
            .map_or_else(NativeCheckMode::empty, |context| context.mode);
        self.expressions().retain_capture(key, context, mode);
        let mut checker = self.checker(queries);
        checker.type_context = Some(key);
        // Signature metadata is declaration-relative. The native signature
        // mapper instantiates it; only body checks carry the capture capability
        // in Checker and map complete source annotations themselves.
        checker.captured_function = None;
        checker.check_mode = mode;
        let metadata = checker.signature_parts(self.function(key));
        if publishes(mode) {
            self.merge_report(checker.report);
        }
        metadata
    }
    pub(super) fn source_parameters_in_context(
        &self,
        key: NativeFunctionKey,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        self.metadata_in_context(key, context, queries).0
    }
    pub(super) fn source_declared_return_in_context(
        &self,
        key: NativeFunctionKey,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        self.metadata_in_context(key, context, queries).1
    }
    pub(super) fn metadata_in_context(
        &self,
        key: NativeFunctionKey,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> (Vec<Parameter>, Option<Type>) {
        self.expressions().retain_context(key, context);
        let mut checker = self.checker(queries);
        checker.type_context = Some(key);
        checker.contextual_function = Some(*context);
        if let Some(captured) = checker.engine.current_captured_function() {
            let value = checker.engine.captured_function_value(&captured);
            // An ambient parent environment is not the child's signature
            // facet. Bind final validation to the source declaration that
            // actually owns this capture capability.
            if checker.engine.native_function_key(&value) == Some(key) {
                self.expressions()
                    .retain_capture(key, &captured, context.mode);
                self.expressions()
                    .retain_context_capture(key, context, &captured);
            }
        }
        // Explicit signature annotations remain declaration-space even when
        // this contextual facet also retains a lexical capture environment.
        checker.captured_function = None;
        checker.check_mode = context.mode;
        let metadata = checker.signature_parts(self.function(key));
        if publishes(context.mode) {
            self.merge_report(checker.report);
        }
        metadata
    }
    pub(super) fn contextual_report(
        &self,
        context: &ContextualFunctionContext,
        report: CheckReport,
    ) {
        if publishes(context.mode) {
            self.merge_report(report);
        }
    }
}
