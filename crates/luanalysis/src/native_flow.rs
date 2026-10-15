//! Original-Lua graph recipes at the native flow boundary.
//!
//! This adapter canonicalizes source handles and frozen addresses. It performs
//! no type narrowing, type joins, reachability traversal or loop iteration.

use super::{bind::BindingId, flow};
use luanalysis_engine::{
    CapturedFunctionContext, ContextualFunctionContext, NativeCallEffects, NativeFlowError,
    NativeFlowKey, NativeFlowNode, NativeFlowProvider, NativeFunctionKey, NativeGraphKey,
    NativeIteratorProjection, NativeIteratorStage, NativeNumericConstant, NativeNumericInput,
    NativeNumericIterationStage, NativePackProjection, NativePredicate, NativeReferenceKey,
    NativeRuntimeType, NativeValueKey, NativeWriteRelation, Type, TypeQueries, ValuePack,
    NativeFunctionCreation, NativeVariableDeclaration, NativeVariableWrite,
};
use luanalysis_syntax::{BinaryOperator, Expr, ExprKind, File, Span};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub(super) enum ValueRecipe {
    Evaluation(flow::EvaluationId),
    PackElement {
        evaluation: flow::EvaluationId,
        index: usize,
    },
    Nil,
    Function(u32),
    NumericIteration,
    IteratorHeader(flow::IteratorId),
    IteratorResult {
        iterator: flow::IteratorId,
        index: usize,
    },
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
    iterator_values: HashMap<flow::IteratorId, u32>,
    write_values: HashMap<flow::FlowId, u32>,
    declaration_writes: HashMap<BindingId, flow::FlowId>,
    binding_owners: HashMap<BindingId, u32>,
    expression_evaluations: HashMap<(u32, usize, usize), flow::EvaluationId>,
    runtime_type_calls: HashSet<flow::SourceExpressionId>,
    runtime_type_intrinsic_enabled: bool,
    pack_projections: HashMap<BindingId, (flow::EvaluationId, usize)>,
    iterator_projections: HashMap<BindingId, (flow::IteratorId, usize)>,
    contexts: RefCell<HashMap<FlowContext, u64>>,
}

