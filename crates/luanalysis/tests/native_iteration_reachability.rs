use luanalysis::{CheckReport, check};

fn accepted(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}

fn inferred(report: &CheckReport, name: &str) -> String {
    report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .unwrap()
        .ty
        .clone()
}

// Reachability is structural, as in TypeScript: declared iterator result types
// never make a loop body or its exit dead code. Such findings belong to
// inspections, not checker reachability.
#[test]
fn zero_results_leave_body_returns_structurally_reachable() {
    let report = accepted(
        "---@param state nil\n---@param control nil\n---@return void\nlocal function stop(state,control) end\nlocal function read()\nfor key,value in stop,nil,nil do return \"unreachable\" end\nreturn 42\nend\nlocal actual=read()",
    );
    assert_eq!(inferred(&report, "actual"), "string | number");
}

#[test]
fn a_guaranteed_first_result_does_not_exclude_implicit_fallthrough() {
    let report = check(
        "---@param state nil\n---@param control nil|number\n---@return number,string\nlocal function next(state,control) return 1,\"payload\" end\n---@return string\nlocal function read()\nfor key,value in next,nil,nil do return value end\nend\nlocal actual=read()",
    );
    let codes: Vec<_> = report.diagnostics.iter().map(|diagnostic| &diagnostic.code[..]).collect();
    assert_eq!(codes, ["TS2355"]);
    assert_eq!(inferred(&report, "actual"), "string");
}

#[test]
fn a_nonreturning_iterator_type_leaves_body_and_after_loop_returns_reachable() {
    let report = accepted(
        "---@param state nil\n---@param control nil\n---@return never\nlocal function stop(state,control) while true do end end\nlocal function read()\nfor key,value in stop,nil,nil do return \"unreachable\" end\nreturn 42\nend\nlocal actual=read()",
    );
    assert_eq!(inferred(&report, "actual"), "string | number");
}
