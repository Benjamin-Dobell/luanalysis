//! Original-Lua graph recipes at the native flow boundary.
//!
//! This adapter canonicalizes source handles and frozen addresses. It performs
//! no type narrowing, type joins, reachability traversal or loop iteration.

#[cfg(test)]
#[path = "native_assertion_flow_tests.rs"]
mod assertion_tests;

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

/// Exact native signature capabilities from one lexical creation chain.
/// These are provenance handles; type resolution and substitution stay native.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SignatureFrame {
    pub function: NativeFunctionKey,
    pub contextual: Option<ContextualFunctionContext>,
    pub captured: CapturedFunctionContext,
}

#[derive(Default)]
struct FlowContexts {
    by_context: HashMap<FlowContext, u64>,
    by_id: Vec<FlowContext>,
    ancestry: HashMap<CapturedFunctionContext, Vec<SignatureFrame>>,
    contextual_owners: HashMap<ContextualFunctionContext, NativeFunctionKey>,
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
    runtime_type_operations: HashMap<flow::EvaluationId, flow::EvaluationId>,
    runtime_type_intrinsic_enabled: bool,
    loader_mutations: HashSet<(usize, usize)>,
    original_require_capability: bool,
    capability_bindings: HashSet<BindingId>,
    environment_candidates: HashMap<BindingId, HashSet<Vec<flow::Key>>>,
    environment_cell_written: bool,
    pack_projections: HashMap<BindingId, (flow::EvaluationId, usize)>,
    iterator_projections: HashMap<BindingId, (flow::IteratorId, usize)>,
    contexts: RefCell<FlowContexts>,
}

impl NativeFlowSources {
    pub(super) fn set_runtime_type_intrinsic_enabled(&mut self, enabled: bool) {
        self.runtime_type_intrinsic_enabled = enabled;
    }
    pub(super) fn set_original_require_capability(&mut self, enabled: bool) { self.original_require_capability = enabled; }
    pub(super) fn environment_cell_is_immutable(&self) -> bool { !self.environment_cell_written }
    fn loader_address_invalidates(&self, address: &flow::CapturedAddress) -> bool {
        if !self.loader_mutations.contains(&(address.span.start, address.span.end)) { return false; }
        if self.original_require_capability && address.reference.root == flow::ReferenceRoot::EnvironmentCell {
            match address.reference.path.as_slice() {
                [] => return false,
                [flow::Key::String(name)] if name == b"require" => return false,
                [flow::Key::String(root), flow::Key::String(name)] if root == b"_G" && name == b"require" => return false,
                _ => {}
            }
        }
        true
    }

