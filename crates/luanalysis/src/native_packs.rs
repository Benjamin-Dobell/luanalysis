//! Immutable original-source recipes for Lua result-list adjustment.
//!
//! This module records where Lua preserves multiple results. Native pack
//! queries perform projection, padding, joins, relations and inference.

use luanalysis_engine::{
    NativeExpressionKey, NativeFunctionKey, PackAlternative, PackSlot, Type, ValuePack,
};
use luanalysis_syntax::{
    Expr, ExprKind, File, Function, Span, Stmt, StmtKind, TableFieldKind, TypeAnnotation, TypeExpr,
    TypePacks,
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Adjustment {
    /// Exactly one result, including nil when the underlying pack is empty.
    Scalar,
    /// Preserve every result of an unparenthesized final call or vararg.
    Expand,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct SourcePackListKey {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PackExpression {
    pub expression: NativeExpressionKey,
    pub adjustment: Adjustment,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ListContext {
    Local { destinations: Vec<Span> },
    Assignment { destinations: Vec<Span> },
    Return,
    Arguments { implicit_receiver: bool },
    Iterator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ExpressionList {
    pub key: SourcePackListKey,
    pub owner: Option<NativeFunctionKey>,
    pub context: ListContext,
    pub expressions: Vec<PackExpression>,
    /// Enclosing original statement/call span; retained even for an empty list.
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SequenceElement {
    /// Syntactic one-based list ordinal; keyed fields never consume this index.
    pub index: usize,
    pub expression: PackExpression,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TableSequence {
    pub owner: Option<NativeFunctionKey>,
    pub elements: Vec<SequenceElement>,
    pub span: Span,
}

/// Ranges refer directly to the annotation's canonical type vector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AnnotationAlternative {
    pub fixed: std::ops::Range<usize>,
    pub variadic: Option<usize>,
    pub span: Span,
}

pub(super) struct AnnotationPack<'a> {
    pub types: &'a [TypeExpr],
    pub alternatives: Vec<AnnotationAlternative>,
    pub span: Span,
}

impl<'a> AnnotationPack<'a> {
    pub(super) fn new(types: &'a [TypeExpr], packs: Option<&TypePacks>, span: Span) -> Self {
        let alternatives = packs.map_or_else(
            || {
                vec![AnnotationAlternative {
                    fixed: 0..types.len(),
                    variadic: None,
                    span,
                }]
            },
            |packs| {
                packs
                    .alternatives
                    .iter()
                    .map(|pack| {
                        assert!(
                            pack.fixed.end <= types.len(),
                            "original pack range is canonical"
                        );
                        assert!(
                            pack.variadic
                                .is_none_or(|tail| tail.type_index < types.len()),
                            "original pack tail is canonical"
                        );
                        AnnotationAlternative {
                            fixed: pack.fixed.clone(),
                            variadic: pack.variadic.map(|tail| tail.type_index),
                            span: pack.span,
                        }
                    })
                    .collect()
            },
        );
        Self {
            types,
            alternatives,
            span,
        }
    }

    pub(super) fn annotation(annotation: &'a TypeAnnotation) -> Self {
        Self::new(
            &annotation.types,
            annotation.packs.as_deref(),
            annotation.span,
        )
    }

    /// Lower source annotations once in the declaration's native type scope.
    /// Alternative rows remain separate; native queries own their relations.
    pub(super) fn lower(&self, annotation: &mut dyn FnMut(&TypeExpr) -> Type) -> ValuePack {
        let types: Vec<_> = self.types.iter().map(annotation).collect();
        if types.as_slice() == [Type::Void]
            && self.alternatives.len() == 1
            && self.alternatives[0].fixed == (0..1)
            && self.alternatives[0].variadic.is_none()
        {
            return ValuePack::empty();
        }
        if types.as_slice() == [Type::Never]
            && self.alternatives.len() == 1
            && self.alternatives[0].fixed == (0..1)
            && self.alternatives[0].variadic.is_none()
        {
            return ValuePack::never();
        }
        ValuePack::Alternatives(
            self.alternatives
                .iter()
                .map(|alternative| PackAlternative {
                    fixed: alternative
                        .fixed
                        .clone()
                        .map(|index| PackSlot::required(types[index].clone()))
                        .collect(),
                    tail: alternative
                        .variadic
                        .map(|index| Box::new(types[index].clone())),
                })
                .collect(),
        )
    }
}

#[derive(Clone, Debug)]
pub(super) enum InitializerContext {
    Local(TypeAnnotation),
    LocalDeclarations(Vec<luanalysis_syntax::Name>),
    Assignment(Vec<Expr>),
}

/// The source index stores recipes, never an inferred type or an active facet.
pub(super) struct NativePacks {
    snapshot: u64,
    lists: Vec<ExpressionList>,
    list_spans: HashMap<(usize, usize), SourcePackListKey>,
    table_sequences: HashMap<(usize, usize), TableSequence>,
    initializer_lists: HashMap<SourcePackListKey, InitializerContext>,
    expression_initializers: HashMap<NativeExpressionKey, (SourcePackListKey, usize)>,
}

impl NativePacks {
    pub(super) fn new(
        snapshot: u64,
        file: &File,
        functions: &HashMap<(usize, usize), u32>,
        expression_key: &dyn Fn(&Expr) -> NativeExpressionKey,
    ) -> Self {
        let mut result = Self {
            snapshot,
            lists: Vec::new(),
            list_spans: HashMap::new(),
            table_sequences: HashMap::new(),
            initializer_lists: HashMap::new(),
            expression_initializers: HashMap::new(),
        };
        result.statements(&file.statements, None, functions, expression_key);
        result
    }

    pub(super) fn get(&self, key: SourcePackListKey) -> &ExpressionList {
        assert_eq!(
            key.snapshot, self.snapshot,
            "foreign original source pack list"
        );
        &self.lists[key.index as usize]
    }

    pub(super) fn list_at(&self, span: Span) -> Option<&ExpressionList> {
        self.list_spans
            .get(&(span.start, span.end))
            .map(|key| self.get(*key))
    }

    pub(super) fn table_sequence(&self, span: Span) -> Option<&TableSequence> {
        self.table_sequences.get(&(span.start, span.end))
    }

    pub(super) fn initializer_for_expression(
        &self,
        expression: NativeExpressionKey,
    ) -> Option<(&ExpressionList, &InitializerContext, usize)> {
        let (key, index) = *self.expression_initializers.get(&expression)?;
        Some((self.get(key), self.initializer_lists.get(&key)?, index))
    }

    fn initializer(&mut self, list: SourcePackListKey, context: InitializerContext) {
        for (index, expression) in self.get(list).expressions.clone().iter().enumerate() {
            assert!(
                self.expression_initializers
                    .insert(expression.expression, (list, index))
                    .is_none()
            );
        }
        self.initializer_lists.insert(list, context);
    }

    fn list(
        &mut self,
        values: &[Expr],
        context: ListContext,
        span: Span,
        owner: Option<NativeFunctionKey>,
        expression_key: &dyn Fn(&Expr) -> NativeExpressionKey,
    ) -> SourcePackListKey {
        let key = SourcePackListKey {
            snapshot: self.snapshot,
            index: self.lists.len() as u32,
        };
        let expressions = values
            .iter()
            .enumerate()
            .map(|(index, value)| PackExpression {
                expression: expression_key(value),
                adjustment: adjustment(value, index + 1 == values.len()),
                span: value.span,
            })
            .collect();
        self.lists.push(ExpressionList {
            key,
            owner,
            context,
            expressions,
            span,
        });
        assert!(
            self.list_spans
                .insert((span.start, span.end), key)
                .is_none(),
            "source list has a unique enclosing statement or call"
        );
        key
    }

    fn function(
        &mut self,
        function: &Function,
        functions: &HashMap<(usize, usize), u32>,
        expression_key: &dyn Fn(&Expr) -> NativeExpressionKey,
    ) {
        let key = NativeFunctionKey {
            snapshot: self.snapshot,
            index: functions[&(function.span.start, function.span.end)],
        };
        self.statements(&function.body, Some(key), functions, expression_key);
    }

    fn statements(
        &mut self,
        statements: &[Stmt],
        owner: Option<NativeFunctionKey>,
        functions: &HashMap<(usize, usize), u32>,
        expression_key: &dyn Fn(&Expr) -> NativeExpressionKey,
    ) {
        for statement in statements {
            match &statement.kind {
                StmtKind::Local {
                    names,
                    values,
                    annotation,
                } => {
                    let list = self.list(
                        values,
                        ListContext::Local {
                            destinations: names.iter().map(|name| name.span).collect(),
                        },
                        statement.span,
                        owner,
                        expression_key,
                    );
                    if let Some(annotation) = annotation {
                        self.initializer(list, InitializerContext::Local(annotation.clone()));
                    } else {
                        self.initializer(list, InitializerContext::LocalDeclarations(names.clone()));
                    }
                    for value in values {
                        self.expression(value, owner, functions, expression_key);
                    }
                }
                StmtKind::Assign {
                    targets, values, ..
                } => {
                    let list = self.list(
                        values,
                        ListContext::Assignment {
                            destinations: targets.iter().map(|target| target.span).collect(),
                        },
                        statement.span,
                        owner,
                        expression_key,
                    );
                    self.initializer(list, InitializerContext::Assignment(targets.clone()));
                    for target in targets {
                        self.expression(target, owner, functions, expression_key);
                    }
                    for value in values {
                        self.expression(value, owner, functions, expression_key);
                    }
                }
                StmtKind::Return(values) => {
                    self.list(
                        values,
                        ListContext::Return,
                        statement.span,
                        owner,
                        expression_key,
                    );
                    for value in values {
                        self.expression(value, owner, functions, expression_key);
                    }
                }
                StmtKind::Call(expression) => {
                    self.expression(expression, owner, functions, expression_key)
                }
                StmtKind::LocalFunction { function, .. } => {
                    self.function(function, functions, expression_key)
                }
                StmtKind::FunctionDeclaration {
                    target, function, ..
                } => {
                    self.expression(target, owner, functions, expression_key);
                    self.function(function, functions, expression_key);
                }
                StmtKind::Block { body, .. } => {
                    self.statements(body, owner, functions, expression_key)
                }
                StmtKind::If {
                    branches,
                    else_branch,
                } => {
                    for branch in branches {
                        self.expression(&branch.condition, owner, functions, expression_key);
                        self.statements(&branch.body, owner, functions, expression_key);
                    }
                    if let Some(branch) = else_branch {
                        self.statements(&branch.body, owner, functions, expression_key);
                    }
                }
                StmtKind::While {
                    condition, body, ..
                }
                | StmtKind::Repeat {
                    condition, body, ..
                } => {
                    self.expression(condition, owner, functions, expression_key);
                    self.statements(body, owner, functions, expression_key);
                }
                StmtKind::NumericFor {
                    start,
                    limit,
                    step,
                    body,
                    ..
                } => {
                    self.expression(start, owner, functions, expression_key);
                    self.expression(limit, owner, functions, expression_key);
                    if let Some(step) = step {
                        self.expression(step, owner, functions, expression_key);
                    }
                    self.statements(body, owner, functions, expression_key);
                }
                StmtKind::GenericFor { values, body, .. } => {
                    self.list(
                        values,
                        ListContext::Iterator,
                        statement.span,
                        owner,
                        expression_key,
                    );
                    for value in values {
                        self.expression(value, owner, functions, expression_key);
                    }
                    self.statements(body, owner, functions, expression_key);
                }
                StmtKind::Break => {}
            }
        }
    }

    fn expression(
        &mut self,
        expression: &Expr,
        owner: Option<NativeFunctionKey>,
        functions: &HashMap<(usize, usize), u32>,
        expression_key: &dyn Fn(&Expr) -> NativeExpressionKey,
    ) {
        match &expression.kind {
            ExprKind::Call { callee, args } => {
                self.list(
                    args,
                    ListContext::Arguments {
                        implicit_receiver: false,
                    },
                    expression.span,
                    owner,
                    expression_key,
                );
                self.expression(callee, owner, functions, expression_key);
                for argument in args {
                    self.expression(argument, owner, functions, expression_key);
                }
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                self.list(
                    args,
                    ListContext::Arguments {
                        implicit_receiver: true,
                    },
                    expression.span,
                    owner,
                    expression_key,
                );
                self.expression(receiver, owner, functions, expression_key);
                for argument in args {
                    self.expression(argument, owner, functions, expression_key);
                }
            }
            ExprKind::Function(function) => self.function(function, functions, expression_key),
            ExprKind::Parenthesized(value)
            | ExprKind::Unary { value, .. }
            | ExprKind::Field { base: value, .. } => {
                self.expression(value, owner, functions, expression_key)
            }
            ExprKind::Binary { left, right, .. }
            | ExprKind::Index {
                base: left,
                index: right,
            } => {
                self.expression(left, owner, functions, expression_key);
                self.expression(right, owner, functions, expression_key);
            }
            ExprKind::Table(fields) => {
                let mut elements = Vec::new();
                for (field_index, field) in fields.iter().enumerate() {
                    if let TableFieldKind::Keyed(key) = &field.kind {
                        self.expression(key, owner, functions, expression_key);
                    }
                    if let TableFieldKind::Sequence { index } = &field.kind {
                        elements.push(SequenceElement {
                            index: *index,
                            expression: PackExpression {
                                expression: expression_key(&field.value),
                                adjustment: adjustment(
                                    &field.value,
                                    field_index + 1 == fields.len(),
                                ),
                                span: field.value.span,
                            },
                            span: field.span,
                        });
                    }
                    self.expression(&field.value, owner, functions, expression_key);
                }
                self.table_sequences.insert(
                    (expression.span.start, expression.span.end),
                    TableSequence {
                        owner,
                        elements,
                        span: expression.span,
                    },
                );
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
}

pub(super) fn adjustment(expression: &Expr, final_position: bool) -> Adjustment {
    if final_position
        && matches!(
            expression.kind,
            ExprKind::Call { .. } | ExprKind::MethodCall { .. } | ExprKind::Vararg
        )
    {
        Adjustment::Expand
    } else {
        Adjustment::Scalar
    }
}
