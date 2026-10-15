use luanalysis::{bind, check, flow};

#[test]
fn named_local_definitions_use_existing_bindings_and_native_signatures() {
    let source = "---@type fun(value: number): number\nlocal transform = function(value) return value end\n---@param value number\nfunction transform(value) return value + 1 end\nlocal answer = transform(41)\n---@type string\nlocal wrong = answer";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "answer"
    );
    let bound = report.bound.as_ref().unwrap();
    let declaration = source.find("local transform").unwrap() + 6;
    let write = source.find("function transform").unwrap() + 9;
    assert_eq!(bound.binding_at(declaration), bound.binding_at(write));
    assert_eq!(
        bound.occurrence_at(write).unwrap().kind,
        bind::OccurrenceKind::Write
    );
}

#[test]
fn member_definitions_check_through_declared_fields_and_calls() {
    let source = "---@type {read: fun(): number}\nlocal object = {read = function() return 1 end}\nfunction object.read() return 2 end\nlocal answer = object.read()\n---@type number\nlocal checked = answer";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "answer")
            .unwrap()
            .ty,
        "number"
    );
    let bound = report.bound.as_ref().unwrap();
    let receiver = source.find("function object.read").unwrap() + 9;
    let member = receiver + 7;
    assert!(bound.binding_at(receiver).is_some());
    assert!(bound.occurrence_at(member).is_none());
}

#[test]
fn invalid_member_definition_returns_keep_original_body_ranges() {
    let source = "---@type {read: fun(): number}\nlocal object = {read = function() return 1 end}\n---@return number\nfunction object.read() return \"wrong\" end";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "\"wrong\""
    );
}

#[test]
fn implicit_receivers_stay_explicit_semantic_gates() {
    for (source, origin) in [(
        "local object = {}\nfunction object:read() return 1 end",
        ":",
    )] {
        let report = check(source);
        let gate = report
            .diagnostics
            .iter()
            .find(|error| error.code == "LUA2021")
            .unwrap();
        assert_eq!(&source[gate.span.start..gate.span.end], origin);
    }
}

#[test]
fn original_global_functions_now_infer_through_the_native_global_binding() {
    let report = check("function global() return 1 end\nlocal answer = global()\n");
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert!(
        report
            .bindings
            .iter()
            .any(|binding| binding.name == "answer" && binding.ty == "number"),
        "{:#?}",
        report.bindings
    );
}

#[test]
fn member_graphs_capture_addresses_and_keep_bodies_detached() {
    let source = "local object = {}\nfunction object.read() return 1 end";
    let file = luanalysis_syntax::parse(source);
    assert!(file.diagnostics.is_empty());
    let bound = bind::bind(&file);
    let graph = flow::build(&file, &bound);
    assert_eq!(graph.graphs.len(), 2);
    assert!(graph.graphs[0].returns.is_empty());
    assert_eq!(graph.graphs[1].returns.len(), 1);
    let write = graph.graphs[0]
        .nodes
        .iter()
        .find_map(|node| match &node.kind {
            flow::NodeKind::Write {
                address,
                value: flow::ValueDemand::Function(function),
                declaration,
                ..
            } => Some((address, function, declaration)),
            _ => None,
        })
        .unwrap();
    assert_eq!(*write.1, 1);
    assert!(!write.2);
    assert!(write.0.receiver.is_some());
}
