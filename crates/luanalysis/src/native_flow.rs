//! Original-Lua graph recipes at the native flow boundary.
//!
//! This adapter canonicalizes source handles and frozen addresses. It performs
//! no type narrowing, type joins, reachability traversal or loop iteration.

#[cfg(test)]
#[path = "native_assertion_flow_tests.rs"]
mod assertion_tests;
#[cfg(test)]
#[path = "native_flow_metadata_index.rs"]
mod metadata_index_tests;

use super::{bind::BindingId, flow};
use luanalysis_engine::{
    CapturedFunctionContext, ContextualFunctionContext, NativeCallEffects, NativeFlowError,
    NativeFlowKey, NativeFlowNode, NativeFlowProvider, NativeFunctionKey, NativeGraphKey,
    NativeIteratorProjection, NativeIteratorStage, NativeNumericConstant, NativeNumericInput, NativeOpaqueRuntimeKind,
    NativeNumericIterationStage, NativePackProjection, NativePredicate, NativeReferenceKey,
    NativeRuntimeType, NativeValueKey, NativeWriteRelation, Type, TypeQueries, ValuePack,
    NativeFunctionCreation, NativeVariableDeclaration, NativeVariableWrite,
};
use luanalysis_syntax::{BinaryOperator, Expr, ExprKind, File, Span};
use std::cell::RefCell;
use crate::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(super) enum ValueRecipe {
    Evaluation(flow::EvaluationId),
    PackElement {
        evaluation: flow::EvaluationId,
        index: usize,
    },
    Nil,
    /// The nil a local declaration without a value writes.
    DeclarationNil(BindingId),
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

/// Original write locations only. Lists retain graph/node traversal order;
/// actual flow, alias decisions, and contextual ownership remain native queries.
#[derive(Clone, Default)]
struct OriginalWriteMetadata {
    nodes: Vec<flow::FlowId>,
    exact: HashMap<flow::Reference, Vec<flow::FlowId>>,
    environment_members: HashMap<flow::Key, Vec<usize>>,
    dynamic_environment_members: Vec<usize>,
    loader: [Vec<usize>; 2],
    /// The written references of each assignment transaction that has more
    /// than one write; a single write cannot replace its own receiver.
    transactions: HashMap<flow::AssignmentId, Vec<flow::Reference>>,
}

impl OriginalWriteMetadata {
    fn new(sources: &NativeFlowRecipes) -> Self {
        let mut result = Self::default();
        let mut transactions: HashMap<flow::AssignmentId, Vec<flow::Reference>> = HashMap::default();
        for graph in &sources.program.graphs {
            for node in &graph.nodes {
                let flow::NodeKind::Write { address, .. } = &node.kind else { continue; };
                let ordinal = result.nodes.len();
                result.nodes.push(node.id);
                result.exact.entry(address.reference.clone()).or_default().push(node.id);
                transactions.entry(address.transaction).or_insert_with(Vec::new).push(address.reference.clone());
                // Rebinding an alias changes its cell, never the old table.
                if matches!(address.reference.root, flow::ReferenceRoot::Binding(_))
                    && address.reference.path.is_empty() { continue; }
                let candidates = sources.environment_candidate_paths(&address.reference);
                for path in &candidates {
                    let [key] = path.as_slice() else { continue; };
                    let writes = if matches!(key, flow::Key::Dynamic(_)) {
                        &mut result.dynamic_environment_members
                    } else {
                        result.environment_members.entry(key.clone()).or_default()
                    };
                    if writes.last() != Some(&ordinal) { writes.push(ordinal); }
                }
                for capability in [false, true] {
                    let affects = sources.loader_address_invalidates_with_capability(address, capability)
                        || candidates.iter().any(|path| match path.as_slice() {
                            [flow::Key::Dynamic(_), ..] => true,
                            [flow::Key::String(name)] if name == b"require" => !capability,
                            [flow::Key::String(name), members @ ..] if name == b"package" => crate::bind::modifies_package(&members.iter().map(|key| match key { flow::Key::String(value) => Some(value.as_slice()), _ => None }).collect::<Vec<_>>()),
                            _ => false,
                        });
                    if affects { result.loader[usize::from(capability)].push(ordinal); }
                }
            }
        }
        transactions.retain(|_, writes| writes.len() > 1);
        result.transactions = transactions;
        result
    }

    fn environment_writes<'a>(&'a self, key: Option<&flow::Key>) -> impl Iterator<Item = flow::FlowId> + 'a {
        let (exact, dynamic) = key.map_or((&[][..], &[][..]), |key| {
            (self.environment_members.get(key).map_or(&[][..], Vec::as_slice), self.dynamic_environment_members.as_slice())
        });
        let (mut left, mut right) = (0, 0);
        std::iter::from_fn(move || {
            let ordinal = match (exact.get(left), dynamic.get(right)) {
                (Some(&a), Some(&b)) if a < b => { left += 1; a }
                (Some(&a), Some(&b)) if a > b => { right += 1; b }
                (Some(&a), Some(_)) => { left += 1; right += 1; a }
                (Some(&a), None) => { left += 1; a }
                (None, Some(&b)) => { right += 1; b }
                (None, None) => return None,
            };
            Some(self.nodes[ordinal])
        })
    }
}

/// Source-only graph recipes are prepared before native checking begins.
/// They contain no inferred types, active contexts or checker capabilities.
#[derive(Clone)]
pub(super) struct NativeFlowRecipes {
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
    /// The first evaluation of each predicate expression by its completed
    /// flow position, replacing a scan of every evaluation per predicate node.
    predicate_evaluations: HashMap<(flow::SourceExpressionId, flow::FlowId), flow::EvaluationId>,
    runtime_type_calls: HashSet<flow::SourceExpressionId>,
    runtime_type_operations: HashMap<flow::EvaluationId, flow::EvaluationId>,
    runtime_type_intrinsic_enabled: bool,
    runtime_userdata_enabled: bool,
    runtime_thread_enabled: bool,
    loader_mutations: HashSet<(usize, usize)>,
    original_require_capability: bool,
    /// A full Lua standard library is selected, so `require` is a standard function only where
    /// it is declared: a host's allow-list can leave it out.
    require_needs_declaration: bool,
    capability_bindings: HashSet<BindingId>,
    environment_candidates: HashMap<BindingId, HashSet<Vec<flow::Key>>>,
    environment_cell_written: bool,
    /// The position before which a main-chunk read sees the chunk's original environment: the
    /// first write of `_ENV`, when every write is a top-level statement of the main chunk and no
    /// label there lets a `goto` run an earlier statement again.
    original_environment_until: Option<usize>,
    pack_projections: HashMap<BindingId, (flow::EvaluationId, usize)>,
    iterator_projections: HashMap<BindingId, (flow::IteratorId, usize)>,
    write_metadata: OriginalWriteMetadata,
}

pub(super) struct NativeFlowSources {
    recipes: Arc<NativeFlowRecipes>,
    contexts: RefCell<FlowContexts>,
}

impl Deref for NativeFlowSources {
    type Target = NativeFlowRecipes;
    fn deref(&self) -> &Self::Target { &self.recipes }
}

