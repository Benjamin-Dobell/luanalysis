//! A host that provides only part of the standard library, as a sandbox does, names the parts it
//! provides. Everything else is undeclared, so using it is reported.
use luanalysis::{
    project_check::{ProjectCheckOptions, check_project_with_options, static_require_requests},
    standard_environment::{StandardEnvironmentProfile, StandardLibraryAllowList},
};
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{fs, path::PathBuf, process::{Command, Output}, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);
impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("luanalysis allow list {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        for (path, source) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source).unwrap();
        }
        Self(fs::canonicalize(root).unwrap())
    }
    fn check(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis")).current_dir(&self.0)
            .args(["check", "--module-root", ".", "--dump-types"]).args(arguments).arg("main.lua").output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn text(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

const SANDBOXED: &str = "local now = os.time()\nos.execute('ls')\nlocal handle = io.open('x')\nlocal upper = ('x'):upper()\nlocal chunk = load('return 1')\nlocal other = require('other')\n";

#[test]
fn only_the_named_libraries_and_members_are_declared() {
    let fixture = Fixture::new(&[("main.lua", SANDBOXED), ("other.lua", "return 1\n")]);
    let output = fixture.check(&["--lua-version", "5.2", "--stdlib", "basic,string,os.time", "--stdlib", "require"]);
    let report = text(&output);
    assert!(report.contains("main.lua:1:7: now: number"), "{report}");
    assert!(report.contains("main.lua:2:4: error[TS2339]: Property 'execute' does not exist on type 'std__Os'."), "{report}");
    assert!(report.contains("main.lua:3:16: error[TS2304]: Cannot find name 'io'."), "{report}");
    assert!(report.contains("main.lua:4:7: upper: string"), "{report}");
    assert!(report.contains("main.lua:6:7: other: number"), "{report}");
    assert_eq!(report.matches("error[").count(), 2, "{report}");
}

#[test]
fn a_library_left_out_takes_its_members_with_it() {
    // From Lua 5.1, `require` belongs to the package library, not the base library.
    let fixture = Fixture::new(&[("main.lua", SANDBOXED), ("other.lua", "return 1\n")]);
    let report = text(&fixture.check(&["--lua-version", "5.4", "--stdlib", "basic,os"]));
    assert!(report.contains("main.lua:4:21: error[TS2339]: Property 'upper' does not exist on type '\"x\"'."), "{report}");
    assert!(report.contains("main.lua:6:15: error[TS2304]: Cannot find name 'require'."), "{report}");
    assert!(report.contains("main.lua:3:16: error[TS2304]: Cannot find name 'io'."), "{report}");
    assert!(!report.contains("execute"), "{report}");
    // Strings are still strings.
    let fixture = Fixture::new(&[("main.lua", "---@type string\nlocal text = 'x'\nlocal length = #text\n")]);
    let output = fixture.check(&["--lua-version", "5.4", "--stdlib", "basic"]);
    assert!(output.status.success(), "{}", text(&output));
}

#[test]
fn an_entry_the_version_does_not_have_is_an_error() {
    let fixture = Fixture::new(&[("main.lua", "")]);
    let report = text(&fixture.check(&["--lua-version", "5.2", "--stdlib", "basic,utf8,os.nope"]));
    assert!(report.contains("--stdlib: the Lua 5.2 standard library has no libraries or members os.nope, utf8"), "{report}");
    // Without a version, the list is of Lua 5.4's library.
    let report = text(&fixture.check(&["--stdlib", "basic,bit32"]));
    assert!(report.contains("--stdlib: the Lua 5.4 standard library has no library or member bit32"), "{report}");
}

#[test]
fn without_the_standard_library_a_host_declares_its_own() {
    // `type`, `assert` and `setmetatable` are not replaceable: without them it would not be Lua.
    let fixture = Fixture::new(&[
        ("host.def.lua", "---@param message string\nfunction log(message) end\n"),
        ("main.lua", "---@class Point\nlocal Point = {}\nPoint.__index = Point\n---@param value string | number\nlocal function f(value)\n    if type(value) == 'string' then\n        local text = value\n    end\n    local number = assert(value)\nend\nlocal point = setmetatable({}, Point)\nlog('ready')\nprint('ready')\nlocal other = require('other')\n"),
        ("other.lua", "return 1\n"),
    ]);
    let report = text(&fixture.check(&["--no-stdlib", "--library", "host.def.lua"]));
    assert!(report.contains("main.lua:7:15: text: string"), "{report}");
    assert!(report.contains("main.lua:9:11: number: string | number"), "{report}");
    assert!(report.contains("main.lua:11:7: point: Point"), "{report}");
    assert!(report.contains("main.lua:13:1: error[TS2304]: Cannot find name 'print'."), "{report}");
    assert!(report.contains("main.lua:14:15: error[TS2304]: Cannot find name 'require'."), "{report}");
    assert_eq!(report.matches("error[").count(), 2, "{report}");
    // Parts of the standard library can be added back.
    let report = text(&fixture.check(&["--no-stdlib", "--stdlib", "require", "--lua-version", "5.2", "--library", "host.def.lua"]));
    assert!(report.contains("main.lua:14:7: other: number"), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}

#[test]
fn library_callers_pass_the_allow_list_in_the_options() {
    let fixture = Fixture::new(&[("main.lua", SANDBOXED), ("other.lua", "return 1\n")]);
    let allow = StandardLibraryAllowList::parse("basic, string, os.time, require");
    assert_eq!(allow.validate(StandardEnvironmentProfile::Lua52), Ok(()));
    let resolver = Resolver::new(ProjectConfig::new([fixture.0.clone()])).unwrap();
    let project = Project::load_plan(&resolver, ProjectLoadPlan::new([fixture.0.join("main.lua")]), static_require_requests);
    let report = check_project_with_options(project, ProjectCheckOptions {
        environment: StandardEnvironmentProfile::Lua52,
        standard_library: Some(allow),
        ..ProjectCheckOptions::default()
    });
    let mut codes: Vec<&str> = report.files.iter().flat_map(|file| &file.report.diagnostics).map(|diagnostic| diagnostic.code).collect();
    codes.sort();
    assert_eq!(codes, ["TS2304", "TS2339"]);
}
