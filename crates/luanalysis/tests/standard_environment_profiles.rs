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
fn minimal_profile_attests_original_require_across_successive_imports_and_saved_aliases() {
    let fixture = Fixture::new();
    fixture.file("first.lua", "return 42\n");
    fixture.file("second.lua", "return 'word'\n");
    let source = "local loader=require\nlocal first=require('first')\nlocal second=loader('second')\n---@type number\nlocal numeric=first\n---@type string\nlocal textual=second\n";
    let report = fixture.check(source, StandardEnvironmentProfile::LegacyMinimal);
    clean(&report);
    assert_eq!(report.standard_library_files.len(), 1);
    assert_eq!(report.checked_file_count(), 3);
    let standard = report.source(report.standard_library_files[0]).unwrap();
    assert!(matches!(standard.origin, SourceOrigin::DeclarationLibrary { .. }));
    assert_eq!(standard.path, PathBuf::from("stdlib/LegacyMinimal/require.def.lua"));
    assert_eq!(standard.source, include_str!("../stdlib/LegacyMinimal/require.def.lua"));
    let rejected = fixture.check(&source.replace("local textual=second", "local textual=first"), StandardEnvironmentProfile::LegacyMinimal);
    assert!(code(&rejected, "TS2322"), "{:#?}", rejected.files);
    clean(&fixture.check(source, StandardEnvironmentProfile::LegacyMinimal));
    let cli = fixture.cli(&["--module-root", ".", "main.lua"]);
    assert!(cli.status.success(), "{}{}", String::from_utf8_lossy(&cli.stdout), String::from_utf8_lossy(&cli.stderr));
    assert!(String::from_utf8_lossy(&cli.stdout).contains("No type errors in 3 file(s)."));
}

#[test]
fn minimal_require_registration_does_not_select_other_standard_capabilities_or_the_empty_profile() {
    let fixture = Fixture::new();
    clean(&fixture.check(TYPE_GUARD, StandardEnvironmentProfile::LegacyMinimal));
    for source in ["local value=assert(true)\n", "local value=setmetatable({}, {})\n", "local value=('WORD'):lower()\n"] {
        let report = fixture.check(source, StandardEnvironmentProfile::LegacyMinimal);
        assert!(report.has_errors(), "the require-only resource must not grant another standard capability: {source}");
        assert_eq!(report.standard_library_files.len(), 1);
    }
    let empty = fixture.check("local loader=require\n", StandardEnvironmentProfile::None);
    assert!(code(&empty, "TS2304"), "{:#?}", empty.files);
    assert!(empty.standard_library_files.is_empty());
}

#[test]
fn a_compatible_replacement_does_not_inherit_the_minimal_loader_capability() {
    let fixture = Fixture::new();
    let source = "---@param modname string\n---@return any\nlocal function ordinary(modname) return 42 end\nrequire=ordinary\nlocal value=require('absent')\n";
    let report = fixture.check(source, StandardEnvironmentProfile::LegacyMinimal);
    assert!(code(&report, "LUA2036"), "{:#?}", report.files);
    assert!(!code(&report, "TS2307"), "a rejected runtime callee must not publish its host candidate failure: {:#?}", report.files);
}

#[test]
fn matching_minimal_resource_text_and_display_path_do_not_attest_an_unselected_require() {
    let fixture = Fixture::new();
    fixture.file("first.lua", "---@type number\nlocal poison='wrong'\nreturn 42\n");
    let mut project = fixture.project("local value=require('first')\n");
    project.add_declaration_source(DeclarationSource {
        identity: "user-minimal-require".into(),
        path: PathBuf::from("stdlib/LegacyMinimal/require.def.lua"),
        source: include_str!("../stdlib/LegacyMinimal/require.def.lua").to_owned(),
    }).unwrap();
    let report = check_project_with_options(project, ProjectCheckOptions { environment: StandardEnvironmentProfile::None, ..Default::default() });
    assert!(report.standard_library_files.is_empty());
    assert!(code(&report, "LUA2036"), "{:#?}", report.files);
    assert!(!code(&report, "TS2322"), "an unselected loader cannot schedule the candidate's poison body: {:#?}", report.files);
}

