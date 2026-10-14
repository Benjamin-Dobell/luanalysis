use luanalysis::{
    bind,
    flow::{self, Key, NodeKind, PendingFeature, Predicate, ReferenceRoot, ValueDemand},
};
use luanalysis_syntax::Span;

fn graph(source: &str) -> flow::ProgramFlow {
    let syntax = luanalysis_syntax::parse(source);
    assert!(syntax.diagnostics.is_empty(), "{:#?}", syntax.diagnostics);
    let bound = bind::bind(&syntax);
    flow::build(&syntax, &bound)
}

#[test]
fn if_else_branches_join_original_predicate_edges() {
    let program = graph("local x=1\nif x then x=2 else x=3 end\nlocal y=x");
    let main = &program.graphs[0];
    let predicates: Vec<_> = main
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Predicate { .. }))
        .collect();
    assert_eq!(predicates.len(), 2);
    assert_eq!(predicates[0].antecedents, predicates[1].antecedents);
    assert!(program.is_shared(predicates[0].antecedents[0]));
    let merge = main
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Merge))
        .unwrap();
    assert_eq!(merge.antecedents.len(), 2);
    assert!(
        merge
            .antecedents
            .iter()
            .all(|id| matches!(program.node(*id).kind, NodeKind::Write { .. }))
    );
}

#[test]
fn loop_entry_precedes_back_edge_and_break_exits_only_inner_loop() {
    let program = graph("local x=1\nwhile x do\nwhile x do break end\nx=2\nend");
    let main = &program.graphs[0];
    let loops: Vec<_> = main
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Loop))
        .collect();
    assert_eq!(loops.len(), 2);
    assert_eq!(loops[0].antecedents.len(), 2);
    assert_eq!(
        loops[1].antecedents.len(),
        1,
        "terminated inner body must not supply a back edge"
    );
    assert!(loops[0].antecedents[0].index < loops[0].id.index);
    assert!(loops[0].antecedents[1].index > loops[0].id.index);
    let break_node = main.breaks[0];
    let merges: Vec<_> = main
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Merge))
        .collect();
    assert!(merges[0].antecedents.contains(&break_node));
    assert!(!merges[1].antecedents.contains(&break_node));
}

#[test]
fn repeat_condition_retains_its_body_binding_and_false_back_edge() {
    let source = "repeat local visible=1 until visible";
    let syntax = luanalysis_syntax::parse(source);
    assert!(syntax.diagnostics.is_empty());
    let bound = bind::bind(&syntax);
    let program = flow::build(&syntax, &bound);
    let predicate = program.graphs[0]
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Predicate { sense: false, .. }))
        .unwrap();
    let NodeKind::Predicate {
        predicate: Predicate::Expression(expr),
        ..
    } = predicate.kind
    else {
        panic!("original condition")
    };
    let expression = program.expression(expr);
    let declaration = source.find("visible=").unwrap();
    assert_eq!(
        bound.binding_at(expression.expression.span.start),
        bound.binding_at(declaration)
    );
    assert_eq!(
        expression.scope,
        Some(
            bound
                .declaration(bound.binding_at(declaration).unwrap())
                .scope
        )
    );
    let header = program.graphs[0]
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Loop))
        .unwrap();
    assert_eq!(header.antecedents[1], predicate.id);
}

