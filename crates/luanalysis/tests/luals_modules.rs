//! LuaLS module conventions: a `---@meta name` definition file provides the module `name` to
//! `require`, whatever its path, and its functions with empty bodies only declare. A module that
//! cannot be found is `any` once reported, as an unresolved TypeScript import is.
use std::{fs, path::PathBuf, process::{Command, Output}, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);
impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("luanalysis luals modules {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        for (path, source) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source).unwrap();
        }
        Self(root)
    }
    fn check(&self, paths: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis")).current_dir(&self.0)
            .args(["check", "--lua-version", "5.4", "--module-root", ".", "--dump-types"]).args(paths).output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn text(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

const FILESYSTEM: &str = "---@meta bee.filesystem\n\n---@class fs.path\nlocal path = {}\n\n---@return string\nfunction path:string() end\n\nlocal fs = {}\n\n---@param name string\n---@return fs.path\nfunction fs.path(name) end\n\nreturn fs\n";

#[test]
fn a_meta_file_provides_the_module_it_names() {
    let fixture = Fixture::new(&[
        ("meta/bee/filesystem.lua", FILESYSTEM),
        ("main.lua", "local fs = require 'bee.filesystem'\nlocal name = fs.path('x'):string()\nfs.path(1)\n"),
    ]);
    let output = fixture.check(&["main.lua", "meta"]);
    let report = text(&output);
    assert!(report.contains("main.lua:2:7: name: string"), "{report}");
    assert!(report.contains("main.lua:3:9: error[TS2345]"), "{report}");
    // The meta file's empty bodies declare their results.
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}

#[test]
fn a_meta_file_named_underscore_provides_no_module() {
    let fixture = Fixture::new(&[
        ("meta/hidden.lua", "---@meta _\nreturn {}\n"),
        ("main.lua", "local hidden = require '_'\n"),
    ]);
    let report = text(&fixture.check(&["main.lua", "meta"]));
    assert!(report.contains("main.lua:1:24: error[TS2307]"), "{report}");
}

#[test]
fn an_unresolved_module_is_any_once_reported() {
    let fixture = Fixture::new(&[
        ("main.lua", "local lpeg = require 'lpeglabel'\nlocal pattern = lpeg.P('x') * lpeg.C(1)\n"),
    ]);
    let report = text(&fixture.check(&["main.lua"]));
    assert!(report.contains("main.lua:1:7: lpeg: any"), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
    assert!(report.contains("error[TS2307]"), "{report}");
}

#[test]
fn a_require_before_the_chunk_rebinds_its_environment_is_standard() {
    // LuaLS's relabel and fs-utility modules end their imports with `_ENV = nil`.
    let fixture = Fixture::new(&[
        ("lp.lua", "return { P = function() return 1 end }\n"),
        ("main.lua", "local m = require 'lp'\n_ENV = nil\nlocal p = m.P()\n"),
        ("late.lua", "_ENV = nil\nlocal m = require 'lp'\n"),
    ]);
    let report = text(&fixture.check(&["main.lua"]));
    assert!(report.contains("main.lua:3:7: p: number"), "{report}");
    assert!(!report.contains("error["), "{report}");
    // After the rebinding, `require` is a field of nil.
    assert!(text(&fixture.check(&["late.lua"])).contains("late.lua:2:11: error[LUA2036]"));
}
