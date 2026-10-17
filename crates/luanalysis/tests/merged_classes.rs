//! A class declared in several places is one class, as LuaLS reads repeated `---@class X` and
//! TypeScript merges interface declarations. A module that reopens a class on another module's
//! class table defines members of that class for every importer.
use std::{fs, path::PathBuf, process::{Command, Output}, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);
impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("luanalysis merged classes {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        for (path, source) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source).unwrap();
        }
        Self(root)
    }
    fn check(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis")).current_dir(&self.0)
            .args(["check", "--lua-version", "5.4", "--module-root", ".", "--dump-types", "main.lua"]).output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn text(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn repeated_class_declarations_merge_their_fields() {
    let fixture = Fixture::new(&[
        ("object.lua", "---@class parser.object\n---@field type string\n\nreturn {}\n"),
        ("compiler.lua", "---@class parser.object\n---@field start integer\n\nreturn {}\n"),
        ("main.lua", "require 'object'\nrequire 'compiler'\n---@param source parser.object\n---@return string, integer\nlocal function read(source) return source.type, source.start end\n---@param source parser.object\nlocal function wrong(source) return source.finish end\n"),
    ]);
    let output = fixture.check();
    let report = text(&output);
    assert!(!report.contains("TS2300"), "{report}");
    assert!(report.contains("main.lua:7:44: error[TS2339]: Property 'finish' does not exist on type 'parser.object'"), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}

#[test]
fn a_reopened_class_defines_members_for_every_importer() {
    let fixture = Fixture::new(&[
        ("vm/vm.lua", "---@class vm\nlocal m = {}\nfunction m.compile() return 1 end\nreturn m\n"),
        ("vm/global.lua", "---@class vm\nlocal vm = require 'vm.vm'\nfunction vm.getGlobal() return 'global' end\nlocal own = vm.compile()\nreturn vm\n"),
        ("main.lua", "local vm = require 'vm.vm'\nrequire 'vm.global'\nlocal compiled = vm.compile()\nlocal global = vm.getGlobal()\n"),
    ]);
    let output = fixture.check();
    let report = text(&output);
    assert!(output.status.success(), "{report}");
    assert!(report.contains("main.lua:1:7: vm: vm"), "{report}");
    assert!(report.contains("main.lua:3:7: compiled: number"), "{report}");
    assert!(report.contains("main.lua:4:7: global: string"), "{report}");
}

#[test]
fn a_class_declared_once_still_exports_its_partial_definition() {
    // Declared but never assigned, `x` is not yet a member of the module's table.
    let fixture = Fixture::new(&[
        ("point.lua", "---@class Point\n---@field x number\nlocal point = {}\nreturn point\n"),
        ("main.lua", "local point = require 'point'\n---@param value Point\nlocal function use(value) end\nuse(point)\n"),
    ]);
    let output = fixture.check();
    assert!(text(&output).contains("TS2345"), "{}", text(&output));
}

#[test]
fn appending_with_the_length_operator_builds_a_sequence() {
    let fixture = Fixture::new(&[
        ("main.lua", "local list = {}\nfor i = 1, 3 do list[#list + 1] = { start = i } end\nfor _, item in ipairs(list) do\n    local start = item.start\nend\nlocal sparse = {}\nfor i = 1, 3 do sparse[i * 2] = i end\n---@type number[]\nlocal wrong = sparse\n"),
    ]);
    let output = fixture.check();
    let report = text(&output);
    assert!(report.contains("main.lua:4:11: start: number"), "{report}");
    // A store at any other computed key still makes a numeric dictionary.
    assert!(report.contains("main.lua:9:15: error[TS2322]"), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}

#[test]
fn a_class_table_stored_in_its_own_field_is_the_class() {
    let fixture = Fixture::new(&[
        ("main.lua", "---@class Node\nlocal mt = {}\nmt.__index = mt\nmt.self = mt\nfunction mt:name() return 'node' end\n---@type Node\nlocal index = mt.__index\n"),
    ]);
    let output = fixture.check();
    assert!(output.status.success(), "{}", text(&output));
}

#[test]
fn a_class_field_stored_from_another_field_is_inferred() {
    // Building the class's definition table for the read of `m.y` once requested `x` again,
    // which was reported as excessively deep, and the class was unusable.
    let fixture = Fixture::new(&[
        ("main.lua", "---@class Pair\nlocal m = {}\nm.y = 1\nm.x = m.y\nfunction m.f()\n    m.z = m.missing\nend\nlocal x = m.x\nreturn m\n"),
    ]);
    let output = fixture.check();
    let report = text(&output);
    assert!(report.contains("main.lua:8:7: x: number"), "{report}");
    assert!(report.contains("main.lua:6:13: error[TS2339]: Property 'missing' does not exist on type 'Pair'"), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}

#[test]
fn a_metamethod_stored_from_a_method_of_its_own_class_is_not_too_deep() {
    let fixture = Fixture::new(&[
        ("main.lua", "---@class Hash\nlocal mt = {}\nmt.__index = mt\n---@param text string\n---@return integer\nfunction mt:hash(text) return #text end\nmt.__call = mt.hash\nreturn mt\n"),
    ]);
    let report = text(&fixture.check());
    assert!(!report.contains("TS2589") && !report.contains("LUA2031"), "{report}");
}

#[test]
fn a_class_declared_as_string_extends_the_string_library() {
    // LuaLS adds operators to strings this way. The standard library's own `string` stays the
    // class that the declaration extends.
    let fixture = Fixture::new(&[
        ("format.lua", "---@class string\n---@operator mod(table): string\n\nreturn {}\n"),
        ("main.lua", "require 'format'\nlocal byte = string.byte('x')\nlocal upper = ('x'):upper()\nlocal formatted = ('x') % {}\nlocal wrong = ('x') % 1\n"),
    ]);
    let output = fixture.check();
    let report = text(&output);
    assert!(report.contains("main.lua:3:7: upper: string"), "{report}");
    assert!(report.contains("main.lua:4:7: formatted: string"), "{report}");
    assert!(report.contains("main.lua:5:21: error[TS2365]: Operator '%' cannot be applied to types '\"x\"' and '1'"), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}
