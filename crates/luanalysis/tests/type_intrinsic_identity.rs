use luanalysis::project_check::{check_project, static_require_requests};
use luanalysis::{
    bind::{BuiltinIntrinsic, bind},
    check,
};
use luanalysis_project::{
    Project, ProjectConfig, ProjectInput, ProjectLoadPlan, Resolver, SourceRole,
};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

const CHECK: &str = "---@param value number|string\nlocal function use(value)\nif type(value)=='number' then\n---@type number\nlocal numeric=value\nend\nend\n";
const FAKE: &str =
    "---@param value any\n---@return string\nlocal function fake(value) return 'number' end\n";

fn has_numeric_error(report: &luanalysis::CheckReport) -> bool {
    report
        .diagnostics
        .iter()
        .any(|error| error.code == "TS2322")
}

#[test]
fn legacy_minimal_type_remains_an_explicit_unmodified_compatibility_intrinsic() {
    let report = check(CHECK);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let file = luanalysis_syntax::parse(CHECK);
    let bound = bind(&file);
    assert_eq!(
        bound.intrinsic_at(CHECK.find("type(value)").unwrap()),
        Some(BuiltinIntrinsic::LuaType)
    );
}

#[test]
fn ordinary_global_type_function_does_not_refine_by_its_name_or_schema() {
    let source = format!(
        "---@param value any\n---@return string\nfunction type(value) return 'number' end\n{CHECK}"
    );
    let report = check(&source);
    assert!(has_numeric_error(&report), "{:#?}", report.diagnostics);
}

#[test]
fn direct_type_replacement_and_lexical_shadowing_keep_the_original_union() {
    for replacement in ["type=fake\n", "local type=fake\n"] {
        let report = check(&format!("{FAKE}{replacement}{CHECK}"));
        assert!(
            has_numeric_error(&report),
            "{replacement}: {:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn direct_and_qualified_environment_replacements_invalidate_only_the_type_candidate() {
    for write in [
        "type=custom",
        "_G.type=custom",
        "_ENV['type']=custom",
        "_ENV=custom",
        "_G[key]=custom",
        "rawset(_G,'type',custom)",
        "_ENV.rawset(_ENV,'type',custom)",
    ] {
        let source = format!("type(value); {write}");
        let bound = bind(&luanalysis_syntax::parse(&source));
        assert!(bound.has_modified_type_intrinsic(), "{write}");
        assert_eq!(bound.intrinsic_at(0), None, "{write}");
    }
    let source = "type(value); package.loaded={}";
    let bound = bind(&luanalysis_syntax::parse(source));
    assert!(bound.has_modified_module_environment());
    assert!(!bound.has_modified_type_intrinsic());
    assert_eq!(bound.intrinsic_at(0), Some(BuiltinIntrinsic::LuaType));
}

#[test]
fn lexical_environment_and_shadowed_mutators_do_not_prove_standard_type_identity() {
    let source = "local _ENV={}; type(value)";
    let bound = bind(&luanalysis_syntax::parse(source));
    assert_eq!(bound.intrinsic_at(source.find("type(").unwrap()), None);
    for write in [
        "local _G={}; _G.type=custom",
        "local rawset=custom; rawset(_G,'type',custom)",
        "_G.other=custom",
    ] {
        let source = format!("type(value); {write}");
        let bound = bind(&luanalysis_syntax::parse(&source));
        assert!(!bound.has_modified_type_intrinsic(), "{write}");
        assert_eq!(
            bound.intrinsic_at(0),
            Some(BuiltinIntrinsic::LuaType),
            "{write}"
        );
    }
}

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis type identity {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, source).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn foreign_ambient_type_contract_cannot_grant_standard_predicate_effects_and_cli_correction_works()
{
    let fixture = Fixture::new();
    let contract = fixture.file(
        "custom.def.lua",
        "---@type fun(value:any):string\nfunction type(value) end\n",
    );
    let main = fixture.file("main.lua", CHECK);
    let resolver = Resolver::new(ProjectConfig::new([fixture.0.clone()])).unwrap();
    let mut plan = ProjectLoadPlan::new([main.clone()]);
    plan.libraries
        .push(ProjectInput::new(contract, SourceRole::Declaration));
    let report = check_project(Project::load_plan(&resolver, plan, static_require_requests));
    assert!(
        has_numeric_error(&report.file(report.project.entries[0]).unwrap().report),
        "{:#?}",
        report.files
    );
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&fixture.0)
            .args(["check", "--library", "custom.def.lua", "main.lua"])
            .output()
            .unwrap()
    };
    let rejected = run();
    assert_eq!(rejected.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(stderr.contains("main.lua:5:15: error[TS2322]"), "{stderr}");
    fs::write(
        main,
        CHECK.replace("local numeric=value", "local numeric=42"),
    )
    .unwrap();
    let fixed = run();
    assert_eq!(
        fixed.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&fixed.stderr)
    );
}

#[test]
fn implementation_replacement_in_another_chunk_suppresses_project_type_effects() {
    let fixture = Fixture::new();
    let main = fixture.file("main.lua", CHECK);
    let replacement = fixture.file("replacement.lua", &format!("{FAKE}type=fake\n"));
    let resolver = Resolver::new(ProjectConfig::new([fixture.0.clone()])).unwrap();
    let report = check_project(Project::load(
        &resolver,
        [main, replacement],
        static_require_requests,
    ));
    assert!(
        has_numeric_error(&report.file(report.project.entries[0]).unwrap().report),
        "{:#?}",
        report.files
    );
}
