//! `readonly` fields and record entries: written in annotations, rejected as assignment targets.
use luanalysis::project_check::{ProjectCheckOptions, ProjectCheckReport, check_project_with_options, static_require_requests};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{fs, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn check(source: &str, inspections: bool) -> Vec<String> {
    let root = std::env::temp_dir().join(format!("luanalysis readonly {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&root).unwrap();
    let main: PathBuf = root.join("main.lua");
    fs::write(&main, source).unwrap();
    let project = Project::load_plan(
        &Resolver::new(ProjectConfig::new([root.clone()])).unwrap(),
        ProjectLoadPlan::new([main]),
        static_require_requests,
    );
    let report: ProjectCheckReport = check_project_with_options(project, ProjectCheckOptions {
        environment: StandardEnvironmentProfile::Lua54,
        inspections,
        ..ProjectCheckOptions::default()
    });
    fs::remove_dir_all(&root).unwrap();
    let main = report.file(report.project.entries[0]).unwrap();
    main.report.diagnostics.iter().map(|diagnostic| format!("{} {} | {}", diagnostic.code, &source[diagnostic.span.start..diagnostic.span.end], diagnostic.message)).collect()
}

#[test]
fn readonly_fields_and_entries_reject_assignment() {
    let class = "---@class Point\n---@field readonly x number\n---@field y number\n\n---@param p Point\nlocal function move(p)\n  p.y = 2\n  p.x = 1\n  return p.x\nend\nreturn move\n";
    assert_eq!(check(class, false), ["TS2540 x | Cannot assign to readonly property \"x\"."]);
    let shape = "---@shape Size\n---@field private readonly width number\n\n---@param s Size\nlocal function grow(s) s.width = 3 end\nreturn grow\n";
    assert_eq!(check(shape, false), ["TS2540 width | Cannot assign to readonly property \"width\"."]);
    let record = "---@param p { readonly x: number, y: number, readonly [string]: number }\nlocal function move(p)\n  p.y = 2\n  p.x = 1\n  p.z = 3\n  return p.x + p.z\nend\nreturn move\n";
    assert_eq!(check(record, false), [
        "TS2540 x | Cannot assign to readonly property \"x\".",
        "TS2540 z | Cannot assign to readonly property \"z\".",
    ]);
}

#[test]
fn readonly_does_not_restrict_reading_or_assignability() {
    let source = "---@param p { readonly x: number }\n---@param q { x: number }\nlocal function copy(p, q)\n  q.x = p.x\n  local r = { x = 1 }\n  p = r\n  q = p\n  return q\nend\nreturn copy\n";
    assert!(check(source, false).is_empty());
}

#[test]
fn a_field_named_readonly_is_writable_with_a_hint_unless_it_is_quoted() {
    let bare = "---@class Named\n---@field readonly boolean\n\n---@param n Named\nlocal function set(n) n.readonly = true end\nreturn set\n";
    assert!(check(bare, false).is_empty());
    assert_eq!(check(bare, true), ["LUA4004 readonly | 'readonly' is a reserved word and may cause confusion: this declares a writable field named 'readonly'. Write [\"readonly\"] to name it explicitly."]);
    let inline_locked = "---@param n { readonly readonly: boolean }\nlocal function set(n) n.readonly = true end\nreturn set\n";
    assert_eq!(check(inline_locked, true), ["TS2540 readonly | Cannot assign to readonly property \"readonly\"."]);
    let quoted = "---@class Quoted\n---@field [\"readonly\"] boolean\n\n---@param n Quoted\nlocal function set(n) n.readonly = true end\nreturn set\n";
    assert!(check(quoted, true).is_empty());
    let hint = "LUA4004 readonly | 'readonly' is a reserved word and may cause confusion: this declares a writable field named 'readonly'. Write [\"readonly\"] to name it explicitly.";
    let inline = "---@param n { readonly: boolean }\nlocal function set(n) n.readonly = true end\n---@type { flags: { readonly: boolean } }\nlocal nested = { flags = { readonly = false } }\nreturn set, nested\n";
    assert!(check(inline, false).is_empty());
    assert_eq!(check(inline, true), [hint, hint]);
    let quoted_inline = "---@param n { [\"readonly\"]: boolean }\nlocal function set(n) n.readonly = true end\nreturn set\n";
    assert!(check(quoted_inline, true).is_empty());
    let locked = "---@class Locked\n---@field readonly readonly boolean\n\n---@param n Locked\nlocal function set(n) n.readonly = true end\nreturn set\n";
    assert_eq!(check(locked, true), ["TS2540 readonly | Cannot assign to readonly property \"readonly\"."]);
}

#[test]
fn readme_example() {
    let source = "---@shape Job\n---@field readonly path string\n---@field retries number\n\n---@param job Job\n---@param options { readonly verbose: boolean }\nlocal function retry(job, options)\n    job.retries = job.retries + 1\n    job.path = \"other\" -- error: Cannot assign to readonly property \"path\"\n    options.verbose = true -- error: Cannot assign to readonly property \"verbose\"\nend\n";
    assert_eq!(check(source, false), [
        "TS2540 path | Cannot assign to readonly property \"path\".",
        "TS2540 verbose | Cannot assign to readonly property \"verbose\".",
    ]);
}
