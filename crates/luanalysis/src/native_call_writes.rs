//! What an ordinary call may write that its caller observes, from the original
//! source alone.
//!
//! As in TypeScript, a call keeps the caller's flow facts, with one exception:
//! a function the call names, or a function passed to it, visibly writes the
//! fact's reference (a captured local, a global, or a member reached from a
//! captured value or a parameter). Such a fact cannot outlive the call; the
//! reference has its declared type after it. Callees are the original
//! definitions in this file, found as v1 resolves names: a callee from another
//! file or a computed one writes nothing visible. As elsewhere in ordinary
//! checking, a store through a computed key is an unknown effect, and the
//! environment itself and its standard capabilities keep their own effect
//! policy. Nothing here infers a type or follows a value's provenance, so a call
//! costs a lookup in a summary computed once per source.

use super::{flow, NativeFlowRecipes};
use crate::bind::BindingId;
use crate::collections::{HashMap, HashSet};
use luanalysis_syntax::{ExprKind, TableFieldKind};
use std::collections::VecDeque;

/// Longer written paths are kept as their prefix, which describes more
/// references, never fewer.
const MAX_WRITTEN_PATH: usize = 4;

/// Literal member keys below a root; a computed key is never recorded.
type WrittenPath = Vec<flow::Key>;

fn written_path(keys: &[flow::Key]) -> Option<WrittenPath> {
    (!keys.iter().any(|key| matches!(key, flow::Key::Dynamic(_))))
        .then(|| keys.iter().take(MAX_WRITTEN_PATH).cloned().collect())
}

/// A written key matches the same literal, or a computed key read.
fn same_key(written: &flow::Key, read: &flow::Key) -> bool {
    written == read || matches!(read, flow::Key::Dynamic(_))
}

/// Whether a write of `written` changes the value of a reference with `path`
/// below the same root: it writes the reference or one of its ancestors.
fn writes_path(written: &[flow::Key], path: &[flow::Key]) -> bool {
    written.len() <= path.len() && written.iter().zip(path).all(|(written, read)| same_key(written, read))
}

/// Rebinding the environment (`_ENV`, `_G`) or one of its standard
/// capabilities follows the standard-environment effect policy instead.
fn environment_policy(root: &flow::ReferenceRoot, path: &[flow::Key]) -> bool {
    matches!(root, flow::ReferenceRoot::EnvironmentCell | flow::ReferenceRoot::EnvironmentObject)
        && match path.first() {
            None => true,
            Some(flow::Key::String(name)) => name == b"_G" || crate::bind::STANDARD_NAMES.iter().any(|standard| standard.as_bytes() == name.as_slice()),
            Some(_) => false,
        }
}

#[derive(Clone, Default)]
struct FunctionWrites {
    /// Paths written below captured bindings and environment roots, including
    /// the empty path of a rebinding.
    roots: HashMap<flow::ReferenceRoot, HashSet<WrittenPath>>,
    /// Member paths written below each parameter; the implicit receiver is
    /// parameter zero.
    parameters: HashMap<usize, HashSet<WrittenPath>>,
}

impl FunctionWrites {
    fn root(&mut self, root: &flow::ReferenceRoot, path: WrittenPath) -> bool {
        !environment_policy(root, &path) && self.roots.entry(root.clone()).or_default().insert(path)
    }
    fn parameter(&mut self, index: usize, path: WrittenPath) -> bool {
        self.parameters.entry(index).or_default().insert(path)
    }
}

#[derive(Clone)]
struct CallTargets {
    /// The original definitions the callee may be, called with `arguments`.
    callees: Vec<u32>,
    /// Functions passed as arguments, which the callee may run.
    callbacks: Vec<u32>,
    /// Each argument's original reference; a method call's receiver is first.
    arguments: Vec<Option<flow::Reference>>,
}

#[derive(Clone, Default)]
pub(super) struct CallWrites {
    /// Transitive writes of each function graph.
    functions: Vec<FunctionWrites>,
    /// Calls with at least one original callee or function argument.
    calls: HashMap<flow::EvaluationId, CallTargets>,
}

impl CallWrites {
    pub(super) fn may_write(&self, call: flow::EvaluationId, reference: &flow::Reference) -> bool {
        let Some(call) = self.calls.get(&call) else { return false; };
        let writes = |paths: Option<&HashSet<WrittenPath>>, path: &[flow::Key]| {
            paths.is_some_and(|paths| paths.iter().any(|written| writes_path(written, path)))
        };
        call.callees.iter().chain(&call.callbacks)
            .any(|&function| writes(self.functions[function as usize].roots.get(&reference.root), &reference.path))
            || call.arguments.iter().enumerate().any(|(index, argument)| argument.as_ref().is_some_and(|argument| {
                argument.root == reference.root
                    && written_path(&argument.path).is_some_and(|argument| writes_path(&argument, &reference.path))
                    && call.callees.iter().any(|&function| writes(
                        self.functions[function as usize].parameters.get(&index),
                        &reference.path[argument.path.len()..],
                    ))
            }))
    }
}

