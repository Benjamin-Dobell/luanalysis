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
fn a_global_assigned_in_a_generic_function_is_any_rather_than_its_type_parameter() {
    let source = "---@generic T\n---@param value T\n---@return void\nlocal function publish(value) stored = value end\npublish(42)\nlocal copy = stored\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn globals_assigned_only_inside_functions_are_declared_there() {
    // Lua creates a global wherever one is assigned. A top-level assignment in any file still
    // declares it, and other writes are checked against that.
    let source = "local function init()\n    ROOTDIR = '/tmp/'\n    local function nested() function HANDLER() return 1 end end\nend\ninit()\n---@type string\nlocal path = ROOTDIR .. 'x'\n---@type number\nlocal handled = HANDLER()\nCOUNT = 0\nlocal function bump() COUNT = 'x' end\n";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(&source[report.diagnostics[0].span.start..report.diagnostics[0].span.end], "'x'");
}

#[test]
fn original_global_function_contract_contextualizes_untyped_rhs_parameters() {
    let report = check(
        "---@type fun(value:number):number\ntransform = function(value) return value + 1 end\n---@type number\nlocal result = transform(41)\n",
    );
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}
