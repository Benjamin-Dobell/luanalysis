//! Pure original-source control-flow recipes. This module constructs no types,
//! joins no types and solves no loops. Those queries belong to the native core.

use crate::bind::{BindingId, BoundFile, ScopeId};
use luanalysis_syntax::{
    BinaryOperator, Expr, ExprKind, File, Function, Name, Span, Stmt, StmtKind, TableFieldKind,
    UnaryOperator,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SNAPSHOT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceExpressionId {
    pub snapshot: u64,
    pub index: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FlowId {
    pub snapshot: u64,
    pub graph: u32,
    pub index: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EvaluationId {
    pub snapshot: u64,
    pub index: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AssignmentId {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ReferenceRoot {
    Binding(BindingId),
    /// The immutable physical result list shared by a local binding group.
    Pack(EvaluationId),
    Global(String),
    Temporary(SourceExpressionId),
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    String(Vec<u8>),
    Boolean(bool),
    Integer(i64),
    Float(u64),
    Dynamic(SourceExpressionId),
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Reference {
    pub root: ReferenceRoot,
    pub path: Vec<Key>,
}

#[derive(Clone, Debug)]
pub struct SourceExpression {
    pub id: SourceExpressionId,
    pub expression: Expr,
    pub scope: Option<ScopeId>,
}
#[derive(Clone, Debug)]
pub struct Evaluation {
    pub id: EvaluationId,
    pub expression: SourceExpressionId,
    /// Source state before this expression, not after later assignment stores.
    pub input: FlowId,
    pub completed: FlowId,
    pub operands: Vec<EvaluationId>,
}
#[derive(Clone, Debug)]
pub struct CapturedAddress {
    pub reference: Reference,
    pub receiver: Option<EvaluationId>,
    pub key: Option<EvaluationId>,
    /// All addresses and RHS values in a transaction are frozen before its stores.
    pub transaction: AssignmentId,
    pub epoch: FlowId,
    pub span: Span,
}
#[derive(Clone, Debug)]
pub enum ValueDemand {
    Evaluation(EvaluationId),
    /// A projection of one original expanding RHS evaluation. Sibling writes
    /// retain the same evaluation identity instead of independent scalar lists.
    PackElement {
        evaluation: EvaluationId,
        index: usize,
    },
    Nil,
    Function(u32),
    NumericIteration,
    IteratorResult(usize),
}
#[derive(Clone, Debug)]
pub enum Predicate {
    Expression(SourceExpressionId),
    NumericIteration {
        start: EvaluationId,
        limit: EvaluationId,
        step: Option<EvaluationId>,
        stage: NumericIterationStage,
    },
    GenericIteration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericIterationStage {
    First,
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumericLiteral {
    Integer(i64),
    Float(f64),
}

/// Original lexical literal data only. Comparison/iteration decisions belong
/// to the native worker; large Lua integers must not round through f64 first.
pub fn numeric_literal(expression: &Expr) -> Option<NumericLiteral> {
    match &expression.kind {
        ExprKind::Number(spelling) => {
            if let Some(hex) = spelling
                .strip_prefix("0x")
                .or_else(|| spelling.strip_prefix("0X"))
                && !hex.contains(['.', 'p', 'P'])
            {
                return u64::from_str_radix(hex, 16)
                    .ok()
                    .map(|value| NumericLiteral::Integer(value as i64));
            }
            spelling
                .parse::<i64>()
                .ok()
                .map(NumericLiteral::Integer)
                .or_else(|| crate::number(spelling).map(NumericLiteral::Float))
        }
        ExprKind::Parenthesized(value) => numeric_literal(value),
        ExprKind::Unary {
            op: UnaryOperator::Negate,
            value,
            ..
        } => numeric_literal(value).map(|value| match value {
            NumericLiteral::Integer(value) => NumericLiteral::Integer(value.wrapping_neg()),
            NumericLiteral::Float(value) => NumericLiteral::Float(-value),
        }),
        _ => None,
    }
}
#[derive(Clone, Debug)]
pub enum NodeKind {
    Start,
    Eval(EvaluationId),
    CreateFunction {
        expression: SourceExpressionId,
        graph: u32,
    },
    Write {
        address: CapturedAddress,
        value: ValueDemand,
        store_order: u32,
        declaration: bool,
    },
    Predicate {
        predicate: Predicate,
        sense: bool,
    },
    Merge,
    /// Predecessor zero is the entry; remaining predecessors are back edges.
    Loop,
    Call {
        expression: SourceExpressionId,
        evaluation: EvaluationId,
    },
    Return {
        values: Vec<EvaluationId>,
    },
    Break,
    Unreachable,
}
#[derive(Clone, Debug)]
pub struct Node {
    pub id: FlowId,
    pub kind: NodeKind,
    pub antecedents: Vec<FlowId>,
    pub span: Span,
    pub scope: Option<ScopeId>,
}
#[derive(Clone, Debug)]
pub struct FunctionGraph {
    pub index: u32,
    pub function_span: Option<Span>,
    pub entry: FlowId,
    pub exit: FlowId,
    pub nodes: Vec<Node>,
    pub returns: Vec<FlowId>,
    pub breaks: Vec<FlowId>,
    pub shared: Vec<bool>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingFeature {
    ExpandedArguments,
    NumericIteration,
    GenericIteration,
    DynamicAddress,
    InvalidSource,
}
#[derive(Clone, Debug)]
pub struct Feature {
    pub kind: PendingFeature,
    pub span: Span,
}
#[derive(Clone, Debug)]
pub struct ProgramFlow {
    pub snapshot: u64,
    pub graphs: Vec<FunctionGraph>,
    pub expressions: Vec<SourceExpression>,
    pub evaluations: Vec<Evaluation>,
    pub pending: Vec<Feature>,
    /// Original function source spans select separate graphs, never inline execution.
    pub functions: HashMap<(usize, usize), u32>,
    /// Canonical bound references for original read expressions and predicates.
    /// Temporary roots/dynamic keys remain explicit; the backend decides keyability.
    pub references: HashMap<SourceExpressionId, Reference>,
}
impl ProgramFlow {
    pub fn node(&self, id: FlowId) -> &Node {
        assert_eq!(id.snapshot, self.snapshot, "foreign source graph snapshot");
        &self.graphs[id.graph as usize].nodes[id.index as usize]
    }
    pub fn is_shared(&self, id: FlowId) -> bool {
        assert_eq!(id.snapshot, self.snapshot, "foreign source graph snapshot");
        self.graphs[id.graph as usize].shared[id.index as usize]
    }
    pub fn evaluation(&self, id: EvaluationId) -> &Evaluation {
        assert_eq!(
            id.snapshot, self.snapshot,
            "foreign source evaluation snapshot"
        );
        &self.evaluations[id.index as usize]
    }
    pub fn expression(&self, id: SourceExpressionId) -> &SourceExpression {
        assert_eq!(
            id.snapshot, self.snapshot,
            "foreign source expression snapshot"
        );
        &self.expressions[id.index as usize]
    }
}

/// Builds original-source recipes. Parse diagnostics are retained as explicit
/// invalid-source metadata; callers must not type-check a recovered invalid AST.
pub fn build(file: &File, bound: &BoundFile) -> ProgramFlow {
    let snapshot = NEXT_SNAPSHOT.fetch_add(1, Ordering::Relaxed);
    assert_ne!(snapshot, u64::MAX, "source graph snapshots exhausted");
    let mut builder = Builder {
        program: ProgramFlow {
            snapshot,
            graphs: Vec::new(),
            expressions: Vec::new(),
            evaluations: Vec::new(),
            pending: Vec::new(),
            functions: HashMap::new(),
            references: HashMap::new(),
        },
        bound,
        expression_ids: HashMap::new(),
        assignment: 0,
    };
    if !file.diagnostics.is_empty() {
        builder.feature(PendingFeature::InvalidSource, file.span);
    }
    let graph = builder.new_graph(None, file.span);
    if !file.diagnostics.is_empty() {
        builder.program.graphs[graph as usize].shared = vec![false];
        return builder.program;
    }
    let mut context = Context {
        graph,
        current: builder.program.graphs[graph as usize].entry,
        breaks: Vec::new(),
        live: true,
    };
    builder.statements(&mut context, &file.statements);
    builder.program.graphs[graph as usize].exit = context.current;
    for graph in &mut builder.program.graphs {
        let mut uses = vec![0u32; graph.nodes.len()];
        for node in &graph.nodes {
            for before in &node.antecedents {
                uses[before.index as usize] += 1;
            }
        }
        graph.shared = uses.into_iter().map(|count| count > 1).collect();
    }
    builder.program
}
struct Context {
    graph: u32,
    current: FlowId,
    breaks: Vec<Vec<FlowId>>,
    live: bool,
}
struct Builder<'a> {
    program: ProgramFlow,
    bound: &'a BoundFile,
    expression_ids: HashMap<(usize, usize), SourceExpressionId>,
    assignment: u32,
}
impl Builder<'_> {
    fn scope(&self, span: Span) -> Option<ScopeId> {
        self.bound.scope_at(span.start)
    }
    fn feature(&mut self, kind: PendingFeature, span: Span) {
        self.program.pending.push(Feature { kind, span });
    }
    fn new_graph(&mut self, function_span: Option<Span>, span: Span) -> u32 {
        let graph = self.program.graphs.len() as u32;
        let entry = FlowId {
            snapshot: self.program.snapshot,
            graph,
            index: 0,
        };
        self.program.graphs.push(FunctionGraph {
            index: graph,
            function_span,
            entry,
            exit: entry,
            nodes: vec![Node {
                id: entry,
                kind: NodeKind::Start,
                antecedents: Vec::new(),
                span,
                scope: self.scope(span),
            }],
            returns: Vec::new(),
            breaks: Vec::new(),
            shared: Vec::new(),
        });
        graph
    }
    fn node(
        &mut self,
        context: &mut Context,
        kind: NodeKind,
        antecedents: Vec<FlowId>,
        span: Span,
    ) -> FlowId {
        let id = FlowId {
            snapshot: self.program.snapshot,
            graph: context.graph,
            index: self.program.graphs[context.graph as usize].nodes.len() as u32,
        };
        let scope = self.scope(span);
        self.program.graphs[context.graph as usize]
            .nodes
            .push(Node {
                id,
                kind,
                antecedents,
                span,
                scope,
            });
        context.current = id;
        id
    }
    fn append(&mut self, context: &mut Context, kind: NodeKind, span: Span) -> FlowId {
        self.node(context, kind, vec![context.current], span)
    }
    fn expr_id(&mut self, expr: &Expr) -> SourceExpressionId {
        let key = (expr.span.start, expr.span.end);
        if let Some(id) = self.expression_ids.get(&key) {
            return *id;
        }
        let id = SourceExpressionId {
            snapshot: self.program.snapshot,
            index: self.program.expressions.len() as u32,
        };
        self.program.expressions.push(SourceExpression {
            id,
            expression: expr.clone(),
            scope: self.scope(expr.span),
        });
        self.expression_ids.insert(key, id);
        id
    }
    fn function(&mut self, function: &Function) -> u32 {
        let key = (function.span.start, function.span.end);
        if let Some(graph) = self.program.functions.get(&key) {
            return *graph;
        }
        let graph = self.new_graph(Some(function.span), function.body_span);
        self.program.functions.insert(key, graph);
        let mut context = Context {
            graph,
            current: self.program.graphs[graph as usize].entry,
            breaks: Vec::new(),
            live: true,
        };
        self.statements(&mut context, &function.body);
        self.program.graphs[graph as usize].exit = context.current;
        graph
    }
    fn evaluate(&mut self, context: &mut Context, expr: &Expr) -> EvaluationId {
        let expression = self.expr_id(expr);
        if matches!(
            expr.kind,
            ExprKind::Name(_)
                | ExprKind::Field { .. }
                | ExprKind::Index { .. }
                | ExprKind::Parenthesized(_)
        ) {
            let reference = self.reference(expr);
            if reference
                .path
                .iter()
                .any(|key| matches!(key, Key::Dynamic(_)))
                || (!reference.path.is_empty()
                    && matches!(reference.root, ReferenceRoot::Temporary(_)))
            {
                self.feature(PendingFeature::DynamicAddress, expr.span);
            }
            self.program.references.insert(expression, reference);
        }
        let input = context.current;
        let mut operands = Vec::new();
        match &expr.kind {
            ExprKind::Parenthesized(inner) | ExprKind::Unary { value: inner, .. } => {
                operands.push(self.evaluate(context, inner))
            }
            ExprKind::Field { base, .. } => operands.push(self.evaluate(context, base)),
            ExprKind::Index { base, index } => {
                operands.push(self.evaluate(context, base));
                operands.push(self.evaluate(context, index));
            }
            ExprKind::Table(fields) => {
                for field in fields {
                    if let TableFieldKind::Keyed(key) = &field.kind {
                        operands.push(self.evaluate(context, key));
                    }
                    operands.push(self.evaluate(context, &field.value));
                }
            }
            ExprKind::Binary {
                op, left, right, ..
            } if matches!(op, BinaryOperator::And | BinaryOperator::Or) => {
                operands.push(self.evaluate(context, left));
                let pre = context.current;
                let predicate = Predicate::Expression(self.expr_id(left));
                let right_sense = *op == BinaryOperator::And;
                self.node(
                    context,
                    NodeKind::Predicate {
                        predicate: predicate.clone(),
                        sense: right_sense,
                    },
                    vec![pre],
                    left.span,
                );
                operands.push(self.evaluate(context, right));
                let right_exit = context.current;
                let bypass = self.node(
                    context,
                    NodeKind::Predicate {
                        predicate,
                        sense: !right_sense,
                    },
                    vec![pre],
                    left.span,
                );
                self.node(
                    context,
                    NodeKind::Merge,
                    vec![right_exit, bypass],
                    expr.span,
                );
            }
            ExprKind::Binary { left, right, .. } => {
                operands.push(self.evaluate(context, left));
                operands.push(self.evaluate(context, right));
            }
            ExprKind::Call { callee, args } => {
                self.expanded_arguments(args);
                operands.push(self.evaluate(context, callee));
                for arg in args {
                    operands.push(self.evaluate(context, arg));
                }
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                self.expanded_arguments(args);
                operands.push(self.evaluate(context, receiver));
                for arg in args {
                    operands.push(self.evaluate(context, arg));
                }
            }
            ExprKind::Function(function) => {
                let graph = self.function(function);
                self.append(
                    context,
                    NodeKind::CreateFunction { expression, graph },
                    expr.span,
                );
            }
            ExprKind::Vararg
            | ExprKind::Nil
            | ExprKind::Boolean(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Name(_)
            | ExprKind::Error => {}
        }
        let id = EvaluationId {
            snapshot: self.program.snapshot,
            index: self.program.evaluations.len() as u32,
        };
        let completed = self.append(
            context,
            if matches!(
                expr.kind,
                ExprKind::Call { .. } | ExprKind::MethodCall { .. }
            ) {
                NodeKind::Call {
                    expression,
                    evaluation: id,
                }
            } else {
                NodeKind::Eval(id)
            },
            expr.span,
        );
        self.program.evaluations.push(Evaluation {
            id,
            expression,
            input,
            completed,
            operands,
        });
        id
    }
    fn reference(&mut self, expr: &Expr) -> Reference {
        match &expr.kind {
            ExprKind::Name(name) => Reference {
                root: self.bound.binding_at(name.span.start).map_or_else(
                    || ReferenceRoot::Global(name.text.clone()),
                    ReferenceRoot::Binding,
                ),
                path: Vec::new(),
            },
            ExprKind::Parenthesized(inner) => self.reference(inner),
            ExprKind::Field { base, name } => {
                let mut reference = self.reference(base);
                reference
                    .path
                    .push(Key::String(name.text.as_bytes().to_vec()));
                reference
            }
            ExprKind::Index { base, index } => {
                let mut reference = self.reference(base);
                let key = match crate::native_tables::literal_key(index) {
                    Some(luanalysis_engine::LuaKey::String(bytes)) => Key::String(bytes),
                    Some(luanalysis_engine::LuaKey::Boolean(value)) => Key::Boolean(value),
                    Some(luanalysis_engine::LuaKey::Integer(value)) => Key::Integer(value),
                    Some(luanalysis_engine::LuaKey::Number(value))
                        if value >= i64::MIN as f64
                            && value < -(i64::MIN as f64)
                            && value.fract() == 0.0 =>
                    {
                        Key::Integer(value as i64)
                    }
                    Some(luanalysis_engine::LuaKey::Number(value)) => Key::Float(value.to_bits()),
                    None => Key::Dynamic(self.expr_id(index)),
                };
                reference.path.push(key);
                reference
            }
            _ => Reference {
                root: ReferenceRoot::Temporary(self.expr_id(expr)),
                path: Vec::new(),
            },
        }
    }
    fn address(
        &mut self,
        context: &mut Context,
        target: &Expr,
        transaction: AssignmentId,
        epoch: FlowId,
    ) -> CapturedAddress {
        let reference = self.reference(target);
        let (receiver, key) = match &target.kind {
            ExprKind::Field { base, .. } => (Some(self.evaluate(context, base)), None),
            ExprKind::Index { base, index } => {
                if matches!(reference.path.last(), Some(Key::Dynamic(_))) {
                    self.feature(PendingFeature::DynamicAddress, target.span);
                }
                (
                    Some(self.evaluate(context, base)),
                    Some(self.evaluate(context, index)),
                )
            }
            _ => (None, None),
        };
        CapturedAddress {
            reference,
            receiver,
            key,
            transaction,
            epoch,
            span: target.span,
        }
    }
    fn transaction(&mut self) -> AssignmentId {
        let id = AssignmentId {
            snapshot: self.program.snapshot,
            index: self.assignment,
        };
        self.assignment += 1;
        id
    }
    fn name_address(
        &self,
        name: &Name,
        transaction: AssignmentId,
        epoch: FlowId,
    ) -> CapturedAddress {
        CapturedAddress {
            reference: Reference {
                root: self.bound.binding_at(name.span.start).map_or_else(
                    || ReferenceRoot::Global(name.text.clone()),
                    ReferenceRoot::Binding,
                ),
                path: Vec::new(),
            },
            receiver: None,
            key: None,
            transaction,
            epoch,
            span: name.span,
        }
    }
    fn expanded_arguments(&mut self, values: &[Expr]) {
        if let Some(last) = values.last() {
            if matches!(
                last.kind,
                ExprKind::Call { .. } | ExprKind::MethodCall { .. } | ExprKind::Vararg
            ) {
                self.feature(PendingFeature::ExpandedArguments, last.span);
            }
        }
    }

    fn adjusted_value(&self, values: &[EvaluationId], index: usize) -> ValueDemand {
        if let Some(&last) = values.last() {
            let expression = &self
                .program
                .expression(self.program.evaluation(last).expression)
                .expression;
            let final_index = values.len() - 1;
            if index >= final_index
                && matches!(
                    expression.kind,
                    ExprKind::Call { .. } | ExprKind::MethodCall { .. } | ExprKind::Vararg
                )
            {
                return ValueDemand::PackElement {
                    evaluation: last,
                    index: index - final_index,
                };
            }
        }
        values
            .get(index)
            .copied()
            .map_or(ValueDemand::Nil, ValueDemand::Evaluation)
    }

    fn statements(&mut self, context: &mut Context, statements: &[Stmt]) {
        for statement in statements {
            self.statement(context, statement);
        }
    }
    fn statement(&mut self, context: &mut Context, statement: &Stmt) {
        let span = statement.span;
        match &statement.kind {
            StmtKind::Local { names, values, .. } => {
                let transaction = self.transaction();
                let epoch = context.current;
                let values: Vec<_> = values
                    .iter()
                    .map(|value| self.evaluate(context, value))
                    .collect();
                for (index, name) in names.iter().enumerate() {
                    let address = self.name_address(name, transaction, epoch);
                    let value = self.adjusted_value(&values, index);
                    self.append(
                        context,
                        NodeKind::Write {
                            address,
                            value,
                            store_order: index as u32,
                            declaration: true,
                        },
                        name.span,
                    );
                }
            }
            StmtKind::Assign {
                targets, values, ..
            } => {
                let transaction = self.transaction();
                let epoch = context.current;
                let addresses: Vec<_> = targets
                    .iter()
                    .map(|target| self.address(context, target, transaction, epoch))
                    .collect();
                let values: Vec<_> = values
                    .iter()
                    .map(|value| self.evaluate(context, value))
                    .collect();
                for (order, index) in (0..addresses.len()).rev().enumerate() {
                    let value = self.adjusted_value(&values, index);
                    self.append(
                        context,
                        NodeKind::Write {
                            address: addresses[index].clone(),
                            value,
                            store_order: order as u32,
                            declaration: false,
                        },
                        targets[index].span,
                    );
                }
            }
            StmtKind::LocalFunction { name, function } => {
                let graph = self.function(function);
                let transaction = self.transaction();
                let epoch = context.current;
                let address = self.name_address(name, transaction, epoch);
                self.append(
                    context,
                    NodeKind::Write {
                        address,
                        value: ValueDemand::Function(graph),
                        store_order: 0,
                        declaration: true,
                    },
                    name.span,
                );
            }
            StmtKind::FunctionDeclaration {
                target, function, ..
            } => {
                let transaction = self.transaction();
                let epoch = context.current;
                let address = self.address(context, target, transaction, epoch);
                let graph = self.function(function);
                self.append(
                    context,
                    NodeKind::Write {
                        address,
                        value: ValueDemand::Function(graph),
                        store_order: 0,
                        declaration: false,
                    },
                    target.span,
                );
            }
            StmtKind::Block { body, .. } => self.statements(context, body),
            StmtKind::Call(expr) => {
                self.evaluate(context, expr);
            }
            StmtKind::Return(values) => {
                let values = values
                    .iter()
                    .map(|expr| self.evaluate(context, expr))
                    .collect();
                let node = self.append(context, NodeKind::Return { values }, span);
                if context.live {
                    self.program.graphs[context.graph as usize]
                        .returns
                        .push(node);
                }
                context.live = false;
                self.node(context, NodeKind::Unreachable, Vec::new(), span);
            }
            StmtKind::Break => {
                let node = self.append(context, NodeKind::Break, span);
                if context.live {
                    self.program.graphs[context.graph as usize]
                        .breaks
                        .push(node);
                }
                if let Some(exits) = context.breaks.last_mut() {
                    if context.live {
                        exits.push(node);
                    }
                } else {
                    self.feature(PendingFeature::InvalidSource, span);
                }
                context.live = false;
                self.node(context, NodeKind::Unreachable, Vec::new(), span);
            }
            StmtKind::If {
                branches,
                else_branch,
            } => {
                let mut remaining = context.current;
                let mut exits = Vec::new();
                let incoming_live = context.live;
                for branch in branches {
                    context.current = remaining;
                    context.live = incoming_live;
                    self.evaluate(context, &branch.condition);
                    let pre = context.current;
                    let predicate = Predicate::Expression(self.expr_id(&branch.condition));
                    self.node(
                        context,
                        NodeKind::Predicate {
                            predicate: predicate.clone(),
                            sense: true,
                        },
                        vec![pre],
                        branch.condition.span,
                    );
                    self.statements(context, &branch.body);
                    if context.live {
                        exits.push(context.current);
                    }
                    remaining = self.node(
                        context,
                        NodeKind::Predicate {
                            predicate,
                            sense: false,
                        },
                        vec![pre],
                        branch.condition.span,
                    );
                }
                context.current = remaining;
                context.live = incoming_live;
                if let Some(branch) = else_branch {
                    self.statements(context, &branch.body);
                }
                if context.live {
                    exits.push(context.current);
                }
                context.live = !exits.is_empty();
                self.node(
                    context,
                    if context.live {
                        NodeKind::Merge
                    } else {
                        NodeKind::Unreachable
                    },
                    exits,
                    span,
                );
            }
            StmtKind::While {
                condition, body, ..
            } => {
                let incoming_live = context.live;
                let header = self.append(context, NodeKind::Loop, span);
                self.evaluate(context, condition);
                let pre = context.current;
                let predicate = Predicate::Expression(self.expr_id(condition));
                self.node(
                    context,
                    NodeKind::Predicate {
                        predicate: predicate.clone(),
                        sense: true,
                    },
                    vec![pre],
                    condition.span,
                );
                context.breaks.push(Vec::new());
                self.statements(context, body);
                let back = context.current;
                if context.live {
                    self.program.graphs[context.graph as usize].nodes[header.index as usize]
                        .antecedents
                        .push(back);
                }
                let mut exits = context.breaks.pop().unwrap();
                context.live = incoming_live;
                let exit = self.node(
                    context,
                    NodeKind::Predicate {
                        predicate,
                        sense: false,
                    },
                    vec![pre],
                    condition.span,
                );
                if context.live {
                    exits.push(exit);
                }
                self.node(context, NodeKind::Merge, exits, span);
            }
            StmtKind::Repeat {
                body, condition, ..
            } => {
                let header = self.append(context, NodeKind::Loop, span);
                context.breaks.push(Vec::new());
                self.statements(context, body);
                self.evaluate(context, condition);
                let pre = context.current;
                let predicate = Predicate::Expression(self.expr_id(condition));
                let back = self.node(
                    context,
                    NodeKind::Predicate {
                        predicate: predicate.clone(),
                        sense: false,
                    },
                    vec![pre],
                    condition.span,
                );
                if context.live {
                    self.program.graphs[context.graph as usize].nodes[header.index as usize]
                        .antecedents
                        .push(back);
                }
                let mut exits = context.breaks.pop().unwrap();
                let exit = self.node(
                    context,
                    NodeKind::Predicate {
                        predicate,
                        sense: true,
                    },
                    vec![pre],
                    condition.span,
                );
                if context.live {
                    exits.push(exit);
                }
                context.live = !exits.is_empty();
                self.node(context, NodeKind::Merge, exits, span);
            }
            StmtKind::NumericFor {
                name,
                start,
                limit,
                step,
                body,
                ..
            } => {
                let start = self.evaluate(context, start);
                let limit = self.evaluate(context, limit);
                let step = step.as_ref().map(|step| self.evaluate(context, step));
                self.numeric_iteration(context, name, start, limit, step, body, span);
            }
            StmtKind::GenericFor {
                names,
                values,
                body,
                ..
            } => {
                self.feature(PendingFeature::GenericIteration, span);
                for value in values {
                    self.evaluate(context, value);
                }
                self.iteration(
                    context,
                    Predicate::GenericIteration,
                    names,
                    body,
                    span,
                    false,
                );
            }
        }
    }
    fn numeric_iteration(
        &mut self,
        context: &mut Context,
        name: &Name,
        start: EvaluationId,
        limit: EvaluationId,
        step: Option<EvaluationId>,
        body: &[Stmt],
        span: Span,
    ) {
        let incoming_live = context.live;
        let before = context.current;
        let predicate = |stage| Predicate::NumericIteration {
            start,
            limit,
            step,
            stage,
        };
        let first_exit = self.node(
            context,
            NodeKind::Predicate {
                predicate: predicate(NumericIterationStage::First),
                sense: false,
            },
            vec![before],
            span,
        );
        self.node(
            context,
            NodeKind::Predicate {
                predicate: predicate(NumericIterationStage::First),
                sense: true,
            },
            vec![before],
            span,
        );
        let header = self.append(context, NodeKind::Loop, span);
        let transaction = self.transaction();
        let epoch = context.current;
        let address = self.name_address(name, transaction, epoch);
        // This write resets the visible control variable on every body entry.
        // Lua's hidden counter is distinct from body writes to that variable.
        self.append(
            context,
            NodeKind::Write {
                address,
                value: ValueDemand::NumericIteration,
                store_order: 0,
                declaration: true,
            },
            name.span,
        );
        context.breaks.push(Vec::new());
        self.statements(context, body);
        let back = context.current;
        let mut exits = context.breaks.pop().unwrap();
        if incoming_live {
            exits.push(first_exit);
        }
        if context.live {
            let next_exit = self.node(
                context,
                NodeKind::Predicate {
                    predicate: predicate(NumericIterationStage::Next),
                    sense: false,
                },
                vec![back],
                span,
            );
            let next_body = self.node(
                context,
                NodeKind::Predicate {
                    predicate: predicate(NumericIterationStage::Next),
                    sense: true,
                },
                vec![back],
                span,
            );
            self.program.graphs[context.graph as usize].nodes[header.index as usize]
                .antecedents
                .push(next_body);
            exits.push(next_exit);
        }
        context.live = !exits.is_empty();
        self.node(
            context,
            if context.live {
                NodeKind::Merge
            } else {
                NodeKind::Unreachable
            },
            exits,
            span,
        );
    }

    fn iteration(
        &mut self,
        context: &mut Context,
        predicate: Predicate,
        names: &[Name],
        body: &[Stmt],
        span: Span,
        numeric: bool,
    ) {
        let incoming_live = context.live;
        let header = self.append(context, NodeKind::Loop, span);
        let pre = context.current;
        self.node(
            context,
            NodeKind::Predicate {
                predicate: predicate.clone(),
                sense: true,
            },
            vec![pre],
            span,
        );
        let transaction = self.transaction();
        let epoch = context.current;
        for (index, name) in names.iter().enumerate() {
            let address = self.name_address(name, transaction, epoch);
            let value = if numeric {
                ValueDemand::NumericIteration
            } else {
                ValueDemand::IteratorResult(index)
            };
            self.append(
                context,
                NodeKind::Write {
                    address,
                    value,
                    store_order: index as u32,
                    declaration: true,
                },
                name.span,
            );
        }
        context.breaks.push(Vec::new());
        self.statements(context, body);
        let back = context.current;
        if context.live {
            self.program.graphs[context.graph as usize].nodes[header.index as usize]
                .antecedents
                .push(back);
        }
        let mut exits = context.breaks.pop().unwrap();
        context.live = incoming_live;
        let exit = self.node(
            context,
            NodeKind::Predicate {
                predicate,
                sense: false,
            },
            vec![pre],
            span,
        );
        if context.live {
            exits.push(exit);
        }
        self.node(context, NodeKind::Merge, exits, span);
    }
}
