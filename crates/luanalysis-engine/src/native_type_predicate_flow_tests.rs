use super::*;
use std::cell::Cell;

struct PredicateGraph {
    resolutions: Cell<usize>,
}
impl PredicateGraph {
    fn graph() -> NativeGraphKey { NativeGraphKey { snapshot: 32_144, region: 0, context: 0 } }
    fn at(node: u32) -> NativeFlowKey { NativeFlowKey { graph: Self::graph(), node } }
    fn reference(index: u32) -> NativeReferenceKey { NativeReferenceKey { graph: Self::graph(), reference: index } }
    fn call(index: u32) -> NativeValueKey { NativeValueKey { snapshot: 32_144, index } }
    fn predicate(index: u32) -> NativePredicate {
        NativePredicate::Call { value: Self::call(index), input: Self::at(0), arguments: vec![Some(Self::reference(0)), Some(Self::reference(1))], negated: false }
    }
}
impl NativeFlowProvider for PredicateGraph {
    fn contains_graph(&self, graph: NativeGraphKey) -> bool { graph == Self::graph() }
    fn contains_reference(&self, reference: NativeReferenceKey) -> bool { reference.graph == Self::graph() && reference.reference < 3 }
    fn node(&self, at: NativeFlowKey) -> Option<NativeFlowNode> {
        if at.graph != Self::graph() { return None; }
        Some(match at.node {
            0 => NativeFlowNode::Start,
            1 | 2 => NativeFlowNode::Predicate { before: Self::at(0), predicate: Self::predicate(0), sense: at.node == 1 },
            3 => NativeFlowNode::Predicate { before: Self::at(0), predicate: NativePredicate::Not(Box::new(Self::predicate(0))), sense: true },
            4 => NativeFlowNode::Predicate { before: Self::at(0), predicate: Self::predicate(1), sense: true },
            5 | 6 => {
                let mut predicate = Self::predicate(if at.node == 5 { 0 } else { 2 });
                let NativePredicate::Call { negated, .. } = &mut predicate else { unreachable!() };
                *negated = true;
                NativeFlowNode::Predicate { before: Self::at(0), predicate, sense: true }
            }
            _ => return None,
        })
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool { false }
    fn write_affects(&self, reference: NativeReferenceKey, target: NativeReferenceKey, _: NativeFlowKey) -> NativeWriteRelation {
        if reference == target { NativeWriteRelation::Exact } else { NativeWriteRelation::Unaffected }
    }
    fn value_type(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type { panic!("predicate does not query the body or call value") }
    fn value_literal_truthiness(&self, value: NativeValueKey) -> Option<bool> { (value.index == 2).then_some(false) }
    fn call_effects(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeCallEffects { NativeCallEffects::None }
    fn call_type_predicate(&self, call: NativeValueKey, input: NativeFlowKey, _: &mut dyn TypeQueries) -> Option<TypePredicate> {
        assert_eq!(input, Self::at(0));
        self.resolutions.set(self.resolutions.get() + 1);
        (call.index == 0).then_some(TypePredicate { parameter: 1, ty: Type::String })
    }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) { panic!("unexpected flow error: {error:?}"); }
}

#[test]
fn native_type_predicate_flow_targets_the_selected_parameter_in_both_branches_and_caches_calls() {
    Engine::new().with_session(|queries| {
        let graph = PredicateGraph { resolutions: Cell::new(0) };
        let declared = Type::Union(vec![Type::String, Type::Number]);
        // An unrelated reference never resolves call applicability.
        let unrelated = queries.flow_type(&graph, PredicateGraph::at(1), PredicateGraph::reference(2), &declared, &declared);
        assert!(queries.is_assignable(&unrelated.ty, &declared));
        assert_eq!(graph.resolutions.get(), 0);
        for (node, expected) in [(1, Type::String), (2, Type::Number), (3, Type::Number), (5, Type::Number)] {
            let narrowed = queries.flow_type(&graph, PredicateGraph::at(node), PredicateGraph::reference(1), &declared, &declared);
            assert!(!narrowed.incomplete);
            assert!(queries.is_assignable(&narrowed.ty, &expected));
            assert!(queries.is_assignable(&expected, &narrowed.ty));
        }
        assert_eq!(graph.resolutions.get(), 1);
        let other = queries.flow_type(&graph, PredicateGraph::at(1), PredicateGraph::reference(0), &declared, &declared);
        assert!(queries.is_assignable(&declared, &other.ty));
        let ordinary = queries.flow_type(&graph, PredicateGraph::at(4), PredicateGraph::reference(1), &declared, &declared);
        assert!(queries.is_assignable(&declared, &ordinary.ty));
        assert_eq!(graph.resolutions.get(), 2);
        assert!(!queries.flow_reachable(&graph, PredicateGraph::at(6)), "guard polarity must not invert the complete condition's truthiness");
    });
}

struct TypedAddressGraph {
    enabled: bool,
    gates: Cell<usize>,
    resolutions: Cell<usize>,
    reenter_once: Cell<bool>,
}
impl TypedAddressGraph {
    fn new(enabled: bool, reenter_once: bool) -> Self {
        Self { enabled, gates: Cell::new(0), resolutions: Cell::new(0), reenter_once: Cell::new(reenter_once) }
    }
    fn declared() -> Type { Type::Union(vec![Type::String, Type::Nil]) }
}
impl NativeFlowProvider for TypedAddressGraph {
    fn contains_graph(&self, graph: NativeGraphKey) -> bool { graph == PredicateGraph::graph() }
    fn contains_reference(&self, reference: NativeReferenceKey) -> bool { reference.graph == PredicateGraph::graph() && reference.reference < 3 }
    fn node(&self, at: NativeFlowKey) -> Option<NativeFlowNode> {
        if at.graph != PredicateGraph::graph() { return None; }
        Some(match at.node {
            0 => NativeFlowNode::Start,
            1 | 2 => NativeFlowNode::Predicate {
                before: PredicateGraph::at(0),
                predicate: NativePredicate::Equal { subject: PredicateGraph::reference(1), other: PredicateGraph::call(3), input: PredicateGraph::at(0), equals: false },
                sense: at.node == 1,
            },
            _ => return None,
        })
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool { false }
    fn write_affects(&self, reference: NativeReferenceKey, target: NativeReferenceKey, _: NativeFlowKey) -> NativeWriteRelation {
        if reference == target { NativeWriteRelation::Exact } else { NativeWriteRelation::Unaffected }
    }
    fn value_type(&self, value: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type {
        assert_eq!(value, PredicateGraph::call(3));
        Type::Nil
    }
    fn call_effects(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeCallEffects { NativeCallEffects::None }
    fn predicate_needs_types(&self, _: NativeFlowKey, reference: NativeReferenceKey, _: &NativePredicate) -> bool {
        self.gates.set(self.gates.get() + 1);
        self.enabled && reference.reference < 2
    }
    fn predicate_with_types(&self, at: NativeFlowKey, mut predicate: NativePredicate, queries: &mut dyn TypeQueries) -> Option<NativePredicate> {
        assert!(self.enabled, "an absent typed-address gate must bypass resolution");
        self.resolutions.set(self.resolutions.get() + 1);
        if self.reenter_once.replace(false) {
            let declared = Self::declared();
            let recursive = queries.flow_type(self, at, PredicateGraph::reference(0), &declared, &declared);
            assert!(recursive.incomplete, "the active address query must taint recursive flow");
        }
        let NativePredicate::Equal { subject, .. } = &mut predicate else { panic!("original equality recipe") };
        assert_eq!(*subject, PredicateGraph::reference(1));
        *subject = PredicateGraph::reference(0);
        Some(predicate)
    }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) { panic!("unexpected flow error: {error:?}"); }
}

#[test]
fn native_predicate_address_presence_gate_skips_typed_resolution_and_reachability_stays_structural() {
    Engine::new().with_session(|queries| {
        let graph = TypedAddressGraph::new(false, false);
        let declared = TypedAddressGraph::declared();
        let ordinary = queries.flow_type(&graph, PredicateGraph::at(1), PredicateGraph::reference(0), &declared, &declared);
        assert!(!ordinary.incomplete);
        assert!(queries.is_assignable(&declared, &ordinary.ty));
        assert!(graph.gates.get() > 0);
        assert_eq!(graph.resolutions.get(), 0);
        let gates = graph.gates.get();
        assert!(queries.flow_reachable(&graph, PredicateGraph::at(1)));
        assert!(queries.flow_reachable(&graph, PredicateGraph::at(2)));
        assert_eq!(graph.gates.get(), gates, "structural reachability must skip the address gate");
        assert_eq!(graph.resolutions.get(), 0);
    });
}

#[test]
fn native_typed_predicate_addresses_narrow_both_branches_and_cache_completed_trees() {
    Engine::new().with_session(|queries| {
        let graph = TypedAddressGraph::new(true, false);
        let declared = TypedAddressGraph::declared();
        // Reachability does not need the runtime identity even when the gate is enabled.
        assert!(queries.flow_reachable(&graph, PredicateGraph::at(1)));
        assert_eq!(graph.gates.get(), 0);
        assert_eq!(graph.resolutions.get(), 0);
        let unrelated = queries.flow_type(&graph, PredicateGraph::at(1), PredicateGraph::reference(2), &declared, &declared);
        assert!(queries.is_assignable(&declared, &unrelated.ty));
        assert_eq!(graph.resolutions.get(), 0, "an unrelated walk must not trigger the first address proof");
        for (node, expected) in [(1, Type::String), (2, Type::Nil), (1, Type::String), (2, Type::Nil)] {
            let narrowed = queries.flow_type(&graph, PredicateGraph::at(node), PredicateGraph::reference(0), &declared, &declared);
            assert!(!narrowed.incomplete);
            assert!(queries.is_assignable(&narrowed.ty, &expected));
            assert!(queries.is_assignable(&expected, &narrowed.ty));
        }
        assert_eq!(graph.resolutions.get(), 2, "each original branch resolves its address once");
        for reference in [1, 2] {
            let unrelated = queries.flow_type(&graph, PredicateGraph::at(1), PredicateGraph::reference(reference), &declared, &declared);
            assert!(queries.is_assignable(&declared, &unrelated.ty));
        }
        assert_eq!(graph.resolutions.get(), 2, "different reference walks reuse the completed tree");
    });
}

#[test]
fn native_typed_predicate_address_reentry_does_not_cache_a_provisional_tree() {
    Engine::new().with_session(|queries| {
        let graph = TypedAddressGraph::new(true, true);
        let declared = TypedAddressGraph::declared();
        let pending = queries.flow_type(&graph, PredicateGraph::at(1), PredicateGraph::reference(0), &declared, &declared);
        assert!(pending.incomplete);
        assert!(queries.is_assignable(&declared, &pending.ty));
        assert_eq!(graph.resolutions.get(), 1);
        let resolved = queries.flow_type(&graph, PredicateGraph::at(1), PredicateGraph::reference(0), &declared, &declared);
        assert!(!resolved.incomplete);
        assert!(queries.is_assignable(&resolved.ty, &Type::String));
        assert!(queries.is_assignable(&Type::String, &resolved.ty));
        assert_eq!(graph.resolutions.get(), 2, "the incomplete address demand must be retried");
        queries.flow_type(&graph, PredicateGraph::at(1), PredicateGraph::reference(2), &declared, &declared);
        assert_eq!(graph.resolutions.get(), 2, "only the completed retry may populate the cache");
    });
}

struct EnvironmentReadProvider;
impl NativeTypeProvider for EnvironmentReadProvider {
    fn contains_global_environment(&self, key: NativeEnvironmentKey) -> bool { key.snapshot == 32_144 && key.index < 2 }
    fn contains_function(&self, _: NativeFunctionKey) -> bool { false }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { panic!("no function demand") }
    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> { panic!("no function demand") }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type { panic!("no function demand") }
    fn return_cycle(&self, _: NativeFunctionKey) { panic!("no function demand") }
    fn return_type_too_deep(&self, _: NativeFunctionKey) { panic!("no function demand") }
}

#[test]
fn observed_environment_origins_cache_only_completed_registered_positive_facts() {
    Engine::new().with_native_provider(|| EnvironmentReadProvider, |_, queries| {
        let value = PredicateGraph::call(10);
        let input = PredicateGraph::at(0);
        let environment = NativeEnvironmentKey { snapshot: 32_144, index: 0 };
        assert_eq!(queries.cached_environment_origin(value, input), None);
        let incomplete = queries.begin_environment_read(value, input).expect("fresh read scope");
        queries.end_environment_read(incomplete, Some(environment), false);
        assert_eq!(queries.cached_environment_origin(value, input), None);
        let absent = queries.begin_environment_read(value, input).expect("negative observations are not cached");
        queries.end_environment_read(absent, None, true);
        assert_eq!(queries.cached_environment_origin(value, input), None);
        let foreign = queries.begin_environment_read(value, input).expect("foreign observation scope");
        queries.end_environment_read(foreign, Some(NativeEnvironmentKey { index: 99, ..environment }), true);
        assert_eq!(queries.cached_environment_origin(value, input), None);
        let completed = queries.begin_environment_read(value, input).expect("completed observation scope");
        queries.end_environment_read(completed, Some(environment), true);
        assert_eq!(queries.cached_environment_origin(value, input), Some(environment));
        assert!(queries.begin_environment_read(value, input).is_none(), "an established positive needs no extra observation scope");
        assert_eq!(queries.cached_environment_origin(PredicateGraph::call(11), input), None);
        assert_eq!(queries.cached_environment_origin(value, PredicateGraph::at(1)), None);
        let other_facet = NativeFlowKey { graph: NativeGraphKey { context: 1, ..input.graph }, ..input };
        assert_eq!(queries.cached_environment_origin(value, other_facet), None);
    });
}

#[test]
fn observed_environment_origin_reentry_invalidates_only_the_observation() {
    Engine::new().with_native_provider(|| EnvironmentReadProvider, |_, queries| {
        let value = PredicateGraph::call(12);
        let input = PredicateGraph::at(0);
        let environment = NativeEnvironmentKey { snapshot: 32_144, index: 0 };
        let recursive = queries.begin_environment_read(value, input).expect("outer read scope");
        assert!(queries.begin_environment_read(value, input).is_none(), "the exact receiver read invalidates its active observation");
        // The observer must not taint the source checker or own a provisional
        // capability frame. Independent original work remains fully cacheable.
        let independent_value = PredicateGraph::call(14);
        let independent = queries.begin_environment_read(independent_value, input).expect("source state must stay clean after observer reentry");
        queries.end_environment_read(independent, Some(environment), true);
        assert_eq!(queries.cached_environment_origin(independent_value, input), Some(environment));
        queries.end_environment_read(recursive, Some(environment), true);
        assert_eq!(queries.cached_environment_origin(value, input), None, "a reentered observation cannot publish a stable origin");
        let retried = queries.begin_environment_read(value, input).expect("a later complete observation must retry");
        queries.end_environment_read(retried, Some(environment), true);
        assert_eq!(queries.cached_environment_origin(value, input), Some(environment));
    });
}

struct EnvironmentBackEdgeGraph { reads: Cell<usize> }
impl NativeFlowProvider for EnvironmentBackEdgeGraph {
    fn contains_graph(&self, graph: NativeGraphKey) -> bool { graph == PredicateGraph::graph() }
    fn contains_reference(&self, reference: NativeReferenceKey) -> bool { reference == PredicateGraph::reference(0) }
    fn node(&self, at: NativeFlowKey) -> Option<NativeFlowNode> {
        if at.graph != PredicateGraph::graph() { return None; }
        Some(match at.node {
            0 => NativeFlowNode::Start,
            1 => NativeFlowNode::Loop { edges: vec![PredicateGraph::at(0), PredicateGraph::at(2)] },
            2 => NativeFlowNode::Write { before: PredicateGraph::at(1), target: PredicateGraph::reference(0), value: PredicateGraph::call(20), input: PredicateGraph::at(1) },
            _ => return None,
        })
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool { false }
    fn write_affects(&self, reference: NativeReferenceKey, target: NativeReferenceKey, _: NativeFlowKey) -> NativeWriteRelation {
        if reference == target { NativeWriteRelation::Exact } else { NativeWriteRelation::Unaffected }
    }
    fn value_type(&self, value: NativeValueKey, _: NativeFlowKey, queries: &mut dyn TypeQueries) -> Type {
        assert_eq!(value, PredicateGraph::call(20));
        self.reads.set(self.reads.get() + 1);
        let input = PredicateGraph::at(0);
        assert_eq!(queries.cached_environment_origin(PredicateGraph::call(10), input), None, "backedges must not borrow an outer read's positive origin");
        let local = queries.begin_environment_read(PredicateGraph::call(13), input).expect("backedge-local read");
        let environment = NativeEnvironmentKey { snapshot: 32_144, index: 1 };
        queries.end_environment_read(local, Some(environment), true);
        assert_eq!(queries.cached_environment_origin(PredicateGraph::call(13), input), Some(environment));
        Type::String
    }
    fn call_effects(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeCallEffects { NativeCallEffects::None }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) { panic!("unexpected flow error: {error:?}"); }
}

#[test]
fn observed_environment_origins_are_isolated_and_restored_around_loop_backedges() {
    Engine::new().with_native_provider(|| EnvironmentReadProvider, |_, queries| {
        let graph = EnvironmentBackEdgeGraph { reads: Cell::new(0) };
        let input = PredicateGraph::at(0);
        let environment = NativeEnvironmentKey { snapshot: 32_144, index: 0 };
        let outer = queries.begin_environment_read(PredicateGraph::call(10), input).expect("outer read");
        queries.end_environment_read(outer, Some(environment), true);
        let declared = Type::Union(vec![Type::String, Type::Nil]);
        queries.flow_type(&graph, PredicateGraph::at(1), PredicateGraph::reference(0), &declared, &Type::Nil);
        assert!(graph.reads.get() > 0, "the loop must evaluate its backedge RHS");
        assert_eq!(queries.cached_environment_origin(PredicateGraph::call(10), input), Some(environment), "the outer environment fact must be restored");
        assert_eq!(queries.cached_environment_origin(PredicateGraph::call(13), input), None, "backedge-local observations must not escape");
    });
}
