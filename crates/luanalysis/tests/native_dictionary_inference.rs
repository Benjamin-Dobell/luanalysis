use luanalysis::project_check::{
    ProjectCheckOptions, ProjectCheckReport, check_project_with_options, static_require_requests,
};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const ATTRIBUTES: &str =
    "---@shape Attributes\n---@field id nil|string\n---@field active nil|boolean\n\n";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis dictionary inference {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn check(&self, source: &str) -> ProjectCheckReport {
        let main = self.0.join("main.lua");
        fs::write(&main, source).unwrap();
        let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
        let project = Project::load_plan(
            &resolver,
            ProjectLoadPlan::new([main]),
            static_require_requests,
        );
        check_project_with_options(
            project,
            ProjectCheckOptions {
                environment: StandardEnvironmentProfile::Lua52,
                ..Default::default()
            },
        )
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
fn assignment_error(report: &ProjectCheckReport, source: &str, statement: &str, expression: &str) {
    let start = source.find(statement).unwrap() + statement.rfind(expression).unwrap();
    let end = start + expression.len();
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

fn entries(generic: bool) -> String {
    let (generic, parameter) = if generic {
        ("---@generic A:Attributes\n", "A")
    } else {
        ("", "Attributes")
    };
    format!(
        "{ATTRIBUTES}---@param dictionary table<string,unknown>\nlocal function acceptsDictionary(dictionary) end\n{generic}---@param attributes {parameter}\nlocal function inspect(attributes)\n    acceptsDictionary(attributes)\n    ---@type table<string,unknown>\n    local dictionary=attributes\n    for key,value in pairs(attributes) do\n        ---@type 'id'|'active'\n        local checkedKey=key\n        ---@type nil|string|boolean\n        local checkedValue=value\n    end\nend\n"
    )
}

#[test]
fn pairs_infers_literal_keys_and_all_shape_values_for_direct_and_generic_arguments() {
    let fixture = Fixture::new();
    for generic in [false, true] {
        let nullable = entries(generic);
        let mixed = nullable.replace("---@field active nil|boolean", "---@field active boolean");
        for source in [nullable, mixed] {
            clean(&fixture.check(&source));
        }
    }
}

#[test]
fn inferred_pairs_values_cannot_drop_either_string_or_boolean_and_accept_correction() {
    let fixture = Fixture::new();
    for generic in [false, true] {
        let source = entries(generic);
        for wrong_type in ["nil|boolean", "nil|string"] {
            let wrong = source.replace(
                "---@type nil|string|boolean\n        local checkedValue",
                &format!("---@type {wrong_type}\n        local checkedValue"),
            );
            let rejected = fixture.check(&wrong);
            assignment_error(&rejected, &wrong, "local checkedValue=value", "value");
            clean(&fixture.check(&source));
        }
    }
}

#[test]
fn a_string_dictionary_rejects_a_required_boolean_field_until_the_field_is_corrected() {
    let fixture = Fixture::new();
    let source = "---@shape RequiredAttributes\n---@field id string\n---@field active boolean\n\n---@param attributes RequiredAttributes\nlocal function inspect(attributes)\n    ---@type table<string,string>\n    local dictionary=attributes\nend\n";
    let rejected = fixture.check(source);
    assignment_error(
        &rejected,
        source,
        "local dictionary=attributes",
        "attributes",
    );
    let corrected = source.replace("---@field active boolean", "---@field active string");
    clean(&fixture.check(&corrected));
}

#[test]
fn nil_cannot_be_a_dictionary_key_and_a_string_key_corrects_the_annotation() {
    let fixture = Fixture::new();
    let source = "---@type table<nil,string>\nlocal dictionary={}\n";
    let rejected = fixture.check(source);
    let start = source.find("nil").unwrap();
    let diagnostics = &rejected
        .file(rejected.project.entries[0])
        .unwrap()
        .report
        .diagnostics;
    assert!(
        diagnostics
            .iter()
            .any(|error| matches!(error.code, "LUA2032" | "TS2538")
                && error.span.start == start
                && error.span.end == start + "nil".len()),
        "{diagnostics:#?}"
    );
    clean(&fixture.check(&source.replace("table<nil,string>", "table<string,string>")));
}