    pub(super) fn new(
        file: &File,
        bound: &super::bind::BoundFile,
        native_snapshot: u64,
        functions: &HashMap<(usize, usize), u32>,
    ) -> Self {
        let program = flow::build(file, bound);
        let environment_cell_written = program.graphs.iter().flat_map(|graph| &graph.nodes).any(|node| {
            matches!(&node.kind, flow::NodeKind::Write { address, .. } if address.reference.root == flow::ReferenceRoot::EnvironmentCell && address.reference.path.is_empty())
        });
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
            runtime_type_operations: HashMap::new(),
            runtime_type_intrinsic_enabled: false,
            loader_mutations: bound.module_environment_mutations().iter().map(|span| (span.start, span.end)).collect(),
            original_require_capability: false,
            capability_bindings: HashSet::new(),
            environment_candidates: HashMap::new(),
            environment_cell_written,
            pack_projections: HashMap::new(),
            iterator_projections: HashMap::new(),
            contexts: RefCell::new(FlowContexts::default()),
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
                let _ = callee;
                if args.len() == 1 {
                    result.runtime_type_calls.insert(evaluation.expression);
                    if let [callee, argument] = evaluation.operands.as_slice() {
                        result.runtime_type_operations.insert(*argument, *callee);
                    }
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
        // Retain original candidates only. Native callee provenance still
        // decides whether a value is an intrinsic after each actual write.
        loop {
            let before = result.capability_bindings.len() + result.environment_candidates.values().map(HashSet::len).sum::<usize>();
            for graph in &result.program.graphs {
                for node in &graph.nodes {
                    let flow::NodeKind::Write { address, value: flow::ValueDemand::Evaluation(value), .. } = &node.kind else { continue; };
                    let flow::ReferenceRoot::Binding(binding) = address.reference.root else { continue; };
                    if !address.reference.path.is_empty() { continue; }
                    for reference in result.value_reference_candidates(*value) {
                        let paths = result.environment_candidate_paths(&reference);
                        for path in paths {
                            if matches!(path.as_slice(), [flow::Key::String(name)] if matches!(name.as_slice(), b"require" | b"type" | b"assert" | b"setmetatable")) { result.capability_bindings.insert(binding); }
                            result.environment_candidates.entry(binding).or_default().insert(path);
                        }
                        let candidate = match (&reference.root, reference.path.as_slice()) {
                            (flow::ReferenceRoot::EnvironmentCell, [flow::Key::String(name)]) => matches!(name.as_slice(), b"require" | b"type" | b"assert" | b"setmetatable"),
                            (flow::ReferenceRoot::Binding(parent), []) => result.capability_bindings.contains(parent),
                            _ => false,
                        };
                        if candidate { result.capability_bindings.insert(binding); }
                    }
                }
            }
            let after = result.capability_bindings.len() + result.environment_candidates.values().map(HashSet::len).sum::<usize>();
            if before == after { break; }
        }
        // Environment member references are addresses of an actual table, not
        // whichever local/global alias currently names it. Intern the source
        // keys up front; native queries later prove each receiver's identity.
        let members: Vec<_> = result.references.iter().flatten()
            .filter_map(|reference| reference.path.last().cloned()).collect();
        for region in 0..result.references.len() {
            intern_reference(&mut result.references[region], &mut result.reference_indices[region],
                &flow::Reference { root: flow::ReferenceRoot::EnvironmentCell, path: Vec::new() });
            intern_reference(&mut result.references[region], &mut result.reference_indices[region],
                &flow::Reference { root: flow::ReferenceRoot::LoaderState, path: Vec::new() });
            for key in &members {
                intern_reference(&mut result.references[region], &mut result.reference_indices[region],
                    &flow::Reference { root: flow::ReferenceRoot::EnvironmentObject, path: vec![key.clone()] });
            }
        }
        result
    }

    pub(super) fn environment_reference(&self, graph: NativeGraphKey, binding: Option<BindingId>) -> Option<NativeReferenceKey> {
        self.reference_key(graph, &flow::Reference {
            root: binding.map_or(flow::ReferenceRoot::EnvironmentCell, flow::ReferenceRoot::Binding),
            path: Vec::new(),
        })
    }
    fn environment_candidate_paths(&self, reference: &flow::Reference) -> Vec<Vec<flow::Key>> {
        match &reference.root {
            flow::ReferenceRoot::EnvironmentCell => {
                // This finite candidate domain retains exactly the two keys
                // needed to distinguish a global value from loader namespace
                // descendants. Actual address comparison keeps the full path.
                let path = match reference.path.as_slice() { [flow::Key::String(name), rest @ ..] if name == b"_G" => rest.iter().take(2).cloned().collect(), other => other.iter().take(2).cloned().collect() };
                vec![path]
            }
            flow::ReferenceRoot::Binding(binding) => self.environment_candidates.get(binding).into_iter().flatten().map(|parent| parent.iter().chain(&reference.path).take(2).cloned().collect()).collect(),
            _ => Vec::new(),
        }
    }

    /// Only possible original value sources, never an intrinsic attestation.
    /// Lua's logical operators select either operand's runtime value; treating
    /// their result as an ordinary type can erase a structurally equal callee.
    fn value_reference_candidates(&self, evaluation: flow::EvaluationId) -> Vec<flow::Reference> {
        let mut pending = vec![evaluation];
        let mut references = Vec::new();
        while let Some(evaluation) = pending.pop() {
            let evaluation = self.program.evaluation(evaluation);
            if let Some(reference) = self.program.references.get(&evaluation.expression) {
                references.push(reference.clone());
                continue;
            }
            match &self.program.expression(evaluation.expression).expression.kind {
                ExprKind::Parenthesized(_) | ExprKind::Binary {
                    op: BinaryOperator::And | BinaryOperator::Or, ..
                } => pending.extend(evaluation.operands.iter().copied()),
                _ => {},
            }
        }
        references
    }
    pub(super) fn loader_reference(&self, graph: NativeGraphKey) -> NativeReferenceKey {
        self.reference_key(graph, &flow::Reference { root: flow::ReferenceRoot::LoaderState, path: Vec::new() }).expect("each original graph has its loader state")
    }

    pub(super) fn environment_member_reference(&self, graph: NativeGraphKey, expression: &Expr) -> Option<NativeReferenceKey> {
        let source = self.expression_reference(expression, graph)?;
        let source = self.source_reference(source)?;
        self.reference_key(graph, &flow::Reference {
            root: flow::ReferenceRoot::EnvironmentObject,
            path: vec![source.path.last()?.clone()],
        })
    }

    pub(super) fn reference_declaration(&self, reference: NativeReferenceKey) -> Option<NativeVariableDeclaration> {
        let source = self.source_reference(reference)?;
        if matches!(source.root, flow::ReferenceRoot::LoaderState | flow::ReferenceRoot::EnvironmentObject) {
            let owner = self.graph(0, reference.graph.context);
            let writes = self.program.graphs.iter().flat_map(|graph| graph.nodes.iter()).filter_map(|node| {
                let flow::NodeKind::Write { address, .. } = &node.kind else { return None; };
                // Rebinding a local alias changes its cell, not the table it
                // previously referenced. Only a member store can mutate it.
                if matches!(address.reference.root, flow::ReferenceRoot::Binding(_)) && address.reference.path.is_empty() { return None; }
                let candidates = self.environment_candidate_paths(&address.reference);
                let affects = if source.root == flow::ReferenceRoot::LoaderState {
                    self.loader_address_invalidates(address) || candidates.iter().any(|path| match path.as_slice() {
                        [flow::Key::Dynamic(_), ..] => true,
                        [flow::Key::String(name)] if name == b"require" => !self.original_require_capability,
                        [flow::Key::String(name), members @ ..] if name == b"package" => crate::bind::modifies_package(&members.iter().map(|key| match key { flow::Key::String(value) => Some(value.as_slice()), _ => None }).collect::<Vec<_>>()),
                        _ => false,
                    })
                } else {
                    candidates.iter().any(|path| matches!((path.as_slice(), source.path.first()), ([written], Some(query)) if written == query || matches!(written, flow::Key::Dynamic(_))))
                };
                affects.then_some(NativeVariableWrite { owner: self.graph(node.id.graph, reference.graph.context), statement_end: address.statement_end })
            }).collect();
            return Some(NativeVariableDeclaration { owner, reference: self.reference_key(owner, source)?, writes });
        }
        if !source.path.is_empty() { return None; }
        let owner_region = match source.root {
            flow::ReferenceRoot::Binding(binding) => *self.binding_owners.get(&binding)?,
            flow::ReferenceRoot::EnvironmentCell => 0,
            _ => return None,
        };
        let owner = self.graph(owner_region, reference.graph.context);
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
    pub(super) fn binding_function_owner(&self,binding:BindingId)->Option<NativeFunctionKey> {
        self.function_for_graph(self.graph(*self.binding_owners.get(&binding)?,0))
    }

    pub(super) fn lexical_parent(&self, function: NativeFunctionKey) -> Option<NativeFunctionKey> {
        let region = *self.functions.get(&function)?;
        let creation = self.program.graphs[region as usize].creation?;
        self.function_for_graph(self.graph(creation.input.graph, 0))
    }

    pub(super) fn retain_signature_ancestry(&self, frames: Vec<SignatureFrame>) {
        let Some(first) = frames.first() else { return };
        let mut contexts = self.contexts.borrow_mut();
        for frame in &frames {
            if let Some(contextual) = frame.contextual {
                if let Some(previous) = contexts.contextual_owners.insert(contextual, frame.function) {
                    assert_eq!(previous, frame.function,
                        "a contextual capability has one original owner");
                }
            }
        }
        if let Some(previous) = contexts.ancestry.insert(first.captured, frames.clone()) {
            assert_eq!(previous, frames,
                "a native capture has immutable lexical provenance");
        }
    }

    pub(super) fn retain_contextual_owner(
        &self,
        context: ContextualFunctionContext,
        function: NativeFunctionKey,
    ) {
        let previous = self.contexts.borrow_mut().contextual_owners.insert(context, function);
        assert!(previous.is_none_or(|previous| previous == function),
            "a contextual capability has one original owner");
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
        if let Some(id) = contexts.by_context.get(&context) { return *id; }
        let next = contexts.by_id.len() as u64 + 1;
        contexts.by_id.push(context);
        contexts.by_context.insert(context, next);
        next
    }

    pub(super) fn contextual(&self, graph: NativeGraphKey) -> Option<ContextualFunctionContext> {
        if graph.context == 0 {
            return None;
        }
        let owner = self.function_for_graph(graph)?;
        let contexts = self.contexts.borrow();
        let context = contexts.by_id.get(usize::try_from(graph.context).ok()?.checked_sub(1)?)?;
        if let Some(captured) = context.captured {
            if let Some(frames) = contexts.ancestry.get(&captured) {
                return frames.iter().find(|frame| frame.function == owner)?.contextual;
            }
        }
        context.contextual.filter(|contextual| contexts.contextual_owners.get(contextual) == Some(&owner))
    }

    pub(super) fn captured(&self, graph: NativeGraphKey) -> Option<CapturedFunctionContext> {
        let owner = self.function_for_graph(graph)?;
        let contexts = self.contexts.borrow();
        let context = contexts.by_id.get(usize::try_from(graph.context).ok()?.checked_sub(1)?)?;
        let captured = context.captured?;
        contexts.ancestry.get(&captured)?.iter().find(|frame| frame.function == owner).map(|frame| frame.captured)
    }

    pub(super) fn contains_graph(&self, graph: NativeGraphKey) -> bool {
        graph.snapshot == self.program.snapshot
            && (graph.region as usize) < self.program.graphs.len()
            && (graph.context == 0 || self.contexts.borrow().by_id.get(graph.context as usize - 1).is_some())
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

    fn assertion_operand_is_frozen(&self, evaluation: flow::EvaluationId) -> bool {
        let mut pending = vec![evaluation];
        while let Some(evaluation) = pending.pop() {
            let evaluation = self.program.evaluation(evaluation);
            let expression = &self.program.expression(evaluation.expression).expression;
            match expression.kind {
                ExprKind::Call { .. } if !self.runtime_type_calls.contains(&evaluation.expression) => return false,
                ExprKind::MethodCall { .. } | ExprKind::Vararg => return false,
                _ => {}
            }
            pending.extend(evaluation.operands.iter().copied());
        }
        true
    }

    /// Select the original physical first argument, including a colon receiver.
    /// Unknown argument calls may mutate a reference after its asserted value
    /// was frozen; they cannot establish a later reference predicate here.
    pub(super) fn assertion_call_effect(&self, evaluation: &flow::Evaluation, input: NativeFlowKey) -> NativeCallEffects {
        let expression = &self.program.expression(evaluation.expression).expression;
        let first = match &expression.kind {
            ExprKind::Call { args, .. } if args.is_empty() => return NativeCallEffects::Never,
            ExprKind::Call { .. } => 1,
            ExprKind::MethodCall { .. } => 0,
            _ => return NativeCallEffects::None,
        };
        let arguments = &evaluation.operands[first..];
        let Some(&first) = arguments.first() else { return NativeCallEffects::None };
        if !self.assertion_operand_is_frozen(first) { return NativeCallEffects::None }
        let first = self.program.evaluation(first);
        let input = self.flow_key(first.input, input.graph.context);
        if arguments.iter().skip(1).any(|&argument| !self.assertion_operand_is_frozen(argument)) {
            let predicate = self.predicate_evaluation(input, first);
            let mut pending = vec![&predicate];
            while let Some(predicate) = pending.pop() {
                match predicate {
                    NativePredicate::And(left, right) | NativePredicate::Or(left, right) => { pending.push(left); pending.push(right); }
                    NativePredicate::Not(inner) => pending.push(inner),
                    NativePredicate::Truthy { subject: None, .. } => {}
                    _ => return NativeCallEffects::None,
                }
            }
        }
        NativeCallEffects::Assert { condition: self.evaluation_value(first.id), input }
    }

    fn assertion_predicate(&self, condition: NativeValueKey, input: NativeFlowKey) -> Option<NativePredicate> {
        let ValueRecipe::Evaluation(evaluation) = self.value(condition)? else { return None };
        let evaluation = self.program.evaluation(*evaluation);
        if self.flow_key(evaluation.input, input.graph.context) != input || !self.assertion_operand_is_frozen(evaluation.id) { return None }
        Some(self.predicate_evaluation(input, evaluation))
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
        if query.root == flow::ReferenceRoot::LoaderState {
            return if self.loader_address_invalidates(address) { NativeWriteRelation::Ancestor } else { NativeWriteRelation::Unaffected };
        }
        self.compare_write_references(query, target, address, at)
    }

    fn compare_write_references(&self, query: &flow::Reference, target: &flow::Reference, address: &flow::CapturedAddress, at: NativeFlowKey) -> NativeWriteRelation {
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
        } else if target.path.len() == query.path.len() + 1 && !matches!(target.path.last(), Some(flow::Key::Dynamic(_))) {
            NativeWriteRelation::Descendant
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
    fn environment_access_path(&self, expression: &Expr, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> Option<Vec<flow::Key>>;
    fn original_type_operation(&self, value: &ValueRecipe, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> bool;
    /// The environment named by a real lexical upvalue at an original flow input.
    fn environment_value(&self, binding: Option<BindingId>, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> Type;
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

impl FlowAdapter<'_> {
    /// Compare object origins at the two frozen epochs. Source paths select
    /// which original references to ask about; only the native alias query
    /// decides whether those reads denote the same allocation.
    fn alias_write_affects(&self,reference:NativeReferenceKey,target:NativeReferenceKey,at:NativeFlowKey,queries:&mut dyn TypeQueries)->Option<NativeWriteRelation> {
        use luanalysis_engine::NativeAliasRelation;
        let query=self.sources.source_reference(reference)?;
        let target_source=self.sources.source_reference(target)?;
        if !matches!(query.root,flow::ReferenceRoot::Binding(_)|flow::ReferenceRoot::Temporary(_)) {return None;}
        let node=self.sources.source_node(at)?;
        let flow::NodeKind::Write {address,..}=&node.kind else {return None};
        let receiver=self.sources.program.evaluation(address.receiver?);
        let receiver_value=self.sources.evaluation_value(receiver.id);
        let receiver_input=self.sources.flow_key(receiver.input,at.graph.context);
        let before=self.sources.flow_key(*node.antecedents.first()?,at.graph.context);
        let written=target_source.path.last()?;
        let mut possible=None;
        let mut compared=false;
        let prefixes=(0..query.path.len()).rev().chain(std::iter::once(query.path.len()));
        for length in prefixes {
            let prefix=flow::Reference {root:query.root.clone(),path:query.path[..length].to_vec()};
            let Some(prefix)=self.sources.reference_key(at.graph,&prefix) else {continue};
            let relation=queries.flow_alias_relation(self,prefix,before,receiver_value,receiver_input);
            compared=true;
            if relation==NativeAliasRelation::Different {continue;}
            let suffix=&query.path[length..];
            let affected=if suffix.is_empty() {NativeWriteRelation::Descendant}
                else if matches!(written,flow::Key::Dynamic(_)) || matches!(suffix[0],flow::Key::Dynamic(_)) {NativeWriteRelation::Ancestor}
                else if &suffix[0]!=written {continue;}
                else if suffix.len()==1 {NativeWriteRelation::Exact}
                else {NativeWriteRelation::Ancestor};
            if relation==NativeAliasRelation::Same && !matches!(written,flow::Key::Dynamic(_)) && !suffix.iter().any(|key|matches!(key,flow::Key::Dynamic(_))) {return Some(affected);}
            possible.get_or_insert(match affected {
                NativeWriteRelation::Exact=>NativeWriteRelation::PossibleExact,
                NativeWriteRelation::Ancestor=>NativeWriteRelation::PossibleAncestor,
                NativeWriteRelation::Descendant=>NativeWriteRelation::PossibleDescendant,
                _=>unreachable!("classified original alias path"),
            });
        }
        possible.or_else(||compared.then_some(NativeWriteRelation::Unaffected))
    }
}

impl NativeFlowProvider for FlowAdapter<'_> {
    fn value_alias_source(&self,value:NativeValueKey,input:NativeFlowKey)->luanalysis_engine::NativeAliasValue {
        use luanalysis_engine::NativeAliasValue;
        let Some(recipe)=self.sources.value(value) else {return NativeAliasValue::Unknown};
        let evaluation=match recipe {
            ValueRecipe::PackElement {evaluation,index} => {
                let source=self.sources.program.evaluation(*evaluation);
                return NativeAliasValue::Projection {value:self.sources.evaluation_value(*evaluation),input:self.sources.flow_key(source.input,input.graph.context),index:*index};
            }
            ValueRecipe::Evaluation(evaluation)=>self.sources.program.evaluation(*evaluation),
            ValueRecipe::Nil|ValueRecipe::Function(_)|ValueRecipe::NumericIteration=>return NativeAliasValue::NotTable,
            _=>return NativeAliasValue::Unknown,
        };
        let at=self.sources.flow_key(evaluation.input,input.graph.context);
        if matches!(self.sources.program.expression(evaluation.expression).expression.kind,ExprKind::Parenthesized(_))
            && let Some(inner)=evaluation.operands.first().copied()
        {
            let source=self.sources.program.evaluation(inner);
            return NativeAliasValue::Projection {value:self.sources.evaluation_value(inner),input:self.sources.flow_key(source.input,input.graph.context),index:0};
        }
        if let Some(reference)=self.sources.program.references.get(&evaluation.expression)
            && let Some(reference)=self.sources.reference_key(at.graph,reference)
        {return NativeAliasValue::Reference {reference,input:at};}
        let expression=&self.sources.program.expression(evaluation.expression).expression;
        match &expression.kind {
            ExprKind::Table(_)=>NativeAliasValue::Allocation,
            ExprKind::Call {..}=> {
                let Some(callee)=evaluation.operands.first().copied() else {return NativeAliasValue::Unknown};
                let source=self.sources.program.evaluation(callee);
                NativeAliasValue::Call {callee:self.sources.evaluation_value(callee),input:self.sources.flow_key(source.input,input.graph.context)}
            }
            // A method evaluation's first operand is its receiver, not the
            // selected method. It cannot serve as an implementation origin.
            ExprKind::MethodCall {..}=>NativeAliasValue::Unknown,
            ExprKind::Nil|ExprKind::Number(_)|ExprKind::String(_)|ExprKind::Boolean(_)|ExprKind::Function(_)=>NativeAliasValue::NotTable,
            _=>NativeAliasValue::Unknown,
        }
    }
    fn function_alias_returns(&self,function:NativeFunctionKey)->Option<Vec<luanalysis_engine::NativeAliasReturn>> {
        use luanalysis_engine::{NativeAliasReturn,NativeAliasReturnValue};
        let graph=self.sources.graph_for_function(function,None)?;
        Some(self.sources.program.graphs[graph.region as usize].nodes.iter().filter_map(|node| {
            let flow::NodeKind::Return {values}=&node.kind else {return None};
            let expands_last=values.last().is_some_and(|value|matches!(self.sources.program.expression(self.sources.program.evaluation(*value).expression).expression.kind,ExprKind::Call {..}|ExprKind::MethodCall {..}|ExprKind::Vararg));
            Some(NativeAliasReturn {at:self.sources.flow_key(node.id,graph.context),values:values.iter().map(|value| {
                let source=self.sources.program.evaluation(*value);
                NativeAliasReturnValue {value:self.sources.evaluation_value(*value),input:self.sources.flow_key(source.input,graph.context)}
            }).collect(),expands_last})
        }).collect())
    }
    fn function_alias_exit(&self,function:NativeFunctionKey)->Option<NativeFlowKey> {
        let graph=self.sources.graph_for_function(function,None)?;
        Some(self.sources.flow_key(self.sources.program.graphs[graph.region as usize].exit,graph.context))
    }
    fn reference_tracks_runtime_value(&self, reference: NativeReferenceKey) -> bool {
        let Some(reference) = self.sources.source_reference(reference) else { return false; };
        match (&reference.root, reference.path.as_slice()) {
            (flow::ReferenceRoot::EnvironmentCell | flow::ReferenceRoot::LoaderState, []) => true,
            (flow::ReferenceRoot::EnvironmentObject, [flow::Key::String(name)]) => matches!(name.as_slice(), b"_G" | b"require" | b"type" | b"assert" | b"setmetatable"),
            (flow::ReferenceRoot::Binding(binding), []) => self.sources.capability_bindings.contains(binding),
            _ => false,
        }
    }
    fn reference_follows_creation(&self, reference: NativeReferenceKey) -> bool {
        self.sources.source_reference(reference).is_some_and(|reference| matches!(reference.root, flow::ReferenceRoot::EnvironmentCell | flow::ReferenceRoot::LoaderState | flow::ReferenceRoot::EnvironmentObject))
    }
    fn runtime_type_operation(&self, value: NativeValueKey, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> bool {
        let Some(ValueRecipe::Evaluation(argument)) = self.sources.value(value) else { return false; };
        let Some(callee) = self.sources.runtime_type_operations.get(argument).copied() else { return false; };
        let evaluation = self.sources.program.evaluation(callee);
        self.values.original_type_operation(&ValueRecipe::Evaluation(callee), self.sources.flow_key(evaluation.input, input.graph.context), queries)
    }
    fn assertion_predicate(&self, condition: NativeValueKey, input: NativeFlowKey) -> Option<NativePredicate> {
        self.sources.assertion_predicate(condition, input)
    }
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
    fn write_affects_with_types(&self, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey, queries: &mut dyn TypeQueries) -> NativeWriteRelation {
        let Some(query) = self.sources.source_reference(reference) else { return NativeWriteRelation::Unaffected; };
        if query.root == flow::ReferenceRoot::LoaderState {
            let ordinary = self.sources.write_affects(reference, target, at);
            if ordinary != NativeWriteRelation::Unaffected { return ordinary; }
        }
        if !matches!(query.root, flow::ReferenceRoot::EnvironmentObject | flow::ReferenceRoot::LoaderState) {
            let direct=self.sources.write_affects(reference,target,at);
            return self.alias_write_affects(reference,target,at,queries).unwrap_or(direct);
        }
        let Some(flow::Node { kind: flow::NodeKind::Write { address, .. }, .. }) = self.sources.source_node(at) else {
            return NativeWriteRelation::Unaffected;
        };
        if matches!(address.reference.root, flow::ReferenceRoot::Binding(_)) && address.reference.path.is_empty() { return NativeWriteRelation::Unaffected; }
        let input = self.sources.flow_key(address.epoch, at.graph.context);
        let path = if let Some(receiver) = address.receiver {
            let receiver = self.sources.program.evaluation(receiver);
            let expression = &self.sources.program.expression(receiver.expression).expression;
            self.values.environment_access_path(expression, self.sources.flow_key(receiver.input, at.graph.context), queries)
        } else if address.reference.path.len() == 1 {
            let binding = match address.reference.root {
                flow::ReferenceRoot::EnvironmentCell => None,
                flow::ReferenceRoot::Binding(binding) => Some(binding),
                _ => return NativeWriteRelation::Unaffected,
            };
            let environment = self.values.environment_value(binding, input, queries);
            queries.global_environment_origin(&environment).map(|_| Vec::new())
        } else { None };
        let Some(mut path) = path else {
            // A mutable alias can erase its actual table identity through a
            // written view. Candidate metadata cannot prove a store exact, but
            // it also cannot prove a selected capability survives that store.
            // Keep this unsupported value-origin case conservative at this
            // actual continuation, rather than borrowing standard exports.
            let possible = self.sources.environment_candidate_paths(&address.reference);
            let invalidates = possible.iter().any(|path| {
                if query.root == flow::ReferenceRoot::LoaderState {
                    match path.as_slice() {
                        [flow::Key::Dynamic(_), ..] => true,
                        [flow::Key::String(name)] if name == b"require" => !self.sources.original_require_capability,
                        [flow::Key::String(name), members @ ..] if name == b"package" => crate::bind::modifies_package(&members.iter().map(|key| match key { flow::Key::String(value) => Some(value.as_slice()), _ => None }).collect::<Vec<_>>()),
                        _ => false,
                    }
                } else {
                    matches!((path.first(), query.path.first()), (Some(flow::Key::Dynamic(_)), _) | (Some(_), Some(flow::Key::Dynamic(_))))
                        || matches!((path.as_slice(), query.path.as_slice()), ([written], [read]) if written == read)
                }
            });
            return if invalidates { NativeWriteRelation::Ancestor } else { NativeWriteRelation::Unaffected };
        };
        let Some(key) = address.reference.path.last().cloned() else { return NativeWriteRelation::Unaffected; };
        path.push(key);
        if query.root == flow::ReferenceRoot::LoaderState {
            let affects = match path.as_slice() {
                [flow::Key::Dynamic(_), ..] => true,
                [flow::Key::String(name)] if name == b"require" => !self.sources.original_require_capability,
                [flow::Key::String(name), members @ ..] if name == b"package" => crate::bind::modifies_package(&members.iter().map(|key| match key { flow::Key::String(value) => Some(value.as_slice()), _ => None }).collect::<Vec<_>>()),
                _ => false,
            };
            return if affects { NativeWriteRelation::Ancestor } else { NativeWriteRelation::Unaffected };
        }
        let target = flow::Reference { root: flow::ReferenceRoot::EnvironmentObject, path };
        self.sources.compare_write_references(query, &target, address, at)
    }
    fn call_invalidates_reference(&self, reference: NativeReferenceKey, at: NativeFlowKey, _: &mut dyn TypeQueries) -> bool {
        self.sources.source_reference(reference).is_some_and(|reference| reference.root == flow::ReferenceRoot::LoaderState)
            && self.sources.source_node(at).is_some_and(|node| self.sources.loader_mutations.contains(&(node.span.start, node.span.end)))
    }
    fn write_member_key(&self, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey) -> Option<luanalysis_engine::LuaKey> {
        if reference.graph!=at.graph || target.graph!=at.graph {return None;}
        let target = self.sources.source_reference(target)?;
        Some(match target.path.last()? {
            flow::Key::String(value) => luanalysis_engine::LuaKey::String(value.clone()),
            flow::Key::Boolean(value) => luanalysis_engine::LuaKey::Boolean(*value),
            flow::Key::Integer(value) => luanalysis_engine::LuaKey::Integer(*value),
            flow::Key::Float(value) => luanalysis_engine::LuaKey::Number(f64::from_bits(*value)),
            flow::Key::Dynamic(_) => return None,
        })
    }
    fn reference_member_key(&self, reference: NativeReferenceKey, member: NativeReferenceKey) -> Option<luanalysis_engine::LuaKey> {
        if reference.graph != member.graph { return None; }
        let reference = self.sources.source_reference(reference)?;
        let member = self.sources.source_reference(member)?;
        if reference.root != member.root || member.path.len() != reference.path.len() + 1
            || !member.path.starts_with(&reference.path) { return None; }
        Some(match member.path.last()? {
            flow::Key::String(value) => luanalysis_engine::LuaKey::String(value.clone()),
            flow::Key::Boolean(value) => luanalysis_engine::LuaKey::Boolean(*value),
            flow::Key::Integer(value) => luanalysis_engine::LuaKey::Integer(*value),
            flow::Key::Float(value) => luanalysis_engine::LuaKey::Number(f64::from_bits(*value)),
            flow::Key::Dynamic(_) => return None,
        })
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
