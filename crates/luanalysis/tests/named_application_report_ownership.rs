//! A completed source check owns real annotation errors even when the same
//! project runs through separate native checker tasks.
use luanalysis::{
    project_check::{ProjectCheckOptions, check_project_with_options, static_require_requests},
    standard_environment::StandardEnvironmentProfile,
};
use luanalysis_project::{
    Project, ProjectConfig, ProjectInput, ProjectLoadPlan, Resolver, SourceRole,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis named application reports {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&path).unwrap();
        let path = fs::canonicalize(path).unwrap();
        fs::write(
            path.join("types.def.lua"),
            "---@class Required\n---@field id number\n\n---@class Box<T : Required>\n",
        )
        .unwrap();
        fs::write(path.join("other.lua"), "return 1\n").unwrap();
        Self(path)
    }

    fn body(&self, argument: &str) {
        fs::write(self.0.join("main.lua"), format!(
            "---@param value Box<{argument}>\nlocal function inspect(value) return value end\nreturn inspect\n"
        )).unwrap();
    }

    fn diagnostics(&self, jobs: usize) -> Vec<(String, usize, usize, &'static str, String)> {
        let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
        let mut plan = ProjectLoadPlan::new([self.0.join("main.lua"), self.0.join("other.lua")]);
        plan.libraries = vec![ProjectInput::new(
            self.0.join("types.def.lua"),
            SourceRole::Declaration,
        )];
        let report = check_project_with_options(
            Project::load_plan(&resolver, plan, static_require_requests),
            ProjectCheckOptions {
                jobs,
                environment: StandardEnvironmentProfile::LegacyMinimal,
                ..ProjectCheckOptions::default()
            },
        );
        if jobs > 1 {
            assert!(
                report
                    .execution
                    .as_ref()
                    .is_some_and(|execution| execution.workers > 1),
                "jobs={jobs} must run native checker tasks on multiple workers"
            );
        }
        let mut diagnostics = Vec::new();
        for file in &report.files {
            let original = &report.source(file.file).unwrap().path;
            let path = original
                .strip_prefix(&self.0)
                .unwrap_or(original)
                .display()
                .to_string();
            for diagnostic in &file.report.diagnostics {
                diagnostics.push((
                    path.clone(),
                    diagnostic.span.start,
                    diagnostic.span.end,
                    diagnostic.code,
                    diagnostic.message.clone(),
                ));
            }
        }
        diagnostics.sort();
        diagnostics
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn completed_body_bound_errors_and_corrections_agree_across_workers() {
    let fixture = Fixture::new();
    fixture.body("number");
    let expected = fixture.diagnostics(1);
    assert_eq!(expected.len(), 1, "{expected:#?}");
    assert_eq!(expected[0].0, "main.lua");
    assert_eq!(expected[0].3, "TS2344");
    for jobs in [2, 4] {
        assert_eq!(fixture.diagnostics(jobs), expected, "jobs={jobs}");
    }

    fixture.body("Required");
    for jobs in [1, 2, 4] {
        let corrected = fixture.diagnostics(jobs);
        assert!(corrected.is_empty(), "jobs={jobs}: {corrected:#?}");
    }
}
