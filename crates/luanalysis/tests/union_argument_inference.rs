//! A value of a union type is one of its alternatives: a generic call infers from each, and returns
//! what each alternative would. TypeScript instead takes a common supertype of their candidates,
//! rejecting the rest.
use std::{fs, process::Command, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn types(source: &str) -> String {
    let directory = std::env::temp_dir().join(format!("luanalysis union arguments {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("main.lua"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&directory)
        .args(["check", "--lua-version", "5.4", "--dump-types", "main.lua"]).output().unwrap();
    fs::remove_dir_all(directory).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn a_call_distributes_over_the_alternatives_of_a_union_argument() {
    let report = types("---@generic T\n---@param arr T[]\n---@return T[]\nlocal function same(arr) return arr end\n\n---@param list string[] | number[]\n---@param words string[]\nlocal function f(list, words)\n    local copy = same(list)\n    local key, value = next(list)\n    for _, item in ipairs(list) do local each = item end\n    local plain = same(words)\n    table.insert(words, 1)\nend\n");
    for expected in [
        "main.lua:9:11: copy: string[] | number[]",
        "main.lua:10:16: value: nil | string | number",
        "main.lua:11:42: each: string | number",
        "main.lua:12:11: plain: string[]",
    ] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    // Candidates from separate arguments are not combined: a number is not a string list's.
    assert!(report.contains("main.lua:13:25: error[TS2345]: Argument of type '1' is not assignable to parameter of type 'string'."), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}
