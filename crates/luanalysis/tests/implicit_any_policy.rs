use luanalysis::incremental::ProjectWorkspace;
use luanalysis::project_check::{
    ProjectCheckOptions, ProjectCheckReport, check_project_with_options, static_require_requests,
};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis::{CheckOptions, CheckReport, check, check_with_options};
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

const FALLBACKS: &str = r#"local function unused_parameter(unused) end
local function unused_rest(...) end
local function dynamic(value, ...)
    ---@type number
    local numeric = value
    ---@type string
    local textual = value
    local member = value.missing
    local called = value()
    local rest = ...
    ---@type number
    local rest_number = rest
    local rest_member = rest.missing
    local rest_called = rest()
end
"#;

const CONTEXTUAL: &str = r#"---@param callback fun(value:number):number
---@return number
local function invoke(callback) return callback(42) end
local result = invoke(function(value)
    ---@type number
    local numeric = value
    return value
end)
"#;

fn options(no_implicit_any: bool) -> CheckOptions {
    CheckOptions {
        no_implicit_any,
        ..Default::default()
    }
}

fn clean(report: &CheckReport) {
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

fn binding_type<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .unwrap_or_else(|| panic!("missing binding {name:?}: {:#?}", report.bindings))
        .ty
}

fn implicit_diagnostics(report: &CheckReport, source: &str) {
    assert_eq!(report.diagnostics.len(), 4, "{:#?}", report.diagnostics);
    let mut found: Vec<_> = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code,
                &source[diagnostic.span.start..diagnostic.span.end],
            )
        })
        .collect();
    found.sort();
    assert_eq!(
        found,
        vec![
            ("TS7006", "unused"),
            ("TS7006", "value"),
            ("TS7019", "..."),
            ("TS7019", "..."),
        ],
    );
}

fn explicit_fallbacks() -> String {
    FALLBACKS
        .replace(
            "local function unused_parameter(unused)",
            "---@param unused any\nlocal function unused_parameter(unused)",
        )
        .replace(
            "local function unused_rest(...)",
            "---@vararg any\nlocal function unused_rest(...)",
        )
        .replace(
            "local function dynamic(value, ...)",
            "---@param value any\n---@vararg any\nlocal function dynamic(value, ...)",
        )
}

#[test]
fn unannotated_runtime_parameters_and_rest_keep_dynamic_v1_defaults() {
    assert!(!CheckOptions::default().no_implicit_any);
    assert!(!ProjectCheckOptions::default().no_implicit_any);
    for report in [
        check(FALLBACKS),
        check_with_options(FALLBACKS, options(false)),
    ] {
        clean(&report);
        for name in [
            "unused",
            "value",
            "member",
            "called",
            "rest",
            "rest_member",
            "rest_called",
        ] {
            assert_eq!(binding_type(&report, name), "any", "binding {name}");
        }
        assert_eq!(binding_type(&report, "numeric"), "number");
        assert_eq!(binding_type(&report, "textual"), "string");
        assert_eq!(binding_type(&report, "rest_number"), "number");
    }
}

#[test]
fn no_implicit_any_reports_original_parameter_and_rest_declarations() {
    let report = check_with_options(FALLBACKS, options(true));
    implicit_diagnostics(&report, FALLBACKS);
    // Reporting a missing contract does not change the dynamic fallback used
    // to check the body or create unrelated call/member errors.
    assert_eq!(binding_type(&report, "value"), "any");
    assert_eq!(binding_type(&report, "rest"), "any");
    clean(&check_with_options(&explicit_fallbacks(), options(true)));
}

#[test]
fn contextual_callbacks_keep_precise_parameters_under_both_policies() {
    let wrong = CONTEXTUAL.replace(
        "---@type number\n    local numeric",
        "---@type string\n    local numeric",
    );
    for no_implicit_any in [false, true] {
        let report = check_with_options(CONTEXTUAL, options(no_implicit_any));
        clean(&report);
        assert_eq!(binding_type(&report, "result"), "number");
        let parameter = CONTEXTUAL.find("function(value)").unwrap() + "function(".len();
        assert_eq!(report.type_at(parameter), Some("number"));

        let rejected = check_with_options(&wrong, options(no_implicit_any));
        assert_eq!(rejected.diagnostics.len(), 1, "{:#?}", rejected.diagnostics);
        let diagnostic = &rejected.diagnostics[0];
        assert_eq!(diagnostic.code, "TS2322");
        assert_eq!(&wrong[diagnostic.span.start..diagnostic.span.end], "value");
    }
}

