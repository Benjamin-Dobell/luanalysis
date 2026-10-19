use luanalysis::check;

const FIXED: &str = r#"
---@return string,number
local function produce() return 'ok',42 end
---@generic T
---@param producer fun():T
---@return boolean,T
local function tag(producer) return true,producer() end
---@type boolean,string,number
local flag,text,count=tag(produce)
---@type boolean,boolean,string,number
local flag1,flag2,text2,count2=tag(produce),tag(produce)
"#;

#[test]
fn generic_callback_carries_its_whole_result_sequence_through_a_final_type_parameter() {
    let report = check(FIXED);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let parenthesized = check(&FIXED.replace("fun():T", "fun():(T)"));
    assert!(parenthesized.diagnostics.is_empty(), "{:#?}", parenthesized.diagnostics);
    for (written, replaced) in [
        ("boolean,string,number", "boolean,string,string"),
        ("boolean,string,number", "boolean,number,number"),
        ("boolean,boolean,string,number", "boolean,boolean,string,boolean"),
    ] {
        let source = FIXED.replacen(written, replaced, 1);
        let rejected = check(&source);
        assert_eq!(rejected.diagnostics.len(), 1, "{source}\n{:#?}", rejected.diagnostics);
        assert_eq!(rejected.diagnostics[0].code, "TS2322");
    }
}

#[test]
fn generic_callback_preserves_heterogeneous_prefixes_and_nullable_variadic_tails() {
    let source = r#"
---@return number,boolean...
local function produce() return 1,true,false end
---@generic T
---@param producer fun():T
---@return boolean,T
local function tag(producer) return true,producer() end
---@type boolean,number,boolean|nil,boolean|nil
local flag,count,first,second=tag(produce)
"#;
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let wrong = source.replace("boolean|nil,boolean|nil", "string|nil,boolean|nil");
    let rejected = check(&wrong);
    assert_eq!(rejected.diagnostics.len(), 1, "{:#?}", rejected.diagnostics);
    assert_eq!(rejected.diagnostics[0].code, "TS2322");
}

#[test]
fn generic_result_sequences_keep_scalar_arguments_and_constraints_independent() {
    let source = r#"
---@generic T
---@param value T
---@return T
local function identity(value) return value end
---@type number
local number_result=identity(42)
---@type string
local string_result=identity('ok')
---@return number,string
local function pair() return 1,'ok' end
---@generic T:number
---@param producer fun():T
---@return T
local function constrained(producer) return producer() end
---@type number,string
local first,second=constrained(pair)
"#;
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let wrong = source.replace("---@return number,string\nlocal function pair() return 1,'ok' end", "---@return string,number\nlocal function pair() return 'bad',1 end");
    let rejected = check(&wrong);
    assert!(rejected.diagnostics.iter().any(|error| matches!(error.code, "TS2345" | "TS2769")), "{:#?}", rejected.diagnostics);
}

#[test]
fn ordinary_scalar_callback_results_still_reject_an_incompatible_physical_result_sequence() {
    let source = r#"
---@param producer fun():string
local function consume(producer) return producer() end
---@return string,number
local function pair() return 'ok',1 end
consume(pair)
"#;
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2345");
}

#[test]
fn a_generic_pack_contract_cannot_be_implemented_by_returning_only_its_scalar_first_value() {
    let source = r#"
---@generic T
---@param producer fun():T
---@param value T
---@return T
local function truncated(producer,value) return value end
"#;
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
}

#[test]
fn symbolic_later_results_cannot_be_assumed_to_be_nil_or_an_arbitrary_scalar() {
    let source = r#"
---@generic T
---@param producer fun():T
---@return T
local function forward(producer)
    local first,second=producer()
    return producer()
end
"#;
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    for expected in ["nil", "number", "string"] {
        let wrong = source.replace("    return producer()", &format!("    ---@type {expected}\n    local bad=second\n    return producer()"));
        let report = check(&wrong);
        assert_eq!(report.diagnostics.len(), 1, "{wrong}\n{:#?}", report.diagnostics);
        assert_eq!(report.diagnostics[0].code, "TS2322");
    }
}

#[test]
fn nested_and_recursive_generic_forwarding_retain_the_original_result_sequence() {
    let source = r#"
---@generic R
---@param producer fun():R
---@return R
local function forward(producer) return producer() end
---@generic T
---@param producer fun():T
---@param again boolean
---@return T
local function recursive(producer,again)
    if again then return recursive(producer,false) end
    return forward(producer)
end
---@return string,number
local function produce() return 'ok',42 end
---@type string,number
local first,second=recursive(produce,true)
"#;
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let wrong = source.replace("---@type string,number", "---@type string,string");
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
}

#[test]
fn competing_producers_preserve_all_later_slot_alternatives() {
    let source = r#"
---@return string,number
local function numbered() return 'number',42 end
---@return string,boolean
local function flagged() return 'flag',true end
---@generic T
---@param left fun():T
---@param right fun():T
---@param choose boolean
---@return T
local function either(left,right,choose)
    if choose then return left() else return right() end
end
---@type string,number|boolean
local first,second=either(numbered,flagged,true)
"#;
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    for expected in ["number", "boolean"] {
        let wrong = source.replace("---@type string,number|boolean", &format!("---@type string,{expected}"));
        let report = check(&wrong);
        assert_eq!(report.diagnostics.len(), 1, "{wrong}\n{:#?}", report.diagnostics);
        assert_eq!(report.diagnostics[0].code, "TS2322");
    }
}

#[test]
fn a_scalar_return_branch_cannot_satisfy_a_correlated_generic_result_contract() {
    let source = r#"
---@generic T
---@param producer fun():T
---@param value T
---@param choose boolean
---@return T
local function sometimes_truncated(producer,value,choose)
    if choose then return producer() end
    return value
end
"#;
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
}

