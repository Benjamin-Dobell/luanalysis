use luanalysis::project_check::{
    ProjectCheckOptions, ProjectCheckReport, check_project, check_project_with_options,
    static_require_requests,
};
use luanalysis_engine::NativeLoaderPolicy;
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
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
            "luanalysis modules {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        path
    }
    fn project(&self, entry: PathBuf) -> Project {
        let mut config = ProjectConfig::new([self.0.clone()]);
        config.set_package_path("?.lua;?/init.lua;?.ttslua;?/init.ttslua");
        Project::load_plan(
            &Resolver::new(config).unwrap(),
            ProjectLoadPlan::new([entry]),
            static_require_requests,
        )
    }
    fn check(&self, source: &str) -> ProjectCheckReport {
        check_project(self.project(self.file("main.lua", source)))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn clean(report: &ProjectCheckReport) {
    for file in &report.files {
        assert!(
            file.report.diagnostics.is_empty(),
            "{}: {:#?}",
            report.source(file.file).unwrap().path.display(),
            file.report.diagnostics
        );
    }
    assert!(!report.has_errors());
}
fn ty<'a>(report: &'a ProjectCheckReport, name: &str) -> &'a str {
    &report
        .files
        .iter()
        .flat_map(|file| &file.report.bindings)
        .find(|binding| binding.name == name)
        .unwrap()
        .ty
}

#[test]
fn actual_chunk_scalar_and_table_exports_are_checked_in_the_importer() {
    let fixture = Fixture::new();
    fixture.file("count.lua", "return 42,'discarded'");
    fixture.file("record.lua", "return {id=42,label='item'}");
    let report=fixture.check("local count=require('count')\nlocal record=require('record')\n---@type number\nlocal accepted=count\n---@type string\nlocal label=record.label\n---@type number\nlocal id=record.id\n");
    clean(&report);
    assert_eq!(report.project.files.len() - report.standard_library_files.len(), 3);
    assert_eq!(ty(&report, "count"), "number");
    assert_eq!(ty(&report, "label"), "string");
}
#[test]
fn empty_or_nil_modules_export_true_and_false_is_not_changed_into_true() {
    let fixture = Fixture::new();
    fixture.file("empty.lua", "");
    fixture.file("nil.lua", "return nil");
    fixture.file(
        "false.lua",
        "---@type false\nlocal value=false\nreturn value",
    );
    let report=fixture.check("---@type true\nlocal empty=require('empty')\n---@type true\nlocal nil_value=require('nil')\nlocal false_value=require('false')\n");
    clean(&report);
    assert_eq!(ty(&report, "empty"), "true");
    assert_eq!(ty(&report, "nil_value"), "true");
    assert_ne!(ty(&report, "false_value"), "true");
}
#[test]
fn lua54_loader_data_rows_are_separate_from_discarded_chunk_results() {
    let fixture = Fixture::new();
    fixture.file("tuple.lua", "return 42,false,'not loader data'");
    let report=fixture.check("local value,loader=require('tuple')\n---@type number\nlocal accepted=value\n---@type string|nil\nlocal source=loader\n");
    clean(&report);
    assert_eq!(ty(&report, "value"), "number");
    assert!(ty(&report, "loader").contains("string"));
    assert!(!ty(&report, "loader").contains("false"));
    let old = check_project_with_options(
        fixture.project(fixture.file(
            "old.lua",
            "local value,loader=require('tuple')\n---@type nil\nlocal no_loader=loader\n",
        )),
        ProjectCheckOptions {
            loader_policy: NativeLoaderPolicy::Lua53StandardFile,
            ..ProjectCheckOptions::default()
        },
    );
    clean(&old);
    assert_eq!(ty(&old, "loader"), "nil");
}
#[test]
fn exported_generic_functions_and_captured_result_packs_keep_native_identity() {
    let fixture = Fixture::new();
    fixture.file("identity.lua","---@generic T\n---@param value T\n---@return T\nlocal function identity(value) return value end\nreturn identity");
    fixture.file("closure.lua","---@generic T\n---@param value T\nlocal function outer(value) return function() return value,value end end\nreturn outer");
    let report=fixture.check("local identity=require('identity')\nlocal outer=require('closure')\nlocal numeric=identity(42)\nlocal textual=identity('word')\nlocal number=outer(42)\nlocal text=outer('word')\nlocal one,two=number()\nlocal three,four=text()\n---@type number\nlocal numeric_check=two\n---@type string\nlocal text_check=four\n");
    clean(&report);
    assert_eq!(ty(&report, "numeric"), "number");
    assert_eq!(ty(&report, "textual"), "string");
    assert_eq!(ty(&report, "two"), "number");
    assert_eq!(ty(&report, "four"), "string");
}
#[test]
fn importer_callback_context_does_not_leak_into_an_exported_chunk_initializer() {
    let fixture = Fixture::new();
    fixture.file(
        "value.lua",
        "local value={id=42}\nreturn function() return value.id end",
    );
    let report=fixture.check("---@generic T,R\n---@param value T\n---@param project fun(value:T):R\n---@return R\nlocal function transform(value,project) return project(value) end\nlocal result=transform({label='word'},function(value) local getter=require('value') return getter() end)\n---@type number\nlocal accepted=result\n");
    clean(&report);
    assert_eq!(ty(&report, "result"), "number");
}
#[test]
fn original_chunk_reachability_excludes_unreachable_return_rows() {
    let fixture = Fixture::new();
    fixture.file(
        "selected.lua",
        "if true then return 42 end\nreturn 'unreachable'",
    );
    let report =
        fixture.check("local value=require('selected')\n---@type number\nlocal accepted=value\n");
    clean(&report);
    assert_eq!(ty(&report, "value"), "number");
}
#[test]
fn unused_errors_in_required_modules_keep_their_original_owner_source() {
    let fixture = Fixture::new();
    fixture.file(
        "broken.lua",
        "---@type number\nlocal unused='wrong'\nreturn 42",
    );
    let report = fixture.check("local value=require('broken')");
    assert!(report.has_errors());
    let owner = report
        .files
        .iter()
        .find(|file| {
            report
                .source(file.file)
                .unwrap()
                .path
                .ends_with("broken.lua")
        })
        .unwrap();
    assert!(
        owner
            .report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2322"
                && &report.source(owner.file).unwrap().source[error.span.start..error.span.end]
                    == "'wrong'")
    );
}

