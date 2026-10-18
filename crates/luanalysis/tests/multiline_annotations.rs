//! An annotation continues on the `---` lines after it until the next tag, so a long type can be
//! split over lines, in line comments and in `--[[--- ]]` blocks alike.
use std::{fs, process::Command, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn types(source: &str) -> String {
    let directory = std::env::temp_dir().join(format!("luanalysis multiline annotations {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("main.lua"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&directory)
        .args(["check", "--lua-version", "5.4", "--dump-types", "main.lua"]).output().unwrap();
    fs::remove_dir_all(directory).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn a_type_continues_on_the_lines_after_its_tag() {
    let report = types(r#"---@type
---| number
---| string
local union = 1

---@type {
---    name: string,
---    age: number,
---}
local record = { name = "ada", age = 3 }

---@param callback fun(
---    value: number,
---    index: number
---): boolean
local function each(callback) end

---@shape Point
---@field x
---    number
---@field y number

---@param point Point
local function length(point) return point.x end

---@alias Mode
---| "r" # read
---| "w" # write

---@type Mode
local mode = "r"
"#);
    for expected in [
        "main.lua:4:7: union: number | string",
        "main.lua:10:7: record: { age: number, name: string }",
        "main.lua:16:21: callback: fun(value: number, index: number): boolean",
        "main.lua:24:16: length: fun(point: Point): number",
        "main.lua:31:7: mode: \"r\" | \"w\"",
    ] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    assert!(!report.contains("error["), "{report}");
}

#[test]
fn a_block_annotation_continues_on_its_following_lines() {
    let report = types(r#"--[[---
@param value string | number
    | boolean
@return string
]]
local function describe(value) return "" end
"#);
    assert!(report.contains("main.lua:6:25: value: string | number | boolean"), "{report}");
    assert!(!report.contains("error["), "{report}");
}

#[test]
fn prose_after_a_complete_type_describes_it() {
    let report = types(r#"---@class Point
--- (A point in the plane.)
---@field x number
--- The distance from the y axis.
---@field y number
--- [1] is the origin.

---@param point Point
---@return number
--- The length.
local function length(point) return point.x end
"#);
    assert!(report.contains("main.lua:11:16: length: fun(point: Point): number"), "{report}");
    assert!(!report.contains("error["), "{report}");
}

#[test]
fn an_incomplete_multiline_type_is_still_an_error() {
    let report = types("---@type {\n---    name: string,\nlocal record = {}\n");
    assert!(report.contains("error[LUA1001]"), "{report}");
}
