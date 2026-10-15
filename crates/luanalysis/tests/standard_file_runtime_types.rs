use luanalysis::project_check::{
    ProjectCheckOptions, ProjectCheckReport, check_project_with_options, static_require_requests,
};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const PROFILES: [(StandardEnvironmentProfile, &str); 5] = [
    (StandardEnvironmentProfile::Lua50, "5.0"),
    (StandardEnvironmentProfile::Lua51, "5.1"),
    (StandardEnvironmentProfile::Lua52, "5.2"),
    (StandardEnvironmentProfile::Lua53, "5.3"),
    (StandardEnvironmentProfile::Lua54, "5.4"),
];
const HANDLES: &str = r#"local handle=io.input()
---@type file
local original=handle
---@type userdata
local opaque=handle
handle:flush()
handle:seek('cur',0)

---@param value file|std__Io|string
local function inspect(value)
    if type(value)=='userdata' then
        ---@type file
        local checked=value
        value:flush()
        value:seek('cur',0)
    elseif type(value)=='table' then
        ---@type std__Io
        local namespace=value
        ---@type file
        local current=value.input()
    else
        ---@type string
        local text=value
    end
end

---@class HandleNamespace : userdata
local Namespace={}

---@param choose boolean
local function namespaceAndInstance(choose)
    local value=choose and handle or Namespace
    if type(value)=='userdata' then
        ---@type file
        local checked=value
        value:flush()
    else
        ---@type table
        local namespace=value
    end
end
"#;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis standard file runtime {} {}",
            std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn source(&self, source: &str) -> PathBuf {
        let path = self.0.join("main.lua");
        fs::write(&path, source).unwrap();
        path
    }
    fn check(&self, source: &str, environment: StandardEnvironmentProfile) -> ProjectCheckReport {
        let main = self.source(source);
        let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
        let project = Project::load_plan(&resolver, ProjectLoadPlan::new([main]), static_require_requests);
        check_project_with_options(project, ProjectCheckOptions { environment, ..Default::default() })
    }
    fn cli(&self, source: &str, version: &str, expected_exit: i32) -> String {
        self.source(source);
        let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args(["check", "--format", "json", "--lua-version", version, "main.lua"])
            .output().unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(expected_exit), "{stdout}\n{stderr}");
        assert!(stderr.is_empty(), "{stderr}");
        stdout
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}
fn clean(report: &ProjectCheckReport) {
    assert!(!report.has_errors(), "{:#?}", report.files);
}
fn range(source: &str, statement: &str, expression: &str) -> (usize, usize) {
    let start = source.find(statement).unwrap() + statement.rfind(expression).unwrap();
    (start, start + expression.len())
}
fn anchored(report: &ProjectCheckReport, source: &str, statement: &str, expression: &str, code: &str) {
    let (start, end) = range(source, statement, expression);
    let diagnostics = &report.file(report.project.entries[0]).unwrap().report.diagnostics;
    assert!(diagnostics.iter().any(|error| error.code == code
        && error.span.start == start && error.span.end == end), "{diagnostics:#?}");
    assert!(!diagnostics.iter().any(|error| error.code == "LUA2041"), "{diagnostics:#?}");
}

#[test]
fn all_file_profiles_preserve_userdata_handles_methods_and_physical_namespaces() {
    let fixture = Fixture::new();
    for (profile, _) in PROFILES { clean(&fixture.check(HANDLES, profile)); }
}

#[test]
fn narrowed_file_bodies_and_non_handle_branches_are_checked_and_correctable() {
    let fixture = Fixture::new();
    let wrong_method = HANDLES.replacen("value:seek('cur',0)", "value:seek('cur','wrong')", 1);
    let wrong_branch = HANDLES.replacen("---@type std__Io\n        local namespace", "---@type file\n        local namespace", 1);
    for (profile, _) in PROFILES {
        anchored(&fixture.check(&wrong_method, profile), &wrong_method,
            "value:seek('cur','wrong')", "'wrong'", "TS2345");
        anchored(&fixture.check(&wrong_branch, profile), &wrong_branch,
            "local namespace=value", "value", "TS2322");
        clean(&fixture.check(HANDLES, profile));
    }
}

#[test]
fn cli_file_profiles_reject_a_wrong_narrowed_method_argument_then_accept_its_correction() {
    let fixture = Fixture::new();
    let wrong = HANDLES.replacen("value:seek('cur',0)", "value:seek('cur','wrong')", 1);
    let (start, end) = range(&wrong, "value:seek('cur','wrong')", "'wrong'");
    let range = format!("\"range\":{{\"start\":{start},\"end\":{end}}}");
    for (_, version) in PROFILES {
        let rejected = fixture.cli(&wrong, version, 1);
        assert!(rejected.split("},{\"path\":").any(|diagnostic|
            diagnostic.contains("\"code\":\"TS2345\"") && diagnostic.contains(&range)), "{rejected}");
        assert!(!rejected.contains("\"code\":\"LUA2041\""), "{rejected}");
        let corrected = fixture.cli(HANDLES, version, 0);
        assert!(corrected.contains("\"diagnostics\":[]"), "{corrected}");
    }
}
