use luanalysis::{Diagnostic, DiagnosticCategory, bind};
use luanalysis::project_check::{ProjectCheckOptions, check_project_with_options, static_require_requests};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{Project, ProjectConfig, ProjectInput, ProjectLoadPlan, Resolver, SourceRole};
use std::{fs, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("luanalysis shadow {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); }
}
fn inspect(source: &str, enabled: bool) -> Vec<Diagnostic> {
    let fixture = Fixture::new();
    let main = fixture.0.join("main.lua");
    fs::write(&main, source).unwrap();
    let project = Project::load_plan(&Resolver::new(ProjectConfig::new([fixture.0.clone()])).unwrap(), ProjectLoadPlan::new([main]), static_require_requests);
    let report = check_project_with_options(project, ProjectCheckOptions {
        inspections: enabled, environment: StandardEnvironmentProfile::Lua54, ..Default::default()
    });
    let diagnostics = &report.file(report.project.entries[0]).unwrap().report.diagnostics;
    assert!(!report.has_errors(), "{source}\n{diagnostics:#?}");
    diagnostics.iter().filter(|diagnostic| diagnostic.code == "LUA4001").map(|diagnostic| {
        assert_eq!(diagnostic.category(), DiagnosticCategory::Warning);
        assert!(!diagnostic.is_unnecessary());
        diagnostic.clone()
    }).collect()
}
fn names(source: &str) -> Vec<String> {
    inspect(source, true).iter().map(|diagnostic| source[diagnostic.span.start..diagnostic.span.end].to_owned()).collect()
}

#[test]
fn shadow_warning_names_the_new_declaration_and_is_opt_in_nonfatal_and_not_faded() {
    let source = "-- 🦀\r\nlocal value = 1\r\nprint(value)\r\nlocal value = 'new'\r\nprint(value)\r\n";
    let diagnostics = inspect(source, true);
    assert_eq!(diagnostics.len(), 1);
    let start = source.find("local value = 'new'").unwrap() + "local ".len();
    assert_eq!(diagnostics[0].span, luanalysis_syntax::Span { start, end: start + 5 });
    assert!(diagnostics[0].message.contains("'value'"));
    assert!(inspect(source, false).is_empty());
}

#[test]
fn locals_parameters_loops_and_local_functions_hide_only_active_local_bindings() {
    let source = "local item = 1\nlocal index = 1\n---@param item number\nlocal function read(item) return item end\nprint(read(item))\nfor index = 1, index do print(index) end\nlocal function read() return 2 end\nprint(item, index, read())\n";
    assert_eq!(names(source), ["item", "index", "read"]);
    assert_eq!(names("local value, value = 1, 2\nprint(value)\n"), ["value"]);
    assert_eq!(names("local function recurse(recurse) return recurse end\nreturn recurse\n"), ["recurse"]);
    assert!(names("local function recurse() return recurse end\nreturn recurse\n").is_empty());
}

#[test]
fn initializer_closures_use_the_old_binding_and_never_a_later_enclosing_local() {
    let source = "local value = 1\nlocal value = function(value) return value end\nreturn value\n";
    let parsed = luanalysis_syntax::parse(source);
    let bound = bind::bind(&parsed);
    assert_eq!(bound.declarations.len(), 3);
    assert_eq!(bound.declarations[1].kind, bind::DeclarationKind::Parameter);
    assert_eq!(bound.shadowed_binding(bound.declarations[1].id), Some(bound.declarations[0].id));
    assert_eq!(bound.shadowed_binding(bound.declarations[2].id), Some(bound.declarations[0].id));
    assert_eq!(names(source), ["value", "value"]);
    assert!(names("local value = (function() local value = 1 return value end)()\nreturn value\n").is_empty());
    assert!(names("local read = function(value) return value end\nlocal value = 1\nreturn read(value)\n").is_empty());
}

#[test]
fn globals_sibling_scopes_and_implicit_method_receivers_are_not_shadow_warnings() {
    let source = "do local branch = 1 print(branch) end\ndo local branch = 2 print(branch) end\nlocal print = print\nlocal object = {}\nfunction object:outer()\n local inner = {}\n function inner:method() return self end\n return inner\nend\nprint(object)\n";
    assert!(names(source).is_empty());
}

#[test]
fn explicit_self_and_underscore_names_obey_the_real_binding_and_const_can_be_shadowed() {
    assert_eq!(names("local object = {}\nfunction object:method() local self = 42 return self end\nreturn object\n"), ["self"]);
    assert_eq!(names("local _, _cache = 1, 2\ndo local _, _cache = 3, 4 print(_, _cache) end\nprint(_, _cache)\n"), ["_cache"]);
    assert_eq!(names("local value <const> = 1\ndo local value = 2 print(value) end\nprint(value)\n"), ["value"]);
}

#[test]
fn declaration_library_role_preserves_executable_statement_errors_without_shadow_noise() {
    let fixture = Fixture::new();
    let main = fixture.0.join("main.lua");
    let declarations = fixture.0.join("library.lua");
    fs::write(&main, "return 1\n").unwrap();
    let source = "---@type number\nlocal repeated\n---@type number\nlocal repeated\n";
    let parsed = luanalysis_syntax::parse(source);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let bound = bind::bind(&parsed);
    assert!(bound.declarations.iter().any(|declaration| bound.shadowed_binding(declaration.id).is_some()));
    fs::write(&declarations, source).unwrap();
    let mut plan = ProjectLoadPlan::new([main]);
    plan.libraries.push(ProjectInput::new(declarations, SourceRole::Declaration));
    let project = Project::load_plan(&Resolver::new(ProjectConfig::new([fixture.0.clone()])).unwrap(), plan, static_require_requests);
    let report = check_project_with_options(project, ProjectCheckOptions { inspections: true, ..Default::default() });
    // Runtime locals are invalid in a declaration library. Preserve those errors;
    // optional local inspections must not add implementation-only warning noise.
    let diagnostics = report.files.iter().flat_map(|file| &file.report.diagnostics).collect::<Vec<_>>();
    assert!(report.has_errors());
    assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
    assert!(diagnostics.iter().all(|diagnostic| diagnostic.code == "LUA2034"), "{diagnostics:#?}");
}
