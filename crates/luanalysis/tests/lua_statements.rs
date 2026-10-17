//! Lua statements beyond Lua 5.1's: `goto`, labels and local attributes.
use luanalysis::project_check::{ProjectCheckOptions, ProjectCheckReport, check_project_with_options, static_require_requests};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn check(source: &str, inspections: bool) -> (String, ProjectCheckReport) {
    let root = std::env::temp_dir().join(format!("luanalysis statements {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&root).unwrap();
    let main = root.join("main.lua");
    std::fs::write(&main, source).unwrap();
    let project = Project::load_plan(&Resolver::new(ProjectConfig::new([root.clone()])).unwrap(), ProjectLoadPlan::new([main]), static_require_requests);
    let report = check_project_with_options(project, ProjectCheckOptions { environment: StandardEnvironmentProfile::Lua54, inspections, ..Default::default() });
    std::fs::remove_dir_all(&root).unwrap();
    (source.to_owned(), report)
}

fn found((source, report): &(String, ProjectCheckReport)) -> Vec<String> {
    let main = report.file(report.project.entries[0]).unwrap();
    main.report.diagnostics.iter().map(|diagnostic| format!("{} {}", diagnostic.code, &source[diagnostic.span.start..diagnostic.span.end])).collect()
}

#[test]
fn a_forward_goto_narrows_the_path_it_skips() {
    let checked = check("---@param list (string|nil)[]\nlocal function total(list)\n    local sum = 0\n    for _, item in ipairs(list) do\n        if not item then goto continue end\n        sum = sum + #item\n        ::continue::\n    end\n    return sum\nend\nreturn total\n", false);
    assert!(found(&checked).is_empty(), "{:?}", found(&checked));
}

#[test]
fn code_after_a_backward_label_is_checked_and_reachable() {
    let checked = check("local attempts = 0\n::retry::\nattempts = attempts + 1\nif attempts < 3 then goto retry end\n---@type string\nlocal wrong = attempts\nreturn wrong\n", false);
    assert_eq!(found(&checked), ["TS2322 attempts"]);
}

#[test]
fn statements_after_a_goto_are_unreachable_until_a_label() {
    let checked = check("local function f()\n    goto done\n    print('skipped')\n    ::done::\n    print('reached')\nend\nf()\n", true);
    assert_eq!(found(&checked), ["TS7027 print('skipped')"]);
}

#[test]
fn constant_and_closed_locals_cannot_be_assigned() {
    let checked = check("local limit <const> = 10\nlocal handle <close>, other = nil, 2\nlimit = 11\nhandle = nil\nother = 3\nreturn limit, handle, other\n", false);
    assert_eq!(found(&checked), ["TS2588 limit", "TS2588 handle"]);
}
