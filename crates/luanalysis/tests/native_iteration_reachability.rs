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

#[test]
fn zero_results_exclude_body_returns_and_keep_the_after_loop_return() {
    let report = accepted(
        "---@param state nil\n---@param control nil\n---@return void\nlocal function stop(state,control) end\nlocal function read()\nfor key,value in stop,nil,nil do return \"unreachable\" end\nreturn 42\nend\nlocal actual=read()",
    );
    assert_eq!(inferred(&report, "actual"), "number");
}

#[test]
fn a_guaranteed_first_result_and_body_return_exclude_implicit_fallthrough() {
    let report = accepted(
        "---@param state nil\n---@param control nil|number\n---@return number,string\nlocal function next(state,control) return 1,\"payload\" end\n---@return string\nlocal function read()\nfor key,value in next,nil,nil do return value end\nend\nlocal actual=read()",
    );
    assert_eq!(inferred(&report, "actual"), "string");
}

#[test]
fn a_nonreturning_iterator_call_excludes_both_body_and_after_loop_returns() {
    let report = accepted(
        "---@param state nil\n---@param control nil\n---@return never\nlocal function stop(state,control) while true do end end\nlocal function read()\nfor key,value in stop,nil,nil do return \"unreachable\" end\nreturn 42\nend\nlocal actual=read()",
    );
    assert_eq!(inferred(&report, "actual"), "never");
}