impl NativeFlowSources {
    pub(super) fn set_runtime_type_intrinsic_enabled(&mut self, enabled: bool) {
        self.runtime_type_intrinsic_enabled = enabled;
    }

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
            iterator_values: HashMap::new(),
            write_values: HashMap::new(),
            declaration_writes: HashMap::new(),
            binding_owners: HashMap::new(),
            expression_evaluations: HashMap::new(),
            runtime_type_calls: HashSet::new(),
            runtime_type_intrinsic_enabled: false,
            pack_projections: HashMap::new(),
            iterator_projections: HashMap::new(),
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
                    && matches!(&callee.kind, ExprKind::Name(_))
                    && bound.intrinsic_at(callee.span.start) == Some(crate::bind::BuiltinIntrinsic::LuaType)
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
            for iterator in result
                .program
                .iterators
                .iter()
                .filter(|iterator| iterator.input.graph == graph.index)
            {
                let reference = flow::Reference {
                    root: flow::ReferenceRoot::Iterator(iterator.id),
                    path: Vec::new(),
                };
                intern_reference(
                    &mut result.references[graph.index as usize],
                    &mut result.reference_indices[graph.index as usize],
                    &reference,
                );
                let index = result.values.len() as u32;
                result.values.push(ValueRecipe::IteratorHeader(iterator.id));
                result.iterator_values.insert(iterator.id, index);
            }
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
                                flow::ValueDemand::IteratorResult { iterator, index } => {
                                    ValueRecipe::IteratorResult {
                                        iterator: *iterator,
                                        index: *index,
                                    }
                                }
                                flow::ValueDemand::PackElement { evaluation, index } => {
                                    ValueRecipe::PackElement {
                                        evaluation: *evaluation,
                                        index: *index,
                                    }
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
        let written: HashSet<_> = bound
            .occurrences
            .iter()
            .filter(|occurrence| occurrence.kind == super::bind::OccurrenceKind::Write)
            .filter_map(|occurrence| occurrence.binding)
            .collect();
        let mut mutable_groups = HashSet::new();
        let mut mutable_iterators = HashSet::new();
        for graph in &result.program.graphs {
            for node in &graph.nodes {
                if let flow::NodeKind::Write {
                    address,
                    value: flow::ValueDemand::IteratorResult { iterator, index },
                    declaration: true,
                    ..
                } = &node.kind
                {
                    if let flow::ReferenceRoot::Binding(binding) = address.reference.root {
                        if written.contains(&binding) {
                            mutable_iterators.insert(*iterator);
                        }
                        result
                            .iterator_projections
                            .insert(binding, (*iterator, *index + 2));
                    }
                }
                if let flow::NodeKind::Write {
                    address,
                    value: flow::ValueDemand::PackElement { evaluation, index },
                    declaration: true,
                    ..
                } = &node.kind
                {
                    let flow::ReferenceRoot::Binding(binding) = address.reference.root else {
                        continue;
                    };
                    if written.contains(&binding) {
                        mutable_groups.insert(*evaluation);
                    }
                    result
                        .pack_projections
                        .insert(binding, (*evaluation, *index));
                    let reference = flow::Reference {
                        root: flow::ReferenceRoot::Pack(*evaluation),
                        path: Vec::new(),
                    };
                    intern_reference(
                        &mut result.references[graph.index as usize],
                        &mut result.reference_indices[graph.index as usize],
                        &reference,
                    );
                }
            }
        }
        result
            .pack_projections
            .retain(|_, (evaluation, _)| !mutable_groups.contains(evaluation));
        result
            .iterator_projections
            .retain(|_, (iterator, _)| !mutable_iterators.contains(iterator));
        for declaration in &bound.declarations {
            let mut scope = bound.scope(declaration.scope);
            let owner = loop {
                if scope.kind == super::bind::ScopeKind::Function {
                    break result.program.functions[&(scope.span.start, scope.span.end)];
                }
                let Some(parent) = scope.parent else { break 0; };
                scope = bound.scope(parent);
            };
            result.binding_owners.insert(declaration.id, owner);
            intern_reference(
                &mut result.references[owner as usize],
                &mut result.reference_indices[owner as usize],
                &flow::Reference { root: flow::ReferenceRoot::Binding(declaration.id), path: Vec::new() },
            );
        }
        for scope in bound.scopes.iter().filter(|scope| scope.kind == super::bind::ScopeKind::Function) {
            let graph = result.program.functions[&(scope.span.start, scope.span.end)] as usize;
            for &binding in &scope.captures {
                intern_reference(&mut result.references[graph], &mut result.reference_indices[graph], &flow::Reference {
                    root: flow::ReferenceRoot::Binding(binding),
                    path: Vec::new(),
                });
            }
        }
        result
    }

    pub(super) fn reference_declaration(&self, reference: NativeReferenceKey) -> Option<NativeVariableDeclaration> {
        let source = self.source_reference(reference)?;
        if !source.path.is_empty() { return None; }
        let flow::ReferenceRoot::Binding(binding) = source.root else { return None; };
        let owner = self.graph(*self.binding_owners.get(&binding)?, reference.graph.context);
        let writes = self.program.graphs.iter().flat_map(|graph| graph.nodes.iter()).filter_map(|node| {
            let flow::NodeKind::Write { address, .. } = &node.kind else { return None; };
            (address.reference.root == source.root && address.reference.path.is_empty()).then_some(NativeVariableWrite {
                owner: self.graph(node.id.graph, reference.graph.context),
                statement_end: address.statement_end,
            })
        }).collect();
        Some(NativeVariableDeclaration {
            owner,
            reference: self.reference_key(owner, source)?,
            writes,
        })
    }

    pub(super) fn graph_creation(&self, graph: NativeGraphKey) -> Option<NativeFunctionCreation> {
        if !self.contains_graph(graph) { return None; }
        let creation = self.program.graphs[graph.region as usize].creation?;
        Some(NativeFunctionCreation {
            input: self.flow_key(creation.input, graph.context),
            position: creation.position,
        })
    }

