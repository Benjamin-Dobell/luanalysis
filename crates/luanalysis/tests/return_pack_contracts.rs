use luanalysis::check;

fn accepted(source: &str) {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
}

fn rejected_at(source: &str, expression: &str) {
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{source}\n{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert_eq!(error.code, "TS2322", "{error:#?}");
    assert_eq!(&source[error.span.start..error.span.end], expression, "{error:#?}");
}

#[test]
fn return_pack_errors_identify_the_first_wrong_explicit_value() {
    for annotation in ["---@return number,boolean...\nlocal function values()\n", "local function values()\n---@type number,boolean...\n"] {
        rejected_at(&format!("{annotation}return 1,'wrong',false\nend"), "'wrong'");
        rejected_at(&format!("{annotation}return 'wrong',true,false\nend"), "'wrong'");
        rejected_at(&format!("{annotation}return 1,true,'wrong'\nend"), "'wrong'");
        accepted(&format!("{annotation}return 1,true,false\nend"));
        accepted(&format!("{annotation}return 1\nend"));
    }
}

#[test]
fn return_pack_errors_preserve_correlated_alternatives_when_selecting_the_span() {
    let declaration = "---@param flag boolean\n---@return (true,number) | (false,string)\nlocal function values(flag)\n";
    rejected_at(&format!("{declaration}return true,'wrong'\nend"), "'wrong'");
    rejected_at(&format!("{declaration}return false,42\nend"), "42");
    accepted(&format!("{declaration}if flag then return true,42 end\nreturn false,'text'\nend"));
}

#[test]
fn a_written_variadic_tail_is_the_contract_however_many_values_the_body_returns() {
    // Callers see the written contract, as v1 and TypeScript publish it: `producer` may return
    // fewer than three values, whatever its body happens to return now.
    rejected_at("---@return number,string,boolean...\nlocal function producer() return 1,'text',true end\n---@return number,string,boolean\nlocal function forward() return producer() end\n", "producer()");
    let unknown = "---@param producer fun():number,string,boolean...\n---@return number,string,boolean\nlocal function forward(producer) return producer() end\n";
    rejected_at(unknown, "producer()");
    accepted(&unknown.replace("---@return number,string,boolean\n", "---@return number,string,boolean...\n"));
}

#[test]
fn scalar_any_contracts_do_not_erase_the_physical_shape_of_typed_result_packs() {
    accepted("---@param value any\n---@return any\nlocal function scalar(value) return value end\n");
    // A variadic tail past the written values is dropped, as Lua drops it; a fixed excess value
    // is still an error.
    let variadic = "---@vararg any\n---@return any\nlocal function forward(...) return --[[---@not nil]] ... end\n";
    accepted(variadic);
    accepted(&variadic.replace("---@return any\n", "---@return any...\n"));
    let fixed = "---@return any\nlocal function pair() return 1,true end\n";
    rejected_at(fixed, "true");
    accepted(&fixed.replace("---@return any\n", "---@return any...\n"));
}
