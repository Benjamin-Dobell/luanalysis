use luanalysis::{CheckReport, check};

const RESULT: &str = "---@param ready boolean\n---@return false | (true, number)\nlocal function result(ready) if ready then return true,42 end return false end\n";

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}

#[test]
fn equality_and_truthy_tests_filter_the_complete_original_result_list() {
    for condition in [
        "success",
        "success==true",
        "false~=success",
        "type(success)==\"boolean\" and success",
        "not (not success)",
        "success or false",
    ] {
        let source = format!(
            "{RESULT}---@param ready boolean\nlocal function use(ready) local success,value=result(ready)\nif {condition} then\n---@type number\nlocal present=value\nelse\n---@type nil\nlocal absent=value\nend end"
        );
        clean(&source);
    }
}

#[test]
fn writes_to_either_sibling_disable_immutable_binding_correlation() {
    for write in ["success=true", "value=nil"] {
        let source = format!(
            "{RESULT}---@param ready boolean\nlocal function use(ready) local success,value=result(ready)\n{write}\nif success then\n---@type number\nlocal invalid=value\nend end"
        );
        let report = check(&source);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "TS2322"
                    && &source[diagnostic.span.start..diagnostic.span.end] == "value"),
            "{write}: {:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn independent_calls_do_not_share_discriminant_facts() {
    let source = format!(
        "{RESULT}---@param ready boolean\nlocal function use(ready) local first,one=result(ready) local second,two=result(ready)\nif first then\n---@type number\nlocal present=one\n---@type number\nlocal invalid=two\nend end"
    );
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "two"
    );
}

#[test]
fn single_row_calls_retain_expected_inference_and_declared_member_views() {
    let source = "---@generic T\n---@return T\nlocal function make() return nil end\n---@type number\nlocal value=make()\n---@type number\nlocal same=value\n---@return {id:number, hidden:string}\nlocal function item() return {id=1,hidden=\"text\"} end\n---@type {id:number}\nlocal visible=item()\nlocal invalid=visible.hidden";
    let report = check(source);
    // The invalid generic body and hidden member are real errors; the expected
    // Number binding must not acquire Unknown from a new uncontextualized call.
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| &source[diagnostic.span.start..diagnostic.span.end] == "value"),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "TS2339"
                && &source[diagnostic.span.start..diagnostic.span.end] == "hidden"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn correlated_rows_keep_the_explicit_member_constraint_after_narrowing() {
    let source = "---@param ready boolean\n---@return false | (true, {id:number, hidden:string})\nlocal function item(ready) if ready then return true,{id=1,hidden=\"text\"} end return false end\n---@param ready boolean\nlocal function use(ready)\n---@type boolean, nil|{id:number}\nlocal success,visible=item(ready)\nif success then\n---@type number\nlocal id=visible.id\nlocal invalid=visible.hidden\nend end";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2339");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "hidden"
    );
}
