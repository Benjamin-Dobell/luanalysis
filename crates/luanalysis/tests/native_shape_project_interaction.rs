use luanalysis::project_check::check_project;
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
            "luanalysis-shape-project-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn check(
        &self,
        declaration: &str,
        source: &str,
    ) -> luanalysis::project_check::ProjectCheckReport {
        let library = self.0.join("shared.def.lua");
        let entry = self.0.join("entry.ttslua");
        fs::write(&library, declaration).unwrap();
        fs::write(&entry, source).unwrap();
        let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
        let mut plan = ProjectLoadPlan::new(vec![entry]);
        plan.libraries = vec![ProjectInput::new(library, SourceRole::Declaration)];
        let project = Project::load_plan(&resolver, plan, |_| vec![]);
        check_project(project)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn implicit_shape_contextualizes_a_callback_from_a_foreign_declaration_snapshot() {
    let report = Fixture::new().check(
        "---@alias Reader fun(value:{x:number}):number\n",
        "---@shape Readers\n---@field read Reader\nlocal readers={read=function(value) return value.x end}\nlocal result=readers.read({x=42})",
    );
    for file in &report.files {
        assert!(
            file.report.diagnostics.is_empty(),
            "{:#?}",
            file.report.diagnostics
        );
    }
    assert!(
        report
            .files
            .iter()
            .flat_map(|file| &file.report.bindings)
            .any(|binding| binding.name == "result" && binding.ty == "number")
    );
}
#[test]
fn implicit_shape_checks_foreign_nominal_member_origins() {
    let fixture = Fixture::new();
    let declaration = "---@class Token\n\n---@class Wrong\n";
    let source = "---@param token Wrong\nlocal function wrap(token)\n---@shape Holder\n---@field token Token\nlocal holder={token=token}\nreturn holder.token\nend";
    let rejected = fixture.check(declaration, source);
    let diagnostic = rejected
        .files
        .iter()
        .flat_map(|file| &file.report.diagnostics)
        .find(|diagnostic| diagnostic.code == "TS2322")
        .expect("different source class origins must fail inside the attached shape");
    assert_eq!(
        &source[diagnostic.span.start..diagnostic.span.end],
        "{token=token}"
    );
    let corrected = fixture.check(
        declaration,
        &source.replace("@param token Wrong", "@param token Token"),
    );
    for file in &corrected.files {
        assert!(
            file.report.diagnostics.is_empty(),
            "{:#?}",
            file.report.diagnostics
        );
    }
}
