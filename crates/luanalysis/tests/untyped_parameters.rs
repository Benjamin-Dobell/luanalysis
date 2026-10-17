//! As TypeScript reads a JavaScript function without JSDoc, a function written with neither
//! `---@param` nor `---@type` can be called with fewer arguments: Lua passes nil for the rest.
//! That makes the parameters optional to callers only; in the body they keep their types.
use luanalysis::{CheckOptions, check, check_with_options};

fn codes(source: &str) -> Vec<&'static str> {
    check(source).diagnostics.iter().map(|diagnostic| diagnostic.code).collect()
}

#[test]
fn an_untyped_function_can_be_called_with_fewer_arguments() {
    assert!(codes("local function skip(action, limit) return action end\nskip()\nskip(1)\nskip(1, 2)\n").is_empty());
    assert!(codes("local m = {}\nfunction m.request(name, params, callback) end\nm.request('x', {})\n").is_empty());
    // Too many arguments are still too many.
    assert_eq!(codes("local function one(a) end\none(1, 2)\n"), ["TS2554"]);
}

#[test]
fn an_annotated_function_keeps_its_arity() {
    assert_eq!(codes("---@param a number\nlocal function first(a, b) end\nfirst()\n"), ["TS2554"]);
    assert_eq!(codes("---@type fun(a: number)\nlocal function typed(a) end\ntyped()\n"), ["TS2554"]);
}

#[test]
fn an_untyped_callback_keeps_its_contextual_parameter_types() {
    let report = check("---@param callback fun(value: number)\nlocal function each(callback) end\neach(function(value) local doubled = value * 2 end)\n");
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn strict_checking_still_requires_parameter_types() {
    let report = check_with_options("local function skip(action) end\nskip()\n", CheckOptions { no_implicit_any: true, ..CheckOptions::default() });
    let codes: Vec<_> = report.diagnostics.iter().map(|diagnostic| diagnostic.code).collect();
    assert_eq!(codes, ["TS7006"]);
}