#[test]
fn rejected_candidates_keep_source_identities_without_checking_their_transitive_bodies() {
    let fixture = Fixture::new();
    let candidate = fixture.file("candidate.lua", "local unused=require('poison')\nreturn 42\n");
    let poison = fixture.file("poison.lua", "---@type number\nlocal unused='wrong'\nreturn 42\n");
    let run = |source: &str| check_project_with_options(
        fixture.project(fixture.file("main.lua", source)),
        ProjectCheckOptions {
            environment: luanalysis::standard_environment::StandardEnvironmentProfile::Lua54,
            ..ProjectCheckOptions::default()
        },
    );
    let rejected = run("local loader=require\nloader=function(name) return 42 end\nlocal value=loader('candidate')\n");
    clean(&rejected);
    assert_eq!(rejected.project.dependencies.len(), 2);
    assert_eq!(rejected.checked_file_count(), 1);
    for path in [&candidate, &poison] {
        let original = rejected.project.file_by_path(path).unwrap();
        assert!(rejected.file(original.id).is_none());
    }

    let proven = run("local value=require('candidate')\n");
    assert!(proven.has_errors());
    assert_eq!(proven.checked_file_count(), 3);
    let original = proven.project.file_by_path(&poison).unwrap();
    let report = proven.file(original.id).unwrap();
    assert!(report.report.diagnostics.iter().any(|error| error.code == "TS2322"
        && &original.source[error.span.start..error.span.end] == "'wrong'"));
}

// `require` is recognised by name (Luanalysis v1 semantics): a saved field or a
// returned function holding it is an ordinary call that activates no module.
#[test]
fn saved_field_and_returned_loaders_are_ordinary_calls() {
    let fixture = Fixture::new();
    fixture.file("record.lua", "return {value=42}\n");
    for prefix in [
        "local operations={loader=require}\nlocal saved=operations.loader\n",
        "local function originalLoader() return require end\nlocal saved=originalLoader()\n",
        "local operations={loader=require}\nlocal first=operations['loader']\nlocal saved=first\n",
    ] {
        let source = format!("{prefix}local record=saved('record')\n---@type number\nlocal accepted=record.value\n");
        let run = |source: &str| check_project_with_options(
            fixture.project(fixture.file("main.lua", source)),
            ProjectCheckOptions {
                environment: luanalysis::standard_environment::StandardEnvironmentProfile::Lua54,
                ..ProjectCheckOptions::default()
            },
        );
        let ordinary = run(&source);
        clean(&ordinary);
        assert_eq!(ordinary.checked_file_count(), 1);
    }
}

