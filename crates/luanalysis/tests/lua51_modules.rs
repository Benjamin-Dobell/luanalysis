//! Lua 5.1's `module(name, ...)` and Luanalysis v1's `---@module Name`: after a top-level
//! `module("X")`, the chunk's global environment is the global table `X`. The names it assigns
//! are that table's fields, not globals, and `---@module X` names the table's type, which other
//! files see the fields through.
use std::{fs, path::PathBuf, process::{Command, Output}, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf, Vec<String>);
impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("luanalysis lua51 modules {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        for (path, source) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source).unwrap();
        }
        Self(root, files.iter().map(|(path, _)| (*path).to_owned()).collect())
    }
    fn check(&self, version: &str) -> String {
        let output: Output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&self.0)
            .args(["check", "--lua-version", version, "--module-root", ".", "--dump-types"]).args(&self.1).output().unwrap();
        format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn errors(report: &str) -> Vec<&str> {
    let mut errors: Vec<&str> = report.lines().filter(|line| line.contains(": error[")).collect();
    errors.sort_unstable();
    errors
}

const MODULE_A: &str = "---@module TypeA\nmodule(\"TypeA\")\n\nname = \"a\"\n\nfunction greet() return name end\n";

#[test]
fn a_modules_names_are_its_tables_fields_and_not_globals() {
    let fixture = Fixture::new(&[
        ("moduleA.lua", MODULE_A),
        ("B.lua", "---@type TypeA\nlocal a\n---@type number\nlocal n = a.name\nlocal greeting = a.greet()\nlocal global = TypeA.name\nprint(name, greet)\n"),
    ]);
    let report = fixture.check("5.1");
    assert_eq!(errors(&report), [
        "B.lua:4:11: error[TS2322]: Type 'string' is not assignable to type 'number'.",
        "B.lua:7:13: error[TS2304]: Cannot find name 'greet'.",
        "B.lua:7:7: error[TS2304]: Cannot find name 'name'.",
    ], "{report}");
    for expected in ["B.lua:5:7: greeting: string", "B.lua:6:7: global: string"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn the_chunk_reads_its_tables_fields_and_the_globals() {
    let module = "local tostring = tostring\n\
        before = 1\n\
        ---@module Counter\n\
        ---@field label string\n\
        module(\"Counter\", package.seeall)\n\
        \n\
        count = 0\n\
        \n\
        ---@param by number\n\
        function increment(by)\n\
        \x20   count = count + by\n\
        \x20   local text = tostring(count) .. label\n\
        \x20   local itself, named, package = _M, _NAME, _PACKAGE\n\
        \x20   local name = os.time()\n\
        \x20   return count\n\
        end\n\
        \n\
        function reset()\n\
        \x20   count = \"zero\"\n\
        \x20   resets = 1\n\
        end\n";
    let fixture = Fixture::new(&[
        ("counter.lua", module),
        ("user.lua", "---@type boolean\nlocal resets = Counter.resets\nlocal b = before\nprint(Counter.before)\n"),
    ]);
    let report = fixture.check("5.1");
    assert_eq!(errors(&report), [
        "counter.lua:19:13: error[TS2322]: Type '\"zero\"' is not assignable to type 'number'.",
        "user.lua:2:16: error[TS2322]: Type 'number' is not assignable to type 'boolean'.",
        "user.lua:4:15: error[TS2339]: Property 'before' does not exist on type 'Counter'.",
    ], "{report}");
    for expected in [
        "counter.lua:12:11: text: string", "counter.lua:13:11: itself: Counter", "counter.lua:13:19: named: string",
        "counter.lua:13:26: package: string", "counter.lua:14:11: name: number", "user.lua:3:7: b: number",
    ] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn require_returns_the_table_by_the_name_module_gives_it() {
    let fixture = Fixture::new(&[
        ("Counter.lua", "module(\"Counter\")\ncount = 0\n"),
        // Loaded by another name, `package.loaded` holds no table for it, and `require` returns true.
        ("other.lua", "module(\"Elsewhere\")\nvalue = 1\n"),
        // A chunk's own result is what `require` returns.
        ("Returned.lua", "module(\"Returned\")\nvalue = 1\nreturn \"text\"\n"),
        ("user.lua", "local counter = require(\"Counter\")\nlocal other = require(\"other\")\nlocal returned = require(\"Returned\")\n"),
    ]);
    let report = fixture.check("5.1");
    assert!(errors(&report).is_empty(), "{report}");
    for expected in ["user.lua:1:7: counter: { count: number }", "user.lua:2:7: other: boolean", "user.lua:3:7: returned: string"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn only_lua_51s_module_function_with_a_name_makes_a_table() {
    let fixture = Fixture::new(&[
        // Another function named `module`.
        ("local_function.lua", "local function module(name) end\nmodule(\"Local\")\nfromLocal = 1\n"),
        // A local named as the table would take the names meant for the table.
        ("local_table.lua", "local Shadowed = {}\nmodule(\"Shadowed\")\nfromShadowed = 1\n"),
        // A dotted name.
        ("dotted.lua", "module(\"a.b\")\nfromDotted = 1\n"),
        // Inside a block, it is no statement of the chunk's own.
        ("nested.lua", "do module(\"Nested\") end\nfromNested = 1\n"),
        ("user.lua", "local values = { fromLocal, fromShadowed, fromDotted, fromNested }\n"),
    ]);
    let report = fixture.check("5.1");
    assert!(errors(&report).is_empty(), "{report}");
}

#[test]
fn each_module_call_starts_its_tables_names() {
    let fixture = Fixture::new(&[
        ("two.lua", "module(\"First\")\nfirst = 1\nmodule(\"Second\")\nsecond = \"s\"\nfunction both() return first, second end\n"),
        ("user.lua", "local a = First.first\nlocal b = Second.second\nlocal c = First.second\n"),
    ]);
    let report = fixture.check("5.1");
    assert_eq!(errors(&report), [
        "two.lua:5:24: error[TS2304]: Cannot find name 'first'.",
        "user.lua:3:17: error[TS2339]: Property 'second' does not exist on type '{ first: number }'.",
    ], "{report}");
    assert!(report.contains("user.lua:1:7: a: number") && report.contains("user.lua:2:7: b: string"), "{report}");
}

#[test]
fn v1_reads_a_module_chunk_as_one_at_any_lua_version() {
    // Lua 5.3 has no `module`, which the call reports; the rest of the chunk is still read as
    // the module it declares, as v1 read it.
    let fixture = Fixture::new(&[
        ("moduleA.lua", MODULE_A),
        ("B.lua", "---@type TypeA\nlocal a\nlocal n = a.name\nprint(name)\n"),
    ]);
    let report = fixture.check("5.3");
    assert_eq!(errors(&report), [
        "B.lua:4:7: error[TS2304]: Cannot find name 'name'.",
        "moduleA.lua:2:1: error[TS2304]: Cannot find name 'module'.",
    ], "{report}");
    assert!(report.contains("B.lua:3:7: n: string"), "{report}");
}

#[test]
fn a_module_annotation_without_a_module_call_declares_a_class() {
    let fixture = Fixture::new(&[
        ("settings.lua", "---@module Settings\n---@field path string\nlocal Settings = {}\nSettings.path = \"/\"\nreturn Settings\n"),
    ]);
    let report = fixture.check("5.1");
    assert!(errors(&report).is_empty(), "{report}");
    assert!(report.contains("settings.lua:3:7: Settings: def Settings"), "{report}");
}
