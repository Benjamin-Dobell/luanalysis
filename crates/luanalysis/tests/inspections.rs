//! Inspections: warnings and suggestions an editor shows, reported only when requested and
//! never errors.
use luanalysis::project_check::{ProjectCheckOptions, ProjectCheckReport, check_project_with_options, static_require_requests};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis::DiagnosticCategory;
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{fs, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn check(source: &str, options: ProjectCheckOptions) -> (String, ProjectCheckReport) {
    let root = std::env::temp_dir().join(format!("luanalysis inspections {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&root).unwrap();
    let main: PathBuf = root.join("main.lua");
    fs::write(&main, source).unwrap();
    let project = Project::load_plan(
        &Resolver::new(ProjectConfig::new([root.clone()])).unwrap(),
        ProjectLoadPlan::new([main]),
        static_require_requests,
    );
    let report = check_project_with_options(project, ProjectCheckOptions {
        environment: StandardEnvironmentProfile::Lua54,
        ..options
    });
    fs::remove_dir_all(&root).unwrap();
    (source.to_owned(), report)
}

fn inspect(source: &str) -> (String, ProjectCheckReport) {
    check(source, ProjectCheckOptions { inspections: true, ..ProjectCheckOptions::default() })
}

/// `code text` for each diagnostic of the entry, in source order.
fn found((source, report): &(String, ProjectCheckReport)) -> Vec<String> {
    let main = report.file(report.project.entries[0]).unwrap();
    main.report.diagnostics.iter().map(|diagnostic| format!("{} {}", diagnostic.code, &source[diagnostic.span.start..diagnostic.span.end])).collect()
}

#[test]
fn conditions_that_are_always_true_or_false_are_warnings() {
    let checked = inspect("---@param t table\n---@param s string|nil\nlocal function f(t, s)\n    if t then print(1) end\n    local n = nil\n    if n then print(2) end\n    if not t then print(3) end\n    if s then print(4) end\n    while true do break end\n    if false then print(5) end\nend\nf({}, nil)\n");
    assert_eq!(found(&checked), ["LUA2872 t", "LUA2873 n", "LUA2873 not t"]);
    let (_, report) = &checked;
    let main = report.file(report.project.entries[0]).unwrap();
    assert!(main.report.diagnostics.iter().all(|diagnostic| diagnostic.category() == DiagnosticCategory::Warning));
    assert!(!report.has_errors());
}

#[test]
fn comparisons_of_types_without_overlap_are_warnings_and_nil_is_always_comparable() {
    let checked = inspect("---@param x number\n---@param mode \"solo\"|\"campaign\"\nlocal function f(x, mode)\n    if x == 'foo' then print(1) end\n    if mode ~= 'multiplayer' then print(2) end\n    if mode == 'solo' then print(3) end\n    if x ~= nil then print(4) end\nend\nf(1, 'solo')\n");
    assert_eq!(found(&checked), ["TS2367 x == 'foo'", "TS2367 mode ~= 'multiplayer'"]);
}

#[test]
fn code_after_a_statement_that_never_completes_is_unreachable() {
    let checked = inspect("---@param x number\nlocal function f(x)\n    if x > 1 then return 1 else error('no') end\n    print('after')\n    print('more')\nend\nlocal function g()\n    for i = 1, 2 do\n        break\n        print(i)\n    end\nend\nf(1)\ng()\n");
    assert_eq!(found(&checked), ["TS7027 print('after')\n    print('more')", "TS7027 print(i)"]);
    let (_, report) = &checked;
    let main = report.file(report.project.entries[0]).unwrap();
    assert!(main.report.diagnostics.iter().all(|diagnostic| diagnostic.category() == DiagnosticCategory::Suggestion && diagnostic.is_unnecessary()));
}

#[test]
fn unused_locals_parameters_and_loop_variables_are_suggestions() {
    let checked = inspect("---@param used number\n---@param unused number\nlocal function f(used, unused, _ignored)\n    local written = 1\n    written = 2\n    for index, value in ipairs({ used }) do print(value) end\n    local _skipped = 3\nend\nlocal never = f\n");
    // As in TypeScript, an unused parameter still has its missing type reported.
    assert_eq!(found(&checked), ["TS6133 unused", "TS7044 _ignored", "TS6133 written", "TS6133 index", "TS6133 never"]);
}

#[test]
fn a_parameter_without_a_type_is_a_suggestion_unless_implicit_any_is_an_error() {
    let source = "local function f(value, ...)\n    return value, ...\nend\nreturn f\n";
    assert_eq!(found(&inspect(source)), ["TS7044 value", "TS7047 ..."]);
    let strict = check(source, ProjectCheckOptions { inspections: true, no_implicit_any: true, ..ProjectCheckOptions::default() });
    assert_eq!(found(&strict), ["TS7006 value", "TS7019 ..."]);
    assert!(strict.1.has_errors());
}

#[test]
fn checking_reports_no_inspections_unless_requested() {
    let checked = check("local unused = 1\nlocal t = {}\nif t then print(t) end\n", ProjectCheckOptions::default());
    assert!(found(&checked).is_empty());
}
