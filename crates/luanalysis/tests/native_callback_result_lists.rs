use luanalysis::check;

#[test]
fn v1_callback_bare_results_infer_distinct_dictionary_key_and_value_types() {
    let source = r#"---@type fun<T, RK, RV>(arr: T[], callback: fun(obj: T, i: number): RK, RV): table<RK, RV>
local mapToTable
---@type string[]
local strings
local mapped=mapToTable(strings,function(str,i) return i,str end)
---@type table<number,string>
local accepted=mapped
"#;
    let accepted = check(source);
    assert!(accepted.diagnostics.is_empty(), "{:#?}", accepted.diagnostics);
    let rejected = check(&source.replace("local accepted=mapped", "local accepted=mapped\n---@type table<string,number>\nlocal rejected=mapped"));
    assert_eq!(rejected.diagnostics.len(), 1, "{:#?}", rejected.diagnostics);
    assert_eq!(rejected.diagnostics[0].code, "TS2322");
}

#[test]
fn v1_callback_bare_results_preserve_both_forwarded_generic_slots() {
    let source = r#"---@type fun<A, B>(func: fun(): A, B): A, B
local apply
local first,second=apply(function() return 1,'ok' end)
---@type number,string
local count,text=first,second
"#;
    let accepted = check(source);
    assert!(accepted.diagnostics.is_empty(), "{:#?}", accepted.diagnostics);
    let rejected = check(&source.replace("number,string", "string,number"));
    assert_eq!(rejected.diagnostics.len(), 2, "{:#?}", rejected.diagnostics);
    assert!(rejected.diagnostics.iter().all(|diagnostic| diagnostic.code == "TS2322"));
}

#[test]
fn concrete_callback_bare_results_reject_missing_and_incompatible_second_values() {
    let source = r#"---@type fun(callback: fun(): number,string): void
local consume
consume(function() return 1,'ok' end)
"#;
    let accepted = check(source);
    assert!(accepted.diagnostics.is_empty(), "{:#?}", accepted.diagnostics);
    for wrong in ["return 1", "return 1,true"] {
        let rejected = check(&source.replace("return 1,'ok'", wrong));
        assert_eq!(rejected.diagnostics.len(), 1, "{wrong}: {:#?}", rejected.diagnostics);
        assert_eq!(rejected.diagnostics[0].code, "TS2345");
    }
}
