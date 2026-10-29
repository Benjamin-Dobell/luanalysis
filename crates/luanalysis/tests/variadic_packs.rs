//! Variadic generic packs, TypeScript's variadic tuple generics in EmmyLua Analyzer's spelling:
//! varargs written `...: T...` are the sequence `T`, which a call infers from its arguments or a
//! callback's parameter list.
use std::{fs, process::Command, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn report(source: &str) -> String {
    let directory = std::env::temp_dir().join(format!("luanalysis variadic packs {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
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
fn varargs_written_as_a_pack_are_the_sequence_a_call_gives() {
    let source = "---@generic T\n---@param ... T...\n---@return fun(...: T...)\nlocal function bind(...)\n    return function(...) end\nend\n\
local bound = bind(1, \"a\")\nbound(1, \"a\", true)\nbound(1, 2)\nbound(1, \"b\")\n\
---@generic T\n---@param ... T...\n---@return T\nlocal function all(...) return ... end\nlocal a, b, c = all(1, \"x\", true)\n";
    let checked = report(source);
    for expected in [
        "main.lua:4:16: bind: fun(...: T...): fun(...: T...)",
        "main.lua:7:7: bound: fun(...: (number, string))",
        "main.lua:15:7: a: number", "main.lua:15:10: b: string", "main.lua:15:13: c: boolean",
    ] {
        assert!(checked.contains(expected), "{expected}\n{checked}");
    }
    let errors = errors(&checked);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:8:1: error[TS2554]: Expected 2 arguments, but got 3.")), "{checked}");
    assert!(errors.iter().any(|error| error.starts_with("main.lua:9:10: error[TS2345]: Argument of type '2' is not assignable to parameter of type 'string'.")), "{checked}");
    assert_eq!(errors.len(), 2, "{checked}");
}

#[test]
fn a_callbacks_parameter_list_is_the_sequence_unless_the_arguments_fit_it() {
    // As TypeScript infers `A` in `call<A extends unknown[], R>(f: (...a: A) => R, ...a: A)`.
    let source = "---@generic A, R\n---@param f fun(...: A...): R\n---@param ... A...\n---@return R\nlocal function call(f, ...) return f(...) end\n\
---@param s string\n---@param n integer\n---@return string\nlocal function rep(s, n) return s end\n\
local r = call(rep, \"a\", 2)\ncall(rep, \"a\", \"w\")\ncall(rep, \"a\")\ncall(rep, \"a\", 2, 3)\n";
    let checked = report(source);
    assert!(checked.contains("main.lua:10:7: r: string"), "{checked}");
    let errors = errors(&checked);
    for expected in [
        "main.lua:11:16: error[TS2345]: Argument of type '\"w\"' is not assignable to parameter of type 'number'.",
        "main.lua:12:1: error[TS2554]: Expected 3 arguments, but got 2.",
        "main.lua:13:1: error[TS2554]",
    ] {
        assert!(errors.iter().any(|error| error.starts_with(expected)), "{expected}\n{checked}");
    }
    assert_eq!(errors.len(), 3, "{checked}");
}

#[test]
fn a_tuple_spread_as_varargs_is_its_elements() {
    let source = "---@alias Pair [number, string?]\n---@type fun(...: Pair...)\nlocal pair\npair(1)\npair(1, \"x\")\npair(1, \"x\", 3)\npair(\"x\")\npair(1, nil)\n";
    let checked = report(source);
    let errors = errors(&checked);
    for expected in [
        "main.lua:6:1: error[TS2554]: Expected 1-2 arguments, but got 3.",
        "main.lua:7:6: error[TS2345]",
        // An optional element may be absent, not nil.
        "main.lua:8:6: error[TS2345]",
    ] {
        assert!(errors.iter().any(|error| error.starts_with(expected)), "{expected}\n{checked}");
    }
    assert_eq!(errors.len(), 3, "{checked}");
}

#[test]
fn infer_spreads_a_parameter_list_into_a_tuple() {
    let source = "---@type Parameters<fun(a: number, b: string): boolean>\nlocal p\n\
---@alias Procedure fun(...: any): any\n---@alias MockParameters<T> T extends Procedure and Parameters<T> or never\n\
---@class Mock<T>\n---@overload fun(...: MockParameters<T>...)\n\n\
---@generic T: Procedure\n---@param a T\n---@return Mock<T>\nlocal function fn(a) return --[[---@type Mock<T>]] {} end\n\n\
local sum = fn(function(a, b) return a + b end)\nsum(1, 2, 3)\nsum(1, 2)\n\
---@alias Element<F> F extends fun(...: infer E): any and E or never\n---@type Element<fun(...: number): any>\nlocal e\n";
    let checked = report(source);
    for expected in ["main.lua:2:7: p: [number, string]", "main.lua:18:7: e: number"] {
        assert!(checked.contains(expected), "{expected}\n{checked}");
    }
    let errors = errors(&checked);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:14:1: error[TS2554]")), "{checked}");
    assert_eq!(errors.len(), 1, "{checked}");
}

#[test]
fn pcall_checks_the_arguments_it_forwards() {
    let source = "local ok, text = pcall(string.rep, \"a\", 2)\n---@type string\nlocal checked = text\n\
pcall(string.rep, \"a\", \"w\")\nlocal done = pcall(function() end)\nlocal any_ok = pcall(print, 1, \"x\", true)\n\
local x_ok, value = xpcall(tonumber, print, \"1\")\n";
    let checked = report(source);
    let errors = errors(&checked);
    // Neither overload takes it: `pcall` and its result-row overloads are TypeScript's TS2769.
    assert!(errors.iter().any(|error| error.starts_with("main.lua:4:24: error[TS2769]")), "{checked}");
    assert!(checked.contains("Argument of type '\"w\"' is not assignable to parameter of type 'number'."), "{checked}");
    assert_eq!(errors.len(), 1, "{checked}");
}

#[test]
fn a_sequence_nothing_infers_is_any_number_of_values_and_a_written_tuple_spreads() {
    // TypeScript's `T extends unknown[]` is `unknown[]` when nothing infers it.
    let source = "---@generic T\n---@param a string\n---@param ... T...\nlocal function f(a, ...) end\nf()\nf(\"a\", 1, 2)\n\
---@generic T\n---@param ... T...\n---@return fun(...: T...)\nlocal function bind(...) return function(...) end end\n\
local b1 = bind--[[@<[number, string]>]](1, \"x\")\nlocal b2 = bind--[[@<[number, string]>]](1, 2)\nlocal b3 = bind()\nb3(1)\n";
    let checked = report(source);
    for expected in ["main.lua:11:7: b1: fun(...: (number, string))", "main.lua:13:7: b3: fun()"] {
        assert!(checked.contains(expected), "{expected}\n{checked}");
    }
    let errors = errors(&checked);
    for expected in [
        "main.lua:5:1: error[TS2554]: Expected at least 1 arguments, but got 0.",
        "main.lua:12:45: error[TS2345]: Argument of type '2' is not assignable to parameter of type 'string'.",
        "main.lua:14:1: error[TS2554]: Expected 0 arguments, but got 1.",
    ] {
        assert!(errors.iter().any(|error| error.starts_with(expected)), "{expected}\n{checked}");
    }
    assert_eq!(errors.len(), 3, "{checked}");
}
