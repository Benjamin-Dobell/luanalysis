use super::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

const SNAPSHOT: u64 = 12_431;

fn graph(region: u32, context: u64) -> NativeGraphKey {
    NativeGraphKey { snapshot: SNAPSHOT, region, context }
}

fn at(region: u32, node: u32, context: u64) -> NativeFlowKey {
    NativeFlowKey { graph: graph(region, context), node }
}

fn reference(graph: NativeGraphKey, reference: u32) -> NativeReferenceKey {
    NativeReferenceKey { graph, reference }
}

fn value(index: u32) -> NativeValueKey {
    NativeValueKey { snapshot: SNAPSHOT, index }
}

fn function(index: u32) -> NativeFunctionKey {
    NativeFunctionKey { snapshot: SNAPSHOT, index }
}

/// Immutable original recipes. All type/body inference callbacks deliberately
/// panic: callable structure must never substitute for a runtime callee origin.
struct Recipes {
    routed: bool,
    nodes: HashMap<NativeFlowKey, NativeFlowNode>,
    values: HashMap<(NativeValueKey, NativeFlowKey), NativeAliasValue>,
    calls: HashMap<(NativeValueKey, NativeFlowKey), NativeAliasCall>,
    executions: HashMap<(NativeFunctionKey, NativeFlowKey), NativeAliasExecution>,
    barriers: HashSet<NativeFlowKey>,
    aliased_writes: HashSet<NativeFlowKey>,
    execution_requests: RefCell<Vec<NativeFunctionKey>>,
    reentry: Option<(NativeFlowKey, NativeReferenceKey, NativeValueKey, NativeFlowKey)>,
    reentry_results: RefCell<Vec<NativeCallPreservation>>,
}

impl Recipes {
    fn new(routed: bool) -> Self {
        Self {
            routed,
            nodes: HashMap::from([(at(0, 0, 0), NativeFlowNode::Start)]),
            values: HashMap::new(),
            calls: HashMap::new(),
            executions: HashMap::new(),
            barriers: HashSet::new(),
            aliased_writes: HashSet::new(),
            execution_requests: RefCell::new(Vec::new()),
            reentry: None,
            reentry_results: RefCell::new(Vec::new()),
        }
    }

    fn input(&mut self, node: u32, context: u64) -> NativeFlowKey {
        let input = at(0, node, context);
        self.nodes.entry(at(0, 0, context)).or_insert(NativeFlowNode::Start);
        if node != 0 {
            self.nodes.insert(input, NativeFlowNode::Pass { before: at(0, 0, context) });
        }
        input
    }

    fn execution(&mut self, index: u32, creation: NativeFlowKey, nodes: Vec<NativeFlowNode>) {
        let context = creation.graph.context;
        let terminal = nodes.len() as u32 - 1;
        for (node, recipe) in nodes.into_iter().enumerate() {
            self.nodes.insert(at(index, node as u32, context), recipe);
        }
        self.executions.insert((function(index), creation), NativeAliasExecution {
            entry: at(index, 0, context),
            terminals: vec![at(index, terminal, context)],
        });
    }

    fn writing_function(&mut self, index: u32, creation: NativeFlowKey, target: u32) {
        let context = creation.graph.context;
        self.execution(index, creation, vec![
            NativeFlowNode::Start,
            NativeFlowNode::Write {
                before: at(index, 0, context),
                target: reference(graph(index, context), target),
                value: value(900),
                input: at(index, 0, context),
            },
            NativeFlowNode::Return { before: at(index, 1, context) },
        ]);
    }

    fn call(&mut self, call: u32, input: NativeFlowKey, callee: Option<u32>, creation: NativeFlowKey) {
        let selected = value(call + 10_000);
        self.values.insert((selected, input), callee.map_or(NativeAliasValue::Unknown, |index| {
            NativeAliasValue::Function { function: function(index), creation }
        }));
        self.calls.insert((value(call), input), NativeAliasCall {
            callee: NativeAliasReturnValue { value: selected, input },
            arguments: Vec::new(),
            execution_input: input,
        });
    }

    fn calling_function(&mut self, index: u32, creation: NativeFlowKey, callee: u32) {
        let context = creation.graph.context;
        let nested_call = 100 + index;
        self.call(nested_call, at(index, 0, context), Some(callee), creation);
        self.execution(index, creation, vec![
            NativeFlowNode::Start,
            NativeFlowNode::Call {
                before: at(index, 0, context),
                value: value(nested_call),
                input: at(index, 0, context),
            },
            NativeFlowNode::Return { before: at(index, 1, context) },
        ]);
    }

    fn queried(input: NativeFlowKey) -> NativeReferenceKey {
        reference(input.graph, 0)
    }
}

