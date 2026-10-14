//! Acceptance gates for native Lua result packs. `ignore` means the named
//! backend/source contract is pending, not that an unsupported diagnostic passes.
//! Provenance and intentional legacy differences: docs/LUA-VALUES-ACCEPTANCE.md.

use luanalysis::{CheckReport, check};

const PAIR: &str = "---@return number, string\nlocal function pair() return 11, \"tail\" end\n";

fn accepted(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}

fn binding(report: &CheckReport, name: &str, expected: &str) {
    let actual = report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .unwrap_or_else(|| panic!("missing {name}: {:#?}", report.bindings));
    assert_eq!(actual.ty, expected, "binding {name}");
}

fn type_error(source: &str, code: &str, original: &str) {
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert_eq!(error.code, code, "{error:#?}");
    assert_eq!(&source[error.span.start..error.span.end], original);
}

#[test]
fn fixed_results_expand_only_at_the_end_of_an_assignment_list() {
    // Reduction: legacy function_multiple_returns.lua:15-28. Count warnings
    // are intentionally omitted: unannotated slots pad nil or discard values.
    let source = format!(
        "{PAIR}\
local number,text,missing=pair()\n\
local prefix,lastnumber,lasttext=7,pair()\n\
local single,after,tail=pair(),7\n\
local parenthesized,parenthesized_missing=(pair())"
    );
    let report = accepted(&source);
    for name in [
        "number",
        "prefix",
        "lastnumber",
        "single",
        "after",
        "parenthesized",
    ] {
        binding(&report, name, "number");
    }
    for name in ["text", "lasttext"] {
        binding(&report, name, "string");
    }
    for name in ["missing", "tail", "parenthesized_missing"] {
        binding(&report, name, "nil");
    }
}

#[test]
fn expanded_slot_mismatch_is_reported_at_the_original_call() {
    let source = format!("{PAIR}---@type number, number\nlocal first,second=pair()");
    type_error(&source, "TS2322", "pair()");
}

#[test]
fn no_result_is_empty_in_a_list_and_nil_when_scalarized() {
    let source = "local function nothing() end\n\
local absent1,absent2=nothing()\n\
local scalar=(nothing())\n\
local function forwarded() return nothing() end\n\
local forwarded1,forwarded2=forwarded()";
    let report = accepted(source);
    for name in ["absent1", "absent2", "scalar", "forwarded1", "forwarded2"] {
        binding(&report, name, "nil");
    }
}

#[test]
fn final_call_argument_expands_and_parentheses_remove_the_extra_argument() {
    // Reduction: legacy function_multiple_returns.lua:68-75.
    let consume = "---@param number number\n---@param text string\n\
local function consume(number,text) end\n";
    accepted(&format!("{PAIR}{consume}consume(pair())"));
    let source = format!("{PAIR}{consume}consume((pair()))");
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2554");
    // Arity belongs to the original call, even when its argument is multires.
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "consume((pair()))"
    );
}

#[test]
fn empty_last_argument_does_not_supply_a_required_nil_parameter() {
    let declarations = "local function nothing() end\n---@param value nil\n\
local function consume(value) end\n";
    accepted(&format!("{declarations}consume((nothing()))"));
    let source = format!("{declarations}consume(nothing())");
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2554");
}

#[test]
fn nil_only_annotation_does_not_invent_a_result_for_an_empty_body() {
    let declarations = "---@return nil\nlocal function nothing() end\nlocal function zero() end\n";
    accepted(&format!("{declarations}zero(nothing())"));
    let report = check(&format!("{declarations}zero((nothing()))"));
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2554");

    // The same scalar bound on an explicit return really supplies one nil.
    let report = check(
        "---@return nil\nlocal function one() return nil end\n\
local function zero() end\nzero(one())",
    );
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2554");
}

#[test]
fn final_return_forwards_the_pack_but_operators_return_a_scalar() {
    let source = format!(
        "{PAIR}\
---@return boolean, number, string\n\
local function forwarded() return true,pair() end\n\
local function scalar() return true and pair() end\n\
local flag,number,text=forwarded()\n\
local first,missing=scalar()"
    );
    let report = accepted(&source);
    binding(&report, "flag", "boolean");
    binding(&report, "number", "number");
    binding(&report, "text", "string");
    binding(&report, "first", "number");
    binding(&report, "missing", "nil");
}

#[test]
fn generic_pack_results_do_not_mix_sibling_call_instantiations() {
    let source = "---@generic T\n---@param value T\n\
local function tagged(value) return value,\"tag\" end\n\
local number,number_tag=tagged(42)\n\
local text,text_tag=tagged(\"value\")\n\
local again,again_tag=tagged(7)";
    let report = accepted(source);
    binding(&report, "number", "number");
    binding(&report, "text", "string");
    binding(&report, "again", "number");
    for name in ["number_tag", "text_tag", "again_tag"] {
        binding(&report, name, "string");
    }
}