#[test]
fn selected_profile_registers_active_declarations_without_runtime_membership_or_count_noise()
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
    let mut names = Vec::new();
    for id in &report.standard_library_files {
        let source = report.source(*id).unwrap();
        assert!(matches!(
            source.origin,
            SourceOrigin::DeclarationLibrary { .. }
        ));
        assert!(!report.project.entries.contains(id));
        let name = source.path.file_name().unwrap();
        names.push(name.to_str().unwrap());
        let active = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stdlib/Lua54").join(name);
        assert_eq!(source.source, fs::read_to_string(active).unwrap());
    }
    names.sort_unstable();
    assert_eq!(names, ["builtin.def.lua", "coroutine.def.lua", "debug.def.lua", "global.def.lua", "io.def.lua", "math.def.lua", "os.def.lua", "package.def.lua", "string.def.lua", "table.def.lua", "utf8.def.lua"]);
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
fn captured_string_members_keep_selected_library_contracts_in_nested_argument_queries() {
    let fixture = Fixture::new();
    let source = r#"---@alias HostObject {script_state:string}
---@return HostObject[]
local function all() return {} end
local prefix='saved:'
local function make()
    local self={}
    function self.load()
        for _,object in ipairs(all()) do
            local value=object.script_state
            if value:sub(1,prefix:len())==prefix then
                local rest=value:sub(prefix:len()+1)
            end
        end
    end
    return self
end
return make
"#;
    clean(&fixture.check(source, StandardEnvironmentProfile::Lua54));
    let bad = fixture.check(
        &source.replace("prefix:len()", "prefix:len('extra')"),
        StandardEnvironmentProfile::Lua54,
    );
    assert!(code(&bad, "TS2554"), "{:#?}", bad.files);
    let missing = fixture.check(
        &source.replace("prefix:len()", "prefix:noSuchStringMethod()"),
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
    let unsupported = fixture.cli(&["--lua-version", "5.5", "main.lua"]);
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
fn restoring_the_original_assert_alias_retains_its_checked_result_pack() {
    let fixture = Fixture::new();
    let source =
        "local replacement=assert\nassert=replacement\n---@type number,string\nlocal value,label=assert(42,'word')\n";
    let report = fixture.check(source, StandardEnvironmentProfile::Lua54);
    clean(&report);
    let main = report.file(report.project.entries[0]).unwrap();
    assert!(main.report.bindings.iter().any(|binding| binding.name == "label" && binding.ty == "string"), "{:#?}", main.report.bindings);
}

#[test]
fn a_compatible_custom_assert_replacement_has_only_its_ordinary_call_effects() {
    let fixture = Fixture::new();
    let source = "---@generic T\n---@param value T\n---@vararg unknown\n---@return T\nlocal function custom(value,...) return value end\nassert=custom\n---@param value number|nil\nlocal function consume(value)\nassert(value)\n---@type number\nlocal wrong=value\nend\nconsume(42)\n";
    let report = fixture.check(source, StandardEnvironmentProfile::Lua54);
    assert!(!code(&report, "LUA2039"), "{:#?}", report.files);
    let main = report.file(report.project.entries[0]).unwrap();
    let errors: Vec<_> = main
        .report
        .diagnostics
        .iter()
        .collect();
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_eq!(errors[0].code, "TS2322");
    assert_eq!(&source[errors[0].span.start..errors[0].span.end], "value");
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

#[test]
fn each_lua_version_selects_only_its_original_standard_declarations() {
    for (profile, directory, count, has_bit32, has_utf8, has_package) in [
        (StandardEnvironmentProfile::Lua50, "Lua50", 9, false, false, false),
        (StandardEnvironmentProfile::Lua51, "Lua51", 10, false, false, true),
        (StandardEnvironmentProfile::Lua52, "Lua52", 11, true, false, true),
        (StandardEnvironmentProfile::Lua53, "Lua53", 12, true, true, true),
        (StandardEnvironmentProfile::Lua54, "Lua54", 11, false, true, true),
    ] {
        let fixture = Fixture::new();
        let report = fixture.check("local value=math.abs(-1)\nlocal text=('WORD'):lower()\n", profile);
        clean(&report);
        assert_eq!(report.standard_library_files.len(), count, "{profile:?}");
        assert_eq!(report.checked_file_count(), 1, "{profile:?}");
        let mut names = Vec::new();
        for id in &report.standard_library_files {
            let source = report.source(*id).unwrap();
            assert!(matches!(source.origin, SourceOrigin::DeclarationLibrary { .. }));
            assert_eq!(source.path.parent().unwrap(), std::path::Path::new("stdlib").join(directory));
            names.push(source.path.file_name().unwrap().to_str().unwrap().to_owned());
        }
        assert_eq!(names.iter().any(|name| name == "bit32.def.lua"), has_bit32);
        assert_eq!(names.iter().any(|name| name == "utf8.def.lua"), has_utf8);
        assert_eq!(names.iter().any(|name| name == "package.def.lua"), has_package);
    }
}

#[test]
fn lua52_bit32_checks_original_arguments_and_results_without_later_api_leaks() {
    let fixture = Fixture::new();
    let source = "---@type number\nlocal masked=bit32.band(7,3)\n---@type number\nlocal field=bit32.extract(masked,0)\n---@type number\nlocal width=bit32.extract(masked,0,2)\n";
    clean(&fixture.check(source, StandardEnvironmentProfile::Lua52));
    let bad = fixture.check(&source.replace("bit32.band(7,3)", "bit32.band('wrong',3)"), StandardEnvironmentProfile::Lua52);
    assert!(code(&bad, "TS2345"), "{:#?}", bad.files);
    clean(&fixture.check(source, StandardEnvironmentProfile::Lua52));
    for profile in [StandardEnvironmentProfile::Lua51, StandardEnvironmentProfile::Lua54] {
        let missing = fixture.check("local value=bit32.band(7,3)\n", profile);
        assert!(code(&missing, "TS2304"), "{profile:?}: {:#?}", missing.files);
    }
    let later = fixture.check("local value=utf8.char(65)\n", StandardEnvironmentProfile::Lua52);
    assert!(code(&later, "TS2304"), "{:#?}", later.files);
}

#[test]
fn lua52_selected_original_type_and_string_capabilities_keep_checked_contracts() {
    let fixture = Fixture::new();
    clean(&fixture.check(TYPE_GUARD, StandardEnvironmentProfile::Lua52));
    let source = "---@type string\nlocal lowered=('WORD'):lower()\n";
    clean(&fixture.check(source, StandardEnvironmentProfile::Lua52));
    let wrong = fixture.check(&source.replace("---@type string", "---@type number"), StandardEnvironmentProfile::Lua52);
    assert!(code(&wrong, "TS2322"), "{:#?}", wrong.files);
}

#[test]
fn cli_selects_lua52_bit32_and_accepts_the_corrected_original_source() {
    let fixture = Fixture::new();
    fixture.file("main.lua", "local value=bit32.band('wrong',3)\n");
    let bad = fixture.cli(&["--lua-version", "5.2", "main.lua"]);
    assert_eq!(bad.status.code(), Some(1), "{bad:?}");
    assert!(String::from_utf8_lossy(&bad.stderr).contains("TS2345"), "{bad:?}");
    fixture.file("main.lua", "---@type number\nlocal value=bit32.band(7,3)\n");
    let corrected = fixture.cli(&["--lua-version", "5.2", "main.lua"]);
    assert_eq!(corrected.status.code(), Some(0), "{corrected:?}");
    let unsupported = fixture.cli(&["--lua-version", "5.5", "main.lua"]);
    assert_eq!(unsupported.status.code(), Some(2));
}

#[test]
fn cli_lua_versions_preserve_the_matching_require_result_count() {
    for version in ["5.0", "5.1", "5.2", "5.3", "5.4"] {
        let fixture = Fixture::new();
        fixture.file("value.lua", "return 42\n");
        let metadata_type = if version == "5.4" { "nil|string" } else { "nil" };
        let source = format!("local module,metadata=require('value')\n---@type number\nlocal first=module\n---@type {metadata_type}\nlocal second=metadata\n");
        fixture.file("main.lua", &source);
        let invoke = || fixture.cli(&["--lua-version", version, "--format", "json", "main.lua"]);
        let accepted = invoke();
        assert_eq!(accepted.status.code(), Some(0), "{version}: {accepted:?}");
        let wrong = source.replace(&format!("---@type {metadata_type}"), "---@type boolean");
        fixture.file("main.lua", &wrong);
        let rejected = invoke();
        assert_eq!(rejected.status.code(), Some(1), "{version}: {rejected:?}");
        let report = String::from_utf8(rejected.stdout).unwrap();
        assert!(report.contains("\"code\":\"TS2322\""), "{version}: {report}");
        let start = wrong.rfind("metadata").unwrap();
        assert!(report.contains(&format!("\"range\":{{\"start\":{start},\"end\":{}}}", start + "metadata".len())), "{version}: {report}");
        fixture.file("main.lua", &source);
        let corrected = invoke();
        assert_eq!(corrected.status.code(), Some(0), "{version}: {corrected:?}");
    }
}
