//! Template literal types, `` `prefix${T}` ``, and the intrinsic string types, as TypeScript
//! gives them.
use luanalysis::project_check::{ProjectCheckOptions, check_project_with_options, static_require_requests};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{fs, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Checked {
    diagnostics: Vec<String>,
    bindings: Vec<(String, String)>,
}

impl Checked {
    fn ty(&self, name: &str) -> &str {
        self.bindings.iter().find(|(binding, _)| binding == name).map(|(_, ty)| ty.as_str()).unwrap_or_else(|| panic!("{name}: {:#?}", self.bindings))
    }
}

/// `code text | message` for each diagnostic, and each binding's type.
fn check(source: &str) -> Checked {
    let root = std::env::temp_dir().join(format!("luanalysis template literal types {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&root).unwrap();
    let main: PathBuf = root.join("main.lua");
    fs::write(&main, source).unwrap();
    let project = Project::load_plan(&Resolver::new(ProjectConfig::new([root.clone()])).unwrap(), ProjectLoadPlan::new([main]), static_require_requests);
    let report = check_project_with_options(project, ProjectCheckOptions {
        environment: StandardEnvironmentProfile::Lua54,
        describe_types: true,
        ..ProjectCheckOptions::default()
    });
    fs::remove_dir_all(&root).unwrap();
    let main = report.file(report.project.entries[0]).unwrap();
    Checked {
        diagnostics: main.report.diagnostics.iter().map(|diagnostic| format!("{} {} | {}", diagnostic.code, &source[diagnostic.span.start..diagnostic.span.end], diagnostic.message)).collect(),
        bindings: main.report.bindings.iter().map(|binding| (binding.name.clone(), binding.ty.clone())).collect(),
    }
}

#[test]
fn a_string_matches_a_template_whose_text_and_placeholders_it_has() {
    let checked = check(r#"---@alias EventName `on_${string}`

---@type EventName
local ok = "on_click"
---@type EventName
local bad = "click"
---@type `${number}px`
local width = "12px"
---@type `${number}px`
local notWidth = "wide px"
"#);
    assert_eq!(checked.ty("ok"), "`on_${string}`");
    assert_eq!(checked.diagnostics, [
        "TS2322 \"click\" | Type '\"click\"' is not assignable to type '`on_${string}`'.",
        "TS2322 \"wide px\" | Type '\"wide px\"' is not assignable to type '`${number}px`'.",
    ]);
}

#[test]
fn literal_placeholders_expand_and_inference_reads_a_template() {
    let checked = check(r#"---@alias Side "left" | "right"
---@alias Edge `${Side}_${"top" | "bottom"}`

---@type Edge
local edge = "left_top"
---@type Edge
local wrong = "left_middle"

---@generic T: string
---@param event `on_${T}`
---@return { name: T }
local function handler(event) return { name = --[[---@type T]] event } end
local handled = handler("on_click")
"#);
    assert_eq!(checked.ty("edge"), "\"left_bottom\" | \"left_top\" | \"right_bottom\" | \"right_top\"");
    assert_eq!(checked.ty("handled"), "{ name: \"click\" }");
    assert_eq!(checked.diagnostics, ["TS2322 \"left_middle\" | Type '\"left_middle\"' is not assignable to type '\"left_bottom\" | \"left_top\" | \"right_bottom\" | \"right_top\"'."]);
}

#[test]
fn intrinsic_string_types_map_strings_and_placeholders_are_text() {
    let checked = check(r#"---@alias Side "left" | "right"

---@type Uppercase<"hello">
local loud = "HELLO"
---@type Capitalize<Side>
local capital = "Left"
---@type Lowercase<"LOUD"> | Uncapitalize<"Quiet">
local soft = "quiet"
---@type Uppercase<number>
local nonsense = 1
---@type `${{ a: number }}`
local text = ""
"#);
    assert_eq!(checked.ty("loud"), "\"HELLO\"");
    assert_eq!(checked.ty("capital"), "\"Left\" | \"Right\"");
    assert_eq!(checked.ty("soft"), "\"loud\" | \"quiet\"");
    assert_eq!(checked.diagnostics, [
        "TS2344 number | Type 'number' does not satisfy the constraint 'string'.",
        "TS2322 { a: number } | Type '{ a: number }' is not assignable to type 'string | number | boolean | nil'.",
    ]);
}
