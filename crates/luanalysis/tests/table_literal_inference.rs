//! Arguments that are unrelated candidates for one type parameter infer it as TypeScript does:
//! table literals, whether written in the call or constructed into a local, are unioned; otherwise
//! a candidate that is a supertype of the others, or else the first, fixes it, and an argument it
//! does not accept is an error. Luanalysis v1 unioned every candidate.
use std::{fs, process::Command, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn types(source: &str) -> String {
    let directory = std::env::temp_dir().join(format!("luanalysis table literal inference {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("main.lua"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&directory)
        .args(["check", "--lua-version", "5.4", "--dump-types", "main.lua"]).output().unwrap();
    fs::remove_dir_all(directory).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

const BOTH: &str = "---@generic T\n---@param a T\n---@param b T\n---@return T\nlocal function both(a, b) return a end\n";

#[test]
fn table_literal_candidates_are_unioned() {
    let report = types(&format!("{BOTH}local written = both({{ a = 1 }}, {{ b = 2 }})\nlocal x = {{ x = 1 }}\nlocal y = {{ y = 2 }}\nlocal locals = both(x, y)\nlocal grown = {{ g = 1 }}\ngrown.h = 2\nlocal mixed = both(grown, {{ i = 3 }})\nlocal nested = both({{ inner = {{ a = 1 }} }}, {{ inner = {{ b = 2 }} }})\nlocal function make() return {{ k = 1 }} end\nlocal returned = both(make(), {{ j = \"s\" }})\n"));
    for expected in [
        "main.lua:6:7: written: { a: 1 } | { b: 2 }",
        "main.lua:9:7: locals: { x: 1 } | { y: 2 }",
        "main.lua:12:7: mixed: { g: 1, h: 2 } | { i: 3 }",
        "main.lua:13:7: nested: { inner: { a: 1 } } | { inner: { b: 2 } }",
        "main.lua:15:7: returned: { k: 1 } | { j: \"s\" }",
    ] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    assert!(!report.contains("error["), "{report}");
}

#[test]
fn other_candidates_take_a_supertype_or_the_first() {
    let report = types(&format!("{BOTH}local same = both(1, 2)\nlocal mixed = both(1, \"x\")\n---@class Animal\n---@field name string\nlocal Animal = {{}}\n---@class Dog : Animal\n---@field bark fun()\nlocal Dog = {{}}\n---@type Dog\nlocal dog\n---@type Animal\nlocal animal\nlocal super = both(dog, animal)\n---@type {{ result: 1 | 2 }}\nlocal numbered\n---@type {{ result: \"A\" }}\nlocal lettered\nlocal shapes = both(numbered, lettered)\n---@type {{ x: number }}\nlocal declared\nlocal written = both(declared, {{ y = 2 }})\n"));
    for expected in [
        "main.lua:6:7: same: number",
        "main.lua:7:7: mixed: number",
        "main.lua:7:23: error[TS2345]: Argument of type '\"x\"' is not assignable to parameter of type '1'.",
        "main.lua:18:7: super: Animal",
        "main.lua:23:7: shapes: { result: 1 | 2 }",
        "main.lua:23:31: error[TS2345]: Argument of type '{ result: \"A\" }' is not assignable to parameter of type '{ result: 1 | 2 }'.",
        // A table literal joins the others only as their union's last candidate: a declared table
        // comes first.
        "main.lua:26:7: written: { x: number }",
        "main.lua:26:32: error[TS2345]: Argument of type '{ y: 2 }' is not assignable to parameter of type '{ x: number }'.",
    ] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    assert_eq!(report.matches("error[").count(), 3, "{report}");
}