impl NativeTypeProvider for Recipes {
    fn project_flow_provider(&self) -> Option<&dyn NativeFlowProvider> {
        self.routed.then_some(self as &dyn NativeFlowProvider)
    }

    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        self.executions.keys().any(|(function, _)| *function == key)
    }

    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        panic!("a call-preservation certificate must not inspect callable parameters")
    }

    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> {
        panic!("a call-preservation certificate must not inspect a declared return type")
    }

    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("a call-preservation certificate must not infer a callable result")
    }

    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("origin recursion belongs to the certificate query, not return inference")
    }

    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("origin traversal must not enter return inference")
    }
}

impl NativeFlowProvider for Recipes {
    fn value_alias_source(&self, value: NativeValueKey, input: NativeFlowKey) -> NativeAliasValue {
        if value.index == 900 { return NativeAliasValue::NotTable; }
        self.values.get(&(value, input)).copied().unwrap_or(NativeAliasValue::Unknown)
    }

    fn alias_call_source(&self, value: NativeValueKey, input: NativeFlowKey) -> Option<NativeAliasCall> {
        self.calls.get(&(value, input)).cloned()
    }

    fn alias_node_dispatch(&self, at: NativeFlowKey) -> NativeAliasDispatch {
        if self.barriers.contains(&at) { NativeAliasDispatch::Unknown }
        else { NativeAliasDispatch::NoDispatch }
    }

    fn function_alias_execution(&self, function: NativeFunctionKey, creation: NativeFlowKey) -> Option<NativeAliasExecution> {
        self.execution_requests.borrow_mut().push(function);
        self.executions.get(&(function, creation)).cloned()
    }

    fn reference_in_graph(&self, source: NativeReferenceKey, graph: NativeGraphKey) -> Option<NativeReferenceKey> {
        (self.contains_reference(source) && self.contains_graph(graph)).then_some(reference(graph, source.reference))
    }

    fn contains_graph(&self, graph: NativeGraphKey) -> bool {
        self.nodes.contains_key(&NativeFlowKey { graph, node: 0 })
    }

    fn contains_reference(&self, reference: NativeReferenceKey) -> bool {
        reference.reference < 2 && self.contains_graph(reference.graph)
    }

    fn node(&self, at: NativeFlowKey) -> Option<NativeFlowNode> {
        self.nodes.get(&at).cloned()
    }

    fn is_shared(&self, _: NativeFlowKey) -> bool { false }

    fn write_affects(&self, query: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey) -> NativeWriteRelation {
        if query == target || self.aliased_writes.contains(&at) { NativeWriteRelation::Exact }
        else { NativeWriteRelation::Unaffected }
    }

    fn write_affects_with_types(&self, query: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey, queries: &mut dyn TypeQueries) -> NativeWriteRelation {
        if let Some((trigger, reference, call, input)) = self.reentry
            && at == trigger
        {
            let result = queries.flow_call_preservation(self, reference, call, input);
            self.reentry_results.borrow_mut().push(result);
        }
        self.write_affects(query, target, at)
    }

    fn value_type(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type {
        panic!("original runtime origins must not be discovered through expression types")
    }

    fn call_effects(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeCallEffects {
        panic!("call preservation must not borrow result/reachability effects as purity")
    }

    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) {
        panic!("the original fixture graph is valid: {error:?}")
    }
}

fn not_preserved(result: NativeCallPreservation) {
    assert!(matches!(result, NativeCallPreservation::Unknown | NativeCallPreservation::Pending), "{result:?}");
}

#[test]
fn original_function_body_preserves_unrelated_locals_but_rejects_captured_writes() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || {
            let mut recipes = Recipes::new(routed);
            let input = recipes.input(0, 0);
            recipes.writing_function(1, input, 1);
            recipes.writing_function(2, input, 0);
            recipes.call(0, input, Some(1), input);
            recipes.call(1, input, Some(2), input);
            recipes
        }, |recipes, queries| {
            let input = at(0, 0, 0);
            assert_eq!(queries.flow_call_preservation(recipes, Recipes::queried(input), value(0), input), NativeCallPreservation::Preserved);
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(10_000), input), NativeAliasIntrinsic::Unknown);
            not_preserved(queries.flow_call_preservation(recipes, Recipes::queried(input), value(1), input));
            assert_eq!(queries.flow_call_preservation(recipes, Recipes::queried(input), value(0), input), NativeCallPreservation::Preserved);
        });
    }
}

#[test]
fn structural_callable_contract_does_not_establish_a_runtime_callee() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || {
            let mut recipes = Recipes::new(routed);
            let input = recipes.input(0, 0);
            recipes.call(0, input, None, input);
            recipes
        }, |recipes, queries| {
            let input = at(0, 0, 0);
            let callable = Type::Function(FunctionType { params: Vec::new(), returns: Box::new(Type::Number) });
            assert!(!queries.call_signature_types(&callable).is_empty());
            // Unresolved, so it preserves what it does not visibly write; no
            // body is executed on the strength of a structural contract.
            assert_eq!(queries.flow_call_preservation(recipes, Recipes::queried(input), value(0), input), NativeCallPreservation::Preserved);
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(10_000), input), NativeAliasIntrinsic::Unknown);
            assert!(recipes.execution_requests.borrow().is_empty());
        });
    }
}

