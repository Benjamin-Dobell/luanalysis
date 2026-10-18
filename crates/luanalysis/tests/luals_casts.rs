//! LuaLS casts: `---@cast name T` types a variable from there on, `---@cast name -?` removes nil
//! and `---@cast name -T` removes `T`; `expression --[[@as T]]` types the expression before it.
use luanalysis::{check, CheckReport};

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    report
}

const CLASSES: &str = "---@class A\n---@field a number\n\n---@class B\n---@field b number\n\n---@param x A\nlocal function wantA(x) end\n";

#[test]
fn a_cast_types_the_variable_from_there_on() {
    clean(&format!("{CLASSES}---@param source A | B\nlocal function f(source)\n    ---@cast source A\n    wantA(source)\nend\n"));
    clean(&format!("{CLASSES}---@param source A | nil\nlocal function f(source)\n    ---@cast source -?\n    wantA(source)\nend\n"));
    clean(&format!("{CLASSES}---@param source A | B\nlocal function f(source)\n    ---@cast source -B\n    wantA(source)\nend\n"));
    clean(&format!("{CLASSES}---@param source A | B | nil\nlocal function f(source)\n    ---@cast source -B, -?\n    wantA(source)\nend\n"));
    // Without the cast, the call is an error.
    let report = check(&format!("{CLASSES}---@param source A | B\nlocal function f(source)\n    wantA(source)\nend\n"));
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn as_types_the_expression_before_it() {
    clean(&format!("{CLASSES}---@param source A | B\nlocal function g(source)\n    wantA(source --[[@as A]])\n    local value = source --[[@as A]]\n    wantA(value)\nend\n"));
}

#[test]
fn a_cast_is_not_a_rename_target_at_its_tag() {
    let source = "---@param x number | string\nlocal function f(x)\n    ---@cast x number\n    return x + 1\nend\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let bound = report.bound.as_ref().unwrap();
    let names: Vec<&str> = bound.occurrences.iter()
        .filter(|occurrence| source.get(occurrence.span.start..occurrence.span.end) == Some(occurrence.name.as_str()))
        .filter(|occurrence| occurrence.name == "x")
        .map(|occurrence| &source[occurrence.span.start..occurrence.span.end])
        .collect();
    // The parameter, its @param tag, the cast's name and the read.
    assert_eq!(names.len(), 4, "{names:?}");
}
