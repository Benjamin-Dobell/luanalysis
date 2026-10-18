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
