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