impl NativeFlowSources {
    pub(super) fn shared_recipes(&self) -> Arc<NativeFlowRecipes> { Arc::clone(&self.recipes) }
    pub(super) fn from_shared_recipes(recipes: Arc<NativeFlowRecipes>) -> Self {
        Self { recipes, contexts: RefCell::new(FlowContexts::default()) }
    }
    #[cfg(test)]
    pub(super) fn context_state_counts(&self) -> (usize, usize, usize) {
        let contexts = self.contexts.borrow();
        (contexts.by_id.len(), contexts.ancestry.len(), contexts.contextual_owners.len())
    }

    /// Project setup imports original global declaration names as canonical
    /// addresses. It creates no expression, flow node, value or declaration.
    pub(super) fn seed_original_global_reference_keys(&mut self, names: &[Vec<u8>]) {
        let recipes = Arc::make_mut(&mut self.recipes);
        for region in 0..recipes.references.len() {
            for name in names {
                let path = vec![flow::Key::String(name.clone())];
                for root in [flow::ReferenceRoot::EnvironmentObject, flow::ReferenceRoot::EnvironmentCell] {
                    intern_reference(&mut recipes.references[region], &mut recipes.reference_indices[region],
                        &flow::Reference { root, path: path.clone() });
                }
            }
        }
    }
    pub(super) fn set_runtime_type_intrinsic_enabled(&mut self, enabled: bool) {
        if self.runtime_type_intrinsic_enabled == enabled { return; }
        Arc::make_mut(&mut self.recipes).runtime_type_intrinsic_enabled = enabled;
    }
    pub(super) fn set_opaque_runtime_type_enabled(&mut self, kind: NativeOpaqueRuntimeKind, enabled: bool) {
        let current = match kind {
            NativeOpaqueRuntimeKind::Userdata => self.runtime_userdata_enabled,
            NativeOpaqueRuntimeKind::Thread => self.runtime_thread_enabled,
        };
        if current == enabled { return; }
        let recipes = Arc::make_mut(&mut self.recipes);
        match kind {
            NativeOpaqueRuntimeKind::Userdata => recipes.runtime_userdata_enabled = enabled,
            NativeOpaqueRuntimeKind::Thread => recipes.runtime_thread_enabled = enabled,
        }
    }
    pub(super) fn set_original_require_capability(&mut self, enabled: bool) {
        if self.original_require_capability == enabled { return; }
        Arc::make_mut(&mut self.recipes).original_require_capability = enabled;
    }
    pub(super) fn has_original_require_capability(&self) -> bool { self.original_require_capability }
    fn declaration_value(&self, binding: BindingId) -> Option<&luanalysis_syntax::Expr> {
        let write = self.declaration_write(binding)?;
        let flow::NodeKind::Write { value, .. } = &write.kind else { return None };
        let evaluation = match value {
            flow::ValueDemand::Evaluation(evaluation) | flow::ValueDemand::PackElement { evaluation, index: 0 } => *evaluation,
            _ => return None,
        };
        let mut expression = &self.program.expression(self.program.evaluation(evaluation).expression).expression;
        while let ExprKind::Parenthesized(inner) = &expression.kind { expression = inner; }
        Some(expression)
    }
    /// Whether `binding` is declared with a table constructor (`local t = {}`).
    pub(super) fn initialized_by_table(&self, binding: BindingId) -> bool {
        self.declaration_value(binding).is_some_and(|value| matches!(value.kind, ExprKind::Table(_)))
    }
    /// Whether `binding` saves a standard operation (`local saved = require`), whose capability
    /// the native standard-value queries follow.
    pub(super) fn initialized_by_standard_name(&self, binding: BindingId) -> bool {
        self.declaration_value(binding).is_some_and(|value| match &value.kind {
            ExprKind::Name(name) => crate::bind::STANDARD_NAMES.contains(&name.text.as_str()),
            // A module (`local m = require 'm'`) is never narrowed; its import has no flow to follow.
            ExprKind::Call { callee, .. } => matches!(&callee.kind, ExprKind::Name(name) if name.text == "require"),
            _ => false,
        })
    }
    pub(super) fn set_require_needs_declaration(&mut self, needed: bool) {
        if self.require_needs_declaration == needed { return; }
        Arc::make_mut(&mut self.recipes).require_needs_declaration = needed;
    }
    pub(super) fn require_needs_declaration(&self) -> bool { self.require_needs_declaration }
    pub(super) fn environment_cell_is_immutable(&self) -> bool { !self.environment_cell_written }
    /// Whether a read at `position` in `graph` sees the chunk's original environment.
    pub(super) fn reads_original_environment(&self, graph: NativeGraphKey, position: usize) -> bool {
        !self.environment_cell_written
            || graph.region == 0 && self.original_environment_until.is_some_and(|until| position < until)
    }
    fn loader_address_invalidates(&self, address: &flow::CapturedAddress) -> bool {
        self.loader_address_invalidates_with_capability(address, self.original_require_capability)
    }

    pub(super) fn new(
        file: &File,
        bound: &super::bind::BoundFile,
        native_snapshot: u64,
        functions: &HashMap<(usize, usize), u32>,
    ) -> Self {
        Self::new_with_snapshot(file, bound, native_snapshot, functions, flow::allocate_snapshot())
    }

