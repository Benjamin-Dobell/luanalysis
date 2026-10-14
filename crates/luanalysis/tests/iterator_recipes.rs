use luanalysis::{
    bind,
    flow::{self, NodeKind, Predicate, ReferenceRoot, ValueDemand},
};

fn graph(source: &str) -> flow::ProgramFlow {
    let file = luanalysis_syntax::parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    flow::build(&file, &bind::bind(&file))
}

#[test]
fn generic_headers_keep_original_evaluations_and_hidden_frame() {
    let program =
        graph("for key, value in factory(), state(), initial(), closing() do key = false end");
    let recipe = &program.iterators[0];
    assert_eq!(recipe.values.len(), 4, "closing evaluation is retained");
    assert!(
        recipe
            .values
            .iter()
            .all(|id| program.evaluation(*id).completed.index < recipe.step.index)
    );
    let main = &program.graphs[0];
    let starts: Vec<_> = main
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, NodeKind::IteratorStart { .. }))
        .collect();
    assert_eq!(starts.len(), 1);
    let header = main
        .nodes
        .iter()
        .find(|node| matches!(node.kind, NodeKind::Loop))
        .unwrap();
    let first_body = program.node(header.antecedents[0]);
    let first = program.node(first_body.antecedents[0]);
    assert!(matches!(
        first.kind,
        NodeKind::Iteration {
            stage: flow::IteratorStage::First,
            ..
        }
    ));
    assert_eq!(first.antecedents[0], starts[0].id);
    assert_eq!(header.antecedents.len(), 2, "entry then body back edge");
    assert!(matches!(program.node(recipe.step).kind, NodeKind::Loop));
    let predicates: Vec<_> = main.nodes.iter().filter(|node| matches!(node.kind,
        NodeKind::Predicate { predicate: Predicate::GenericIteration { iterator }, .. } if iterator == recipe.id)).collect();
    assert_eq!(predicates.len(), 4);
    assert!(
        predicates
            .iter()
            .all(|node| matches!(program.node(node.antecedents[0]).kind, NodeKind::Iteration { iterator, .. } if iterator == recipe.id))
    );
    let writes: Vec<_> = main
        .nodes
        .iter()
        .filter_map(|node| match &node.kind {
            NodeKind::Write {
                address,
                value: ValueDemand::IteratorResult { iterator, index },
                declaration: true,
                ..
            } => Some((address, *iterator, *index)),
            _ => None,
        })
        .collect();
    assert_eq!(writes.len(), 2);
    assert!(
        writes
            .iter()
            .all(|(address, iterator, _)| *iterator == recipe.id
                && matches!(address.reference.root, ReferenceRoot::Binding(_)))
    );
    assert_eq!(
        writes
            .iter()
            .map(|(_, _, index)| *index)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert!(!main.nodes.iter().any(|node| matches!(&node.kind,
        NodeKind::Write { address, .. } if matches!(address.reference.root, ReferenceRoot::Iterator(_)))),
        "body stores cannot alter hidden control");
}

#[test]
fn nested_iterators_and_detached_functions_have_distinct_owners() {
    let program = graph(
        "for outer in first() do\nfor inner in second() do break end\nlocal function f() for nested in third() do return nested end end\nend",
    );
    assert_eq!(program.iterators.len(), 3);
    assert_eq!(
        program.iterators[0].input.graph,
        program.iterators[1].input.graph
    );
    assert_ne!(program.iterators[0].id, program.iterators[1].id);
    assert_ne!(
        program.iterators[0].input.graph,
        program.iterators[2].input.graph
    );
    for iterator in &program.iterators {
        assert_eq!(iterator.input.snapshot, iterator.id.snapshot);
        assert_eq!(iterator.step.graph, iterator.input.graph);
    }
    let nested = &program.graphs[program.iterators[1].input.graph as usize];
    let inner_header = program.node(program.iterators[1].step);
    assert_eq!(
        inner_header.antecedents.len(),
        1,
        "break supplies no loop back edge"
    );
    assert_eq!(nested.breaks.len(), 1);
}