    pub(super) fn pack_projection(
        &self,
        reference: NativeReferenceKey,
    ) -> Option<NativePackProjection> {
        let source = self.source_reference(reference)?;
        if !source.path.is_empty() {
            return None;
        }
        let flow::ReferenceRoot::Binding(binding) = source.root else {
            return None;
        };
        let &(evaluation, index) = self.pack_projections.get(&binding)?;
        let evaluation = self.program.evaluation(evaluation);
        // Captured bindings cross an activation/graph boundary; their capture
        // environment remains authoritative until that contract carries groups.
        if evaluation.input.graph != reference.graph.region {
            return None;
        }
        let group = flow::Reference {
            root: flow::ReferenceRoot::Pack(evaluation.id),
            path: Vec::new(),
        };
        Some(NativePackProjection {
            group: self.reference_key(reference.graph, &group)?,
            value: self.evaluation_value(evaluation.id),
            input: self.flow_key(evaluation.input, reference.graph.context),
            index,
        })
    }

    pub(super) fn iterator_reference(
        &self,
        graph: NativeGraphKey,
        iterator: flow::IteratorId,
    ) -> Option<NativeReferenceKey> {
        self.reference_key(
            graph,
            &flow::Reference {
                root: flow::ReferenceRoot::Iterator(iterator),
                path: Vec::new(),
            },
        )
    }

    pub(super) fn iterator_projection(
        &self,
        reference: NativeReferenceKey,
    ) -> Option<NativeIteratorProjection> {
        let source = self.source_reference(reference)?;
        if !source.path.is_empty() {
            return None;
        }
        let flow::ReferenceRoot::Binding(binding) = source.root else {
            return None;
        };
        let &(iterator, index) = self.iterator_projections.get(&binding)?;
        if self.program.iterator(iterator).input.graph != reference.graph.region {
            return None;
        }
        Some(NativeIteratorProjection {
            frame: self.iterator_reference(reference.graph, iterator)?,
            index,
        })
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
        self.predicate_evaluation(key, evaluation)
    }

