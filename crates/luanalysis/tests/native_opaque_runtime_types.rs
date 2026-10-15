use luanalysis::project_check::{
    ProjectCheckOptions, ProjectCheckReport, check_project_with_options, static_require_requests,
};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
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
const HOST: &str = "---@class Host:userdata\n---@field value number\n\n";
const GUARDS: &str = r#"---@param value Host|thread|string
local function inspect(value)
    if type(value)=='userdata' then
        ---@type Host
        local host=value
        ---@type number
        local member=value.value
    elseif 'thread'==(type)(value) then
        ---@type thread
        local routine=value
    else
        ---@type string
        local text=value
    end
end

---@param value Host|thread|string
local function negated(value)
    if type(value)~='userdata' then
        if 'thread'~=type(value) then
            ---@type string
            local text=value
        else
            ---@type thread
            local routine=value
        end
    else
        ---@type Host
        local host=value
        ---@type number
        local member=value.value
    end
end
"#;
const CREATED_THREAD: &str = r#"
local created=coroutine.create(function() end)
---@type thread
local original=created
---@param choose boolean
local function inspectCreated(choose)
    local value=choose and created or 'text'
    if type(value)=='thread' then
        ---@type thread
        local routine=value
    else
        ---@type string
        local text=value
    end
end
"#;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis opaque runtime types {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
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
        let project = Project::load_plan(
            &resolver,
            ProjectLoadPlan::new([main]),
            static_require_requests,
        );
        check_project_with_options(
            project,
            ProjectCheckOptions {
                environment,
                ..Default::default()
            },
        )
    }
    fn cli(&self, source: &str, version: &str) -> Output {
        self.source(source);
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args([
                "check",
                "--format",
                "json",
                "--lua-version",
                version,
                "main.lua",
            ])
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn clean(report: &ProjectCheckReport) {
    assert!(!report.has_errors(), "{:#?}", report.files);
}
fn span(source: &str, statement: &str, expression: &str) -> (usize, usize) {
    let start = source.find(statement).unwrap() + statement.rfind(expression).unwrap();
    (start, start + expression.len())
}
fn api_error(report: &ProjectCheckReport, source: &str, statement: &str, expression: &str) {
    let (start, end) = span(source, statement, expression);
    let diagnostics = &report
        .file(report.project.entries[0])
        .unwrap()
        .report
        .diagnostics;
    assert!(
        diagnostics.iter().any(|error| error.code == "TS2322"
            && error.span.start == start
            && error.span.end == end),
        "{diagnostics:#?}"
    );
}
fn cli_report(output: Output, exit: i32) -> String {
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(exit), "{stdout}\n{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    assert!(stdout.starts_with("{\"formatVersion\":1,"), "{stdout}");
    stdout
}

#[test]
fn all_selected_profiles_narrow_opaque_domains_without_losing_host_members() {
    let fixture = Fixture::new();
    let source = format!("{HOST}{GUARDS}{CREATED_THREAD}");
    for (profile, _) in PROFILES {
        clean(&fixture.check(&source, profile));
    }
}

#[test]
fn cli_checks_the_narrowed_host_members_body_and_accepts_its_correction() {
    let fixture = Fixture::new();
    let source = format!("{HOST}{GUARDS}");
    let wrong = source.replacen(
        "---@type number\n        local member",
        "---@type string\n        local member",
        1,
    );
    let (start, end) = span(&wrong, "local member=value.value", "value.value");
    let range = format!("\"range\":{{\"start\":{start},\"end\":{end}}}");
    for (_, version) in PROFILES {
        let rejected = cli_report(fixture.cli(&wrong, version), 1);
        assert!(
            rejected
                .split("},{\"path\":")
                .any(|diagnostic| diagnostic.contains("\"code\":\"TS2322\"")
                    && diagnostic.contains(&range)),
            "{rejected}"
        );
        let corrected = cli_report(fixture.cli(&source, version), 0);
        assert!(corrected.contains("\"diagnostics\":[]"), "{corrected}");
    }
}

#[test]
fn a_replaced_original_type_keeps_the_opaque_union_in_ordinary_branches() {
    let fixture = Fixture::new();
    let source = format!(
        "{HOST}---@param value unknown\n---@return 'userdata'|'thread'\nlocal function fake(value) return 'userdata' end\ntype=fake\n---@param value Host|thread|string\nlocal function inspect(value)\nif type(value)=='userdata' then\n---@type Host\nlocal host=value\nelseif type(value)=='thread' then\n---@type thread\nlocal routine=value\nend\nend\n"
    );
    for (profile, _) in PROFILES {
        let rejected = fixture.check(&source, profile);
        api_error(&rejected, &source, "local host=value", "value");
        api_error(&rejected, &source, "local routine=value", "value");
        clean(&fixture.check(&source.replace("type=fake\n", ""), profile));
    }
}

#[test]
fn unselected_same_name_classes_cannot_fabricate_opaque_runtime_domains() {
    let fixture = Fixture::new();
    let fake_roots = "---@class userdata\n\n---@class thread\n\n";
    let probes = "---@param value Host|thread|string\nlocal function inspect(value)\nif type(value)=='userdata' then\n---@type Host\nlocal host=value\nelseif type(value)=='thread' then\n---@type thread\nlocal routine=value\nend\nend\n";
    for kind in ["userdata", "thread"] {
        let source = format!(
            "{fake_roots}{HOST}---@param value Host|thread|string\nlocal function inspect(value)\nif type(value)=='{kind}' then\nlocal kept=value\nend\nend\n"
        );
        let legacy = fixture.check(&source, StandardEnvironmentProfile::LegacyMinimal);
        assert!(
            legacy
                .files
                .iter()
                .flat_map(|file| &file.report.diagnostics)
                .any(|error| error.code == "LUA2020"),
            "{kind}: {:#?}",
            legacy.files
        );
    }
    let ordinary_type = "---@param value unknown\n---@return string\nlocal function type(value) return 'userdata' end\n";
    let source = format!("{fake_roots}{HOST}{ordinary_type}{probes}");
    let empty = fixture.check(&source, StandardEnvironmentProfile::None);
    assert!(empty.standard_library_files.is_empty());
    api_error(&empty, &source, "local host=value", "value");
    api_error(&empty, &source, "local routine=value", "value");
    let corrected = source
        .replace(
            "---@type Host\nlocal host",
            "---@type Host|thread|string\nlocal host",
        )
        .replace(
            "---@type thread\nlocal routine",
            "---@type Host|thread|string\nlocal routine",
        );
    clean(&fixture.check(&corrected, StandardEnvironmentProfile::None));
}
