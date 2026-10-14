use crate::*;
use std::cell::RefCell;

struct Graph {
    writes: Vec<NativeVariableWrite>,
    creation: NativeFunctionCreation,
    errors: RefCell<Vec<NativeFlowError>>,
}
impl Graph {
    fn graph(region: u32) -> NativeGraphKey {
        NativeGraphKey { snapshot: 1201, region, context: 0 }
    }
    fn at(region: u32, node: u32) -> NativeFlowKey {
        NativeFlowKey { graph: Self::graph(region), node }
    }
    fn reference(region: u32) -> NativeReferenceKey {
        NativeReferenceKey { graph: Self::graph(region), reference: 0 }
    }
    fn new() -> Self {
        Self {
            writes: vec![NativeVariableWrite { owner: Self::graph(0), statement_end: 50 }],
            creation: NativeFunctionCreation { input: Self::at(0, 1), position: 100 },
            errors: RefCell::new(Vec::new()),
        }
    }
}
impl NativeFlowProvider for Graph {
    fn reference_declaration(&self, _: NativeReferenceKey) -> Option<NativeVariableDeclaration> {
        Some(NativeVariableDeclaration { owner: Self::graph(0), reference: Self::reference(0), writes: self.writes.clone() })
    }
    fn graph_creation(&self, graph: NativeGraphKey) -> Option<NativeFunctionCreation> {
        (graph == Self::graph(1)).then_some(self.creation)
    }
    fn reference_in_graph(&self, _: NativeReferenceKey, graph: NativeGraphKey) -> Option<NativeReferenceKey> {
        self.contains_graph(graph).then_some(NativeReferenceKey { graph, reference: 0 })
    }
    fn contains_graph(&self, graph: NativeGraphKey) -> bool {
        graph.snapshot == 1201 && graph.region <= 1 && graph.context == 0
    }
    fn contains_reference(&self, reference: NativeReferenceKey) -> bool {
        self.contains_graph(reference.graph) && reference.reference == 0
    }
    fn node(&self, at: NativeFlowKey) -> Option<NativeFlowNode> {
        if at.graph.region == 1 { return (at.node == 0).then_some(NativeFlowNode::Start); }
        Some(match at.node {
            0 => NativeFlowNode::Start,
            1 | 2 => NativeFlowNode::Write {
                before: Self::at(0, 0), target: Self::reference(0),
                value: NativeValueKey { snapshot: 1201, index: at.node }, input: Self::at(0, 0),
            },
            3 => NativeFlowNode::Merge { edges: vec![Self::at(0, 1), Self::at(0, 2)] },
            4 => NativeFlowNode::Loop { edges: vec![Self::at(0, 0), Self::at(0, 1)] },
            5 => NativeFlowNode::Write {
                before: Self::at(0, 3), target: Self::reference(0),
                value: NativeValueKey { snapshot: 1201, index: 2 }, input: Self::at(0, 3),
            },
            _ => return None,
        })
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool { false }
    fn write_affects(&self, reference: NativeReferenceKey, target: NativeReferenceKey, _: NativeFlowKey) -> NativeWriteRelation {
        if reference == target { NativeWriteRelation::Exact } else { NativeWriteRelation::Unaffected }
    }
    fn value_type(&self, value: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type {
        match value.index { 1 => Type::NumberLiteral(42.0), 2 => Type::StringLiteral(b"later".to_vec()), _ => unreachable!() }
    }
    fn call_effects(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeCallEffects { NativeCallEffects::None }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) { self.errors.borrow_mut().push(error); }
}

#[test]
fn automatic_constraint_keeps_nil_initial_and_shared_assignment_join_and_loop_types() {
    Engine::new().with_session(|queries| {
        let graph = Graph::new();
        let declared = queries.auto_variable_constraint();
        assert!(matches!(declared, Type::SessionType(_)));
        assert!(queries.is_auto_variable_constraint(&declared));
        assert_eq!(queries.assignment_value(&declared, &Type::NumberLiteral(42.0)), Type::Number);
        for (node, expected) in [(0, Type::Nil), (1, Type::Number), (2, Type::String), (5, Type::String)] {
            let actual = queries.flow_type(&graph, Graph::at(0, node), Graph::reference(0), &declared, &Type::Nil);
            assert_eq!(actual.ty, expected);
            assert!(!actual.unresolved_auto);
        }
        let branch = queries.flow_type(&graph, Graph::at(0, 3), Graph::reference(0), &declared, &Type::Nil);
        assert_eq!(branch.ty, queries.union_reduced(&[Type::Number, Type::String]));
        let loop_value = queries.flow_type(&graph, Graph::at(0, 4), Graph::reference(0), &declared, &Type::Nil);
        assert_eq!(loop_value.ty, queries.union_reduced(&[Type::Number, Type::Nil]));
        assert!(graph.errors.borrow().is_empty());
    });
}

#[test]
fn automatic_captures_cross_only_after_last_write_without_nested_writes() {
    Engine::new().with_session(|queries| {
        let mut graph = Graph::new();
        let declared = queries.auto_variable_constraint();
        let read = queries.flow_type(&graph, Graph::at(1, 0), Graph::reference(1), &declared, &Type::Nil);
        assert_eq!(read.ty, Type::Number);
        assert!(!read.unresolved_auto);
        graph.writes.push(NativeVariableWrite { owner: Graph::graph(0), statement_end: 150 });
        let capture = queries.flow_capture_type(&graph, Graph::at(1, 0), Graph::reference(1), &declared, &Type::Nil);
        assert!(queries.is_auto_variable_constraint(&capture.ty));
        assert!(!capture.unresolved_auto);
        let read = queries.flow_type(&graph, Graph::at(1, 0), Graph::reference(1), &declared, &Type::Nil);
        assert_eq!(read.ty, Type::Any);
        assert!(read.unresolved_auto);
        graph.writes.truncate(1);
        graph.writes.push(NativeVariableWrite { owner: Graph::graph(1), statement_end: 40 });
        let read = queries.flow_type(&graph, Graph::at(1, 0), Graph::reference(1), &declared, &Type::Nil);
        assert_eq!(read.ty, Type::Any);
        assert!(read.unresolved_auto);
    });
}

#[test]
fn automatic_capture_rejects_cyclic_original_creation_edges() {
    Engine::new().with_session(|queries| {
        let mut graph = Graph::new();
        graph.creation.input = Graph::at(1, 0);
        let declared = queries.auto_variable_constraint();
        let read = queries.flow_type(&graph, Graph::at(1, 0), Graph::reference(1), &declared, &Type::Nil);
        assert_eq!(read.ty, Type::Any);
        assert!(matches!(graph.errors.borrow().as_slice(), [NativeFlowError::TooDeep(_)]));
    });
}

#[test]
fn automatic_constraint_cannot_escape_its_checker_session() {
    let engine = Engine::new();
    let escaped = engine.with_session(|queries| queries.auto_variable_constraint());
    let error = std::panic::catch_unwind(|| engine.with_session(|queries| queries.is_auto_variable_constraint(&escaped)));
    assert!(error.is_err());
    engine.with_session(|queries| {
        let declared = queries.auto_variable_constraint();
        assert!(queries.is_auto_variable_constraint(&declared));
        assert_eq!(queries.assignment_value(&declared, &Type::StringLiteral(b"okay".to_vec())), Type::String);
    });
}
