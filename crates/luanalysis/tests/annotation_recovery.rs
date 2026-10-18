use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis annotation recovery {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, source: &str) {
        fs::write(self.0.join(name), source).unwrap();
    }
    fn check(&self, library: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"));
        command
            .current_dir(&self.0)
            .args(["check", "--no-stdlib", "--stdlib", "require", "--dump-types"]);
        if library {
            command.args(["--library", "api.def.lua"]);
        }
        command.arg("main.lua").output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn pass(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{}", text(output));
}
fn one_annotation_error(output: &Output) {
    assert_eq!(output.status.code(), Some(1), "{}", text(output));
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        diagnostics.matches("error[LUA1001]").count(),
        1,
        "{}",
        text(output)
    );
    assert!(!diagnostics.contains("TS2304"), "{}", text(output));
    assert!(!diagnostics.contains("LUA200"), "{}", text(output));
}
#[test]
fn malformed_ui_field_retains_valid_before_after_aliases_and_cross_file_uses() {
    let fixture = Fixture::new();
    let library = "---@shape Before\n---@field value number\n\n---@shape Broken\n---@field tooltipPosition nil |\n\n---@shape After\n---@field label string\n\n---@alias API {left:Before,right:After}\n";
    fixture.file("api.def.lua", library);
    fixture.file("main.lua", "---@type Before\nlocal before={value=42}\n---@type After\nlocal after={label='valid'}\n---@type API\nlocal api={left=before,right=after}\n---@type number\nlocal value=api.left.value\n---@type string\nlocal label=api.right.label\n");
    let output = fixture.check(true);
    one_annotation_error(&output);
    assert!(
        text(&output).contains("api.def.lua:5:"),
        "{}",
        text(&output)
    );
    assert!(text(&output).contains("value: number"), "{}", text(&output));
    fixture.file("api.def.lua", &library.replace("nil |\n", "nil | string\n"));
    pass(&fixture.check(true));
}
#[test]
fn malformed_json_param_preserves_real_require_discovery_and_module_exports() {
    let fixture = Fixture::new();
    let module = "---@param value\n---@return number\nlocal function encode(value) return 42 end\nlocal dependency=require('dependency')\nreturn {value=dependency.value,encoded=encode(false)}\n";
    fixture.file("module.lua", module);
    fixture.file("dependency.lua", "return {value=42}\n");
    fixture.file("main.lua", "local module=require('module')\nlocal direct=module.value\n---@type number\nlocal value=module.value\n---@type number\nlocal encoded=module.encoded\n");
    let resolver = luanalysis_project::Resolver::new(luanalysis_project::ProjectConfig::new([fixture.0.clone()])).unwrap();
    let project = luanalysis_project::Project::load_plan(&resolver,
        luanalysis_project::ProjectLoadPlan::new([fixture.0.join("main.lua")]),
        luanalysis::project_check::static_require_requests);
    assert_eq!(project.files.len(), 3, "annotation errors cannot erase original dependency discovery");
    let output = fixture.check(false);
    one_annotation_error(&output);
    assert!(text(&output).contains("module.lua:1:"), "{}", text(&output));
    assert!(text(&output).contains("value: number"), "{}", text(&output));
    assert!(text(&output).contains("direct: number"), "{}", text(&output));
    fixture.file(
        "module.lua",
        &module.replace("---@param value\n", "---@param value boolean\n"),
    );
    let output = fixture.check(false);
    pass(&output);
    assert!(text(&output).contains("3 file(s)"), "{}", text(&output));
}
#[test]
fn annotation_error_keeps_independent_lua_checks_and_never_hides_other_type_errors() {
    let report = luanalysis::check(
        "---@param value\n---@return number\nlocal function encode(value) return 42 end\n---@type string\nlocal rejected=42\nlocal after=encode(false)\n",
    );
    assert!(report.bound.is_some(), "{:#?}", report.diagnostics);
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code == "LUA1001")
            .count(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code == "TS2322")
            .count(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        report
            .bindings
            .iter()
            .any(|b| b.name == "after" && b.ty == "number"),
        "{:#?}",
        report.bindings
    );
}
#[test]
fn genuinely_invalid_lua_still_refuses_the_entire_source_registry_and_flow_graph() {
    let source = "---@shape Visible\n---@field value number\nfunction damaged(\n";
    let fixture = Fixture::new();
    fixture.file("api.def.lua", source);
    fixture.file("main.lua", "---@type Visible\nlocal value={value=42}\n");
    let output = fixture.check(true);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("LUA1001"), "{}", text(&output));
    assert!(text(&output).contains("TS2304"), "{}", text(&output));
    let report = luanalysis::check(source);
    assert!(report.bound.is_none());
    assert!(report.bindings.is_empty());
    let file = luanalysis_syntax::parse(source);
    let bound = luanalysis::bind::bind(&file);
    let flow = luanalysis::flow::build(&file, &bound);
    assert!(
        flow.pending
            .iter()
            .any(|pending| pending.kind == luanalysis::flow::PendingFeature::InvalidSource)
    );
}