#[test]
fn explicit_unknown_parameters_and_rest_are_strict_under_both_policies() {
    let source = r#"---@param value unknown
---@vararg unknown
local function inspect(value, ...)
    ---@type number
    local numeric = value
    local member = value.missing
    local called = value()
    local rest = ...
    ---@type number
    local rest_number = rest
    local rest_member = rest.missing
    local rest_called = rest()
end
"#;
    let default = check_with_options(source, options(false));
    let strict = check_with_options(source, options(true));
    assert_eq!(default.diagnostics, strict.diagnostics);
    for code in ["TS2322", "TS2339", "TS2349"] {
        assert_eq!(
            default
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == code)
                .count(),
            2,
            "{code}: {:#?}",
            default.diagnostics,
        );
    }
    assert_eq!(default.diagnostics.len(), 6, "{:#?}", default.diagnostics);
    assert_eq!(binding_type(&default, "value"), "unknown");
    assert_eq!(binding_type(&default, "rest"), "unknown");
}

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "luanalysis implicit any policy {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }

    fn file(&self, name: &str, source: &str) {
        fs::write(self.0.join(name), source).unwrap();
    }

    fn cli(&self, jobs: usize, no_implicit_any: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis"));
        command
            .current_dir(&self.0)
            .args([
                "check",
                "--no-stdlib",
                "--format",
                "json",
                "--dump-types",
                "--jobs",
            ])
            .arg(jobs.to_string());
        if no_implicit_any {
            command.arg("--no-implicit-any");
        }
        command.arg(".").output().unwrap()
    }

    fn project(&self, workspace: Option<&mut ProjectWorkspace>) -> Project {
        let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
        let plan = ProjectLoadPlan::new([self.0.join("main.lua")]);
        match workspace {
            Some(workspace) => Project::load_plan(&resolver, plan, |source| {
                workspace.static_require_requests(source)
            }),
            None => Project::load_plan(&resolver, plan, static_require_requests),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn json(output: Output, exit: i32) -> String {
    assert_eq!(output.status.code(), Some(exit), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(
        report.starts_with("{\"formatVersion\":1,") && report.ends_with("}\n"),
        "{report}"
    );
    report
}

#[test]
fn cli_default_strict_and_explicit_correction_match_with_one_two_and_four_workers() {
    let fixture = Fixture::new();
    // The initial warmup task leaves four independent roots for the parallel step.
    let names = [
        "first.lua",
        "second.lua",
        "third.lua",
        "fourth.lua",
        "fifth.lua",
    ];
    for name in names {
        fixture.file(name, FALLBACKS);
    }
    for no_implicit_any in [false, true] {
        let mut previous = None;
        for jobs in [1, 2, 4] {
            let report = json(
                fixture.cli(jobs, no_implicit_any),
                i32::from(no_implicit_any),
            );
            assert!(report.contains("\"filesChecked\":5"), "{report}");
            if no_implicit_any {
                assert_eq!(
                    report.matches("\"code\":\"TS7006\"").count(),
                    10,
                    "{report}"
                );
                assert_eq!(
                    report.matches("\"code\":\"TS7019\"").count(),
                    10,
                    "{report}"
                );
            } else {
                assert!(
                    report.contains("\"success\":true") && report.contains("\"diagnostics\":[]"),
                    "{report}"
                );
            }
            if let Some(previous) = previous {
                assert_eq!(
                    report, previous,
                    "no_implicit_any={no_implicit_any}, jobs={jobs}"
                );
            }
            previous = Some(report);
        }
    }

    for name in names {
        fixture.file(name, &explicit_fallbacks());
    }
    let mut previous = None;
    for no_implicit_any in [false, true] {
        for jobs in [1, 2, 4] {
            let report = json(fixture.cli(jobs, no_implicit_any), 0);
            assert!(
                report.contains("\"success\":true") && report.contains("\"diagnostics\":[]"),
                "{report}"
            );
            if let Some(previous) = previous {
                assert_eq!(
                    report, previous,
                    "no_implicit_any={no_implicit_any}, jobs={jobs}"
                );
            }
            previous = Some(report);
        }
    }
}

fn original_report(report: &ProjectCheckReport) -> &CheckReport {
    assert_eq!(report.checked_file_count(), 1, "{report:#?}");
    &report.files[0].report
}

#[test]
fn workspace_reuses_syntax_but_rechecks_semantics_when_the_policy_changes() {
    let fixture = Fixture::new();
    fixture.file("main.lua", FALLBACKS);
    let mut workspace = ProjectWorkspace::new();
    let mut retained = Vec::new();
    for (revision, no_implicit_any) in [false, true, false, true].into_iter().enumerate() {
        let options = ProjectCheckOptions {
            jobs: 2,
            environment: StandardEnvironmentProfile::None,
            no_implicit_any,
            ..Default::default()
        };
        let project = fixture.project(Some(&mut workspace));
        let warm = workspace.check_project(project, options);
        let cold = check_project_with_options(fixture.project(None), options);
        let warm_report = original_report(&warm);
        let cold_report = original_report(&cold);
        assert_eq!(warm_report.diagnostics, cold_report.diagnostics);
        assert_eq!(warm_report.bindings, cold_report.bindings);
        assert_eq!(warm_report.types, cold_report.types);
        assert_eq!(warm_report.bound, cold_report.bound);
        if no_implicit_any {
            implicit_diagnostics(warm_report, FALLBACKS);
            assert!(warm.has_errors());
        } else {
            clean(warm_report);
            assert!(!warm.has_errors(), "{warm:#?}");
        }
        let stats = workspace.last_preparation_stats();
        assert_eq!(stats.semantic_reused, 0, "{stats:?}");
        if revision > 0 {
            assert_eq!((stats.parsed, stats.bound), (0, 0), "{stats:?}");
            assert_eq!((stats.parse_reused, stats.bind_reused), (1, 1), "{stats:?}");
        }
        retained.push(warm);
    }
    // Later policy choices must not mutate reports already returned to callers.
    clean(original_report(&retained[0]));
    implicit_diagnostics(original_report(&retained[1]), FALLBACKS);
    clean(original_report(&retained[2]));
    implicit_diagnostics(original_report(&retained[3]), FALLBACKS);
}