#[test]
fn a_holder_loader_field_never_activates_a_module() {
    let fixture = Fixture::new();
    let target = fixture.file("record.lua", "---@type number\nlocal invalid='wrong'\nreturn 9\n");
    let source = "---@param name string\n---@return number\nlocal function ordinary(name) return 7 end\nlocal operations={loader=require}\nlocal copy=operations\ncopy.loader=ordinary\nlocal savedLoader=operations.loader\nlocal result=savedLoader('record')\n---@type number\nlocal accepted=result\n";
    let run = |source: &str| check_project_with_options(
        fixture.project(fixture.file("main.lua", source)),
        ProjectCheckOptions {
            environment: luanalysis::standard_environment::StandardEnvironmentProfile::Lua54,
            ..ProjectCheckOptions::default()
        },
    );
    for source in [source.to_owned(), source.replace("copy.loader=ordinary", "copy.loader=ordinary\ncopy.loader=require")] {
        let ordinary = run(&source);
        clean(&ordinary);
        assert_eq!(ordinary.checked_file_count(), 1);
        assert!(ordinary.file(ordinary.project.file_by_path(&target).unwrap().id).is_none());
    }
}

#[test]
fn ordinary_value_candidates_do_not_check_target_bodies_or_publish_host_errors() {
    let fixture = Fixture::new();
    let candidate = fixture.file("candidate.lua", "type=nil\nlocal ignored=require('poison')\nreturn 42\n");
    let poison = fixture.file("poison.lua", "---@type number\nlocal invalid='wrong'\nreturn 42\n");
    let broken = fixture.file("broken.lua", "local invalid=\n");
    for prefix in [
        "local operations={loader=ordinary}\nlocal saved=operations.loader\n",
        "local function getLoader() return ordinary end\nlocal saved=getLoader()\n",
    ] {
        for environment in [
            luanalysis::standard_environment::StandardEnvironmentProfile::Lua54,
            luanalysis::standard_environment::StandardEnvironmentProfile::None,
        ] {
            let mut source = format!("---@param name string\n---@return number\nlocal function ordinary(name) return 42 end\n{prefix}local result=saved('candidate')\nsaved('broken')\nsaved('absent')\n---@type number\nlocal accepted=result\n");
            if environment == luanalysis::standard_environment::StandardEnvironmentProfile::Lua54 {
                source.push_str("---@param value number|string\nlocal function probe(value)\nif type(value)=='number' then\n---@type number\nlocal narrowed=value\nend\nend\nprobe(42)\n");
            }
            let report = check_project_with_options(
                fixture.project(fixture.file("main.lua", &source)),
                ProjectCheckOptions { environment, ..ProjectCheckOptions::default() },
            );
            clean(&report);
            assert_eq!(report.checked_file_count(), 1);
            assert_eq!(ty(&report, "accepted"), "number");
            for path in [&candidate, &poison, &broken] {
                let original = report.project.file_by_path(path).unwrap();
                assert!(report.file(original.id).is_none());
            }
        }
    }
}

#[test]
fn value_discovery_does_not_hide_nonreturning_call_effects() {
    let fixture = Fixture::new();
    for prefix in [
        "local operations={stop=error}\nlocal stop=operations.stop\n",
        "local function getStop() return error end\nlocal stop=getStop()\n",
    ] {
        let source = format!("{prefix}local function infer()\nstop('absent')\nreturn 'unreachable'\nend\nlocal result=infer()\n---@type number\nlocal accepted=result\n");
        let report = check_project_with_options(
            fixture.project(fixture.file("main.lua", &source)),
            ProjectCheckOptions {
                environment: luanalysis::standard_environment::StandardEnvironmentProfile::Lua54,
                ..ProjectCheckOptions::default()
            },
        );
        clean(&report);
        assert_eq!(ty(&report, "result"), "never");
        assert_eq!(report.checked_file_count(), 1);
    }
}