impl NativeFlowRecipes {
    pub(super) fn compute_call_writes(&self) -> CallWrites {
        let program = &self.program;
        let mut result = CallWrites { functions: vec![FunctionWrites::default(); program.graphs.len()], calls: HashMap::default() };
        // Original function definitions: a declaration or store of a function,
        // or a function field of a stored table constructor.
        let mut definitions: HashMap<flow::Reference, Vec<u32>> = HashMap::default();
        let mut methods: HashMap<Vec<u8>, Vec<u32>> = HashMap::default();
        let mut receivers: HashMap<BindingId, flow::Reference> = HashMap::default();
        for node in program.graphs.iter().flat_map(|graph| &graph.nodes) {
            let flow::NodeKind::Write { address, value, .. } = &node.kind else { continue; };
            for (key, function) in self.written_functions(value) {
                let mut reference = address.reference.clone();
                reference.path.extend(key);
                definitions.entry(reference.clone()).or_default().push(function);
                // `function owner:name()` is called on its owner's values: its
                // receiver's members are the owner's, and other receivers
                // calling `:name` may be instances of it.
                if let Some(&receiver) = self.implicit_receivers.get(&function)
                    && let Some(flow::Key::String(name)) = reference.path.pop() {
                    methods.entry(name).or_default().push(function);
                    receivers.insert(receiver, reference);
                }
            }
        }
        let resolve = |reference: &flow::Reference| -> Vec<u32> {
            if let Some(functions) = definitions.get(reference) { return functions.clone(); }
            let flow::ReferenceRoot::Binding(binding) = &reference.root else { return Vec::new(); };
            let Some(owner) = receivers.get(binding) else { return Vec::new(); };
            let mut member = owner.clone();
            member.path.extend(reference.path.iter().cloned());
            definitions.get(&member).cloned().unwrap_or_default()
        };
        let owner = |binding: &BindingId| self.binding_owners.get(binding).copied();
        let mut calls: Vec<Vec<flow::EvaluationId>> = vec![Vec::new(); program.graphs.len()];
        for graph in &program.graphs {
            let writes = &mut result.functions[graph.index as usize];
            for node in &graph.nodes {
                match &node.kind {
                    flow::NodeKind::Write { address, .. } => {
                        let reference = &address.reference;
                        let Some(path) = written_path(&reference.path) else { continue; };
                        // A global is read as a member of the original environment.
                        for path in self.environment_candidate_paths(reference).iter().filter_map(|path| written_path(path)) {
                            writes.root(&flow::ReferenceRoot::EnvironmentObject, path);
                        }
                        match &reference.root {
                            flow::ReferenceRoot::Binding(binding) if owner(binding) == Some(graph.index) => {
                                if let Some(&index) = self.parameter_indices.get(binding) && !path.is_empty() {
                                    writes.parameter(index, path);
                                }
                            },
                            root @ (flow::ReferenceRoot::Binding(_) | flow::ReferenceRoot::EnvironmentCell
                                | flow::ReferenceRoot::EnvironmentObject) => {
                                writes.root(root, path);
                            },
                            _ => {},
                        }
                    },
                    flow::NodeKind::Call { evaluation, .. } => {
                        let Some(targets) = self.call_targets(*evaluation, &resolve, &methods) else { continue; };
                        calls[graph.index as usize].push(*evaluation);
                        result.calls.insert(*evaluation, targets);
                    },
                    _ => {},
                }
            }
        }
        // Propagate callee writes to callers until no summary grows. Summaries
        // only grow and paths are bounded, so this terminates.
        let mut callers = vec![Vec::new(); program.graphs.len()];
        for (caller, evaluations) in calls.iter().enumerate() {
            for evaluation in evaluations {
                let targets = &result.calls[evaluation];
                for &function in targets.callees.iter().chain(&targets.callbacks) {
                    let list: &mut Vec<u32> = &mut callers[function as usize];
                    if list.last() != Some(&(caller as u32)) { list.push(caller as u32); }
                }
            }
        }
        let mut queue: VecDeque<u32> = (0..program.graphs.len() as u32).collect();
        let mut queued = vec![true; program.graphs.len()];
        while let Some(caller) = queue.pop_front() {
            queued[caller as usize] = false;
            let mut roots = Vec::new();
            let mut parameters = Vec::new();
            for evaluation in &calls[caller as usize] {
                let targets = &result.calls[evaluation];
                for (callback, &function) in targets.callees.iter().map(|function| (false, function))
                    .chain(targets.callbacks.iter().map(|function| (true, function))) {
                    if function == caller { continue; }
                    let callee = &result.functions[function as usize];
                    for (root, paths) in &callee.roots {
                        // A write of the caller's own local is invisible to its callers.
                        if matches!(root, flow::ReferenceRoot::Binding(binding) if owner(binding) == Some(caller)) { continue; }
                        roots.extend(paths.iter().map(|path| (root.clone(), path.clone())));
                    }
                    // A callback's arguments are supplied by its unknown caller.
                    if callback { continue; }
                    for (&index, paths) in &callee.parameters {
                        let Some(Some(argument)) = targets.arguments.get(index) else { continue; };
                        let Some(prefix) = written_path(&argument.path) else { continue; };
                        let full = |path: &WrittenPath| prefix.iter().chain(path).take(MAX_WRITTEN_PATH).cloned().collect::<WrittenPath>();
                        match &argument.root {
                            flow::ReferenceRoot::Binding(binding) if owner(binding) == Some(caller) => {
                                if let Some(&index) = self.parameter_indices.get(binding) {
                                    parameters.extend(paths.iter().map(|path| (index, full(path))));
                                }
                            },
                            root @ (flow::ReferenceRoot::Binding(_) | flow::ReferenceRoot::EnvironmentCell
                                | flow::ReferenceRoot::EnvironmentObject) => {
                                roots.extend(paths.iter().map(|path| (root.clone(), full(path))));
                            },
                            _ => {},
                        }
                    }
                }
            }
            let writes = &mut result.functions[caller as usize];
            let mut changed = false;
            for (root, path) in roots { changed |= writes.root(&root, path); }
            for (index, path) in parameters { changed |= writes.parameter(index, path); }
            if changed {
                for &function in &callers[caller as usize] {
                    if !std::mem::replace(&mut queued[function as usize], true) { queue.push_back(function); }
                }
            }
        }
        result
    }

