use luanalysis::check;

#[test]
fn implementation_global_initialization_and_read_use_original_source_queries() {
    let report = check("answer = 42\n---@type number\nlocal result = answer\n");
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert!(
        report
            .bindings
            .iter()
            .any(|binding| binding.name == "result" && binding.ty == "number"),
        "{:#?}",
        report.bindings
    );
}

#[test]
fn implementation_global_reassignment_checks_writes_against_one_inferred_binding() {
    let report = check("answer = 1\nanswer = 2\n---@type number\nlocal result = answer\n");
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let invalid = check("answer = 1\nanswer = 'wrong'\n");
    assert!(
        invalid
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "TS2322" && diagnostic.span.start == 20),
        "{:#?}",
        invalid.diagnostics
    );
    assert!(
        !invalid
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "TS2403"),
        "{:#?}",
        invalid.diagnostics
    );
}

#[test]
fn self_referential_later_writes_and_written_union_contracts_use_the_original_introduction() {
    for source in [
        "count = 0\ncount = count + 1\n---@type number\nlocal value = count\n",
        "---@type number|string\nanswer = 42\nanswer = 'valid'\n---@type number|string\nlocal value = answer\n",
    ] {
        let report = check(source);
        assert!(
            report.diagnostics.is_empty(),
            "{source}\n{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn global_writes_from_generic_bodies_cannot_silently_escape_original_type_parameters() {
    let source = "---@generic T\n---@param value T\n---@return void\nlocal function publish(value) stored = value end\npublish(42)\n";
    let report = check(source);
    let gate = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "LUA2037")
        .expect("generic global write needs an explicit environment contract");
    assert_eq!(&source[gate.span.start..gate.span.end], "stored");
}

#[test]
fn original_global_function_contract_contextualizes_untyped_rhs_parameters() {
    let report = check(
        "---@type fun(value:number):number\ntransform = function(value) return value + 1 end\n---@type number\nlocal result = transform(41)\n",
    );
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}