#[test]
fn creating_a_nested_function_does_not_execute_its_mutating_body() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || {
            let mut recipes = Recipes::new(routed);
            let input = recipes.input(0, 0);
            recipes.writing_function(2, at(1, 0, 0), 0);
            // Original CreateFunction is a dispatch-free Pass in the core graph.
            recipes.execution(1, input, vec![NativeFlowNode::Start,
                NativeFlowNode::Pass { before: at(1, 0, 0) },
                NativeFlowNode::Return { before: at(1, 1, 0) }]);
            recipes.values.insert((value(50), at(1, 0, 0)), NativeAliasValue::Function { function: function(2), creation: at(1, 0, 0) });
            recipes.call(0, input, Some(1), input);
            recipes
        }, |recipes, queries| {
            let input = at(0, 0, 0);
            assert_eq!(queries.flow_call_preservation(recipes, Recipes::queried(input), value(0), input), NativeCallPreservation::Preserved);
            assert!(!recipes.execution_requests.borrow().contains(&function(2)));
        });
    }
}

#[test]
fn a_known_nested_call_must_certify_the_body_that_actually_executes() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || {
            let mut recipes = Recipes::new(routed);
            let input = recipes.input(0, 0);
            recipes.writing_function(2, input, 0);
            recipes.calling_function(1, input, 2);
            recipes.call(0, input, Some(1), input);
            recipes
        }, |recipes, queries| {
            let input = at(0, 0, 0);
            not_preserved(queries.flow_call_preservation(recipes, Recipes::queried(input), value(0), input));
            assert!(recipes.execution_requests.borrow().contains(&function(2)));
        });
    }
}

#[test]
fn recursive_execution_and_reentrant_queries_never_publish_preservation() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || {
            let mut recipes = Recipes::new(routed);
            let input = recipes.input(0, 0);
            recipes.calling_function(1, input, 1);
            recipes.writing_function(2, input, 1);
            recipes.writing_function(3, input, 1);
            recipes.call(0, input, Some(1), input);
            recipes.call(1, input, Some(2), input);
            recipes.call(2, input, Some(3), input);
            recipes.reentry = Some((at(2, 1, 0), Recipes::queried(input), value(1), input));
            recipes
        }, |recipes, queries| {
            let input = at(0, 0, 0);
            not_preserved(queries.flow_call_preservation(recipes, Recipes::queried(input), value(0), input));
            not_preserved(queries.flow_call_preservation(recipes, Recipes::queried(input), value(1), input));
            assert!(!recipes.reentry_results.borrow().is_empty());
            for result in recipes.reentry_results.borrow().iter().copied() { not_preserved(result); }
            assert_eq!(queries.flow_call_preservation(recipes, Recipes::queried(input), value(2), input), NativeCallPreservation::Preserved);
        });
    }
}

#[test]
fn pass_nodes_with_unknown_dispatch_are_execution_barriers() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || {
            let mut recipes = Recipes::new(routed);
            let input = recipes.input(0, 0);
            recipes.execution(1, input, vec![NativeFlowNode::Start,
                NativeFlowNode::Pass { before: at(1, 0, 0) },
                NativeFlowNode::Return { before: at(1, 1, 0) }]);
            recipes.barriers.insert(at(1, 1, 0));
            recipes.call(0, input, Some(1), input);
            recipes
        }, |recipes, queries| {
            let input = at(0, 0, 0);
            not_preserved(queries.flow_call_preservation(recipes, Recipes::queried(input), value(0), input));
        });
    }
}

#[test]
fn call_preservation_cache_keeps_frozen_inputs_and_body_facets_distinct() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || {
            let mut recipes = Recipes::new(routed);
            let first = recipes.input(0, 0);
            let later = recipes.input(1, 0);
            recipes.writing_function(1, first, 1);
            recipes.writing_function(2, first, 0);
            recipes.call(0, first, Some(1), first);
            recipes.call(0, later, Some(2), first);
            for context in [1, 2] {
                let input = recipes.input(0, context);
                recipes.writing_function(1, input, 1);
                recipes.call(1, input, Some(1), input);
            }
            // The same original store addresses an alias only in the second
            // body facet; its syntax and target reference stay unchanged.
            recipes.aliased_writes.insert(at(1, 1, 2));
            recipes
        }, |recipes, queries| {
            for (input, call, expected) in [
                (at(0, 0, 0), value(0), true),
                (at(0, 1, 0), value(0), false),
                (at(0, 0, 1), value(1), true),
                (at(0, 0, 2), value(1), false),
                (at(0, 0, 0), value(0), true),
                (at(0, 0, 1), value(1), true),
            ] {
                let result = queries.flow_call_preservation(recipes, Recipes::queried(input), call, input);
                if expected { assert_eq!(result, NativeCallPreservation::Preserved); }
                else { not_preserved(result); }
            }
        });
    }
}