    pub(super) fn new_with_snapshot(
        file: &File,
        bound: &super::bind::BoundFile,
        native_snapshot: u64,
        functions: &HashMap<(usize, usize), u32>,
        flow_snapshot: u64,
    ) -> Self {
        let program = flow::build_with_snapshot(file, bound, flow_snapshot);
        let environment_cell_written = program.graphs.iter().flat_map(|graph| &graph.nodes).any(|node| {
            matches!(&node.kind, flow::NodeKind::Write { address, .. } if address.reference.root == flow::ReferenceRoot::EnvironmentCell && address.reference.path.is_empty())
        });
        let original_environment_until = environment_cell_written.then(|| original_environment_until(file, bound)).flatten();
        let mut result = NativeFlowRecipes {
            references: vec![Vec::new(); program.graphs.len()],
            reference_indices: vec![HashMap::default(); program.graphs.len()],
            functions: HashMap::default(),
            values: Vec::new(),
            evaluation_values: HashMap::default(),
            iterator_values: HashMap::default(),
            write_values: HashMap::default(),
            declaration_writes: HashMap::default(),
            binding_owners: HashMap::default(),
            expression_evaluations: HashMap::default(),
            predicate_evaluations: HashMap::default(),
            runtime_type_calls: HashSet::default(),
            runtime_type_operations: HashMap::default(),
            runtime_type_intrinsic_enabled: false,
            runtime_userdata_enabled: false,
            runtime_thread_enabled: false,
            loader_mutations: bound.module_environment_mutations().iter().chain(bound.computed_environment_mutations()).chain(bound.environment_rebindings()).map(|span| (span.start, span.end)).collect(),
            original_require_capability: false,
            require_needs_declaration: false,
            capability_bindings: HashSet::default(),
            environment_candidates: HashMap::default(),
            environment_cell_written,
            original_environment_until,
            pack_projections: HashMap::default(),
            iterator_projections: HashMap::default(),
            write_metadata: OriginalWriteMetadata::default(),
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
            result.predicate_evaluations.entry((evaluation.expression, evaluation.completed)).or_insert(evaluation.id);
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
                                flow::ValueDemand::Nil => match &address.reference.root {
                                    flow::ReferenceRoot::Binding(binding) if *declaration && address.reference.path.is_empty() => {
                                        ValueRecipe::DeclarationNil(*binding)
                                    }
                                    _ => ValueRecipe::Nil,
                                },
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
        let mut foreign_mutable_groups = HashSet::default();
        let mut write_owners: HashMap<BindingId, HashSet<u32>> = HashMap::default();
        for graph in &result.program.graphs {
            for node in &graph.nodes {
                if let flow::NodeKind::Write { address, .. } = &node.kind
                    && address.reference.path.is_empty()
                    && let flow::ReferenceRoot::Binding(binding) = address.reference.root
                {
                    write_owners.entry(binding).or_default().insert(graph.index);
                }
            }
        }
        let mut mutable_iterators = HashSet::default();
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
                    if write_owners.get(&binding).is_some_and(|owners| owners.iter().any(|owner| *owner != graph.index)) {
                        foreign_mutable_groups.insert(*evaluation);
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
            .retain(|_, (evaluation, _)| !foreign_mutable_groups.contains(evaluation));
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
        // Each key once, in order of first occurrence: a repeated key interns nothing.
        let mut seen = HashSet::default();
        let members: Vec<_> = result.references.iter().flatten()
            .filter_map(|reference| reference.path.last().cloned())
            .filter(|key| seen.insert(key.clone()))
            .collect();
        for region in 0..result.references.len() {
            intern_reference(&mut result.references[region], &mut result.reference_indices[region],
                &flow::Reference { root: flow::ReferenceRoot::EnvironmentCell, path: Vec::new() });
            intern_reference(&mut result.references[region], &mut result.reference_indices[region],
                &flow::Reference { root: flow::ReferenceRoot::LoaderState, path: Vec::new() });
            intern_reference(&mut result.references[region], &mut result.reference_indices[region],
                &flow::Reference { root: flow::ReferenceRoot::EnvironmentObject, path: Vec::new() });
            // Reads of the original environment: everywhere, unless the chunk rebinds `_ENV`, and
            // then main-chunk reads before the first rebinding (`original_environment_until`).
            if !result.environment_cell_written || region == 0 && result.original_environment_until.is_some() {
                let original: Vec<_> = result.references[region].iter().filter(|reference| reference.root == flow::ReferenceRoot::EnvironmentCell)
                    .map(|reference| flow::Reference { root: flow::ReferenceRoot::EnvironmentObject, path: reference.path.clone() }).collect();
                for reference in original {
                    intern_reference(&mut result.references[region], &mut result.reference_indices[region], &reference);
                }
            }
            for key in &members {
                intern_reference(&mut result.references[region], &mut result.reference_indices[region],
                    &flow::Reference { root: flow::ReferenceRoot::EnvironmentObject, path: vec![key.clone()] });
                intern_reference(&mut result.references[region], &mut result.reference_indices[region],
                    &flow::Reference { root: flow::ReferenceRoot::EnvironmentCell, path: vec![key.clone()] });
            }
        }
        result.write_metadata = OriginalWriteMetadata::new(&result);
        Self::from_shared_recipes(Arc::new(result))
    }

    pub(super) fn environment_reference(&self, graph: NativeGraphKey, binding: Option<BindingId>) -> Option<NativeReferenceKey> {
        self.reference_key(graph, &flow::Reference {
            root: binding.map_or(flow::ReferenceRoot::EnvironmentCell, flow::ReferenceRoot::Binding),
            path: Vec::new(),
        })
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
            let writes = if source.root == flow::ReferenceRoot::LoaderState {
                self.write_metadata.loader[usize::from(self.original_require_capability)].iter()
                    .map(|&ordinal| self.variable_write(self.write_metadata.nodes[ordinal], reference.graph.context)).collect()
            } else {
                self.write_metadata.environment_writes(source.path.first())
                    .map(|node| self.variable_write(node, reference.graph.context)).collect()
            };
            return Some(NativeVariableDeclaration { owner, reference: self.reference_key(owner, source)?, writes });
        }
        if !source.path.is_empty() { return None; }
        let owner_region = match source.root {
            flow::ReferenceRoot::Binding(binding) => *self.binding_owners.get(&binding)?,
            flow::ReferenceRoot::EnvironmentCell => 0,
            _ => return None,
        };
        let owner = self.graph(owner_region, reference.graph.context);
        let writes = self.write_metadata.exact.get(source).into_iter().flatten()
            .map(|&node| self.variable_write(node, reference.graph.context)).collect();
        Some(NativeVariableDeclaration {
            owner,
            reference: self.reference_key(owner, source)?,
            writes,
        })
    }

    fn variable_write(&self, node: flow::FlowId, context: u64) -> NativeVariableWrite {
        let flow::NodeKind::Write { address, .. } = &self.program.node(node).kind else {
            unreachable!("original write metadata contains only writes")
        };
        NativeVariableWrite { owner: self.graph(node.graph, context), statement_end: address.statement_end }
    }

    /// Whether some original store gives `binding` a value that may be the
    /// global environment or one of its members.
    pub(super) fn environment_candidate(&self, binding: BindingId) -> bool {
        self.environment_candidates.contains_key(&binding)
    }

    pub(super) fn immutable_initializer(&self, binding: BindingId) -> Option<&flow::Evaluation> {
        let reference = flow::Reference { root: flow::ReferenceRoot::Binding(binding), path: Vec::new() };
        let [write] = self.write_metadata.exact.get(&reference)?.as_slice() else { return None; };
        let flow::NodeKind::Write { value: flow::ValueDemand::Evaluation(initializer), declaration: true, .. } = &self.program.node(*write).kind else { return None; };
        Some(self.program.evaluation(*initializer))
    }

    /// A binding owned by `region` that no original store writes, such as an
    /// unassigned parameter: it holds its entry value throughout that body.
    pub(super) fn unwritten_binding_of(&self, binding: BindingId, region: u32) -> bool {
        self.binding_owners.get(&binding) == Some(&region)
            && self.write_metadata.exact.get(&flow::Reference { root: flow::ReferenceRoot::Binding(binding), path: Vec::new() }).is_none_or(Vec::is_empty)
    }

    pub(super) fn graph_entry(&self, region: u32) -> flow::FlowId { self.program.graphs[region as usize].entry }

    /// What original stores in this source may replace through the environment
    /// or a local alias of it (`environment_candidate_paths`): `bind::STANDARD_NAMES`
    /// bits, and whether the module loader (`require`, `package` loader fields)
    /// may change. Computed environment keys (`_G[k] = v`) do not count: as for
    /// other globals, they are unknown effects in ordinary checking. Direct
    /// writes and `rawset` calls are also recorded by the binder.
    pub(super) fn environment_replacements(&self) -> (u8, bool) {
        let (mut names, mut loader) = (0, false);
        for node in self.program.graphs.iter().flat_map(|graph| &graph.nodes) {
            let flow::NodeKind::Write { address, .. } = &node.kind else { continue; };
            if matches!(address.reference.root, flow::ReferenceRoot::Binding(_)) && address.reference.path.is_empty() { continue; }
            // Rebinding `_ENV` or `_G` itself replaces no member of the global
            // environment: `_ENV = nil` changes only this chunk's free names,
            // which `environment_cell_written` records.
            if matches!(address.reference.root, flow::ReferenceRoot::EnvironmentCell)
                && (address.reference.path.is_empty() || matches!(address.reference.path.as_slice(), [flow::Key::String(name)] if name == b"_G"))
            { continue; }
            for path in self.environment_candidate_paths(&address.reference) {
                match path.as_slice() {
                    [] => { names = u8::MAX; loader = true; },
                    [flow::Key::String(name), rest @ ..] => {
                        if let Some(bit) = super::bind::STANDARD_NAMES.iter().position(|standard| standard.as_bytes() == name.as_slice()) && rest.is_empty() { names |= 1 << bit; }
                        loader |= match name.as_slice() {
                            b"require" => rest.is_empty(),
                            b"package" => crate::bind::modifies_package(&rest.iter().map(|key| match key { flow::Key::String(value) => Some(value.as_slice()), _ => None }).collect::<Vec<_>>()),
                            _ => false,
                        };
                    },
                    _ => {},
                }
            }
        }
        (names, loader)
    }

    /// What an original callee names lexically (Luanalysis v1 semantics). A
    /// local that no original write can give an intrinsic is ordinary; an
    /// immutable one names its initializer. An unbound name other than a
    /// standard name is ordinary, and an unreplaced standard name is the
    /// original environment member. Members spelled with a standard name,
    /// mutable candidate locals, replaced names and a lexical or written
    /// `_ENV` are left to the provenance walk. Calls, operators, other members
    /// and computed keys never name an intrinsic.
    pub(super) fn lexical_standard(&self, bound: &super::bind::BoundFile, replaced: u8, evaluation: flow::EvaluationId, graph: NativeGraphKey) -> luanalysis_engine::NativeLexicalStandard {
        use luanalysis_engine::NativeLexicalStandard as Lexical;
        use super::bind::STANDARD_NAMES;
        let standard = |name: &[u8]| STANDARD_NAMES.iter().position(|standard| standard.as_bytes() == name);
        let mut expression = &self.program.expression(self.program.evaluation(evaluation).expression).expression;
        loop {
            match &expression.kind {
                ExprKind::Parenthesized(inner) => expression = inner,
                ExprKind::Name(name) => {
                    if let Some(binding) = bound.binding_at(name.span.start) {
                        if !self.capability_bindings.contains(&binding) { return Lexical::Ordinary; }
                        let Some(initializer) = self.immutable_initializer(binding) else { return Lexical::Unknown; };
                        expression = &self.program.expression(initializer.expression).expression;
                        continue;
                    }
                    let Some(bit) = standard(name.text.as_bytes()) else { return Lexical::Ordinary; };
                    if !self.reads_original_environment(graph, name.span.start) || replaced & 1 << bit != 0 || bound.environment_binding_at(name.span.start).is_some() {
                        return Lexical::Unknown;
                    }
                    let reference = flow::Reference { root: flow::ReferenceRoot::EnvironmentObject, path: vec![flow::Key::String(name.text.as_bytes().to_vec())] };
                    return self.reference_key(graph, &reference).map_or(Lexical::Unknown, Lexical::Global);
                }
                ExprKind::Field { name, .. } => return if standard(name.text.as_bytes()).is_some() { Lexical::Unknown } else { Lexical::Ordinary },
                ExprKind::Index { index, .. } => return match super::native_tables::literal_key(index) {
                    Some(luanalysis_engine::LuaKey::String(key)) if standard(&key).is_some() => Lexical::Unknown,
                    _ => Lexical::Ordinary,
                },
                _ => return Lexical::Ordinary,
            }
        }
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

    /// The evaluation that initialises `binding` at its declaration, whatever
    /// later writes rebind it (as a binder sees the declaration).
    /// The value of the latest write of `binding` in graph `region` whose
    /// statement ends at or before `position`: the syntactically reaching
    /// assignment in straight-line code. Writes in nested functions and in the
    /// same statement do not count.
    pub(super) fn latest_binding_write_before(&self, binding: BindingId, region: u32, position: usize) -> Option<&flow::Evaluation> {
        let reference = flow::Reference { root: flow::ReferenceRoot::Binding(binding), path: Vec::new() };
        self.write_metadata.exact.get(&reference)?.iter().filter_map(|&write| {
            let flow::NodeKind::Write { address, value, .. } = &self.program.node(write).kind else { return None; };
            let value = written_expression(value)?;
            (write.graph == region && address.statement_end <= position).then_some((address.statement_end, value))
        }).max_by_key(|(end, _)| *end).map(|(_, value)| self.program.evaluation(value))
    }

    pub(super) fn declaration_initializer(&self, binding: BindingId) -> Option<&flow::Evaluation> {
        let flow::NodeKind::Write { value, .. } = &self.declaration_write(binding)?.kind else { return None; };
        Some(self.program.evaluation(written_expression(value)?))
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
        let Some(evaluation) = self.predicate_evaluations.get(&(expression, before)).map(|&id| self.program.evaluation(id)) else {
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
            b"userdata" if self.runtime_userdata_enabled => NativeRuntimeType::Userdata,
            b"thread" if self.runtime_thread_enabled => NativeRuntimeType::Thread,
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
        let flow::NodeKind::Write { address, value, declaration, .. } = &node.kind else {
            return NativeWriteRelation::Unaffected;
        };
        if let flow::ReferenceRoot::Pack(evaluation) = query.root {
            let flow::ReferenceRoot::Binding(binding) = target.root else { return NativeWriteRelation::Unaffected; };
            if !target.path.is_empty() || !self.pack_projections.get(&binding).is_some_and(|(group, _)| *group == evaluation) {
                return NativeWriteRelation::Unaffected;
            }
            // The original simultaneous declaration stores create the relation;
            // any reached later rebind of a sibling breaks that physical tuple.
            return if *declaration && matches!(value, flow::ValueDemand::PackElement { evaluation: original, .. } if *original == evaluation) {
                NativeWriteRelation::Unaffected
            } else {
                NativeWriteRelation::Ancestor
            };
        }
        if query.root == flow::ReferenceRoot::LoaderState {
            return if self.loader_address_invalidates(address) { NativeWriteRelation::Ancestor } else { NativeWriteRelation::Unaffected };
        }
        self.compare_write_references(query, target, address)
    }

    /// All target addresses were captured before stores. A later/current path
    /// to a replacement receiver cannot inherit a write to the old one.
    fn receiver_replaced_in_transaction(&self, target: &flow::Reference, address: &flow::CapturedAddress) -> bool {
        !target.path.is_empty() && self.write_metadata.transactions.get(&address.transaction).is_some_and(|writes| {
            writes.iter().any(|replacement| replacement.root == target.root
                && replacement.path.len() < target.path.len() && target.path.starts_with(&replacement.path))
        })
    }

    fn compare_write_references(&self, query: &flow::Reference, target: &flow::Reference, address: &flow::CapturedAddress) -> NativeWriteRelation {
        // Different roots are unaffected; check that before the graph scan.
        if query.root != target.root {
            return NativeWriteRelation::Unaffected;
        }
        if self.receiver_replaced_in_transaction(target, address) { return NativeWriteRelation::Unaffected; }
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
    fn runtime_table_store_in_scope(&self, _: luanalysis_engine::NativeTypeDeclarationKey, _: NativeFlowKey) -> bool { false }
    fn alias_value_type(&self, sources:&NativeFlowSources, value:&ValueRecipe, input:NativeFlowKey, queries:&mut dyn TypeQueries)->Type {self.flow_value_type(sources,value,input,queries)}
    fn binding_has_written_contract(&self, _: BindingId) -> bool { false }
    fn written_binding_contract(&self, _: BindingId, _: &mut dyn TypeQueries) -> Option<Type> { None }
    fn original_alias_initial(&self, _: &flow::Reference) -> luanalysis_engine::NativeAliasInitial { luanalysis_engine::NativeAliasInitial::Opaque }
    /// See `NativeFlowSources::lexical_standard`; Unknown keeps the provenance walk.
    fn lexical_standard(&self, _: &NativeFlowSources, _: flow::EvaluationId, _: NativeGraphKey) -> luanalysis_engine::NativeLexicalStandard { luanalysis_engine::NativeLexicalStandard::Unknown }
    /// No implementation source in the project may mutate the module loader.
    fn loader_never_mutated(&self) -> bool { false }
    fn original_alias_execution(&self, _: NativeFunctionKey, _: NativeFlowKey) -> Option<luanalysis_engine::NativeAliasExecution> { None }
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
    /// Some(None) certifies raw absence in this complete original literal;
    /// None leaves the recipe unsupported. No inferred property view is used.
    fn alias_literal_member(&self, value: NativeValueKey, input: NativeFlowKey, key: &luanalysis_engine::LuaKey) -> Option<Option<luanalysis_engine::NativeAliasReturnValue>> {
        let luanalysis_engine::LuaKey::String(_) = key else { return None; };
        let mut evaluation = match self.sources.value(value)? {
            ValueRecipe::Evaluation(evaluation) | ValueRecipe::PackElement { evaluation, index: 0 } => self.sources.program.evaluation(*evaluation),
            _ => return None,
        };
        while matches!(self.sources.program.expression(evaluation.expression).expression.kind, ExprKind::Parenthesized(_)) {
            evaluation = self.sources.program.evaluation(*evaluation.operands.first()?);
        }
        let ExprKind::Table(fields) = &self.sources.program.expression(evaluation.expression).expression.kind else { return None };
        let mut selected = None;
        for field in fields {
            let field_key = match &field.kind {
                luanalysis_syntax::TableFieldKind::Named(name) => luanalysis_engine::LuaKey::String(name.text.as_bytes().to_vec()),
                luanalysis_syntax::TableFieldKind::Keyed(expression) => crate::native_tables::literal_key(expression)?,
                luanalysis_syntax::TableFieldKind::Sequence { index } => luanalysis_engine::LuaKey::Integer((*index).try_into().ok()?),
            };
            if &field_key != key { continue; }
            if selected.is_some() || matches!(field.kind, luanalysis_syntax::TableFieldKind::Sequence { .. }) { return None; }
            selected = Some(&field.value);
        }
        let Some(selected) = selected else { return Some(None) };
        let selected = self.sources.evaluation_for_expression(selected, input.graph)?;
        Some(Some(luanalysis_engine::NativeAliasReturnValue {
            value: self.sources.evaluation_value(selected.id),
            input: self.sources.flow_key(selected.input, input.graph.context),
        }))
    }

    fn alias_read_reference(&self, original: &flow::Reference, graph: NativeGraphKey, position: usize) -> Option<NativeReferenceKey> {
        if original.root == flow::ReferenceRoot::EnvironmentCell
            && (self.sources.environment_cell_is_immutable() || self.sources.reads_original_environment(graph, position))
        {
            self.sources.reference_key(graph, &flow::Reference {
                root: flow::ReferenceRoot::EnvironmentObject, path: original.path.clone(),
            })
        } else { self.sources.reference_key(graph, original) }
    }

    fn alias_function_value(&self, region: u32, input: NativeFlowKey) -> luanalysis_engine::NativeAliasValue {
        let graph = self.sources.graph(region, input.graph.context);
        let Some(function) = self.sources.function_for_graph(graph) else { return luanalysis_engine::NativeAliasValue::Unknown; };
        let Some(creation) = self.sources.graph_creation(graph) else { return luanalysis_engine::NativeAliasValue::Unknown; };
        luanalysis_engine::NativeAliasValue::Function { function, creation: creation.input }
    }
    /// Compare object origins at the two frozen epochs. Source paths select
    /// which original references to ask about; only the native alias query
    /// decides whether those reads denote the same allocation.
    /// `descendants: false` serves a consumer that treats descendant relations
    /// as unaffected. Without a possible proper-prefix relation, the remaining
    /// full-reference proof could only distinguish those, so it is omitted.
    fn alias_write_affects(&self,reference:NativeReferenceKey,target:NativeReferenceKey,at:NativeFlowKey,_queries:&mut dyn TypeQueries,descendants:bool)->Option<NativeWriteRelation> {
        use luanalysis_engine::NativeAliasRelation;
        let query=self.sources.source_reference(reference)?;
        let target_source=self.sources.source_reference(target)?;
        if !matches!(query.root,flow::ReferenceRoot::Binding(_)|flow::ReferenceRoot::Temporary(_)) {return None;}
        let node=self.sources.source_node(at)?;
        let flow::NodeKind::Write {address,..}=&node.kind else {return None};
        address.receiver?;
        let written=target_source.path.last()?;
        // As in Bun (`isMatchingReference`), the written receiver is this
        // prefix only when it is syntactically the same reference, after
        // parentheses and casts, and the same assignment did not replace it.
        // Different spellings are different tables: no provenance proof.
        let receiver_reference=flow::Reference {root:target_source.root.clone(),path:target_source.path[..target_source.path.len()-1].to_vec()};
        let receiver_replaced=self.sources.receiver_replaced_in_transaction(target_source,address);
        let mut possible=None;
        let mut compared=false;
        let prefixes=(0..query.path.len()).rev().chain(std::iter::once(query.path.len()));
        for length in prefixes {
            let prefix=flow::Reference {root:query.root.clone(),path:query.path[..length].to_vec()};
            if self.sources.reference_key(at.graph,&prefix).is_none() {continue};
            let suffix=&query.path[length..];
            if suffix.is_empty() && !descendants && possible.is_none() {continue;}
            // A different written key leaves this prefix unaffected whatever
            // the receiver aliases; classify the path before proving aliases.
            // A computed key may name exactly this child of the receiver.
            let affected=if suffix.is_empty() {NativeWriteRelation::Descendant}
                else if suffix.len()==1 && matches!(written,flow::Key::Dynamic(_)) && !matches!(suffix[0],flow::Key::Dynamic(_)) {NativeWriteRelation::Exact}
                else if matches!(written,flow::Key::Dynamic(_)) || matches!(suffix[0],flow::Key::Dynamic(_)) {NativeWriteRelation::Ancestor}
                else if &suffix[0]!=written {compared=true; continue;}
                else if suffix.len()==1 {NativeWriteRelation::Exact}
                else {NativeWriteRelation::Ancestor};
            let relation=if prefix==receiver_reference && !receiver_replaced {NativeAliasRelation::Same} else {NativeAliasRelation::Different};
            compared=true;
            if relation==NativeAliasRelation::Different {continue;}
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
    fn typed_write_affects(&self, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey, queries: &mut dyn TypeQueries, descendants: bool) -> NativeWriteRelation {
        let Some(query) = self.sources.source_reference(reference) else { return NativeWriteRelation::Unaffected; };
        if matches!(query.root, flow::ReferenceRoot::Pack(_)) {
            return self.sources.write_affects(reference, target, at);
        }
        if query.root == flow::ReferenceRoot::LoaderState {
            let ordinary = self.sources.write_affects(reference, target, at);
            if ordinary != NativeWriteRelation::Unaffected { return ordinary; }
        }
        if !matches!(query.root, flow::ReferenceRoot::EnvironmentObject | flow::ReferenceRoot::LoaderState) {
            let direct=self.sources.write_affects(reference,target,at);
            return self.alias_write_affects(reference,target,at,queries,descendants).unwrap_or(direct);
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
        // Ordinary checking treats a computed environment key as an unknown
        // effect, not a replacement of any particular global; strict runtime
        // effects keep it as a possible replacement.
        if matches!(path.first(), Some(flow::Key::Dynamic(_))) && !queries.strict_runtime_effects() {
            return NativeWriteRelation::Unaffected;
        }
        let target = flow::Reference { root: flow::ReferenceRoot::EnvironmentObject, path };
        self.sources.compare_write_references(query, &target, address)
    }
}

impl NativeFlowProvider for FlowAdapter<'_> {
    fn alias_environment_candidate(&self, value:NativeValueKey, _:NativeFlowKey)->bool {
        let evaluation=match self.sources.value(value) {
            Some(ValueRecipe::Evaluation(evaluation)|ValueRecipe::PackElement {evaluation,index:0})=>self.sources.program.evaluation(*evaluation),
            _=>return false,
        };
        self.sources.program.references.get(&evaluation.expression).is_some_and(|reference| {
            self.sources.environment_candidate_paths(reference).iter().any(Vec::is_empty)
        })
    }
    fn reference_alias_initial(&self, reference: NativeReferenceKey) -> luanalysis_engine::NativeAliasInitial {
        self.sources.source_reference(reference).map_or(luanalysis_engine::NativeAliasInitial::Opaque,
            |reference| self.values.original_alias_initial(reference))
    }

    fn function_alias_execution(&self, function: NativeFunctionKey, creation: NativeFlowKey) -> Option<luanalysis_engine::NativeAliasExecution> {
        self.values.original_alias_execution(function, creation)
    }

    fn alias_call_source(&self, value: NativeValueKey, input: NativeFlowKey) -> Option<luanalysis_engine::NativeAliasCall> {
        let ValueRecipe::Evaluation(evaluation) = self.sources.value(value)? else { return None; };
        let evaluation = self.sources.program.evaluation(*evaluation);
        if !matches!(self.sources.program.expression(evaluation.expression).expression.kind, ExprKind::Call { .. }) { return None; }
        let completed = self.sources.program.node(evaluation.completed);
        if !matches!(completed.kind, flow::NodeKind::Call { evaluation: called, .. } if called == evaluation.id) { return None; }
        let mut operands = evaluation.operands.iter();
        let callee = self.sources.program.evaluation(*operands.next()?);
        let callee = luanalysis_engine::NativeAliasReturnValue {
            value: self.sources.evaluation_value(callee.id),
            input: self.sources.flow_key(callee.input, input.graph.context),
        };
        let arguments = operands.map(|&operand| {
            let source = self.sources.program.evaluation(operand);
            luanalysis_engine::NativeAliasReturnValue {
                value: self.sources.evaluation_value(source.id),
                input: self.sources.flow_key(source.input, input.graph.context),
            }
        }).collect();
        Some(luanalysis_engine::NativeAliasCall {
            callee, arguments,
            execution_input: self.sources.flow_key(*completed.antecedents.first()?, input.graph.context),
        })
    }

    fn alias_node_dispatch(&self, _: NativeFlowKey) -> luanalysis_engine::NativeAliasDispatch {
        // Metatables are not traced (Luanalysis v1 semantics). A read, store,
        // operator or iteration is a raw access for provenance; declared
        // indexer and callable types describe what a metamethod implements.
        luanalysis_engine::NativeAliasDispatch::NoDispatch
    }

    fn alias_callee_type(&self, value:NativeValueKey, input:NativeFlowKey, queries:&mut dyn TypeQueries)->Type {
        self.alias_value_type(value, input, queries)
    }
    fn alias_value_type(&self, value:NativeValueKey, input:NativeFlowKey, queries:&mut dyn TypeQueries)->Type {
        let recipe=self.sources.value(value).expect("original alias callee value");
        self.values.alias_value_type(self.sources,recipe,input,queries)
    }
    fn reference_is_cell(&self, reference: NativeReferenceKey) -> bool {
        self.sources.source_reference(reference).is_some_and(|reference| {
            matches!(reference.root, flow::ReferenceRoot::Binding(_)) && reference.path.is_empty()
        })
    }
    fn reference_written_contract(&self, reference: NativeReferenceKey, queries: &mut dyn TypeQueries) -> Option<Type> {
        let source = self.sources.source_reference(reference)?;
        let flow::ReferenceRoot::Binding(binding) = source.root else { return None };
        let mut contract = self.values.written_binding_contract(binding, queries)?;
        for key in &source.path {
            let key = match key {
                flow::Key::String(value) => luanalysis_engine::LuaKey::String(value.clone()),
                flow::Key::Integer(value) => luanalysis_engine::LuaKey::Integer(*value),
                flow::Key::Float(value) => luanalysis_engine::LuaKey::Number(f64::from_bits(*value)),
                flow::Key::Boolean(value) => luanalysis_engine::LuaKey::Boolean(*value),
                flow::Key::Dynamic(_) => return None,
            };
            contract = queries.table_lookup(&contract, &luanalysis_engine::TableKey::Literal(key)).ok()?;
        }
        Some(contract)
    }
    fn value_alias_member_source(&self, value: NativeValueKey, input: NativeFlowKey, key: &luanalysis_engine::LuaKey) -> Option<luanalysis_engine::NativeAliasReturnValue> {
        // String field identity is byte-exact. Numeric equivalence and sequence
        // expansion need the native key/collision projection before they can
        // establish a unique original value source.
        self.alias_literal_member(value, input, key).flatten()
    }
    fn value_alias_member_absent(&self, value: NativeValueKey, input: NativeFlowKey, key: &luanalysis_engine::LuaKey) -> bool {
        matches!(self.alias_literal_member(value, input, key), Some(None))
    }
    fn value_alias_source(&self,value:NativeValueKey,input:NativeFlowKey)->luanalysis_engine::NativeAliasValue {
        use luanalysis_engine::NativeAliasValue;
        let Some(recipe)=self.sources.value(value) else {return NativeAliasValue::Unknown};
        let evaluation=match recipe {
            ValueRecipe::PackElement {evaluation,index} => {
                let source=self.sources.program.evaluation(*evaluation);
                return NativeAliasValue::Projection {value:self.sources.evaluation_value(*evaluation),input:self.sources.flow_key(source.input,input.graph.context),index:*index};
            }
            ValueRecipe::Evaluation(evaluation)=>self.sources.program.evaluation(*evaluation),
            ValueRecipe::Function(region) => return self.alias_function_value(*region, input),
            ValueRecipe::Nil | ValueRecipe::DeclarationNil(_)=>return NativeAliasValue::NotTable,
            ValueRecipe::NumericIteration=>return NativeAliasValue::NonNilScalar,
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
            && let Some(reference)=self.alias_read_reference(reference,at.graph,self.sources.program.expression(evaluation.expression).expression.span.start)
        {return NativeAliasValue::Reference {reference,input:at};}
        let expression=&self.sources.program.expression(evaluation.expression).expression;
        match &expression.kind {
            ExprKind::Table(_)=>NativeAliasValue::Allocation,
            ExprKind::Function(function) => self.sources.program.functions.get(&(function.span.start, function.span.end))
                .map_or(NativeAliasValue::Unknown, |&region| self.alias_function_value(region, input)),
            ExprKind::Call {..}=> {
                let Some(callee)=evaluation.operands.first().copied() else {return NativeAliasValue::Unknown};
                let source=self.sources.program.evaluation(callee);
                NativeAliasValue::Call {callee:self.sources.evaluation_value(callee),input:self.sources.flow_key(source.input,input.graph.context)}
            }
            // A method evaluation's first operand is its receiver, not the
            // selected method. It cannot serve as an implementation origin.
            ExprKind::MethodCall {..}=>NativeAliasValue::Unknown,
            ExprKind::Nil=>NativeAliasValue::NotTable,
            ExprKind::Number(_)|ExprKind::String(_)|ExprKind::Boolean(_)=>NativeAliasValue::NonNilScalar,
            _=>NativeAliasValue::Unknown,
        }
    }
    fn value_alias_environment_member(&self, value: NativeValueKey, input: NativeFlowKey) -> Option<luanalysis_engine::NativeAliasEnvironmentMember> {
        let ValueRecipe::Evaluation(evaluation) = self.sources.value(value)? else { return None };
        let evaluation = self.sources.program.evaluation(*evaluation);
        let expression = &self.sources.program.expression(evaluation.expression).expression;
        match &expression.kind {
            ExprKind::Field { .. } => {},
            ExprKind::Index { index, .. } if super::native_tables::literal_key(index).is_some() => {},
            _ => return None,
        }
        let receiver = self.sources.program.evaluation(*evaluation.operands.first()?);
        // This is only a candidate address. A matching spelling or a static
        // assertion cannot prove that the frozen receiver is the environment.
        let reference = self.sources.environment_member_reference(input.graph, expression)?;
        let lookup = *self.sources.program.node(evaluation.completed).antecedents.first()?;
        Some(luanalysis_engine::NativeAliasEnvironmentMember {
            receiver: luanalysis_engine::NativeAliasReturnValue {
                value: self.sources.evaluation_value(receiver.id),
                input: self.sources.flow_key(receiver.input, input.graph.context),
            },
            reference,
            input: self.sources.flow_key(lookup, input.graph.context),
        })
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
        let Some(source) = self.sources.source_reference(reference) else { return false; };
        match source.root {
            flow::ReferenceRoot::EnvironmentCell | flow::ReferenceRoot::LoaderState | flow::ReferenceRoot::EnvironmentObject => true,
            // A captured written contract may request the native creation
            // bridge; its owner/write/last-position guards remain authoritative.
            // Inferred runtime objects keep their existing continuation path.
            // As TypeScript 5.4 keeps narrowing in closures created after a variable's last
            // assignment, an inferred local does too. A table built by assignments enters a
            // closure with every stored field instead.
            flow::ReferenceRoot::Binding(binding) if source.path.is_empty() => self.sources.binding_owners.get(&binding).is_some_and(|&owner| owner != reference.graph.region)
                && (self.values.binding_has_written_contract(binding)
                    || !self.sources.initialized_by_table(binding) && !self.sources.initialized_by_standard_name(binding)),
            _ => false,
        }
    }
    fn loader_never_mutated(&self, _: NativeGraphKey) -> bool { self.values.loader_never_mutated() }
    fn value_lexical_standard(&self, value: NativeValueKey, input: NativeFlowKey) -> luanalysis_engine::NativeLexicalStandard {
        let Some(ValueRecipe::Evaluation(evaluation)) = self.sources.value(value) else { return luanalysis_engine::NativeLexicalStandard::Unknown; };
        self.values.lexical_standard(self.sources, *evaluation, input.graph)
    }
    fn value_literal_truthiness(&self, value: NativeValueKey) -> Option<bool> {
        let Some(ValueRecipe::Evaluation(evaluation)) = self.sources.value(value) else { return None; };
        let mut expression = &self.sources.program.expression(self.sources.program.evaluation(*evaluation).expression).expression;
        while let ExprKind::Parenthesized(inner) = &expression.kind { expression = inner; }
        match &expression.kind {
            ExprKind::Nil => Some(false),
            ExprKind::Boolean(value) => Some(*value),
            ExprKind::Number(_) | ExprKind::String(_) | ExprKind::Table(_) | ExprKind::Function(_) => Some(true),
            _ => None,
        }
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
        self.typed_write_affects(reference, target, at, queries, true)
    }
    fn write_affects_ignoring_descendants(&self, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey, queries: &mut dyn TypeQueries) -> NativeWriteRelation {
        self.typed_write_affects(reference, target, at, queries, false)
    }
    fn call_invalidates_reference(&self, reference: NativeReferenceKey, at: NativeFlowKey, queries: &mut dyn TypeQueries) -> bool {
        if !self.sources.source_reference(reference).is_some_and(|reference| reference.root == flow::ReferenceRoot::LoaderState) {
            return false;
        }
        let Some(node) = self.sources.source_node(at) else { return true; };
        if self.sources.loader_mutations.contains(&(node.span.start, node.span.end)) { return true; }
        let Some(NativeFlowNode::Call { value, input, .. }) = self.sources.node(at) else { return true; };
        queries.flow_call_preservation(self, reference, value, input)
            != luanalysis_engine::NativeCallPreservation::Preserved
    }
    fn known_standard_loader_write(&self, at: NativeFlowKey) -> bool {
        self.sources.source_node(at).is_some_and(|node| self.sources.loader_mutations.contains(&(node.span.start, node.span.end)))
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
    fn write_index_value(&self, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey) -> Option<(NativeValueKey, NativeFlowKey)> {
        if reference.graph != at.graph || target.graph != at.graph { return None; }
        let target = self.sources.source_reference(target)?;
        // The existing frozen-receiver relation selected this descendant.
        // Transport its key without rediscovering aliases or re-reading the
        // receiver after a key expression may have rebound its source path.
        if !matches!(target.path.last(), Some(flow::Key::Dynamic(_))) { return None; }
        let flow::NodeKind::Write { address, .. } = &self.sources.source_node(at)?.kind else { return None; };
        let evaluation = self.sources.program.evaluation(address.key?);
        Some((self.sources.evaluation_value(evaluation.id), self.sources.flow_key(evaluation.input, at.graph.context)))
    }
    fn write_index_appends(&self, at: NativeFlowKey) -> bool {
        let Some(flow::Node { kind: flow::NodeKind::Write { address, .. }, .. }) = self.sources.source_node(at) else { return false };
        let (Some(key), Some(receiver)) = (address.key, address.receiver) else { return false };
        let expression = |evaluation| &self.sources.program.expression(self.sources.program.evaluation(evaluation).expression).expression;
        appends(expression(key), expression(receiver))
    }
    fn runtime_table_store_in_scope(&self, declaration: luanalysis_engine::NativeTypeDeclarationKey, at: NativeFlowKey) -> bool {
        self.values.runtime_table_store_in_scope(declaration, at)
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
    fn reference_member_path(&self, reference: NativeReferenceKey, member: NativeReferenceKey) -> Option<Vec<luanalysis_engine::LuaKey>> {
        if reference.graph != member.graph { return None; }
        let reference = self.sources.source_reference(reference)?;
        let member = self.sources.source_reference(member)?;
        if reference.root != member.root || member.path.len() <= reference.path.len()
            || !member.path.starts_with(&reference.path) { return None; }
        member.path[reference.path.len()..].iter().map(|key| Some(match key {
            flow::Key::String(value) => luanalysis_engine::LuaKey::String(value.clone()),
            flow::Key::Boolean(value) => luanalysis_engine::LuaKey::Boolean(*value),
            flow::Key::Integer(value) => luanalysis_engine::LuaKey::Integer(*value),
            flow::Key::Float(value) => luanalysis_engine::LuaKey::Number(f64::from_bits(*value)),
            flow::Key::Dynamic(_) => return None,
        })).collect()
    }
    fn reference_proper_prefixes(&self, reference: NativeReferenceKey) -> Option<Vec<NativeReferenceKey>> {
        let original = self.sources.source_reference(reference)?;
        if !matches!(original.root, flow::ReferenceRoot::Binding(_) | flow::ReferenceRoot::Temporary(_)) || original.path.is_empty() { return None; }
        (0..original.path.len()).map(|length| self.sources.reference_key(reference.graph, &flow::Reference {
            root: original.root.clone(), path: original.path[..length].to_vec(),
        })).collect()
    }
    fn write_alias_receiver(&self, at: NativeFlowKey) -> Option<luanalysis_engine::NativeAliasReturnValue> {
        let flow::NodeKind::Write { address, .. } = &self.sources.source_node(at)?.kind else { return None };
        let receiver = self.sources.program.evaluation(address.receiver?);
        Some(luanalysis_engine::NativeAliasReturnValue {
            value: self.sources.evaluation_value(receiver.id),
            input: self.sources.flow_key(receiver.input, at.graph.context),
        })
    }
    fn read_alias_receiver(&self, at: NativeFlowKey) -> Option<luanalysis_engine::NativeAliasReturnValue> {
        let flow::NodeKind::Eval(evaluation) = &self.sources.source_node(at)?.kind else { return None };
        let evaluation = self.sources.program.evaluation(*evaluation);
        if !matches!(self.sources.program.expression(evaluation.expression).expression.kind, ExprKind::Field { .. } | ExprKind::Index { .. }) { return None }
        let receiver = self.sources.program.evaluation(*evaluation.operands.first()?);
        Some(luanalysis_engine::NativeAliasReturnValue {
            value: self.sources.evaluation_value(receiver.id),
            input: self.sources.flow_key(receiver.input, at.graph.context),
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

impl NativeFlowRecipes {
    fn loader_address_invalidates_with_capability(&self, address: &flow::CapturedAddress, capability: bool) -> bool {
        if !self.loader_mutations.contains(&(address.span.start, address.span.end)) { return false; }
        if capability && address.reference.root == flow::ReferenceRoot::EnvironmentCell {
            match address.reference.path.as_slice() {
                [] => return false,
                [flow::Key::String(name)] if name == b"require" => return false,
                [flow::Key::String(root), flow::Key::String(name)] if root == b"_G" && name == b"require" => return false,
                _ => {}
            }
        }
        true
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
}

/// `#receiver + 1` (or `1 + #receiver`): the key that appends to `receiver`'s sequence.
fn appends(key: &Expr, receiver: &Expr) -> bool {
    fn place(left: &Expr, right: &Expr) -> bool {
        match (&left.kind, &right.kind) {
            (ExprKind::Parenthesized(inner), _) => place(inner, right),
            (_, ExprKind::Parenthesized(inner)) => place(left, inner),
            (ExprKind::Name(left), ExprKind::Name(right)) => left.text == right.text,
            (ExprKind::Field { base: left, name: a }, ExprKind::Field { base: right, name: b }) => a.text == b.text && place(left, right),
            _ => false,
        }
    }
    let length = |expression: &Expr| matches!(&expression.kind, ExprKind::Unary { op: luanalysis_syntax::UnaryOperator::Length, value, .. } if place(value, receiver));
    let one = |expression: &Expr| matches!(&expression.kind, ExprKind::Number(text) if text == "1");
    let mut key = key;
    while let ExprKind::Parenthesized(inner) = &key.kind { key = inner; }
    matches!(&key.kind, ExprKind::Binary { op: BinaryOperator::Add, left, right, .. } if length(left) && one(right) || one(left) && length(right))
}

/// The expression a write stores: its value, or the call whose first result it is
/// (`local t = setmetatable({}, mt)`).
fn written_expression(value: &flow::ValueDemand) -> Option<flow::EvaluationId> {
    match value {
        flow::ValueDemand::Evaluation(evaluation) | flow::ValueDemand::PackElement { evaluation, index: 0 } => Some(*evaluation),
        _ => None,
    }
}

fn original_environment_until(file: &luanalysis_syntax::File, bound: &super::bind::BoundFile) -> Option<usize> {
    use luanalysis_syntax::StmtKind;
    fn has_label(statements: &[luanalysis_syntax::Stmt]) -> bool {
        statements.iter().any(|statement| match &statement.kind {
            StmtKind::Label(_) => true,
            StmtKind::Block { body, .. } | StmtKind::While { body, .. } | StmtKind::Repeat { body, .. }
            | StmtKind::NumericFor { body, .. } | StmtKind::GenericFor { body, .. } => has_label(body),
            StmtKind::If { branches, else_branch } => branches.iter().any(|branch| has_label(&branch.body))
                || else_branch.as_ref().is_some_and(|branch| has_label(&branch.body)),
            _ => false,
        })
    }
    if has_label(&file.statements) { return None }
    bound.environment_rebindings().iter().map(|span| {
        file.statements.iter().find(|statement| matches!(&statement.kind,
            StmtKind::Assign { targets, .. } if targets.iter().any(|target| target.span == *span)))
            .map(|statement| statement.span.start)
    }).collect::<Option<Vec<_>>>()?.into_iter().min()
}