#[test]
fn varargs_forward_and_scalarize_using_the_original_runtime_scope() {
    let source = "---@vararg number\n---@return number...\n\
local function forward(...) return ... end\n\
---@vararg number\n---@return nil|number, nil\n\
local function scalar(...) return (...),nil end\n\
local a,b,c=forward(1,2)\n\
local head,missing=scalar(1,2)\n\
---@type nil|number\nlocal accepted_a=a\n\
---@type nil|number\nlocal accepted_b=b\n\
---@type nil|number\nlocal accepted_c=c\n\
---@type nil|number\nlocal accepted_head=head\n\
---@type nil\nlocal accepted_missing=missing";
    // An unconstrained homogeneous variadic tail can be empty; do not infer
    // guaranteed positions merely from the body returning an ellipsis.
    accepted(source);
}

#[test]
fn annotated_vararg_parameters_check_every_expanded_argument_slot() {
    let declarations = "---@param head number\n---@vararg number\n\
local function consume(head,...) end\n";
    accepted(&format!("{declarations}consume(1,2,3)"));
    type_error(
        &format!("{declarations}consume(1,2,\"bad\")"),
        "TS2345",
        "\"bad\"",
    );
}

#[test]
fn coroutine_yield_result_keeps_its_boolean_discriminant_and_payload_correlated() {
    // Exact annotation pattern from ge_tts/Coroutine.ttslua:27. Reduce actual
    // scheduling/Wait/metatable APIs, retain generic false|(true,R) alternatives.
    let source = "---@generic R\n---@param ready boolean\n---@param value R\n\
---@return false | (true, R)\n\
local function yieldResult(ready,value)\n\
if ready then return true,value end\nreturn false\nend\n\
---@param ready boolean\n\
local function use(ready)\n\
local success,result=yieldResult(ready,42)\n\
if success then\n---@type number\nlocal number=result\n\
else\n---@type nil\nlocal absent=result\nend\nend";
    accepted(source);
}

#[test]
fn correlated_return_contract_rejects_a_crossed_discriminant_payload() {
    let source = "---@return (false, string) | (true, number)\n\
local function invalid() return false,42 end";
    let report = check(source);
    assert!(
        !report.diagnostics.is_empty(),
        "crossed alternatives silently accepted"
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|error| error.code == "TS2322"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
#[ignore = "pending native iterator pack return plus generic-for flow effects"]
fn fixed_length_iterator_retains_the_key_value_return_pair() {
    // ge_tts/TableUtils.ttslua:9-17 reduction: std__Packed<T> is replaced by
    // one explicit record field so this gate isolates iterator result packs.
    let source = "---@param length number\n\
---@return fun(state: {value: string}, i: number): nil | (number, string)\n\
local function fixedLengthIterator(length)\n\
---@type fun(state: {value: string}, i: number): nil | (number, string)\n\
local iterator=function(state,i)\ni=i+1\n\
if i<=length then return i,state.value end\nend\nreturn iterator\nend\n\
for index,value in fixedLengthIterator(1),{value=\"item\"},0 do\n\
---@type number\nlocal accepted_index=index\n\
---@type string\nlocal accepted_value=value\nend";
    accepted(source);
}

#[test]
fn coroutine_start_pattern_forwards_its_arguments_and_prefixes_the_result_pack() {
    // ge_tts/Coroutine.ttslua:254-258 reduction: replace coroutine library
    // execution by an explicitly typed callable while retaining both ellipses.
    let source = "---@param func fun(head: number, ...: number): number, string\n\
---@param head number\n---@vararg number\n---@return boolean, number, string\n\
local function start(func,head,...) return true,func(head,...) end\n\
---@param head number\n---@return number, string\n\
local function pair(head) return head,\"done\" end\n\
local success,value,text=start(pair,42,7)\n\
---@type boolean\nlocal accepted_success=success\n\
---@type number\nlocal accepted_value=value\n\
---@type string\nlocal accepted_text=text";
    accepted(source);
}

#[test]
fn cli_expanded_slot_error_disappears_after_original_annotation_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis pack acceptance {}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("packs.lua");
    let source = format!("{PAIR}---@type number, number\nlocal first,second=pair()");
    fs::write(&path, &source).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .arg("check")
            .arg("--dump-types")
            .arg(&path)
            .output()
            .unwrap()
    };
    let bad = run();
    assert_eq!(bad.status.code(), Some(1));
    let error = String::from_utf8_lossy(&bad.stderr);
    assert!(error.contains("TS2322"), "{error}");
    assert!(
        !error.contains("LUA2016"),
        "unsupported packs do not pass an acceptance gate: {error}"
    );
    fs::write(
        &path,
        source.replace("---@type number, number", "---@type number, string"),
    )
    .unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    let types = String::from_utf8_lossy(&corrected.stdout);
    assert!(
        types.contains("first: number") && types.contains("second: string"),
        "{types}"
    );
    fs::remove_dir_all(directory).unwrap();
}
