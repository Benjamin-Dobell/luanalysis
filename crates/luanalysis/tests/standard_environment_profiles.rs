use luanalysis::project_check::{
    ProjectCheckOptions, ProjectCheckReport, check_project_with_options, static_require_requests,
};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{
    DeclarationSource, Project, ProjectConfig, ProjectLoadPlan, Resolver, SourceOrigin,
};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis standard profiles {} {}",
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
    fn project(&self, source: &str) -> Project {
        let main = self.file("main.lua", source);
        let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
        Project::load_plan(
            &resolver,
            ProjectLoadPlan::new([main]),
            static_require_requests,
        )
    }
    fn check(&self, source: &str, environment: StandardEnvironmentProfile) -> ProjectCheckReport {
        check_project_with_options(
            self.project(source),
            ProjectCheckOptions {
                environment,
                ..Default::default()
            },
        )
    }
    fn cli(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .arg("check")
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn clean(report: &ProjectCheckReport) {
    if report.has_errors() {
        let diagnostics: Vec<_> = report
            .files
            .iter()
            .filter(|file| !file.report.diagnostics.is_empty())
            .map(|file| {
                (
                    report.source(file.file).map(|source| &source.path),
                    &file.report.diagnostics,
                )
            })
            .collect();
        panic!(
            "source diagnostics: {diagnostics:#?}\nenvironment errors: {:#?}\nentry errors: {:#?}\nlibrary errors: {:#?}\nrole conflicts: {:#?}\nunresolved dependencies: {:#?}",
            report.environment_errors,
            report.project.entry_errors,
            report.project.library_errors,
            report.project.role_conflicts,
            report
                .project
                .dependencies
                .iter()
                .filter(|dependency| !matches!(
                    dependency.outcome,
                    luanalysis_project::DependencyOutcome::Resolved(_)
                ))
                .collect::<Vec<_>>()
        );
    }
}
fn code(report: &ProjectCheckReport, code: &str) -> bool {
    report
        .files
        .iter()
        .flat_map(|f| &f.report.diagnostics)
        .any(|d| d.code == code)
}
const TYPE_GUARD: &str = "---@param value number|string\nlocal function check(value)\nif type(value)=='number' then\n---@type number\nlocal numeric=value\nend\nend\n";

#[test]
fn bundled_lua54_resources_match_exact_pristine_import_and_documented_patch_replay() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/stdlib-lua54.sh");
    let output = Command::new("bash").arg(script).arg("--verify").output().unwrap();
    assert!(output.status.success(), "{}{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[test]
fn selected_profile_registers_exact_replayed_declarations_without_runtime_membership_or_count_noise()
 {
    let fixture = Fixture::new();
    let report = fixture.check(
        "local value=math.abs(-1)\n---@type string\nlocal rendered=tostring(value)\n",
        StandardEnvironmentProfile::Lua54,
    );
    clean(&report);
    assert_eq!(report.standard_library_files.len(), 11);
    assert_eq!(report.checked_file_count(), 1);
    assert_eq!(report.project.entries.len(), 1);
    assert!(report.project.dependencies.is_empty());
    for id in &report.standard_library_files {
        let source = report.source(*id).unwrap();
        assert!(matches!(
            source.origin,
            SourceOrigin::DeclarationLibrary { .. }
        ));
        assert!(!report.project.entries.contains(id));
        let name = source.path.file_name().unwrap();
        let replayed = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stdlib/Lua54").join(name);
        assert_eq!(source.source, fs::read_to_string(replayed).unwrap());
    }
}

#[test]
fn selected_string_namespace_supports_dot_colon_and_chained_original_result_packs() {
    let fixture = Fixture::new();
    let source = "local string=42\nlocal normalized=('GE_TTS.Debug'):lower()\nlocal identifier=normalized:gsub('^ge_tts%.',''):gsub('[/%.]','__')\nlocal part=identifier:sub(1,3)\n---@type string\nlocal checked=part\nlocal replaced,count=identifier:gsub('_','-')\n---@type number\nlocal checked_count=count\n";
    clean(&fixture.check(source, StandardEnvironmentProfile::Lua54));
    let bad = fixture.check(
        "local wrong=('word'):sub('bad')\n",
        StandardEnvironmentProfile::Lua54,
    );
    assert!(code(&bad, "TS2345"), "{:#?}", bad.files);
    let missing = fixture.check(
        "local wrong=('word'):absent()\n",
        StandardEnvironmentProfile::Lua54,
    );
    assert!(code(&missing, "TS2339"), "{:#?}", missing.files);
}

#[test]
fn original_selected_type_declaration_grants_flow_but_compatible_replacement_does_not() {
    let fixture = Fixture::new();
    clean(&fixture.check(TYPE_GUARD, StandardEnvironmentProfile::Lua54));
    let replacement = format!(
        "---@param value any\n---@return 'number'\nlocal function fake(value) return 'number' end\ntype=fake\n{TYPE_GUARD}"
    );
    let report = fixture.check(&replacement, StandardEnvironmentProfile::Lua54);
    assert!(code(&report, "TS2322"), "{:#?}", report.files);
}

#[test]
fn explicit_empty_environment_disables_legacy_minimal_type_fallback() {
    let fixture = Fixture::new();
    clean(&fixture.check(TYPE_GUARD, StandardEnvironmentProfile::LegacyMinimal));
    let report = fixture.check("local kind=type(1)", StandardEnvironmentProfile::None);
    assert!(code(&report, "TS2304"), "{:#?}", report.files);
    assert!(report.standard_library_files.is_empty());
}

#[test]
fn arbitrary_library_origin_and_matching_display_path_do_not_grant_primitive_capabilities() {
    let fixture = Fixture::new();
    let mut project = fixture.project("local wrong=('word'):lower()\n");
    project.add_declaration_source(DeclarationSource {identity:"user-selected-string".into(),path:PathBuf::from("stdlib/Lua54/string.def.lua"),source:"---@shape UserString\nstring={}\n---@param value string\n---@return number\nfunction string.lower(value) end\n".into()}).unwrap();
    let report = check_project_with_options(
        project,
        ProjectCheckOptions {
            environment: StandardEnvironmentProfile::None,
            ..Default::default()
        },
    );
    assert!(code(&report, "TS2339"), "{:#?}", report.files);
    assert!(report.standard_library_files.is_empty());
    assert_eq!(report.checked_file_count(), 2);
}

#[test]
fn conflicting_bundled_identity_fails_explicitly_without_replacing_user_original_source() {
    let fixture = Fixture::new();
    let mut project = fixture.project("return 1");
    let original = project
        .add_declaration_source(DeclarationSource {
            identity: format!("luanalysis.std.Lua54.string@{}", env!("CARGO_PKG_VERSION")),
            path: PathBuf::from("stdlib/Lua54/string.def.lua"),
            source: "---@type number\ncustom=42\n".into(),
        })
        .unwrap();
    let report = check_project_with_options(
        project,
        ProjectCheckOptions {
            environment: StandardEnvironmentProfile::Lua54,
            ..Default::default()
        },
    );
    assert!(report.has_errors());
    assert_eq!(report.environment_errors.len(), 1);
    assert!(
        report
            .source(original)
            .unwrap()
            .source
            .contains("custom=42")
    );
}

#[test]
fn profile_declaration_membership_does_not_expand_runtime_module_roots() {
    let fixture = Fixture::new();
    let report = fixture.check(
        "local value=require('luanalysis.std.Lua54.math')\n",
        StandardEnvironmentProfile::Lua54,
    );
    assert!(code(&report, "TS2307"), "{:#?}", report.files);
}

#[test]
fn cli_profile_selection_original_error_correction_and_explicit_empty_mode_are_verified() {
    let fixture = Fixture::new();
    fixture.file("main.lua", "local value=('word'):sub('bad')\n");
    let bad = fixture.cli(&["--lua-version", "5.4", "main.lua"]);
    assert_eq!(bad.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("main.lua:1:26: error[TS2345]"),
        "{}",
        String::from_utf8_lossy(&bad.stderr)
    );
    fixture.file("main.lua", "local value=('word'):sub(2)\n");
    let fixed = fixture.cli(&["--lua-version", "5.4", "main.lua"]);
    assert_eq!(
        fixed.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&fixed.stderr)
    );
    assert!(String::from_utf8_lossy(&fixed.stdout).contains("1 file(s)"));
    fixture.file("main.lua", "local kind=type(1)\n");
    let empty = fixture.cli(&["--no-stdlib", "main.lua"]);
    assert_eq!(empty.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&empty.stderr).contains("TS2304"));
    let unsupported = fixture.cli(&["--lua-version", "5.1", "main.lua"]);
    assert_eq!(unsupported.status.code(), Some(2));
}

