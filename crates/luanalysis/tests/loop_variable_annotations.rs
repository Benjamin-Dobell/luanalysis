//! A loop's variables take a written type as Luanalysis v1 and LuaLS read it: `---@param name T`
//! above a `for ... in` loop types its variable `name`, and `---@type T` above a numeric loop types
//! its variable. Each is the variable's contract, as a typed local's is.
use std::{fs, process::Command, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn checked(source: &str) -> String {
    let directory = std::env::temp_dir().join(format!("luanalysis loop annotations {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("main.lua"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&directory)
        .args(["check", "--lua-version", "5.4", "--dump-types", "main.lua"]).output().unwrap();
    fs::remove_dir_all(directory).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

fn errors(report: &str) -> Vec<&str> {
    report.lines().filter(|line| line.contains(": error[")).collect()
}

#[test]
fn a_param_tag_types_the_generic_loop_variable_it_names() {
    let report = checked("---@class Progress\n---@field title string\n\n---@type table<string, any>\nlocal map = {}\n\n\
        ---@param progress Progress\nfor key, progress in pairs(map) do\n    local title = progress.title\n    local nothing = progress.nothing\nend\n");
    assert!(report.contains("main.lua:8:10: progress: Progress"), "{report}");
    assert!(report.contains("main.lua:9:11: title: string"), "{report}");
    assert!(report.contains("main.lua:8:5: key: string"), "{report}");
    assert_eq!(errors(&report), ["main.lua:10:30: error[TS2339]: Property 'nothing' does not exist on type 'Progress'."], "{report}");
}

#[test]
fn the_iterators_values_are_checked_against_a_written_type() {
    let report = checked("---@param name number\nfor _, name in ipairs({ \"a\", \"b\" }) do\nend\n");
    assert_eq!(errors(&report), ["main.lua:2:8: error[TS2322]: Type '\"a\" | \"b\"' is not assignable to type 'number'."], "{report}");
    let report = checked("---@param name string\nfor _, name in ipairs({ \"a\", \"b\" }) do\n    local copy = name\nend\n");
    assert!(errors(&report).is_empty(), "{report}");
    // As a typed local, it reads as its written type.
    assert!(report.contains("main.lua:3:11: copy: string"), "{report}");
}

#[test]
fn a_type_tag_types_a_numeric_loop_variable() {
    let report = checked("---@alias Slot 1 | 2 | 3\n\n---@type Slot\nfor slot = 1, 3 do\n    local copy = slot\nend\n");
    assert!(errors(&report).is_empty(), "{report}");
    assert!(report.contains("main.lua:5:11: copy: 1 | 2 | 3"), "{report}");
    let report = checked("---@type string\nfor index = 1, 3 do\nend\n");
    assert_eq!(errors(&report), ["main.lua:2:5: error[TS2322]: Type 'number' is not assignable to type 'string'."], "{report}");
}

#[test]
fn tags_no_loop_variable_takes_stay_unattached() {
    // A parameter tag naming no loop variable, and a `---@type` above a `for ... in` loop, which
    // LuaLS gives no variable either.
    let report = checked("---@param missing number\nfor key in pairs({}) do\nend\n\n---@type string\nfor key in pairs({}) do\nend\n");
    assert_eq!(errors(&report), [
        "main.lua:1:4: error[LUA1001]: Luanalysis annotation is not attached to a compatible declaration or function",
        "main.lua:5:4: error[LUA1001]: Luanalysis annotation is not attached to a compatible declaration or function",
    ], "{report}");
}

#[test]
fn a_generic_loop_variable_type_may_name_its_functions_type_parameter() {
    let report = checked("---@generic T\n---@param list T[]\n---@return T?\nlocal function first(list)\n    \
        ---@param value T\n    for _, value in ipairs(list) do\n        return value\n    end\nend\n\nlocal one = first({ 1 })\n");
    assert!(errors(&report).is_empty(), "{report}");
    assert!(report.contains("main.lua:6:12: value: T"), "{report}");
}
