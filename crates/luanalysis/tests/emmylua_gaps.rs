//! Gaps that the comparison with EmmyLua Analyzer Rust's checker tests found, each closed as
//! TypeScript has the feature.
use std::{fs, process::Command, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn report(source: &str) -> String {
    checked(&[("main.lua", source)], &[])
}

fn checked(files: &[(&str, &str)], options: &[&str]) -> String {
    let directory = std::env::temp_dir().join(format!("luanalysis emmylua gaps {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&directory).unwrap();
    for (name, source) in files {
        fs::write(directory.join(name), source).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&directory)
        .args(["check", "--lua-version", "5.4", "--dump-types"]).args(options)
        .args(files.iter().map(|(name, _)| *name)).output().unwrap();
    fs::remove_dir_all(directory).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

fn errors(report: &str) -> Vec<&str> {
    report.lines().filter(|line| line.contains(": error[")).collect()
}

#[test]
fn a_result_list_that_needs_values_needs_a_return_even_after_any() {
    let report = report("---@return any, number\nlocal function f()\n    X = 1\nend\n\
---@return number, any\nlocal function g() end\n\
---@return any\nlocal function h() end\n\
---@return any, number\nlocal function i(x)\n    if x then return 1, 2 end\nend\n\
---@return any, number?\nlocal function j() end\n");
    let errors = errors(&report);
    for expected in ["main.lua:1:4: error[TS2355]", "main.lua:5:4: error[TS2355]", "main.lua:9:4: error[TS2355]"] {
        assert!(errors.iter().any(|error| error.starts_with(expected)), "{expected}\n{report}");
    }
    // A lone `any` result ends without a value, as does a list whose shortest row is one.
    assert_eq!(errors.len(), 3, "{report}");
}

#[test]
fn a_constructor_key_written_twice_is_reported() {
    let source = "local a = { b = 1, b = 2 }\nlocal c = { [1] = 1, [1.0] = 2 }\nlocal d = { \"x\", [1] = \"y\" }\n\
local e = { [\"b\"] = 1, b = 2 }\nlocal k = \"b\"\nlocal f = { b = 1, [k] = 2 }\n\
local function two() return 1, 2 end\nlocal g = { [1] = 0, two() }\nlocal h = { [2] = 0, 1, 2, 3 }\n";
    let report = report(source);
    let errors = errors(&report);
    for expected in ["main.lua:1:20: error[TS1117]", "main.lua:2:23: error[TS1117]", "main.lua:3:19: error[TS1117]",
        "main.lua:4:24: error[TS1117]", "main.lua:9:25: error[TS1117]"] {
        assert!(errors.iter().any(|error| error.starts_with(expected)), "{expected}\n{report}");
    }
    // A computed key that is not a literal, and a final call's values, are not written keys.
    assert_eq!(errors.len(), 5, "{report}");
    // Lua leaves the order of a constructor's assignments undefined: the field has either value.
    assert!(report.contains("main.lua:1:7: a: { b: 1 | 2 }"), "{report}");
}

#[test]
fn an_enum_on_a_global_table_names_its_values() {
    let report = report("---@enum Flags\nFlags = { b = 1, c = 2 }\nif Flags.a then end\nlocal value = Flags.b\n\
---@param flag Flags\nlocal function use(flag) end\nuse(Flags.c)\nuse(3)\n\
---@enum (key) Names\nNamesTable = { x = 1 }\n");
    let errors = errors(&report);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:3:10: error[TS2339]: Property 'a' does not exist")), "{report}");
    assert!(errors.iter().any(|error| error.starts_with("main.lua:8:5: error[TS2345]")), "{report}");
    assert_eq!(errors.len(), 2, "{report}");
    assert!(report.contains("main.lua:6:20: flag: 1 | 2"), "{report}");
}

#[test]
fn self_outside_any_member_is_reported() {
    let source = "---@class A\nlocal A = {}\n\n---@param init fun(self: self)\nlocal function test(init) end\n\n\
---@param superInit fun(self: A)\nlocal function declare(superInit)\n    test(superInit)\nend\n\n\
---@alias Callback fun(self: self)\n\n\
---@return self\nfunction A:new() return self end\n\n\
local M = {}\n---@return self\nfunction M:chain()\n    local function inner()\n        ---@type self\n        local t = self\n        return t\n    end\n    return self\nend\n\n\
local T = {\n    ---@return self\n    make = function(self) return self end,\n}\n";
    let report = report(source);
    let errors = errors(&report);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:4:26: error[TS2526]: A 'self' type is available only in a member")), "{report}");
    assert!(errors.iter().any(|error| error.starts_with("main.lua:12:30: error[TS2526]")), "{report}");
    // The annotation is reported once; its uses are not reported again.
    assert_eq!(errors.len(), 2, "{report}");
}

#[test]
fn a_table_type_is_a_base_as_a_record_type_is() {
    let source = "---@class D4.A: table<integer, string>\n\n---@type D4.A\nlocal x\nlocal s = x[1]\n\n\
---@shape S: table<string, number>\n---@field n number\n\n---@type S\nlocal shape = { n = 1, other = 2 }\nlocal other = shape.other\n\
---@type S\nlocal wrong = { n = 1, other = \"x\" }\n";
    let report = report(source);
    for expected in ["main.lua:5:7: s: string", "main.lua:12:7: other: number"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    let errors = errors(&report);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:14:15: error[TS2322]")), "{report}");
    assert_eq!(errors.len(), 1, "{report}");
}

#[test]
fn a_method_added_to_the_string_library_takes_a_string() {
    let source = "---@param start string\n---@return boolean\nfunction string:startswith(start)\n    return self:sub(1, #start) == start\nend\n\
---@return self\nfunction string:me() return self end\n\
local b = (\"abc\"):startswith(\"a\")\nlocal s = \"x\"\nlocal c = s:startswith(\"y\")\nlocal d = string.startswith(\"abc\", \"a\")\n\
local m = (\"q\"):me()\nstring.startswith(1, \"x\")\n";
    let checked = report(source);
    for expected in ["main.lua:3:16: self: string", "main.lua:8:7: b: boolean", "main.lua:10:7: c: boolean", "main.lua:12:7: m: string"] {
        assert!(checked.contains(expected), "{expected}\n{checked}");
    }
    let errors = errors(&checked);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:13:19: error[TS2345]")), "{checked}");
    assert_eq!(errors.len(), 1, "{checked}");
    // A local table named `string` is not the library.
    let local = report("local string = {}\nfunction string:f() return self end\nlocal t = string:f()\n");
    assert!(!local.contains("main.lua:2:16: self: string"), "{local}");
}

#[test]
fn explicit_call_type_arguments_are_checked_as_typescripts_are() {
    let source = "---@generic T\n---@param x T\n---@return T\nlocal function id(x) return x end\n\
local a = id--[[@<string>]](1)\nlocal b = id--[[@<number>]](1)\nlocal c = id--[[@<{ a: number }>]]{ a = 1 }\nlocal d = id--[[@<string, number>]](\"a\")\n\
---@generic T: string\n---@param x T\nlocal function only(x) end\nonly--[[@<number>]](1)\n\
---@class Box\nlocal Box = {}\n---@generic T\n---@param x T\n---@return T\nfunction Box:put(x) return x end\n\
local p = Box:put--[[@<string>]](\"s\")\nlocal q = Box:put --[[@<number>]] (\"s\")\nlocal n = id--[[@<number>]](id(1))\n\
local v = id --[[@<string>]] .x\n\
---@generic U, T extends U extends string and number or boolean\n---@param val T\nlocal function process(val) return val end\n\
process--[[@<string, boolean>]](true)\nprocess--[[@<string, number>]](123)\n";
    let checked = report(source);
    for expected in ["main.lua:5:7: a: string", "main.lua:6:7: b: number", "main.lua:7:7: c: { a: number }", "main.lua:19:7: p: string", "main.lua:21:7: n: number"] {
        assert!(checked.contains(expected), "{expected}\n{checked}");
    }
    let errors = errors(&checked);
    for expected in [
        "main.lua:5:29: error[TS2345]: Argument of type '1' is not assignable to parameter of type 'string'.",
        "main.lua:8:17: error[TS2558]: Expected 1 type arguments, but got 2.",
        "main.lua:12:11: error[TS2344]: Type 'number' does not satisfy the constraint 'string'.",
        "main.lua:20:36: error[TS2345]",
        "main.lua:22:18: error[LUA1001]: type arguments must come just before a call's arguments",
        "main.lua:26:22: error[TS2344]: Type 'boolean' does not satisfy the constraint 'number'.",
    ] {
        assert!(errors.iter().any(|error| error.starts_with(expected)), "{expected}\n{checked}");
    }
    assert_eq!(errors.iter().filter(|error| !error.contains("TS2339")).count(), 6, "{checked}");
}

#[test]
fn a_conditional_types_true_branch_knows_its_check_type_satisfies_the_extends_type() {
    let source = "---@alias OnlyStr<T extends string> T\n---@alias Wrap<T> T extends string and OnlyStr<T> or never\n\
---@type Wrap<\"a\">\nlocal w = \"a\"\n---@type Wrap<number>\nlocal n\n\
---@alias Bad<T> T extends string and never or OnlyStr<T>\n\
---@alias Nested<T> T extends string and (T extends \"a\" | \"b\" and OnlyStr<T> or OnlyStr<T>) or never\n\
---@alias Prefix<T> T extends string and `p_${T}` or never\n---@type Prefix<\"x\">\nlocal p\n";
    let checked = report(source);
    for expected in ["main.lua:4:7: w: \"a\"", "main.lua:6:7: n: never", "main.lua:11:7: p: \"p_x\""] {
        assert!(checked.contains(expected), "{expected}\n{checked}");
    }
    // The false branch knows nothing of the extends type, as in TypeScript.
    let errors = errors(&checked);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:7:56: error[TS2344]")), "{checked}");
    assert_eq!(errors.len(), 1, "{checked}");
}

#[test]
fn a_private_or_protected_member_is_accessible_only_within_its_class() {
    let module = "---@class Foo\n---@field private x integer\n---@field protected y integer\n---@field z integer\nlocal Foo = {}\nFoo.__index = Foo\n\n\
---@private\n---@return integer\nfunction Foo.init() return 1 end\n\n---@private\nFoo.count = 0\n\n\
---@return Foo\nfunction Foo.new()\n    local self = setmetatable({}, Foo)\n    self.x = Foo.init()\n    self.y = Foo.count\n    return self\nend\n\n\
---@return integer\nfunction Foo:sum()\n    local function inner() return self.x end\n    return inner() + self.y\nend\n\nreturn Foo\n";
    let subclass = "local Foo = require(\"mod\")\n\n---@class Bar: Foo\nlocal Bar = setmetatable({}, { __index = Foo })\n\n\
---@return integer\nfunction Bar:protected() return self.y end\n\n---@return integer\nfunction Bar:private() return self.x end\n\n\
---@param f Foo\n---@return integer\nlocal function free(f) return f.x + f.y + f.z end\n\nlocal n = Foo.init()\nFoo.count = 0\n\nreturn Bar\n";
    let library = "local M = {}\n\n---@package\nfunction M.internal() end\n\n---@private\nfunction M.init() end\n\nfunction M.open() M.init() end\n\nreturn M\n";
    let user = "local M = require(\"lib\")\nM.open()\nM.init()\nM.internal()\n";
    let checked = checked(&[("mod.lua", module), ("other.lua", subclass), ("lib.lua", library), ("main.lua", user)], &[]);
    let errors = errors(&checked);
    for expected in [
        "other.lua:10:36: error[TS2341]: Property 'x' is private and only accessible within class 'Foo'.",
        "other.lua:14:33: error[TS2341]: Property 'x' is private",
        "other.lua:14:39: error[TS2445]: Property 'y' is protected and only accessible within class 'Foo' and its subclasses.",
        "other.lua:16:15: error[TS2341]: Property 'init' is private",
        "other.lua:17:5: error[TS2341]: Property 'count' is private",
        "main.lua:3:3: error[TS2341]: Property 'init' is private and only accessible within its file.",
        "main.lua:4:3: error[TS2341]: Property 'internal' is package-private and only accessible within its file.",
    ] {
        assert!(errors.iter().any(|error| error.starts_with(expected)), "{expected}\n{checked}");
    }
    // The class's own file and methods, and a subclass's methods for a protected member, may.
    assert_eq!(errors.len(), 7, "{checked}");
}