#[test]
fn trusted_assert_preserves_original_argument_packs_and_removes_falsey_first_rows() {
    let fixture = Fixture::new();
    let source = "---@type number,string,false\nlocal first,second,third=assert(42,'word',false)\nlocal length=assert(tonumber('12'))\n---@type number\nlocal checked_length=length\n---@param mode number\n---@return (number,string)|(nil,boolean)|(false,number)\nlocal function produce(mode)\nif mode==1 then return 42,'word' end\nif mode==2 then return nil,true end\nreturn false,1\nend\nlocal count,label=assert(produce(1))\n---@type number\nlocal checked_count=count\n---@type string\nlocal checked_label=label\n";
    clean(&fixture.check(source, StandardEnvironmentProfile::Lua54));
}

#[test]
fn trusted_assert_capability_survives_an_unannotated_local_alias() {
    let fixture = Fixture::new();
    let source =
        "local enforce=assert\n---@type number,string\nlocal value,label=enforce(42,'word')\n";
    clean(&fixture.check(source, StandardEnvironmentProfile::Lua54));
}

#[test]
fn selected_assert_narrows_original_statement_continuations_and_aliases() {
    let fixture = Fixture::new();
    for assertion in ["assert(value)", "local enforce=assert\nenforce(value)"] {
        clean(&fixture.check(
            &format!("---@param value number|nil\nlocal function consume(value)\n{assertion}\n---@type number\nlocal checked=value\nend\nconsume(42)\n"),
            StandardEnvironmentProfile::Lua54,
        ));
    }
    let bad = fixture.check(
        "---@param value number|nil\nlocal function consume(value)\nlocal function assert(value) end\nassert(value)\n---@type number\nlocal checked=value\nend\nconsume(42)\n",
        StandardEnvironmentProfile::Lua54,
    );
    assert!(code(&bad, "TS2322"), "{:#?}", bad.files);
}