#[test]
fn invalid_candidate_syntax_is_published_only_after_the_original_target_is_required() {
    let fixture = Fixture::new();
    let broken = fixture.file("broken.lua", "local invalid=\n");
    let run = |source: &str| check_project_with_options(
        fixture.project(fixture.file("main.lua", source)),
        ProjectCheckOptions {
            environment: luanalysis::standard_environment::StandardEnvironmentProfile::Lua54,
            ..ProjectCheckOptions::default()
        },
    );
    let rejected = run("local loader=require\nloader=function(name) return 42 end\nlocal value=loader('broken')\n");
    clean(&rejected);
    let original = rejected.project.file_by_path(&broken).unwrap();
    assert!(rejected.file(original.id).is_none());
    assert_eq!(rejected.checked_file_count(), 1);

    let proven = run("local value=require('broken')\n");
    assert!(proven.has_errors());
    let original = proven.project.file_by_path(&broken).unwrap();
    assert!(proven.file(original.id).unwrap().report.diagnostics.iter().any(|error| error.code == "LUA1001"));
    assert!(proven.file(proven.project.entries[0]).unwrap().report.diagnostics.iter().any(|error| error.code == "LUA2036"));
    assert_eq!(proven.checked_file_count(), 2);
}

#[test]
fn explicit_entries_and_implementation_libraries_remain_checking_roots() {
    let fixture = Fixture::new();
    let main = fixture.file("main.lua", "local loader=require\nloader=function(name) return 42 end\nlocal value=loader('candidate')\n");
    let candidate = fixture.file("candidate.lua", "---@type number\nlocal ignored='candidate error'\nreturn 42\n");
    let entry = fixture.file("entry.lua", "---@type number\nlocal invalid='entry error'\n");
    let library = fixture.file("library.lua", "---@type number\nlocal invalid='library error'\n");
    let project = Project::load_plan(
        &Resolver::new(ProjectConfig::new([fixture.0.clone()])).unwrap(),
        ProjectLoadPlan {
            entries: vec![main, entry.clone()],
            libraries: vec![luanalysis_project::ProjectInput::new(&library, luanalysis_project::SourceRole::Implementation)],
        },
        static_require_requests,
    );
    let report = check_project_with_options(project, ProjectCheckOptions {
        environment: luanalysis::standard_environment::StandardEnvironmentProfile::Lua54,
        ..ProjectCheckOptions::default()
    });
    assert_eq!(report.checked_file_count(), 3);
    assert!(report.has_errors());
    for path in [&entry, &library] {
        let original = report.project.file_by_path(path).unwrap();
        assert!(report.file(original.id).unwrap().report.diagnostics.iter().any(|error| error.code == "TS2322"));
    }
    let original = report.project.file_by_path(&candidate).unwrap();
    assert!(report.file(original.id).is_none());
}
#[test]
fn missing_dynamic_and_invalid_require_requests_fail_at_original_call_arguments() {
    let fixture = Fixture::new();
    for (source, code, origin) in [
        ("local value=require('absent')", "TS2307", "'absent'"),
        (
            "local name='absent'\nlocal value=require(name)",
            "LUA2036",
            "name",
        ),
        ("local value=require()", "TS2554", "require"),
    ] {
        let report = fixture.check(source);
        assert!(report.has_errors());
        let main = report.file(report.project.entries[0]).unwrap();
        assert!(
            main.report
                .diagnostics
                .iter()
                .any(|error| error.code == code
                    && &source[error.span.start..error.span.end] == origin),
            "{source}: {:#?}",
            main.report.diagnostics
        );
    }
}
#[test]
fn shadowed_require_and_environment_loader_mutation_do_not_publish_phantom_module_errors() {
    let fixture = Fixture::new();
    let shadow=fixture.check("---@param name string\nlocal function require(name) return 42 end\nlocal value=require('absent')");
    clean(&shadow);
    assert_eq!(shadow.project.files.len() - shadow.standard_library_files.len(), 1);
    assert!(shadow.project.dependencies.is_empty());
    for source in [
        "local _ENV={}\nlocal value=require('absent')",
        "require=custom\nlocal value=require('absent')",
        "package.loaded.absent={}\nlocal value=require('absent')",
    ] {
        let report = fixture.check(source);
        assert!(report.has_errors());
        assert_eq!(report.project.files.len() - report.standard_library_files.len(), 1, "{source}: {:#?}", report.project.files);
        assert!(!report.files.iter().flat_map(|file| &file.report.diagnostics).any(|error| error.code == "TS2307"), "{source}: {:#?}", report.files);
    }
}
#[test]
fn alias_only_module_cycles_report_a_native_guard_error_without_final_any_exports() {
    let fixture = Fixture::new();
    fixture.file("first.lua", "local next=require('second')\nreturn next");
    fixture.file("second.lua", "local next=require('first')\nreturn next");
    let report = fixture.check("local value=require('first')\nlocal again=require('first')");
    assert!(report.has_errors());
    assert!(!report.project.cycles.is_empty());
    assert!(
        report.files.iter().any(|file| file
            .report
            .diagnostics
            .iter()
            .any(|error| error.code == "LUA2036" && error.message.contains("circular require"))),
        "{:#?}",
        report.files
    );
}
#[test]
fn cli_import_error_is_fixed_in_original_lua_and_explicit_module_roots_are_used() {
    let fixture = Fixture::new();
    fixture.file("modules/count.ttslua", "return 42");
    let main = fixture.file(
        "src/main.lua",
        "-- café\r\nlocal value=require('count')\r\n---@type string\r\nlocal invalid=value\r\n",
    );
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&fixture.0)
            .args([
                "check",
                "--dump-types",
                "--module-root",
                "modules",
                "--package-path",
                "?.ttslua;?.lua",
                "src/main.lua",
            ])
            .output()
            .unwrap()
    };
    let wrong = run();
    assert_eq!(
        wrong.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&wrong.stderr)
    );
    assert!(
        String::from_utf8_lossy(&wrong.stderr).contains("src/main.lua:4:15: error[TS2322]"),
        "{}",
        String::from_utf8_lossy(&wrong.stderr)
    );
    let original = fs::read_to_string(&main).unwrap();
    fs::write(main, original.replace("---@type string", "---@type number")).unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    assert!(String::from_utf8_lossy(&corrected.stdout).contains("No type errors in 2 file(s)."));
}
#[test]
fn lua54_require_expansion_keeps_optional_loader_arity_while_scalar_use_is_one_value() {
    let fixture = Fixture::new();
    fixture.file("count.lua", "return 42");
    let report=fixture.check("local function zero() end\n---@param value number\nlocal function one(value) end\none((require('count')))\none(require('count'))\n");
    assert!(report.has_errors());
    assert!(
        report
            .files
            .iter()
            .flat_map(|file| &file.report.diagnostics)
            .any(|error| error.code == "TS2554"),
        "{:#?}",
        report.files
    );
}

