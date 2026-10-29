//! A `*.def.lua` or `*.def.ttslua` file is a declaration file wherever it is found, as
//! TypeScript's `*.d.ts` is: its stubs describe what other code provides, and are not run.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Project(PathBuf);

impl Project {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("luanalysis declaration files {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&root).unwrap();
        let project = Self(fs::canonicalize(root).unwrap());
        for (path, text) in files {
            let path = project.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        project
    }

    /// The diagnostics of checking `paths`, relative to the project.
    fn check(&self, paths: &[&Path]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&self.0).args(["check", "--lua-version", "5.4"]).args(paths).output().unwrap();
        (String::from_utf8(output.stdout).unwrap() + &String::from_utf8(output.stderr).unwrap())
            .replace(&format!("{}{}", self.0.display(), std::path::MAIN_SEPARATOR), "")
            // Paths as written, whichever separator the platform prints.
            .replace(std::path::MAIN_SEPARATOR, "/")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const HAND: &str = "---@class Hand\nlocal Hand\n\n---@return string\nfunction Hand.getValue() end\n";
const USE: &str = "---@param hand Hand\nlocal function value(hand)\n    ---@type number\n    local wrong = hand.getValue()\nend\n";

#[test]
fn a_def_file_in_a_checked_directory_declares_and_is_not_run() {
    let project = Project::new(&[("types/Hand.def.lua", HAND), ("types/Pile.def.ttslua", "---@class Pile\nlocal Pile\n\n---@return number\nfunction Pile.count() end\n"), ("main.lua", USE)]);
    let report = project.check(&[Path::new(".")]);
    // The stub's local is no nil value, and its empty function returns nothing it promises.
    assert!(!report.contains("Hand.def.lua") && !report.contains("Pile.def.ttslua"), "{report}");
    assert!(report.contains("main.lua:4:19: error[TS2322]: Type 'string' is not assignable to type 'number'."), "{report}");
    // As one named explicitly, and as a library, it is the same declaration.
    let report = project.check(&[Path::new("types/Hand.def.lua"), Path::new("main.lua")]);
    assert!(!report.contains("Hand.def.lua") && report.contains("main.lua:4:19: error[TS2322]"), "{report}");
    let report = project.check(&[Path::new("--library"), Path::new("types"), Path::new(".")]);
    assert!(!report.contains("Hand.def.lua") && report.contains("main.lua:4:19: error[TS2322]"), "{report}");
    // An ordinary file is still run: its local is nil.
    fs::rename(project.0.join("types/Hand.def.lua"), project.0.join("types/Hand.lua")).unwrap();
    let report = project.check(&[Path::new(".")]);
    assert!(report.contains("types/Hand.lua:5:15: error[TS7053]"), "{report}");
}

#[test]
fn a_def_file_declares_nested_namespaces_through_their_parents_types() {
    let project = Project::new(&[
        ("ngx.def.lua", "---@class ngx\nngx = {}\n\n---@param status number\nfunction ngx.exit(status) end\n\n---@class ngx__req\nngx.req = {}\n\n---@param max_bytes? number\n---@return nil | string\nfunction ngx.req.get_body_data(max_bytes) end\n"),
        ("main.lua", "ngx.exit(1)\nlocal body = ngx.req.get_body_data()\n---@type number\nlocal wrong = body\nngx.req.missing()\n"),
    ]);
    let report = project.check(&[Path::new(".")]);
    assert!(!report.contains("ngx.def.lua"), "{report}");
    assert!(report.contains("main.lua:4:15: error[TS2322]: Type 'nil | string' is not assignable to type 'number'."), "{report}");
    assert!(report.contains("main.lua:5:9: error[TS2339]: Property 'missing' does not exist on type 'ngx__req'."), "{report}");
}