#[test]
fn returning_a_typed_closure_does_not_turn_its_captured_scalar_parameter_into_a_result_pack() {
    let source = r#"
---@generic T
---@param value T
---@return fun():T
local function make(value)
    ---@return T
    local function get() return value end
    return get
end
local get=make(42)
---@type number
local value=get()
"#;
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let wrong = source.replace("---@type number", "---@type string");
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
}

#[test]
fn an_explicit_variadic_generic_output_repeats_the_input_producers_result_domain() {
    let source = r#"
---@generic T
---@param producer fun():T
---@return boolean,T...
local function repeat_first(producer) return true,producer(),producer(),producer() end
---@return string
local function scalar() return 'ok' end
---@type boolean,string|nil,string|nil,string|nil
local flag,first,second,third=repeat_first(scalar)
"#;
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let multiple = source.replace("---@return string\nlocal function scalar() return 'ok' end", "---@return string,number\nlocal function scalar() return 'ok',42 end");
    let multiple = multiple.replace("string|nil,string|nil,string|nil", "string|nil,string|number|nil,string|number|nil");
    let report = check(&multiple);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let wrong = multiple.replace("string|nil,string|number|nil", "string|nil,number|nil");
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
}

#[test]
fn a_variadic_input_annotation_does_not_disable_a_final_generic_result_sequence() {
    let source = FIXED.replace("---@return boolean,T\nlocal function tag(producer)", "---@vararg T\n---@return boolean,T\nlocal function tag(producer,...)");
    let report = check(&source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn the_cli_preserves_generic_producer_results_and_rejects_a_later_slot_mismatch() {
    use std::{fs, process::Command};
    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }
    let fixture = Fixture(std::env::temp_dir().join(format!("luanalysis generic result packs {}", std::process::id())));
    fs::create_dir_all(&fixture.0).unwrap();
    let path = fixture.0.join("generic.lua");
    let run = |source: &str| {
        fs::write(&path, source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .args(["check", "--dump-types"]).arg(&path).output().unwrap()
    };
    let rejected = run(&FIXED.replacen("boolean,string,number", "boolean,string,string", 1));
    let errors = String::from_utf8_lossy(&rejected.stderr);
    assert_eq!(rejected.status.code(), Some(1), "{errors}");
    assert!(errors.contains("error[TS2322]"), "{errors}");
    assert!(!errors.contains("LUA2019"), "{errors}");
    let accepted = run(FIXED);
    let errors = String::from_utf8_lossy(&accepted.stderr);
    assert_eq!(accepted.status.code(), Some(0), "{errors}");
    assert!(errors.is_empty(), "{errors}");
    assert!(String::from_utf8_lossy(&accepted.stdout).contains("count: number"));
}

const REDUCE: &str = r#"
---@generic V,R
---@param value V
---@param initial R
---@param func nil|fun(memo:R,value:V):R
---@return R
local function reduce(value,initial,func)
    ---@type R
    local memo
    ---@type fun(memo:R,value:V):R
    local reducer
    if func then
        memo=initial
        reducer=func
    else
        return --[[---@type any]] nil
    end
    memo=reducer(memo,value)
    return memo
end
local total=reduce(1,0,function(memo,value) return memo+value end)
---@type number
local accepted=total
"#;

#[test]
fn a_reducer_callback_keeps_its_shared_input_and_output_generic_scalar() {
    let report = check(REDUCE);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let wrong = REDUCE.replace("---@type number", "---@type string");
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    let wrong_body = REDUCE.replace("return memo\nend", "return 'wrong'\nend");
    let report = check(&wrong_body);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    let wrong_reducer = REDUCE.replace("return memo+value", "return 'wrong'");
    let report = check(&wrong_reducer);
    assert!(report.diagnostics.iter().any(|error| matches!(error.code, "TS2322" | "TS2345")), "{:#?}", report.diagnostics);
}

#[test]
fn a_producer_with_unrelated_scalar_inputs_still_preserves_its_complete_results() {
    let source = r#"
---@param seed number
---@return string,number
local function produce(seed) return 'ok',seed end
---@generic T
---@param producer fun(seed:number):T
---@return T
local function run(producer) return producer(42) end
---@type string,number
local text,count=run(produce)
"#;
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let wrong = source.replace("---@type string,number", "---@type string,string");
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
}

#[test]
fn an_input_callables_own_shadowing_generic_does_not_scalarize_its_outer_result() {
    let source = r#"
---@param seed fun<R>(value:R):R
---@return string,number
local function produce(seed) return 'ok',42 end
---@generic R
---@param producer fun(seed:fun<R>(value:R):R):R
---@return R
local function run(producer)
    return producer(function(value) return value end)
end
---@type string,number
local text,count=run(produce)
"#;
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let wrong = source.replace("---@type string,number", "---@type string,string");
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
}

#[test]
fn nested_contextual_producers_keep_actual_heterogeneous_results_over_weak_contextual_candidates() {
    let source = r#"
---@generic T,R
---@param value T
---@param project fun(value:T):R
---@return R
local function transform(value,project) return project(value) end
local text,count=transform(42,function(value)
    return transform('text',function(inner) return inner,value end)
end)
---@type string
local accepted_text=text
---@type number
local accepted_count=count
"#;
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    for name in ["accepted_text", "accepted_count"] {
        let (written, wrong) = if name == "accepted_text" { ("---@type string", "---@type number") } else { ("---@type number", "---@type string") };
        let source = source.replace(written, wrong);
        let report = check(&source);
        assert_eq!(report.diagnostics.len(), 1, "{name}: {:#?}", report.diagnostics);
        assert_eq!(report.diagnostics[0].code, "TS2322");
    }
}
