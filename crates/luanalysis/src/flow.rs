//! Pure original-source control-flow recipes. This module constructs no types,
//! joins no types and solves no loops. Those queries belong to the native core.

use crate::bind::{BindingId, BoundFile, ScopeId};
use luanalysis_syntax::{
    BinaryOperator, Expr, ExprKind, File, Function, Name, Span, Stmt, StmtKind, TableFieldKind,
    UnaryOperator,
};
use crate::collections::{HashMap, HashSet};
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IteratorId {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ReferenceRoot {
    Binding(BindingId),
    /// The immutable physical result list shared by a local binding group.
    Pack(EvaluationId),
    /// Lua's hidden iterator/state/control frame; body bindings cannot write it.
    Iterator(IteratorId),
    Global(String),
    /// The real implicit `_ENV` upvalue owned by this Lua chunk.
    EnvironmentCell,
    /// The selected original environment table, independent of its aliases.
    EnvironmentObject,
    /// The standard file loader policy at one actual source continuation.
    LoaderState,
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
    /// Original enclosing statement extent, used by the backend capture policy.
    pub statement_end: usize,
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
    IteratorResult {
        iterator: IteratorId,
        index: usize,
    },
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
    GenericIteration {
        iterator: IteratorId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericIterationStage {
    First,
    Next,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IteratorStage {
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
    IteratorStart {
        iterator: IteratorId,
    },
    Iteration {
        iterator: IteratorId,
        stage: IteratorStage,
    },
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
    pub creation: Option<FunctionCreation>,
    pub entry: FlowId,
    pub exit: FlowId,
    pub nodes: Vec<Node>,
    pub returns: Vec<FlowId>,
    pub breaks: Vec<FlowId>,
    pub shared: Vec<bool>,
}
#[derive(Clone, Copy, Debug)]
pub struct FunctionCreation {
    pub input: FlowId,
    pub position: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingFeature {
    ExpandedArguments,
    NumericIteration,
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
    pub iterators: Vec<IteratorRecipe>,
    pub pending: Vec<Feature>,
    /// Original function source spans select separate graphs, never inline execution.
    pub functions: HashMap<(usize, usize), u32>,
    /// Canonical bound references for original read expressions and predicates.
    /// Temporary roots/dynamic keys remain explicit; the backend decides keyability.
    pub references: HashMap<SourceExpressionId, Reference>,
}
#[derive(Clone, Debug)]
pub struct IteratorRecipe {
    pub id: IteratorId,
    pub span: Span,
    pub input: FlowId,
    pub values: Vec<EvaluationId>,
    /// Successful first/later steps join here before body-visible stores.
    pub step: FlowId,
}
impl ProgramFlow {
    pub fn iterator(&self, id: IteratorId) -> &IteratorRecipe {
        assert_eq!(
            id.snapshot, self.snapshot,
            "foreign source iterator snapshot"
        );
        &self.iterators[id.index as usize]
    }
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

/// Builds original-source recipes. Lua syntax failures retain explicit invalid
/// source metadata. Annotation failures preserve real Lua graphs and error types.
pub fn build(file: &File, bound: &BoundFile) -> ProgramFlow {
    build_with_snapshot(file, bound, allocate_snapshot())
}

pub(crate) fn allocate_snapshot() -> u64 {
    let snapshot = NEXT_SNAPSHOT.fetch_add(1, Ordering::Relaxed);
    assert_ne!(snapshot, u64::MAX, "source graph snapshots exhausted");
    snapshot
}

pub(crate) fn build_with_snapshot(file: &File, bound: &BoundFile, snapshot: u64) -> ProgramFlow {
    let mut builder = Builder {
        program: ProgramFlow {
            snapshot,
            graphs: Vec::new(),
            expressions: Vec::new(),
            evaluations: Vec::new(),
            iterators: Vec::new(),
            pending: Vec::new(),
            functions: HashMap::default(),
            references: HashMap::default(),
        },
        bound,
        expression_ids: HashMap::default(),
        assignment: 0,
    };
    if file.has_lua_syntax_errors() {
        builder.feature(PendingFeature::InvalidSource, file.span);
    }
    let graph = builder.new_graph(None, file.span);
    let mut context = Context {
        graph,
        current: builder.program.graphs[graph as usize].entry,
        breaks: Vec::new(),
        live: true,
        enclosing_statements: Vec::new(),
        labels: Vec::new(),
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
    enclosing_statements: Vec<Span>,
    /// The labels of each enclosing block of this function, innermost last.
    labels: Vec<LabelScope>,
}

/// One block's labels, as `goto` resolves them: a label is visible in its block and the blocks
/// nested in it.
#[derive(Default)]
struct LabelScope {
    /// Labels a later `goto` in the block jumps back to: their node is a loop head.
    backward: HashSet<String>,
    /// The labels defined so far, with their node.
    defined: Vec<(String, FlowId)>,
    /// Forward `goto`s not yet resolved: the label they name and the position they jump from.
    pending: Vec<(String, FlowId)>,
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
            creation: None,
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
    fn function(&mut self, function: &Function, input: FlowId) -> u32 {
        let key = (function.span.start, function.span.end);
        if let Some(graph) = self.program.functions.get(&key) {
            return *graph;
        }
        let graph = self.new_graph(Some(function.span), function.body_span);
        self.program.graphs[graph as usize].creation = Some(FunctionCreation {
            input,
            position: function.span.start,
        });
        self.program.functions.insert(key, graph);
        let mut context = Context {
            graph,
            current: self.program.graphs[graph as usize].entry,
            breaks: Vec::new(),
            live: true,
            enclosing_statements: Vec::new(),
            labels: Vec::new(),
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
                let graph = self.function(function, context.current);
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
            ExprKind::Name(name) => self.name_reference(name),
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
    fn name_reference(&self, name: &Name) -> Reference {
        if let Some(binding) = self.bound.binding_at(name.span.start) {
            return Reference { root: ReferenceRoot::Binding(binding), path: Vec::new() };
        }
        let root = self.bound.environment_binding_at(name.span.start)
            .map_or(ReferenceRoot::EnvironmentCell, ReferenceRoot::Binding);
        let path = if name.text == "_ENV" { Vec::new() }
            else { vec![Key::String(name.text.as_bytes().to_vec())] };
        Reference { root, path }
    }
    fn address(
        &mut self,
        context: &mut Context,
        target: &Expr,
        transaction: AssignmentId,
        epoch: FlowId,
        statement_end: usize,
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
        let statement_end = self.extended_statement_end(context, &reference, statement_end);
        CapturedAddress {
            reference,
            receiver,
            key,
            transaction,
            epoch,
            span: target.span,
            statement_end,
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
        context: &Context,
        name: &Name,
        transaction: AssignmentId,
        epoch: FlowId,
        statement_end: usize,
    ) -> CapturedAddress {
        let reference = self.name_reference(name);
        CapturedAddress {
            statement_end: self.extended_statement_end(context, &reference, statement_end),
            reference,
            receiver: None,
            key: None,
            transaction,
            epoch,
            span: name.span,
        }
    }
    fn extended_statement_end(
        &self,
        context: &Context,
        reference: &Reference,
        statement_end: usize,
    ) -> usize {
        let ReferenceRoot::Binding(binding) = reference.root else {
            return statement_end;
        };
        let declaration = self.bound.declaration(binding).span.start;
        context.enclosing_statements.iter()
            .filter(|span| span.start > declaration)
            .fold(statement_end, |end, span| end.max(span.end))
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
        context.labels.push(LabelScope { backward: backward_labels(statements), ..LabelScope::default() });
        for statement in statements {
            self.statement(context, statement);
        }
        let scope = context.labels.pop().expect("the block's own label scope");
        // A forward `goto` may name a label later in an enclosing block. One that names no
        // visible label is a syntax error the parser reports; its path simply ends.
        if let Some(parent) = context.labels.last_mut() {
            parent.pending.extend(scope.pending);
        }
    }
    fn statement(&mut self, context: &mut Context, statement: &Stmt) {
        let span = statement.span;
        let compound = matches!(statement.kind,
            StmtKind::Block { .. } | StmtKind::If { .. } | StmtKind::While { .. }
            | StmtKind::Repeat { .. } | StmtKind::NumericFor { .. } | StmtKind::GenericFor { .. }
        );
        if compound {
            context.enclosing_statements.push(span);
        }
        match &statement.kind {
            StmtKind::Local { names, values, .. } => {
                let transaction = self.transaction();
                let epoch = context.current;
                let values: Vec<_> = values
                    .iter()
                    .map(|value| self.evaluate(context, value))
                    .collect();
                for (index, name) in names.iter().enumerate() {
                    let address = self.name_address(context, name, transaction, epoch, span.end);
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
                    .map(|target| self.address(context, target, transaction, epoch, span.end))
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
                let graph = self.function(function, context.current);
                let transaction = self.transaction();
                let epoch = context.current;
                let address = self.name_address(context, name, transaction, epoch, span.end);
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
                let address = self.address(context, target, transaction, epoch, span.end);
                let graph = self.function(function, context.current);
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
            StmtKind::Goto(name) => {
                if context.live {
                    let at = context.current;
                    // A label already defined in this or an enclosing block is a loop head this
                    // jump returns to; any other is resolved when its label is reached.
                    let head = context.labels.iter().rev()
                        .find_map(|scope| scope.defined.iter().find(|(label, _)| *label == name.text).map(|(_, node)| *node));
                    if let Some(head) = head {
                        self.program.graphs[context.graph as usize].nodes[head.index as usize].antecedents.push(at);
                    } else if let Some(scope) = context.labels.last_mut() {
                        scope.pending.push((name.text.clone(), at));
                    }
                }
                context.live = false;
                self.node(context, NodeKind::Unreachable, Vec::new(), span);
            }
            StmtKind::Label(name) => {
                let scope = context.labels.last_mut().expect("a statement belongs to a block");
                let mut entries: Vec<FlowId> = context.live.then_some(context.current).into_iter().collect();
                let pending = std::mem::take(&mut scope.pending);
                let (jumps, others): (Vec<_>, Vec<_>) = pending.into_iter().partition(|(label, _)| *label == name.text);
                scope.pending = others;
                entries.extend(jumps.into_iter().map(|(_, at)| at));
                let backward = scope.backward.contains(&name.text);
                context.live = !entries.is_empty() || backward;
                let entry = match entries.as_slice() {
                    [] => self.node(context, NodeKind::Unreachable, Vec::new(), span),
                    [only] if backward => *only,
                    _ => self.node(context, NodeKind::Merge, entries, span),
                };
                let node = if backward { self.node(context, NodeKind::Loop, vec![entry], span) } else { entry };
                context.labels.last_mut().expect("a statement belongs to a block").defined.push((name.text.clone(), node));
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
                let input = context.current;
                let values = values
                    .iter()
                    .map(|value| self.evaluate(context, value))
                    .collect();
                let iterator = IteratorId {
                    snapshot: self.program.snapshot,
                    index: self.program.iterators.len() as u32,
                };
                self.program.iterators.push(IteratorRecipe {
                    id: iterator,
                    span,
                    input,
                    values,
                    step: context.current,
                });
                self.generic_iteration(context, iterator, names, body, span);
            }
        }
        if compound {
            context.enclosing_statements.pop();
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
        let address = self.name_address(context, name, transaction, epoch, span.end);
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

    fn generic_iteration(
        &mut self,
        context: &mut Context,
        iterator: IteratorId,
        names: &[Name],
        body: &[Stmt],
        span: Span,
    ) {
        let incoming_live = context.live;
        self.append(context, NodeKind::IteratorStart { iterator }, span);
        let first = self.append(
            context,
            NodeKind::Iteration {
                iterator,
                stage: IteratorStage::First,
            },
            span,
        );
        let predicate = Predicate::GenericIteration { iterator };
        let first_exit = self.node(
            context,
            NodeKind::Predicate {
                predicate: predicate.clone(),
                sense: false,
            },
            vec![first],
            span,
        );
        self.node(
            context,
            NodeKind::Predicate {
                predicate: predicate.clone(),
                sense: true,
            },
            vec![first],
            span,
        );
        let header = self.append(context, NodeKind::Loop, span);
        self.program.iterators[iterator.index as usize].step = header;
        let transaction = self.transaction();
        let epoch = context.current;
        for (index, name) in names.iter().enumerate() {
            let address = self.name_address(context, name, transaction, epoch, span.end);
            let value = ValueDemand::IteratorResult { iterator, index };
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
        let mut exits = context.breaks.pop().unwrap();
        if incoming_live {
            exits.push(first_exit);
        }
        if context.live {
            let next = self.node(
                context,
                NodeKind::Iteration {
                    iterator,
                    stage: IteratorStage::Next,
                },
                vec![back],
                span,
            );
            let next_exit = self.node(
                context,
                NodeKind::Predicate {
                    predicate: predicate.clone(),
                    sense: false,
                },
                vec![next],
                span,
            );
            let next_body = self.node(
                context,
                NodeKind::Predicate {
                    predicate,
                    sense: true,
                },
                vec![next],
                span,
            );
            self.program.graphs[context.graph as usize].nodes[header.index as usize]
                .antecedents
                .push(next_body);
            exits.push(next_exit);
        }
        context.live = !exits.is_empty();
        self.node(context, NodeKind::Merge, exits, span);
    }
}

/// The labels of `statements` that a `goto` after them, in the block or a block nested in it but
/// not in a nested function, jumps back to.
fn backward_labels(statements: &[Stmt]) -> HashSet<String> {
    fn gotos(statements: &[Stmt], names: &mut HashSet<String>) {
        for statement in statements {
            match &statement.kind {
                StmtKind::Goto(name) => { names.insert(name.text.clone()); }
                StmtKind::Block { body, .. } | StmtKind::While { body, .. } | StmtKind::Repeat { body, .. }
                | StmtKind::NumericFor { body, .. } | StmtKind::GenericFor { body, .. } => gotos(body, names),
                StmtKind::If { branches, else_branch } => {
                    for branch in branches { gotos(&branch.body, names); }
                    if let Some(branch) = else_branch { gotos(&branch.body, names); }
                }
                _ => {}
            }
        }
    }
    let mut backward = HashSet::default();
    for (index, statement) in statements.iter().enumerate() {
        if let StmtKind::Label(name) = &statement.kind {
            let mut later = HashSet::default();
            gotos(&statements[index + 1..], &mut later);
            if later.contains(&name.text) { backward.insert(name.text.clone()); }
        }
    }
    backward
}