#[test]
fn simultaneous_property_writes_capture_addresses_and_evaluate_rhs_once() {
    let source = "local item={x=1}\nitem,item.x={x=2},call()";
    let program = graph(source);
    let writes: Vec<_> = program.graphs[0]
        .nodes
        .iter()
        .filter(|n| {
            matches!(
                n.kind,
                NodeKind::Write {
                    declaration: false,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(writes.len(), 2);
    let NodeKind::Write {
        address: field,
        value: ValueDemand::Evaluation(rhs),
        store_order: 0,
        ..
    } = &writes[0].kind
    else {
        panic!("field is stored first")
    };
    let NodeKind::Write {
        address: root,
        store_order: 1,
        ..
    } = &writes[1].kind
    else {
        panic!("root is stored second")
    };
    assert_eq!(field.transaction, root.transaction);
    assert_eq!(field.epoch, root.epoch);
    assert_eq!(field.reference.path, [Key::String(b"x".to_vec())]);
    let receiver = program.evaluation(field.receiver.unwrap());
    assert!(receiver.completed.index < program.evaluation(*rhs).completed.index);
    assert!(program.evaluation(*rhs).completed.index < writes[0].id.index);
    assert_eq!(
        program.graphs[0]
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Call { .. }))
            .count(),
        1
    );
    let rhs_expression = program.expression(program.evaluation(*rhs).expression);
    assert_eq!(
        &source[rhs_expression.expression.span.start..rhs_expression.expression.span.end],
        "call()"
    );
}

#[test]
fn nested_function_creation_does_not_execute_body_in_outer_graph() {
    let source =
        "local x=1\nlocal function outer()\nreturn function()x=2 return x end\nend\nlocal y=x";
    let program = graph(source);
    assert_eq!(program.graphs.len(), 3);
    assert!(program.graphs[0].returns.is_empty());
    assert_eq!(program.graphs[1].returns.len(), 1);
    assert_eq!(program.graphs[2].returns.len(), 1);
    assert!(program.graphs[0].nodes.iter().all(|n| !matches!(
        n.kind,
        NodeKind::Write {
            declaration: false,
            ..
        }
    )));
    assert!(
        program.graphs[1]
            .nodes
            .iter()
            .any(|n| matches!(n.kind, NodeKind::CreateFunction { graph: 2, .. }))
    );
    assert!(program.graphs[2].nodes.iter().any(|n| matches!(
        n.kind,
        NodeKind::Write {
            declaration: false,
            ..
        }
    )));
}

#[test]
fn numeric_and_string_keys_are_distinct_and_integral_floats_match_integer_keys() {
    let program = graph("local item={}\nitem[1]=2\nitem[\"1\"]=3\nitem[1.0]=4\nitem[key]=5");
    let addresses: Vec<_> = program.graphs[0]
        .nodes
        .iter()
        .filter_map(|n| {
            if let NodeKind::Write {
                address,
                declaration: false,
                ..
            } = &n.kind
            {
                Some(address)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(addresses[0].reference.path, [Key::Integer(1)]);
    assert_eq!(addresses[1].reference.path, [Key::String(b"1".to_vec())]);
    assert_eq!(addresses[2].reference.path, addresses[0].reference.path);
    assert!(matches!(addresses[3].reference.path[0], Key::Dynamic(_)));
    assert!(
        program
            .pending
            .iter()
            .any(|f| f.kind == PendingFeature::DynamicAddress)
    );
}

#[test]
fn short_circuit_rhs_has_a_predicate_predecessor_and_join() {
    let program = graph("local x=true\nlocal y=x and call()");
    let call = program.graphs[0]
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Call { .. }))
        .unwrap();
    let mut id = call.antecedents[0];
    while !matches!(program.node(id).kind, NodeKind::Predicate { .. }) {
        id = program.node(id).antecedents[0];
    }
    assert!(matches!(
        program.node(id).kind,
        NodeKind::Predicate { sense: true, .. }
    ));
    assert!(
        program.graphs[0]
            .nodes
            .iter()
            .any(|n| matches!(n.kind, NodeKind::Merge) && n.antecedents.len() == 2)
    );
}

#[test]
fn returns_keep_original_unicode_spans_and_exclude_structurally_dead_sites() {
    let source = "local function f() do return \"é😀\" end return 2 end";
    let program = graph(source);
    let function = &program.graphs[1];
    assert_eq!(function.returns.len(), 1);
    let NodeKind::Return { values } = &program.node(function.returns[0]).kind else {
        panic!("return")
    };
    let expression = program.expression(program.evaluation(values[0]).expression);
    assert_eq!(
        &source[expression.expression.span.start..expression.expression.span.end],
        "\"é😀\""
    );
    assert_eq!(
        expression.expression.span.end - expression.expression.span.start,
        8
    );
}

#[test]
fn for_headers_record_binding_ids_and_explicit_unimplemented_runtime_recipes() {
    let program = graph("for i=1,3 do break end\nfor key,value in items() do break end");
    let nodes = &program.graphs[0].nodes;
    assert_eq!(
        nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Loop))
            .count(),
        2
    );
    let names: Vec<_> = nodes
        .iter()
        .filter_map(|n| match &n.kind {
            NodeKind::Write {
                address,
                declaration: true,
                ..
            } => Some(&address.reference.root),
            _ => None,
        })
        .collect();
    assert_eq!(names.len(), 3);
    assert!(names.iter().all(|r| matches!(r, ReferenceRoot::Binding(_))));
    assert!(
        program
            .pending
            .iter()
            .any(|f| f.kind == PendingFeature::NumericIteration)
    );
    assert!(
        program
            .pending
            .iter()
            .any(|f| f.kind == PendingFeature::GenericIteration)
    );
}

#[test]
fn independent_builds_namespace_all_source_identities() {
    let first = graph("local x=1");
    let second = graph("local x=1");
    assert_ne!(first.snapshot, second.snapshot);
    assert_ne!(first.graphs[0].entry, second.graphs[0].entry);
    assert_eq!(first.graphs[0].nodes[0].span, Span::new(0, 9));
}

#[test]
fn expanding_final_calls_retain_explicit_result_pack_requirement() {
    let program = graph("local a,b=f()\na,b=(f())\n");
    let packs: Vec<_> = program
        .pending
        .iter()
        .filter(|f| f.kind == PendingFeature::ResultPacks)
        .collect();
    assert_eq!(packs.len(), 1, "parentheses scalarize a final call");
}

#[test]
fn canonical_read_references_preserve_bound_roots_and_typed_paths() {
    let program = graph("local item={}\nlocal a=item[1]\nlocal b=item[\"1\"]");
    let mut paths: Vec<_> = program
        .references
        .values()
        .filter(|r| !r.path.is_empty())
        .map(|r| r.path.clone())
        .collect();
    paths.sort_by_key(|p| format!("{p:?}"));
    assert_eq!(paths.len(), 2);
    assert!(paths.contains(&vec![Key::Integer(1)]));
    assert!(paths.contains(&vec![Key::String(b"1".to_vec())]));
    assert!(
        program
            .references
            .values()
            .all(|r| matches!(r.root, ReferenceRoot::Binding(_)))
    );
}

#[test]
fn recovered_invalid_source_never_produces_execution_recipes() {
    let syntax = luanalysis_syntax::parse("local x=\nlocal later=1");
    assert!(!syntax.diagnostics.is_empty());
    let bound = bind::bind(&syntax);
    let program = flow::build(&syntax, &bound);
    assert!(
        program
            .pending
            .iter()
            .any(|f| f.kind == PendingFeature::InvalidSource)
    );
    assert!(program.evaluations.is_empty());
    assert_eq!(program.graphs[0].nodes.len(), 1);
}