#[test]
fn profile_cli_json_preserves_transport_and_excludes_only_selected_standard_files() {
    let fixture = Fixture::new();
    fixture.file("main.lua", "local value=math.abs(-1)\n");
    let good = fixture.cli(&["--format", "json", "--lua-version", "5.4", "main.lua"]);
    assert_eq!(good.status.code(), Some(0), "{:?}", good);
    assert!(good.stderr.is_empty(), "{:?}", good);
    let json = String::from_utf8_lossy(&good.stdout);
    assert!(json.contains("\"filesChecked\":1"), "{json}");
    let bad = fixture.cli(&["--lua-version", "unsupported", "--format", "json", "main.lua"]);
    assert_eq!(bad.status.code(), Some(2), "{:?}", bad);
    assert!(bad.stderr.is_empty(), "{:?}", bad);
    let json = String::from_utf8_lossy(&bad.stdout);
    assert!(json.contains("LUA0000"), "{json}");
}

#[test]
fn changed_standard_assert_value_is_an_explicit_original_source_capability_gate() {
    let fixture = Fixture::new();
    let source =
        "local replacement=assert\nassert=replacement\nlocal value,label=assert(42,'word')\n";
    let report = fixture.check(source, StandardEnvironmentProfile::Lua54);
    assert!(code(&report, "LUA2039"), "{:#?}", report.files);
    let main = report.file(report.project.entries[0]).unwrap();
    let gate = main
        .report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "LUA2039")
        .unwrap();
    assert_eq!(&source[gate.span.start..gate.span.end], "assert");
}

#[test]
fn legacy_or_user_library_schemas_do_not_gain_standard_assert_pack_effects() {
    let fixture = Fixture::new();
    let mut project =
        fixture.project("---@type number,string\nlocal first,second=assert(42,'word')\n");
    project
        .add_declaration_source(DeclarationSource {
            identity: "custom-assert".into(),
            path: PathBuf::from("stdlib/Lua54/global.def.lua"),
            source: "---@type fun<T>(value?:T,...:unknown):T\nfunction assert(value,...) end\n"
                .into(),
        })
        .unwrap();
    let report = check_project_with_options(
        project,
        ProjectCheckOptions {
            environment: StandardEnvironmentProfile::None,
            ..Default::default()
        },
    );
    assert!(code(&report, "TS2322"), "{:#?}", report.files);
}

#[test]
fn primitive_string_members_retain_original_context_inside_generic_callbacks() {
    let fixture = Fixture::new();
    let source = "---@generic T:string,R\n---@param value T\n---@param callback fun(value:T):R\n---@return R\nlocal function apply(value,callback) return callback(value) end\nlocal lowered=apply('WORD',function(value) return value:lower() end)\n---@type string\nlocal checked=lowered\n";
    clean(&fixture.check(source, StandardEnvironmentProfile::Lua54));
}
