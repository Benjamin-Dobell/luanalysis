//! A project's LuaLS configuration (`.luarc.json`), what discovery skips, and modules found by
//! their path's suffix, through `luanalysis check`.
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
        let root = std::env::temp_dir().join(format!("luanalysis project configuration {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&root).unwrap();
        let project = Self(fs::canonicalize(root).unwrap());
        for (path, text) in files {
            project.write(path, text);
        }
        project
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// The exit code and diagnostics of checking the project's root with `options`.
    fn check(&self, options: &[&str]) -> (Option<i32>, String) {
        self.check_paths(options, &[&self.0])
    }

    fn check_paths(&self, options: &[&str], paths: &[&Path]) -> (Option<i32>, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).arg("check").args(options).args(paths).output().unwrap();
        let text = String::from_utf8(output.stdout).unwrap() + &String::from_utf8(output.stderr).unwrap();
        (output.status.code(), text.replace(&format!("{}{}", self.0.display(), std::path::MAIN_SEPARATOR), "").replace('\\', "/"))
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const UTILS: &str = "local Utils = {}\n---@param value table\n---@return number\nfunction Utils.count(value) return #value end\nreturn Utils\n";

#[test]
fn a_module_no_root_provides_is_the_shortest_checked_file_ending_in_its_path() {
    let project = Project::new(&[
        ("src/main.ttslua", "local Utils = require('ge_tts.Utils')\nlocal Feature = require('Feature')\nlocal n = Utils.count('x')\nreturn Feature, n\n"),
        ("src/Feature.ttslua", "return {}\n"),
        ("lib/ge_tts/Utils.ttslua", UTILS),
        ("lib/vendor/copy/ge_tts/Utils.ttslua", "return 1\n"),
    ]);
    let (code, output) = project.check(&[]);
    // The module's type proves which file it is: the shorter path's.
    assert_eq!(code, Some(1), "{output}");
    assert!(output.contains("src/main.ttslua:3:23: error[TS2345]"), "{output}");
    assert!(!output.contains("TS2307"), "{output}");
    // Strictly, only a root provides a module.
    let (code, output) = project.check(&["--path-strict"]);
    assert_eq!(code, Some(1), "{output}");
    assert!(output.contains("src/main.ttslua:1:23: error[TS2307]"), "{output}");
    assert!(output.contains("src/main.ttslua:2:25: error[TS2307]"), "{output}");
    // A root's module comes first.
    let (_, output) = project.check(&["--module-root", project.0.join("lib/vendor/copy").to_str().unwrap()]);
    assert!(!output.contains("TS2345") && !output.contains("TS2307"), "{output}");
}

#[test]
fn discovery_skips_vscode_gitignored_and_ignore_dir_paths() {
    let invalid = "---@type number\nlocal value = 'text'\nreturn value\n";
    let project = Project::new(&[
        ("src/main.lua", "return 1\n"),
        ("build/bundle.lua", invalid), ("node_modules/x/y.lua", invalid), (".vscode/settings.lua", invalid), ("gen/out.lua", invalid),
        (".gitignore", "/build/\nnode_modules/\n"),
    ]);
    let (code, output) = project.check(&[]);
    assert_eq!(code, Some(1), "{output}");
    assert!(output.contains("gen/out.lua:2:15: error[TS2322]") && output.matches("TS2322").count() == 1, "{output}");
    project.write(".luarc.json", r#"{ "workspace": { "ignoreDir": [".vscode", "/gen/"] } }"#);
    assert_eq!(project.check(&[]), (Some(0), "No type errors in 1 file(s).\n".into()));
    // Without .gitignore, only ignoreDir's paths are skipped.
    project.write(".luarc.json", r#"{ "workspace.ignoreDir": ["gen"], "workspace.useGitIgnore": false }"#);
    let (_, output) = project.check(&[]);
    assert!(output.contains("build/bundle.lua") && output.contains("node_modules/x/y.lua") && output.contains(".vscode/settings.lua"), "{output}");
    assert!(!output.contains("gen/out.lua"), "{output}");
    // A file named explicitly is checked wherever it is.
    let (_, output) = project.check_paths(&[], &[&project.0.join("src"), &project.0.join("build/bundle.lua")]);
    assert!(output.contains("build/bundle.lua:2:15: error[TS2322]"), "{output}");
}

#[test]
fn luarc_version_path_strictness_and_libraries_apply_unless_options_decide() {
    let project = Project::new(&[
        ("src/main.lua", "local feature = require('feature')\nlocal first = unpack({ 1 })\nlocal answer = host_answer()\nreturn feature, first, answer\n"),
        ("src/feature.ttslua", "return {}\n"),
        ("types/host.lua", "---@return number\nfunction host_answer() end\n"),
        (".luarc.jsonc", "// LuaLS settings\n{\n  \"runtime.version\": \"Lua 5.1\",\n  \"runtime\": { \"path\": [\"src/?.lua\"], \"pathStrict\": true },\n  \"workspace.library\": [\"types\", \"${3rd}/love2d/library\"],\n}\n"),
    ]);
    // Lua 5.1's `unpack`, the library's global, and src/?.lua's .ttslua file, strictly.
    assert_eq!(project.check(&[]), (Some(0), "No type errors in 3 file(s).\n".into()));
    let (code, output) = project.check(&["--lua-version", "5.4"]);
    assert_eq!(code, Some(1));
    assert!(output.contains("src/main.lua:2:15: error[TS2304]: Cannot find name 'unpack'"), "{output}");
    let (_, output) = project.check(&["--package-path", "?.lua"]);
    assert!(output.contains("src/main.lua:1:25: error[TS2307]"), "{output}");
    // A .luarc.json is read before a .luarc.jsonc.
    project.write(".luarc.json", r#"{ "runtime.version": "Lua 5.4" }"#);
    let (_, output) = project.check(&[]);
    assert!(output.contains("TS2304") && output.contains("Cannot find name 'unpack'"), "{output}");
}

#[test]
fn an_invalid_configuration_is_reported_at_its_file() {
    let project = Project::new(&[("main.lua", "return 1\n"), (".luarc.json", r#"{ "runtime.version": "Lua 6", "runtime.pathStrict": 1 "#)]);
    // As a path that cannot be read is, a configuration that cannot be is a host error.
    let (code, output) = project.check(&[]);
    assert_eq!(code, Some(2), "{output}");
    assert!(output.contains(".luarc.json: error[LUA0001]"), "{output}");
    project.write(".luarc.json", r#"{ "runtime.version": "Lua 6", "runtime.pathStrict": 1 }"#);
    let (code, output) = project.check(&["--format", "json"]);
    assert_eq!(code, Some(2), "{output}");
    assert!(output.contains("runtime.version must be") && output.contains("runtime.pathStrict must be true or false"), "{output}");
}
