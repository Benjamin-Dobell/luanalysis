//! A function statement repeating an earlier one's target unconditionally: an error if the
//! signatures differ, otherwise something only an editor points out. An assignment is neither.
use luanalysis::{Diagnostic, DiagnosticCategory};
use luanalysis::project_check::{ProjectCheckOptions, check_project_with_options, static_require_requests};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{Project, ProjectConfig, ProjectInput, ProjectLoadPlan, Resolver, SourceRole};
use std::{fs, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("luanalysis repeated {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); }
}

/// The source's diagnostics, the inspections included, as `code@line`, in a file named `name`.
fn reports(name: &str, source: &str) -> Vec<String> {
    let fixture = Fixture::new();
    let file = fixture.0.join(name);
    fs::write(&file, source).unwrap();
    let plan = if name.ends_with(".def.lua") {
        let mut plan = ProjectLoadPlan::new(Vec::<PathBuf>::new());
        plan.libraries.push(ProjectInput::new(file.clone(), SourceRole::Declaration));
        plan
    } else { ProjectLoadPlan::new([file.clone()]) };
    let project = Project::load_plan(&Resolver::new(ProjectConfig::new([fixture.0.clone()])).unwrap(), plan, static_require_requests);
    let id = project.file_by_path(&file).unwrap().id;
    let report = check_project_with_options(project, ProjectCheckOptions {
        inspections: true, environment: StandardEnvironmentProfile::Lua54, ..Default::default()
    });
    report.file(id).unwrap().report.diagnostics.iter()
        // Stubs need no use and no parameter suggestion to say anything here.
        .filter(|diagnostic| !matches!(diagnostic.code, "TS6133" | "TS7044" | "LUA4001"))
        .map(|diagnostic: &Diagnostic| format!("{}@{}", diagnostic.code, source[..diagnostic.span.start].matches('\n').count() + 1))
        .collect()
}

fn check(source: &str) -> Vec<String> { reports("main.lua", source) }

const CLASS: &str = "---@class Mod\nMod = {}\n";

#[test]
fn the_same_signature_again_is_only_pointed_out() {
    assert_eq!(check("---@param a string\nfunction g(a) end\n---@param a string\nfunction g(a) end\n"), ["LUA4006@4"]);
    assert_eq!(check(&format!("{CLASS}---@param a string\nfunction Mod.f(a) end\n---@param b string\nfunction Mod.f(b) end\n")), ["LUA4006@6"]);
    assert_eq!(check("---@class Box\nlocal Box = {}\n---@param a string\nfunction Box:pick(a) end\n---@param a string\nfunction Box:pick(a) end\nreturn Box\n"), ["LUA4006@6"]);
    assert_eq!(check("local M = {}\n---@param a string\nfunction M.f(a) end\n---@param a string\nfunction M.f(a) end\nreturn M\n"), ["LUA4006@5"]);
    // Neither has a written signature to differ, and inferred results are not one.
    assert_eq!(check("local M = {}\nfunction M.f() return 1 end\nfunction M.f() return 'x' end\nreturn M\n"), ["LUA4006@3"]);
}

#[test]
fn pointing_out_a_repeat_is_a_weak_inspection() {
    let diagnostic = Diagnostic { code: "LUA4006", message: String::new(), span: luanalysis_syntax::Span { start: 0, end: 0 } };
    assert_eq!(diagnostic.category(), DiagnosticCategory::Suggestion);
}

#[test]
fn another_signature_is_an_error_at_the_repeat() {
    for (first, later) in [
        ("---@param a string", "---@param a number"),
        ("---@param a string", "---@param a string|number"),
        ("---@param a string|number", "---@param a string"),
        ("---@param a string", "---@param a string\n---@param b number"),
        ("---@return string", "---@return number"),
    ] {
        let params = |doc: &str| if doc.contains("@param b") { "a, b" } else if doc.contains("@param") { "a" } else { "" };
        let body = |doc: &str| if doc.contains("string") && doc.contains("@return") { "return 'x'" } else if doc.contains("@return") { "return 1" } else { "" };
        let definitions = |target: &str| format!("{first}\nfunction {target}({}) {} end\n{later}\nfunction {target}({}) {} end\n", params(first), body(first), params(later), body(later));
        let line = 2 + first.matches('\n').count() + 1 + later.matches('\n').count() + 1;
        // The first statement is what the member's other reads see; the check is not a second error.
        assert_eq!(check(&format!("{CLASS}{}", definitions("Mod.f"))), [format!("TS2717@{}", line + 2)], "{first} {later}");
        assert_eq!(check(&format!("local M = {{}}\n{}return M\n", definitions("M.f"))), [format!("TS2717@{}", line + 1)], "{first} {later}");
    }
    let report = check("---@param a string\nfunction g(a) end\n---@param a number\nfunction g(a) end\n");
    assert_eq!(report, ["TS2403@2", "TS2403@4"]);
    // A differing parameter list is another signature without any annotation.
    assert_eq!(check("local M = {}\nfunction M.f(a) end\nfunction M.f(a, b) end\nreturn M\n"), ["TS2717@3"]);
    assert_eq!(check("function g(a) end\nfunction g(a, b) end\n"), ["TS2403@2"]);
}

#[test]
fn what_only_one_definition_writes_is_not_compared() {
    assert_eq!(check(&format!("{CLASS}---@param a string\nfunction Mod.f(a) end\nfunction Mod.f(a) end\n")), ["LUA4006@5"]);
    assert_eq!(check(&format!("{CLASS}function Mod.f(a) end\n---@param a string\nfunction Mod.f(a) end\n")), ["LUA4006@5"]);
}

#[test]
fn an_assignment_is_only_type_checked() {
    assert_eq!(check("local M = {}\nM.f = function() end\nM.f = function() end\nreturn M\n"), Vec::<String>::new());
    assert_eq!(check("local M = {}\nfunction M.f() end\nM.f = function() end\nreturn M\n"), Vec::<String>::new());
    assert_eq!(check("local M = {}\nM.f = function(a) end\nM.f = function(a, b) end\nreturn M\n"), Vec::<String>::new());
    // Against the member's type, as any write to it.
    assert_eq!(check(&format!("{CLASS}---@param a string\nMod.f = function(a) end\n---@param a number\nMod.f = function(a) end\n")), ["TS2322@6"]);
}

#[test]
fn an_assignment_starts_the_target_again() {
    assert_eq!(check("local M = {}\nfunction M.f() end\nM.f = nil\nfunction M.f() end\nreturn M\n"), Vec::<String>::new());
    assert_eq!(check("local M = {}\nfunction M.f() end\nM = {}\nfunction M.f() end\nreturn M\n"), Vec::<String>::new());
}

#[test]
fn defining_a_function_conditionally_repeats_nothing() {
    assert_eq!(check("local M = {}\nlocal jit = math.random() > 0.5\nif jit then\n    function M.f(a) end\nelse\n    function M.f(a, b) end\nend\nreturn M\n"), Vec::<String>::new());
    assert_eq!(check("local M = {}\nfunction M.f() end\nfor _ = 1, 2 do\n    function M.f() end\nend\nreturn M\n"), Vec::<String>::new());
    // Within one branch they still both run, and `do` runs what it holds.
    assert_eq!(check("local M = {}\nlocal jit = math.random() > 0.5\nif jit then\n    function M.f() end\n    function M.f() end\nend\nreturn M\n"), ["LUA4006@5"]);
    assert_eq!(check("local M = {}\nfunction M.f() end\ndo\n    function M.f() end\nend\nreturn M\n"), ["LUA4006@4"]);
}

#[test]
fn another_function_and_another_local_are_other_targets() {
    assert_eq!(check("local M = {}\nlocal function setup()\n    function M.f() end\nend\nfunction M.f() end\nsetup()\nreturn M\n"), Vec::<String>::new());
    assert_eq!(check("local M = {}\nfunction M.f() end\nlocal N = M\nlocal M = {}\nfunction M.f() end\nreturn N, M\n"), Vec::<String>::new());
    assert_eq!(check("local M = {}\nfunction M.f() end\nfunction M.g() end\nreturn M\n"), Vec::<String>::new());
}

#[test]
fn a_local_function_declares_another_local_which_shadowing_reports() {
    assert_eq!(check("---@param a string\nlocal function f(a) end\n---@param a number\nlocal function f(a) end\nf(1)\n"), Vec::<String>::new());
}

#[test]
fn a_definition_for_one_lua_version_is_an_alternative() {
    assert_eq!(check(&format!("{CLASS}function Mod.f(a) end\n---@version >5.4\nfunction Mod.f(a, b) end\n")), Vec::<String>::new());
    assert_eq!(check(&format!("{CLASS}---@version 5.1\nfunction Mod.f(a) end\nfunction Mod.f(a, b) end\n")), Vec::<String>::new());
}

#[test]
fn the_same_rule_holds_where_functions_are_only_declared() {
    let members = "---@class Mod\nMod = {}\n---@param a string\nfunction Mod.f(a) end\n---@param a number\nfunction Mod.f(a) end\n";
    assert_eq!(reports("api.def.lua", members), ["TS2717@6"]);
    assert_eq!(check(&format!("---@meta\n{members}")), ["TS2717@7"]);
    let globals = "---@param a string\nfunction g(a) end\n---@param a number\nfunction g(a) end\n";
    assert_eq!(reports("api.def.lua", globals), ["TS2403@4"]);
    assert_eq!(reports("api.def.lua", "---@param a string\nfunction g(a) end\n---@param a string\nfunction g(a) end\n"), ["LUA4006@4"]);
}