    fn predicate_evaluation(
        &self,
        key: NativeFlowKey,
        evaluation: &flow::Evaluation,
    ) -> NativePredicate {
        let expression = evaluation.expression;
        let source = &self.program.expression(expression).expression;
        match &source.kind {
            ExprKind::Parenthesized(_) => {
                if let Some(inner) = evaluation.operands.first() {
                    return self.predicate_evaluation(key, self.program.evaluation(*inner));
                }
            }
            ExprKind::Unary {
                op: luanalysis_syntax::UnaryOperator::Not,
                ..
            } => {
                if let Some(inner) = evaluation.operands.first() {
                    return NativePredicate::Not(Box::new(
                        self.predicate_evaluation(key, self.program.evaluation(*inner)),
                    ));
                }
            }
            ExprKind::Binary {
                op: BinaryOperator::And | BinaryOperator::Or,
                ..
            } if evaluation.operands.len() == 2 => {
                let left = Box::new(
                    self.predicate_evaluation(key, self.program.evaluation(evaluation.operands[0])),
                );
                let right = Box::new(
                    self.predicate_evaluation(key, self.program.evaluation(evaluation.operands[1])),
                );
                return if matches!(
                    &source.kind,
                    ExprKind::Binary {
                        op: BinaryOperator::And,
                        ..
                    }
                ) {
                    NativePredicate::And(left, right)
                } else {
                    NativePredicate::Or(left, right)
                };
            }
            _ => {}
        }
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
        if !self.runtime_type_intrinsic_enabled { return None; }
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
            b"table" => NativeRuntimeType::Table,
            b"function" => NativeRuntimeType::Function,
            b"userdata" | b"thread" => {
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

    fn numeric_input(&self, evaluation: flow::EvaluationId, context: u64) -> NativeNumericInput {
        let evaluation = self.program.evaluation(evaluation);
        let expression = &self.program.expression(evaluation.expression).expression;
        NativeNumericInput {
            value: self.evaluation_value(evaluation.id),
            input: self.flow_key(evaluation.input, context),
            literal: flow::numeric_literal(expression).map(|literal| match literal {
                flow::NumericLiteral::Integer(value) => NativeNumericConstant::Integer(value),
                flow::NumericLiteral::Float(value) => NativeNumericConstant::Float(value),
            }),
        }
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
                let input = match value {
                    flow::ValueDemand::Evaluation(evaluation)
                    | flow::ValueDemand::PackElement { evaluation, .. } => {
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
            flow::NodeKind::Predicate {
                predicate: flow::Predicate::Expression(expression),
                sense,
            } => NativeFlowNode::Predicate {
                before: before()?,
                predicate: self.predicate(key, *expression, node.antecedents[0]),
                sense: *sense,
            },
            flow::NodeKind::Predicate {
                predicate:
                    flow::Predicate::NumericIteration {
                        start,
                        limit,
                        step,
                        stage,
                    },
                sense,
            } => NativeFlowNode::Predicate {
                before: before()?,
                predicate: NativePredicate::NumericIteration {
                    start: self.numeric_input(*start, key.graph.context),
                    limit: self.numeric_input(*limit, key.graph.context),
                    step: step.map(|step| self.numeric_input(step, key.graph.context)),
                    stage: match stage {
                        flow::NumericIterationStage::First => NativeNumericIterationStage::First,
                        flow::NumericIterationStage::Next => NativeNumericIterationStage::Next,
                    },
                },
                sense: *sense,
            },
            flow::NodeKind::Predicate {
                predicate: flow::Predicate::GenericIteration { iterator },
                sense,
            } => NativeFlowNode::Predicate {
                before: before()?,
                predicate: NativePredicate::GenericIteration {
                    frame: self.iterator_reference(key.graph, *iterator)?,
                    input: before()?,
                },
                sense: *sense,
            },
            flow::NodeKind::IteratorStart { iterator } => NativeFlowNode::IteratorStart {
                before: before()?,
                frame: self.iterator_reference(key.graph, *iterator)?,
                header: NativeValueKey {
                    snapshot: self.program.snapshot,
                    index: self.iterator_values[iterator],
                },
                input: edge(self.program.iterator(*iterator).input),
            },
            flow::NodeKind::Iteration { iterator, stage } => NativeFlowNode::Iteration {
                before: before()?,
                frame: self.iterator_reference(key.graph, *iterator)?,
                stage: match stage {
                    flow::IteratorStage::First => NativeIteratorStage::First,
                    flow::IteratorStage::Next => NativeIteratorStage::Next,
                },
            },
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
        for (query_key, target_key) in query.path.iter().zip(&target.path) {
            if matches!(query_key, flow::Key::Dynamic(_))
                || matches!(target_key, flow::Key::Dynamic(_))
            {
                // A computed key may overwrite this member. Reset through the
                // native declared-type transfer rather than claiming key identity.
                return NativeWriteRelation::Ancestor;
            }
            if query_key != target_key {
                return NativeWriteRelation::Unaffected;
            }
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
    fn report_iterator_error(
        &self,
        at: NativeFlowKey,
        stage: NativeIteratorStage,
        callee: &Type,
        error: luanalysis_engine::CallError,
        queries: &mut dyn TypeQueries,
    );
    fn flow_value_pack(
        &self,
        sources: &NativeFlowSources,
        value: &ValueRecipe,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        ValuePack::single(self.flow_value_type(sources, value, input, queries))
    }
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
    fn reference_in_graph(&self, reference: NativeReferenceKey, graph: NativeGraphKey) -> Option<NativeReferenceKey> {
        self.sources.reference_key(graph, self.sources.source_reference(reference)?)
    }
    fn reference_declaration(&self, reference: NativeReferenceKey) -> Option<NativeVariableDeclaration> {
        self.sources.reference_declaration(reference)
    }
    fn graph_creation(&self, graph: NativeGraphKey) -> Option<NativeFunctionCreation> {
        self.sources.graph_creation(graph)
    }
    fn iterator_error(
        &self,
        at: NativeFlowKey,
        stage: NativeIteratorStage,
        callee: &Type,
        error: luanalysis_engine::CallError,
        queries: &mut dyn TypeQueries,
    ) {
        self.values
            .report_iterator_error(at, stage, callee, error, queries);
    }
    fn iterator_projection(
        &self,
        reference: NativeReferenceKey,
    ) -> Option<NativeIteratorProjection> {
        self.sources.iterator_projection(reference)
    }
    fn pack_projection(&self, reference: NativeReferenceKey) -> Option<NativePackProjection> {
        self.sources.pack_projection(reference)
    }
    fn value_pack(
        &self,
        value: NativeValueKey,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        let recipe = self
            .sources
            .value(value)
            .expect("native pack query uses an original source evaluation");
        self.values
            .flow_value_pack(self.sources, recipe, input, queries)
    }
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
