//! Original-Lua graph recipes at the native flow boundary.
//!
//! This adapter canonicalizes source handles and frozen addresses. It performs
//! no type narrowing, type joins, reachability traversal or loop iteration.

use super::{bind::BindingId, flow};
use luanalysis_engine::{
    CapturedFunctionContext, ContextualFunctionContext, NativeCallEffects, NativeFlowError,
    NativeFlowKey, NativeFlowNode, NativeFlowProvider, NativeFunctionKey, NativeGraphKey,
    NativePredicate, NativeReferenceKey, NativeRuntimeType, NativeValueKey, NativeWriteRelation,
    Type, TypeQueries,
};
use luanalysis_syntax::{BinaryOperator, Expr, ExprKind, File, Span};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub(super) enum ValueRecipe {
    Evaluation(flow::EvaluationId),
    Nil,
    Function(u32),
    NumericIteration,
    IteratorResult(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FlowContext {
    contextual: Option<ContextualFunctionContext>,
    captured: Option<CapturedFunctionContext>,
}

pub(super) struct NativeFlowSources {
    pub program: flow::ProgramFlow,
    functions: HashMap<NativeFunctionKey, u32>,
    references: Vec<Vec<flow::Reference>>,
    reference_indices: Vec<HashMap<flow::Reference, u32>>,
    values: Vec<ValueRecipe>,
    evaluation_values: HashMap<flow::EvaluationId, u32>,
    write_values: HashMap<flow::FlowId, u32>,
    declaration_writes: HashMap<BindingId, flow::FlowId>,
    expression_evaluations: HashMap<(u32, usize, usize), flow::EvaluationId>,
    runtime_type_calls: HashSet<flow::SourceExpressionId>,
    contexts: RefCell<HashMap<FlowContext, u64>>,
}

impl NativeFlowSources {
    pub(super) fn new(
        file: &File,
        bound: &super::bind::BoundFile,
        native_snapshot: u64,
        functions: &HashMap<(usize, usize), u32>,
    ) -> Self {
        let program = flow::build(file, bound);
        let mut result = Self {
            references: vec![Vec::new(); program.graphs.len()],
            reference_indices: vec![HashMap::new(); program.graphs.len()],
            functions: HashMap::new(),
            values: Vec::new(),
            evaluation_values: HashMap::new(),
            write_values: HashMap::new(),
            declaration_writes: HashMap::new(),
            expression_evaluations: HashMap::new(),
            runtime_type_calls: HashSet::new(),
            contexts: RefCell::new(HashMap::new()),
            program,
        };
        for (span, index) in functions {
            let graph = result.program.functions[span];
            result.functions.insert(
                NativeFunctionKey {
                    snapshot: native_snapshot,
                    index: *index,
                },
                graph,
            );
        }
        for evaluation in &result.program.evaluations {
            let expression = &result.program.expression(evaluation.expression).expression;
            if let ExprKind::Call { callee, args } = &expression.kind {
                let mut callee = callee.as_ref();
                while let ExprKind::Parenthesized(inner) = &callee.kind {
                    callee = inner;
                }
                if args.len() == 1
                    && matches!(&callee.kind, ExprKind::Name(name) if name.text == "type" && bound.binding_at(name.span.start).is_none())
                {
                    result.runtime_type_calls.insert(evaluation.expression);
                }
            }
            result.expression_evaluations.insert(
                (
                    evaluation.input.graph,
                    expression.span.start,
                    expression.span.end,
                ),
                evaluation.id,
            );
            let index = result.values.len() as u32;
            result.values.push(ValueRecipe::Evaluation(evaluation.id));
            result.evaluation_values.insert(evaluation.id, index);
            if let Some(reference) = result.program.references.get(&evaluation.expression) {
                let region = evaluation.input.graph as usize;
                intern_reference(
                    &mut result.references[region],
                    &mut result.reference_indices[region],
                    reference,
                );
            }
        }
        for graph in &result.program.graphs {
            for node in &graph.nodes {
                if let flow::NodeKind::Write {
                    address,
                    value,
                    declaration,
                    ..
                } = &node.kind
                {
                    let region = graph.index as usize;
                    intern_reference(
                        &mut result.references[region],
                        &mut result.reference_indices[region],
                        &address.reference,
                    );
                    let index = match value {
                        flow::ValueDemand::Evaluation(evaluation) => {
                            result.evaluation_values[evaluation]
                        }
                        other => {
                            let recipe = match other {
                                flow::ValueDemand::Nil => ValueRecipe::Nil,
                                flow::ValueDemand::Function(graph) => ValueRecipe::Function(*graph),
                                flow::ValueDemand::NumericIteration => {
                                    ValueRecipe::NumericIteration
                                }
                                flow::ValueDemand::IteratorResult(index) => {
                                    ValueRecipe::IteratorResult(*index)
                                }
                                flow::ValueDemand::Evaluation(_) => unreachable!(),
                            };
                            let index = result.values.len() as u32;
                            result.values.push(recipe);
                            index
                        }
                    };
                    result.write_values.insert(node.id, index);
                    if *declaration {
                        if let flow::ReferenceRoot::Binding(binding) = address.reference.root {
                            result.declaration_writes.entry(binding).or_insert(node.id);
                        }
                    }
                }
            }
        }
        result
    }

    pub(super) fn graph_for_function(
        &self,
        function: NativeFunctionKey,
        contextual: Option<&ContextualFunctionContext>,
    ) -> Option<NativeGraphKey> {
        let region = *self.functions.get(&function)?;
        Some(self.graph(region, self.context_id(contextual, None)))
    }

    pub(super) fn graph(&self, region: u32, context: u64) -> NativeGraphKey {
        NativeGraphKey {
            snapshot: self.program.snapshot,
            region,
            context,
        }
    }

    pub(super) fn graph_for_region(
        &self,
        region: u32,
        contextual: Option<&ContextualFunctionContext>,
    ) -> NativeGraphKey {
        self.graph(region, self.context_id(contextual, None))
    }

    pub(super) fn graph_with_contexts(
        &self,
        region: u32,
        contextual: Option<&ContextualFunctionContext>,
        captured: Option<&CapturedFunctionContext>,
    ) -> NativeGraphKey {
        self.graph(region, self.context_id(contextual, captured))
    }

    pub(super) fn function_for_graph(&self, graph: NativeGraphKey) -> Option<NativeFunctionKey> {
        self.functions
            .iter()
            .find_map(|(key, region)| (*region == graph.region).then_some(*key))
    }

    pub(super) fn evaluation_for_expression(
        &self,
        expression: &Expr,
        graph: NativeGraphKey,
    ) -> Option<&flow::Evaluation> {
        self.expression_evaluations
            .get(&(graph.region, expression.span.start, expression.span.end))
            .map(|id| self.program.evaluation(*id))
    }

    pub(super) fn expression_reference(
        &self,
        expression: &Expr,
        graph: NativeGraphKey,
    ) -> Option<NativeReferenceKey> {
        let evaluation = self.evaluation_for_expression(expression, graph)?;
        self.program
            .references
            .get(&evaluation.expression)
            .and_then(|reference| self.reference_key(graph, reference))
    }

    pub(super) fn write_value(&self, node: &flow::Node) -> Option<NativeValueKey> {
        self.write_values.get(&node.id).map(|index| NativeValueKey {
            snapshot: self.program.snapshot,
            index: *index,
        })
    }

    fn context_id(
        &self,
        contextual: Option<&ContextualFunctionContext>,
        captured: Option<&CapturedFunctionContext>,
    ) -> u64 {
        if contextual.is_none() && captured.is_none() {
            return 0;
        }
        let context = FlowContext {
            contextual: contextual.copied(),
            captured: captured.copied(),
        };
        let mut contexts = self.contexts.borrow_mut();
        let next = contexts.len() as u64 + 1;
        *contexts.entry(context).or_insert(next)
    }

    pub(super) fn contextual(&self, graph: NativeGraphKey) -> Option<ContextualFunctionContext> {
        if graph.context == 0 {
            return None;
        }
        self.contexts.borrow().iter().find_map(|(context, id)| {
            (*id == graph.context)
                .then_some(context.contextual)
                .flatten()
        })
    }

    pub(super) fn captured(&self, graph: NativeGraphKey) -> Option<CapturedFunctionContext> {
        self.contexts
            .borrow()
            .iter()
            .find_map(|(context, id)| (*id == graph.context).then_some(context.captured).flatten())
    }

    pub(super) fn contains_graph(&self, graph: NativeGraphKey) -> bool {
        graph.snapshot == self.program.snapshot
            && (graph.region as usize) < self.program.graphs.len()
            && (graph.context == 0
                || self.contextual(graph).is_some()
                || self.captured(graph).is_some())
    }

    pub(super) fn source_node(&self, key: NativeFlowKey) -> Option<&flow::Node> {
        if !self.contains_graph(key.graph) {
            return None;
        }
        self.program.graphs[key.graph.region as usize]
            .nodes
            .get(key.node as usize)
    }

    pub(super) fn source_reference(&self, key: NativeReferenceKey) -> Option<&flow::Reference> {
        if !self.contains_graph(key.graph) {
            return None;
        }
        self.references[key.graph.region as usize].get(key.reference as usize)
    }

    pub(super) fn value(&self, key: NativeValueKey) -> Option<&ValueRecipe> {
        (key.snapshot == self.program.snapshot)
            .then(|| self.values.get(key.index as usize))
            .flatten()
    }

    pub(super) fn flow_key(&self, source: flow::FlowId, context: u64) -> NativeFlowKey {
        assert_eq!(source.snapshot, self.program.snapshot);
        NativeFlowKey {
            graph: self.graph(source.graph, context),
            node: source.index,
        }
    }

    pub(super) fn reference_key(
        &self,
        graph: NativeGraphKey,
        reference: &flow::Reference,
    ) -> Option<NativeReferenceKey> {
        if !self.contains_graph(graph) {
            return None;
        }
        self.reference_indices[graph.region as usize]
            .get(reference)
            .map(|reference| NativeReferenceKey {
                graph,
                reference: *reference,
            })
    }

    pub(super) fn evaluation_value(&self, evaluation: flow::EvaluationId) -> NativeValueKey {
        NativeValueKey {
            snapshot: self.program.snapshot,
            index: self.evaluation_values[&evaluation],
        }
    }

    pub(super) fn declaration_write(&self, binding: BindingId) -> Option<&flow::Node> {
        self.declaration_writes
            .get(&binding)
            .map(|node| self.program.node(*node))
    }

    fn predicate(
        &self,
        key: NativeFlowKey,
        expression: flow::SourceExpressionId,
        before: flow::FlowId,
    ) -> NativePredicate {
        let Some(evaluation) = self.program.evaluations.iter().find(|evaluation| {
            evaluation.expression == expression && evaluation.completed == before
        }) else {
            return NativePredicate::Unsupported(
                "predicate evaluation is missing its frozen input",
            );
        };
        let source = &self.program.expression(expression).expression;
        if let ExprKind::Binary {
            op: BinaryOperator::Equal | BinaryOperator::NotEqual,
            ..
        } = &source.kind
        {
            if evaluation.operands.len() == 2 {
                let left = self.program.evaluation(evaluation.operands[0]);
                let right = self.program.evaluation(evaluation.operands[1]);
                let equals = matches!(
                    &source.kind,
                    ExprKind::Binary {
                        op: BinaryOperator::Equal,
                        ..
                    }
                );
                if let Some(predicate) = self
                    .runtime_type_predicate(key, left, right, equals)
                    .or_else(|| self.runtime_type_predicate(key, right, left, equals))
                {
                    return predicate;
                }
                let pair = self
                    .program
                    .references
                    .get(&left.expression)
                    .map(|reference| (reference, right))
                    .or_else(|| {
                        self.program
                            .references
                            .get(&right.expression)
                            .map(|reference| (reference, left))
                    });
                if let Some((reference, other)) = pair {
                    if let Some(subject) = self.reference_key(key.graph, reference) {
                        return NativePredicate::Equal {
                            subject,
                            other: self.evaluation_value(other.id),
                            input: self.flow_key(other.input, key.graph.context),
                            equals: matches!(
                                source.kind,
                                ExprKind::Binary {
                                    op: BinaryOperator::Equal,
                                    ..
                                }
                            ),
                        };
                    }
                }
            }
        }
        NativePredicate::Truthy {
            subject: self
                .program
                .references
                .get(&expression)
                .and_then(|reference| self.reference_key(key.graph, reference)),
            value: self.evaluation_value(evaluation.id),
            input: self.flow_key(evaluation.input, key.graph.context),
        }
    }

    fn runtime_type_predicate(
        &self,
        key: NativeFlowKey,
        call: &flow::Evaluation,
        literal: &flow::Evaluation,
        equals: bool,
    ) -> Option<NativePredicate> {
        let mut call = call;
        while matches!(
            self.program.expression(call.expression).expression.kind,
            ExprKind::Parenthesized(_)
        ) {
            call = self.program.evaluation(*call.operands.first()?);
        }
        if !self.runtime_type_calls.contains(&call.expression) {
            return None;
        }
        let mut literal = &self.program.expression(literal.expression).expression;
        while let ExprKind::Parenthesized(inner) = &literal.kind {
            literal = inner;
        }
        let ExprKind::String(name) = &literal.kind else {
            return None;
        };
        let kind = match name.as_slice() {
            b"nil" => NativeRuntimeType::Nil,
            b"boolean" => NativeRuntimeType::Boolean,
            b"number" => NativeRuntimeType::Number,
            b"string" => NativeRuntimeType::String,
            b"table" | b"function" | b"userdata" | b"thread" => {
                return Some(NativePredicate::Unsupported(
                    "nonprimitive Lua runtime type predicates require native runtime-domain types",
                ));
            }
            _ => return None,
        };
        let argument = self.program.evaluation(*call.operands.get(1)?);
        let reference = self.program.references.get(&argument.expression)?;
        Some(NativePredicate::RuntimeType {
            subject: self.reference_key(key.graph, reference)?,
            value: self.evaluation_value(argument.id),
            input: self.flow_key(argument.input, key.graph.context),
            kind,
            equals,
        })
    }

    pub(super) fn node(&self, key: NativeFlowKey) -> Option<NativeFlowNode> {
        let node = self.source_node(key)?;
        let edge = |source| self.flow_key(source, key.graph.context);
        let before = || node.antecedents.first().copied().map(edge);
        Some(match &node.kind {
            flow::NodeKind::Start => NativeFlowNode::Start,
            flow::NodeKind::Eval(_)
            | flow::NodeKind::CreateFunction { .. }
            | flow::NodeKind::Break => NativeFlowNode::Pass { before: before()? },
            flow::NodeKind::Write { address, value, .. } => {
                if address
                    .reference
                    .path
                    .iter()
                    .any(|key| matches!(key, flow::Key::Dynamic(_)))
                {
                    NativeFlowNode::Unsupported("dynamic Lua addresses require native key matching")
                } else {
                    let input = match value {
                        flow::ValueDemand::Evaluation(evaluation) => {
                            self.program.evaluation(*evaluation).input
                        }
                        _ => address.epoch,
                    };
                    NativeFlowNode::Write {
                        before: before()?,
                        target: self.reference_key(key.graph, &address.reference)?,
                        value: NativeValueKey {
                            snapshot: self.program.snapshot,
                            index: self.write_values[&node.id],
                        },
                        input: edge(input),
                    }
                }
            }
            flow::NodeKind::Predicate {
                predicate: flow::Predicate::Expression(expression),
                sense,
            } => NativeFlowNode::Predicate {
                before: before()?,
                predicate: self.predicate(key, *expression, node.antecedents[0]),
                sense: *sense,
            },
            flow::NodeKind::Predicate { .. } => NativeFlowNode::Unsupported(
                "Lua iteration predicates require native iteration effects",
            ),
            flow::NodeKind::Merge => NativeFlowNode::Merge {
                edges: node.antecedents.iter().copied().map(edge).collect(),
            },
            flow::NodeKind::Loop => NativeFlowNode::Loop {
                edges: node.antecedents.iter().copied().map(edge).collect(),
            },
            flow::NodeKind::Call { evaluation, .. } => NativeFlowNode::Call {
                before: before()?,
                value: self.evaluation_value(*evaluation),
                input: edge(self.program.evaluation(*evaluation).input),
            },
            flow::NodeKind::Return { .. } => NativeFlowNode::Return { before: before()? },
            flow::NodeKind::Unreachable => NativeFlowNode::Unreachable,
        })
    }

    pub(super) fn write_affects(
        &self,
        query: NativeReferenceKey,
        target: NativeReferenceKey,
        at: NativeFlowKey,
    ) -> NativeWriteRelation {
        if query.graph != at.graph || target.graph != at.graph {
            return NativeWriteRelation::Unaffected;
        }
        let (Some(query), Some(target), Some(node)) = (
            self.source_reference(query),
            self.source_reference(target),
            self.source_node(at),
        ) else {
            return NativeWriteRelation::Unaffected;
        };
        let flow::NodeKind::Write { address, .. } = &node.kind else {
            return NativeWriteRelation::Unaffected;
        };
        // All target addresses were captured before stores. A later/current
        // path to a replacement receiver cannot inherit a write to the old one.
        if !target.path.is_empty() && self.program.graphs[at.graph.region as usize].nodes.iter().any(|candidate| {
            matches!(&candidate.kind, flow::NodeKind::Write { address: replacement, .. }
                if replacement.transaction == address.transaction && replacement.reference.root == target.root
                && replacement.reference.path.len() < target.path.len() && target.path.starts_with(&replacement.reference.path))
        }) { return NativeWriteRelation::Unaffected; }
        if query.root != target.root {
            return NativeWriteRelation::Unaffected;
        }
        if query.path == target.path {
            NativeWriteRelation::Exact
        } else if target.path.len() < query.path.len() && query.path.starts_with(&target.path) {
            NativeWriteRelation::Ancestor
        } else {
            NativeWriteRelation::Unaffected
        }
    }
}

fn intern_reference(
    references: &mut Vec<flow::Reference>,
    indices: &mut HashMap<flow::Reference, u32>,
    reference: &flow::Reference,
) {
    if !indices.contains_key(reference) {
        let index = references.len() as u32;
        references.push(reference.clone());
        indices.insert(reference.clone(), index);
    }
}

pub(super) trait FlowValueQueries {
    fn flow_value_type(
        &self,
        sources: &NativeFlowSources,
        value: &ValueRecipe,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> Type;
    fn flow_call_effects(
        &self,
        sources: &NativeFlowSources,
        value: &ValueRecipe,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> NativeCallEffects;
    fn report_flow_error(&self, span: Span, error: NativeFlowError);
}

pub(super) struct FlowAdapter<'a> {
    pub sources: &'a NativeFlowSources,
    pub values: &'a dyn FlowValueQueries,
}

impl NativeFlowProvider for FlowAdapter<'_> {
    fn contains_graph(&self, graph: NativeGraphKey) -> bool {
        self.sources.contains_graph(graph)
    }
    fn contains_reference(&self, reference: NativeReferenceKey) -> bool {
        self.sources.source_reference(reference).is_some()
    }
    fn node(&self, node: NativeFlowKey) -> Option<NativeFlowNode> {
        self.sources.node(node)
    }
    fn is_shared(&self, node: NativeFlowKey) -> bool {
        self.sources
            .source_node(node)
            .is_some_and(|node| self.sources.program.is_shared(node.id))
    }
    fn write_affects(
        &self,
        reference: NativeReferenceKey,
        target: NativeReferenceKey,
        at: NativeFlowKey,
    ) -> NativeWriteRelation {
        self.sources.write_affects(reference, target, at)
    }
    fn value_type(
        &self,
        value: NativeValueKey,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        let recipe = self
            .sources
            .value(value)
            .expect("native query uses an original source value");
        self.values
            .flow_value_type(self.sources, recipe, input, queries)
    }
    fn call_effects(
        &self,
        value: NativeValueKey,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> NativeCallEffects {
        let recipe = self
            .sources
            .value(value)
            .expect("native query uses an original source call value");
        self.values
            .flow_call_effects(self.sources, recipe, input, queries)
    }
    fn flow_error(&self, at: NativeFlowKey, error: NativeFlowError) {
        let span = self
            .sources
            .source_node(at)
            .map_or(Span::default(), |node| node.span);
        self.values.report_flow_error(span, error);
    }
}
