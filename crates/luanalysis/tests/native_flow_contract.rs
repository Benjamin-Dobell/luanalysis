//! Source adapter contracts only: no frontend type-flow computation.
use luanalysis::{
    bind,
    flow::{self, NodeKind, Predicate, ReferenceRoot, ValueDemand},
};
use luanalysis_syntax::{ExprKind, StmtKind};

fn source(source: &str) -> (luanalysis_syntax::File, bind::BoundFile, flow::ProgramFlow) {
    let file = luanalysis_syntax::parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let bound = bind::bind(&file);
    let graph = flow::build(&file, &bound);
    (file, bound, graph)
}

#[test]
fn original_function_spans_associate_native_declarations_with_detached_graphs() {
    let (file, _, graph) = source("local function outer() return function() return 1 end end");
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!("function declaration")
    };
    let outer = graph.functions[&(function.span.start, function.span.end)];
    assert_eq!(
        graph.graphs[outer as usize].function_span,
        Some(function.span)
    );
    let StmtKind::Return(values) = &function.body[0].kind else {
        panic!("return")
    };
    let ExprKind::Function(inner) = &values[0].kind else {
        panic!("closure")
    };
    let inner_graph = graph.functions[&(inner.span.start, inner.span.end)];
    assert_ne!(inner_graph, outer);
    let creation = graph.graphs[outer as usize]
        .nodes
        .iter()
        .find(|node| matches!(node.kind,NodeKind::CreateFunction {graph,..} if graph==inner_graph))
        .unwrap();
    let NodeKind::CreateFunction { expression, .. } = creation.kind else {
        unreachable!()
    };
    assert_eq!(graph.expression(expression).expression.span, inner.span);
    assert!(graph.graphs[0].returns.is_empty());
}

#[test]
fn return_expression_demands_preserve_distinct_branch_flow_points() {
    let (_, bound, graph) = source(
        "---@param flag boolean\nlocal function choose(flag) if flag then return 1 else return 2 end end",
    );
    let function = &graph.graphs[1];
    assert_eq!(function.returns.len(), 2);
    let inputs: Vec<_> = function
        .returns
        .iter()
        .map(|node| {
            let NodeKind::Return { values } = &graph.node(*node).kind else {
                panic!("return")
            };
            let value = graph.evaluation(values[0]);
            assert_eq!(value.completed, graph.node(*node).antecedents[0]);
            value.input
        })
        .collect();
    assert_ne!(inputs[0], inputs[1]);
    for (index, input) in inputs.iter().enumerate() {
        let NodeKind::Predicate {
            predicate: Predicate::Expression(expression),
            sense,
        } = graph.node(*input).kind
        else {
            panic!("branch input")
        };
        assert_eq!(sense, index == 0);
        let reference = &graph.references[&expression];
        let ReferenceRoot::Binding(binding) = reference.root else {
            panic!("bound parameter")
        };
        assert_eq!(bound.declaration(binding).name, "flag");
    }
    assert!(matches!(
        graph.node(function.exit).kind,
        NodeKind::Unreachable
    ));
}

#[test]
fn declaration_constraint_demand_uses_original_initializer_input_not_capture_point() {
    let (_, bound, graph) = source(
        "---@type number|string\nlocal source=1\nsource=\"text\"\nlocal copied=source\nlocal function read() return copied end",
    );
    let copied = bound
        .declarations
        .iter()
        .find(|decl| decl.name == "copied")
        .unwrap()
        .id;
    let write=graph.graphs[0].nodes.iter().find(|node|matches!(&node.kind,NodeKind::Write {address,..} if address.reference.root==ReferenceRoot::Binding(copied))).unwrap();
    let NodeKind::Write {
        value: ValueDemand::Evaluation(value),
        ..
    } = write.kind
    else {
        panic!("initializer demand")
    };
    let evaluation = graph.evaluation(value);
    let NodeKind::Write {
        declaration: false, ..
    } = graph.node(evaluation.input).kind
    else {
        panic!("initializer observes source's preceding assignment")
    };
    let source = bound
        .declarations
        .iter()
        .find(|decl| decl.name == "source")
        .unwrap()
        .id;
    assert_eq!(
        graph.references[&evaluation.expression].root,
        ReferenceRoot::Binding(source)
    );
    let returned = &graph.graphs[1];
    let NodeKind::Return { values } = &graph.node(returned.returns[0]).kind else {
        panic!("return")
    };
    assert_eq!(
        graph.references[&graph.evaluation(values[0]).expression].root,
        ReferenceRoot::Binding(copied)
    );
    assert_ne!(evaluation.input.graph, returned.entry.graph);
}

#[test]
fn loop_rhs_demands_keep_input_epoch_before_store_and_form_visible_back_edge() {
    let (_, _, graph) = source("local x=1\nwhile x do x=x end");
    let write = graph.graphs[0]
        .nodes
        .iter()
        .find(|node| {
            matches!(
                node.kind,
                NodeKind::Write {
                    declaration: false,
                    ..
                }
            )
        })
        .unwrap();
    let NodeKind::Write {
        address,
        value: ValueDemand::Evaluation(value),
        ..
    } = &write.kind
    else {
        panic!("loop write")
    };
    let evaluation = graph.evaluation(*value);
    assert_eq!(evaluation.input, address.epoch);
    assert_eq!(evaluation.completed, write.antecedents[0]);
    assert!(evaluation.input.index < write.id.index);
    let header = graph.graphs[0]
        .nodes
        .iter()
        .find(|node| matches!(node.kind, NodeKind::Loop))
        .unwrap();
    assert_eq!(header.antecedents[1], write.id);
}

#[test]
fn simultaneous_owner_replacement_keeps_old_receiver_and_all_transaction_writes() {
    for assignment in ["item,item.x={x=2},3", "item.x,item=3,{x=2}"] {
        let (_, _, graph) = source(&format!(
            "local item={{x=1}}\n{assignment}\nlocal observed=item.x"
        ));
        let writes: Vec<_> = graph.graphs[0]
            .nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.kind,
                    NodeKind::Write {
                        declaration: false,
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(writes.len(), 2);
        let addresses: Vec<_> = writes
            .iter()
            .map(|node| match &node.kind {
                NodeKind::Write { address, .. } => address,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(addresses[0].transaction, addresses[1].transaction);
        assert_eq!(addresses[0].epoch, addresses[1].epoch);
        let field = addresses
            .iter()
            .find(|address| !address.reference.path.is_empty())
            .unwrap();
        let receiver = graph.evaluation(field.receiver.unwrap());
        assert!(receiver.completed.index < writes[0].id.index);
        assert_eq!(receiver.input, field.epoch);
        assert_eq!(addresses[0].reference.root, addresses[1].reference.root);
    }
}