#[test]
fn actual_cli_deep_import_chain_refuses_with_guard_diagnostics_without_stack_abort() {
    let fixture = Fixture::new();
    for index in 0..512 {
        fixture.file(
            &format!("chain{index}.lua"),
            &format!("return require('chain{}')", index + 1),
        );
    }
    fixture.file("chain512.lua", "return 42");
    fixture.file("entry.lua", "local value=require('chain0')");
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .current_dir(&fixture.0)
        .args(["check", "entry.lua"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "process should refuse, not abort: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(
        errors.contains("LUA2036") && errors.contains("depth or stack limit"),
        "{errors}"
    );
    assert!(!errors.contains("stack overflow"), "{errors}");
}

#[test]
fn unsupported_dynamic_module_resolution_still_checks_original_argument_expressions() {
    let fixture = Fixture::new();
    let source = "---@param value number\n---@return string\nlocal function name(value) return 'module' end\nlocal result=require(name('wrong'))";
    let report = fixture.check(source);
    let main = &report.file(report.project.entries[0]).unwrap().report;
    assert!(main.diagnostics.iter().any(|error| error.code == "LUA2036"));
    assert!(
        main.diagnostics.iter().any(|error| error.code == "TS2345"
            && &source[error.span.start..error.span.end] == "'wrong'"),
        "{:#?}",
        main.diagnostics
    );
}

#[test]
fn missing_static_import_reports_its_host_error_even_in_an_unreachable_branch() {
    let fixture = Fixture::new();
    fixture.file("entry.lua", "if false then require('absent') end");
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .current_dir(&fixture.0)
        .args(["check", "entry.lua"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("entry.lua:1:23: error[TS2307]"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