    /// The functions a stored value directly provides, by the member key
    /// below the written reference (none for the value itself).
    fn written_functions(&self, value: &flow::ValueDemand) -> Vec<(Option<flow::Key>, u32)> {
        let evaluation = match value {
            flow::ValueDemand::Function(graph) => return vec![(None, *graph)],
            flow::ValueDemand::Evaluation(evaluation) => *evaluation,
            _ => return Vec::new(),
        };
        let expression = &self.program.expression(self.program.evaluation(evaluation).expression).expression;
        match &expression.kind {
            ExprKind::Table(fields) => fields.iter().filter_map(|field| {
                let TableFieldKind::Named(name) = &field.kind else { return None; };
                Some((Some(flow::Key::String(name.text.as_bytes().to_vec())), self.function_literal(&field.value)?))
            }).collect(),
            _ => self.function_literal(expression).map(|graph| vec![(None, graph)]).unwrap_or_default(),
        }
    }

    fn function_literal(&self, mut expression: &luanalysis_syntax::Expr) -> Option<u32> {
        while let ExprKind::Parenthesized(inner) = &expression.kind { expression = inner; }
        let ExprKind::Function(function) = &expression.kind else { return None; };
        self.program.functions.get(&(function.span.start, function.span.end)).copied()
    }

    fn call_targets(&self, evaluation: flow::EvaluationId, resolve: &dyn Fn(&flow::Reference) -> Vec<u32>, methods: &HashMap<Vec<u8>, Vec<u32>>) -> Option<CallTargets> {
        let program = &self.program;
        let evaluation = program.evaluation(evaluation);
        let reference = |operand: &flow::EvaluationId| program.references.get(&program.evaluation(*operand).expression).cloned();
        let (callee, arguments, method) = match &program.expression(evaluation.expression).expression.kind {
            ExprKind::Call { .. } => (
                evaluation.operands.first().and_then(reference),
                evaluation.operands.get(1..).unwrap_or_default(),
                None,
            ),
            ExprKind::MethodCall { name, .. } => (
                evaluation.operands.first().and_then(reference).map(|mut receiver| {
                    receiver.path.push(flow::Key::String(name.text.as_bytes().to_vec()));
                    receiver
                }),
                evaluation.operands.as_slice(),
                Some(name.text.as_bytes()),
            ),
            _ => return None,
        };
        let mut callees = callee.as_ref().map(resolve).unwrap_or_default();
        if callees.is_empty() && let Some(name) = method {
            callees = methods.get(name).cloned().unwrap_or_default();
        }
        let first_argument = usize::from(method.is_some());
        let callbacks: Vec<u32> = arguments.iter().skip(first_argument).flat_map(|argument| {
            let expression = &program.expression(program.evaluation(*argument).expression).expression;
            match self.function_literal(expression) {
                Some(function) => vec![function],
                None => reference(argument).map(|argument| resolve(&argument)).unwrap_or_default(),
            }
        }).collect();
        (!callees.is_empty() || !callbacks.is_empty()).then(|| CallTargets {
            callees,
            callbacks,
            arguments: arguments.iter().map(reference).collect(),
        })
    }
}
