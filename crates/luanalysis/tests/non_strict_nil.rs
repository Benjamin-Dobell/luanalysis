use luanalysis::incremental::ProjectWorkspace;
use luanalysis::project_check::{ProjectCheckOptions, ProjectCheckReport, static_require_requests};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis::{CheckOptions, CheckReport, check_with_options};
use luanalysis_engine::{Engine, FunctionType, Property, Type, ValuePack};
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn check(source: &str, strict_nil: bool) -> CheckReport {
    check_with_options(
        source,
        CheckOptions {
            strict_nil,
            ..Default::default()
        },
    )
}

fn clean(source: &str, strict_nil: bool) -> CheckReport {
    let report = check(source, strict_nil);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

#[test]
fn strict_nil_stays_the_default_and_permissive_assignments_still_check_other_values() {
    assert!(CheckOptions::default().strict_nil);
    assert!(ProjectCheckOptions::default().strict_nil);
    let source = "---@type number\nlocal value = nil\n---@param maybe number|nil\nlocal function store(maybe)\n value = maybe\nend\nstore(nil)\nreturn value\n";
    assert_eq!(check(source, true).diagnostics.len(), 2);
    clean(source, false);
    let wrong = source.replace("local value = nil", "local value = 'wrong'");
    for strict_nil in [false, true] {
        let report = check(&wrong, strict_nil);
        assert!(
            report.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "TS2322"
                    && &wrong[diagnostic.span.start..diagnostic.span.end] == "'wrong'"
            }),
            "{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn permissive_nil_applies_to_call_arguments_and_written_result_packs() {
    let source = "---@param value number\nlocal function take(value) return value end\n---@return number, string\nlocal function give() return nil, 'tail' end\ntake(nil)\n---@type number, string\nlocal first, second = give()\nreturn first, second\n";
    let strict = check(source, true);
    assert!(
        strict
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "TS2345"),
        "{:?}",
        strict.diagnostics
    );
    assert!(
        strict
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "TS2322"),
        "{:?}",
        strict.diagnostics
    );
    clean(source, false);
    let wrong = source.replace("return nil, 'tail'", "return nil, 42");
    assert!(
        check(&wrong, false)
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "TS2322")
    );
}

#[test]
fn missing_initializers_preserve_written_nonnullable_contracts_through_calls() {
    let source = "---@shape Entry\n---@field name string\n\n---@type Entry\nlocal instance\n---@type fun(value:number):Entry\nlocal create\n---@type string\nlocal direct=instance.name\nlocal made=create(1)\n---@type string\nlocal produced=made.name\nreturn direct,produced\n";
    for strict_nil in [false, true] {
        clean(source, strict_nil);
        for (wrong, expected) in [
            (source.replace("create(1)", "create('wrong')"), "TS2345"),
            (source.replace("---@type string\nlocal produced", "---@type number\nlocal produced"), "TS2322"),
        ] {
            let report = check(&wrong, strict_nil);
            assert_eq!(report.diagnostics.len(), 1, "strict_nil={strict_nil}: {:#?}", report.diagnostics);
            assert_eq!(report.diagnostics[0].code, expected);
        }
    }
}

#[test]
fn declaration_contracts_do_not_erase_automatic_nullable_or_explicit_nil_values() {
    let source = "local automatic\n---@type number|nil\nlocal nullable\n---@type number\nlocal declared\nlocal observed_automatic=automatic\nlocal observed_nullable=nullable\nlocal observed_declared=declared\nreturn observed_automatic,observed_nullable,observed_declared\n";
    for strict_nil in [false, true] {
        let report = clean(source, strict_nil);
        for (name, expected) in [("observed_automatic", "nil"), ("observed_nullable", "nil"), ("observed_declared", "number")] {
            let binding = report.bindings.iter().find(|binding| binding.name == name).unwrap();
            assert_eq!(binding.ty, expected, "strict_nil={strict_nil}: {binding:?}");
        }
    }
    let source = "---@type number\nlocal initialized=nil\n---@type number\nlocal written\nwritten=nil\nlocal observed_initial=initialized\nlocal observed_write=written\nreturn observed_initial,observed_write\n";
    let report = clean(source, false);
    for name in ["observed_initial", "observed_write"] {
        let binding = report.bindings.iter().find(|binding| binding.name == name).unwrap();
        assert_eq!(binding.ty, "nil", "{binding:?}");
    }
    assert_eq!(check(source, true).diagnostics.len(), 2);
}

#[test]
fn uninitialized_factory_contracts_reach_generic_iterator_body_types_in_both_policies() {
    let source = "---@shape Entry\n---@field name string\n\n---@type fun():table<string,Entry>\nlocal create\nfor key,entry in pairs(create()) do\n---@type string\nlocal name=entry.name\nend\n";
    let fixture = Fixture::new(source);
    for strict_nil in [false, true] {
        let report = fixture.check(&mut ProjectWorkspace::new(), strict_nil, 1);
        assert!(!report.has_errors(), "strict_nil={strict_nil}: {:#?}", report.files);
        let wrong = source.replace("---@type string\nlocal name", "---@type number\nlocal name");
        std::fs::write(fixture.0.join("main.lua"), &wrong).unwrap();
        let report = fixture.check(&mut ProjectWorkspace::new(), strict_nil, 1);
        let errors = &report.file(report.project.entries[0]).unwrap().report.diagnostics;
        assert_eq!(errors.len(), 1, "strict_nil={strict_nil}: {errors:#?}");
        assert_eq!(errors[0].code, "TS2322");
        assert_eq!(&wrong[errors[0].span.start..errors[0].span.end], "entry.name");
        std::fs::write(fixture.0.join("main.lua"), source).unwrap();
    }
}

#[test]
fn nil_compatibility_preserves_runtime_tops_never_and_physical_union_membership() {
    for strict_nil in [true, false] {
        Engine::new()
            .with_strict_nil(strict_nil)
            .with_session(|queries| {
                for target in [
                    Type::Number,
                    Type::RuntimeTable,
                    Type::RuntimeFunction,
                    Type::Shape(vec![Property::new("value", Type::Number)]),
                    Type::Function(FunctionType {
                        params: Vec::new(),
                        returns: Box::new(Type::Number),
                    }),
                ] {
                    assert_eq!(
                        queries.is_assignable(&Type::Nil, &target),
                        !strict_nil,
                        "{target:?}"
                    );
                    assert!(!queries.contains_nil(&target), "{target:?}");
                }
                assert!(!queries.is_assignable(&Type::Nil, &Type::Never));
                assert!(!queries.types_comparable(&Type::Nil, &Type::Number));
                let maybe = queries.union_reduced(&[Type::Nil, Type::Number]);
                assert!(queries.contains_nil(&maybe));
                let excluded = queries.exclude_type(&maybe, &Type::Number);
                assert!(queries.contains_nil(&excluded));
                assert!(queries.types_comparable(&excluded, &Type::Nil));
                assert!(!queries.types_comparable(&excluded, &Type::Number));
            });
    }
}

#[test]
fn permissive_pack_relations_do_not_erase_nil_slots_or_change_exclusion() {
    for strict_nil in [true, false] {
        Engine::new()
            .with_strict_nil(strict_nil)
            .with_session(|queries| {
                let nil = ValuePack::single(Type::Nil);
                let numeric = ValuePack::single(Type::Number);
                assert_eq!(queries.pack_assignable(&nil, &numeric), !strict_nil);
                let slot = queries.pack_slot(&nil, 0);
                assert!(queries.contains_nil(&slot));
                let remaining = queries.exclude_pack(&nil, &[Type::Number], None);
                let slot = queries.pack_slot(&remaining, 0);
                assert!(queries.contains_nil(&slot));
                assert!(!queries.types_comparable(&slot, &Type::Number));
                // One physical nil value remains distinct from zero results.
                assert!(!queries.pack_assignable(&ValuePack::empty(), &numeric));
            });
    }
}

#[test]
fn explicit_nil_stores_can_delete_required_fields_without_loosening_structural_presence() {
    let source = "---@param record {value:number}\nlocal function clear(record)\n record.value = nil\nend\nreturn clear\n";
    assert!(!check(source, true).diagnostics.is_empty());
    clean(source, false);
    for strict_nil in [true, false] {
        let absent = "---@type {value:number}\nlocal record = {}\nreturn record\n";
        assert!(!check(absent, strict_nil).diagnostics.is_empty());
        let wrong = source.replace("record.value = nil", "record.value = 'wrong'");
        assert!(!check(&wrong, strict_nil).diagnostics.is_empty());
        let wrong_keys = "---@type table<number,number>\nlocal values = {word=1}\nreturn values\n";
        assert!(!check(wrong_keys, strict_nil).diagnostics.is_empty());
    }
}

#[test]
fn permissive_assignments_keep_actual_nil_for_guards_and_type_exclusion() {
    let source = "---@type number|string\nlocal value = 1\nvalue = nil\nlocal observed = value\n---@param maybe number|nil\nlocal function inspect(maybe)\n local excluded = --[[---@not number]] maybe\n if maybe == nil then\n  local absent = maybe\n  return absent, excluded\n end\n return maybe, excluded\nend\nreturn observed, inspect\n";
    let report = clean(source, false);
    for name in ["observed", "excluded", "absent"] {
        let binding = report
            .bindings
            .iter()
            .find(|binding| binding.name == name)
            .unwrap();
        assert_eq!(binding.ty, "nil", "{name}: {binding:?}");
    }
}

#[test]
fn physical_exclusion_stays_strict_through_nested_structural_members() {
    let source = "---@param value {tag:nil}|{tag:number}\nlocal function inspect(value)\n local remaining = --[[---@not {tag:number}]] value\n local tag = remaining.tag\n return tag\nend\nreturn inspect\n";
    for strict_nil in [false, true] {
        let report = clean(source, strict_nil);
        let binding = report
            .bindings
            .iter()
            .find(|binding| binding.name == "tag")
            .unwrap();
        assert_eq!(binding.ty, "nil", "strict_nil={strict_nil}: {binding:?}");
    }
}

#[test]
fn permissive_mode_still_rejects_deletion_of_a_required_never_member() {
    let source = "---@param record {value:never}\nlocal function clear(record)\n record.value = nil\nend\nreturn clear\n";
    for strict_nil in [false, true] {
        assert!(!check(source, strict_nil).diagnostics.is_empty());
    }
}

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis nil compatibility {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("main.lua"), source).unwrap();
        Self(path)
    }

    fn project(&self) -> Project {
        let mut entries = vec![self.0.join("main.lua")];
        if self.0.join("other.lua").is_file() {
            entries.push(self.0.join("other.lua"));
        }
        Project::load_plan(
            &Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap(),
            ProjectLoadPlan::new(entries),
            static_require_requests,
        )
    }

    fn check(
        &self,
        workspace: &mut ProjectWorkspace,
        strict_nil: bool,
        jobs: usize,
    ) -> ProjectCheckReport {
        workspace.check_project(
            self.project(),
            ProjectCheckOptions {
                environment: StandardEnvironmentProfile::Lua53,
                strict_nil,
                jobs,
                ..Default::default()
            },
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn projects_recheck_original_sparse_unpack_and_policy_changes_with_reused_syntax() {
    let source = "---@type nil|number\nlocal maybe\n---@type number\nlocal numeric\nmaybe = nil\nnumeric = nil\nnumeric = maybe\nlocal buffer = {1,nil,3}\nlocal a,b,c = table.unpack(buffer)\nnumeric = a\nnumeric = b\nnumeric = c\nreturn numeric\n";
    let fixture = Fixture::new(source);
    let mut workspace = ProjectWorkspace::new();
    for strict_nil in [true, false, true] {
        let report = fixture.check(&mut workspace, strict_nil, 1);
        let diagnostics = &report
            .file(report.project.entries[0])
            .unwrap()
            .report
            .diagnostics;
        assert_eq!(
            diagnostics.is_empty(),
            !strict_nil,
            "strict_nil={strict_nil}: {diagnostics:?}"
        );
    }
    assert_eq!(workspace.last_preparation_stats().parsed, 0);
}

#[test]
fn parallel_tasks_use_the_same_nil_policy_as_serial_checks() {
    let source = "---@type number\nlocal value = nil\nreturn value\n";
    let fixture = Fixture::new(source);
    std::fs::write(fixture.0.join("other.lua"), source).unwrap();
    for strict_nil in [false, true] {
        for jobs in [1, 2] {
            let report = fixture.check(&mut ProjectWorkspace::new(), strict_nil, jobs);
            let diagnostics: Vec<_> = report
                .files
                .iter()
                .filter_map(|file| {
                    (!file.report.diagnostics.is_empty()).then_some((
                        report.source(file.file).unwrap().path.clone(),
                        &file.report.diagnostics,
                    ))
                })
                .collect();
            assert_eq!(
                report.has_errors(),
                strict_nil,
                "jobs={jobs}, strict_nil={strict_nil}: {diagnostics:?}"
            );
            if jobs == 2 {
                assert!(report.execution.is_some());
            }
        }
    }
}

#[test]
fn cli_explicitly_selects_permissive_nil_and_preserves_default_errors() {
    let fixture = Fixture::new("---@type number\nlocal value = nil\nreturn value\n");
    let run = |permissive: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"));
        command.arg("check");
        if permissive {
            command.arg("--no-strict-nil");
        }
        command.arg(fixture.0.join("main.lua")).output().unwrap()
    };
    let strict = run(false);
    assert_eq!(strict.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&strict.stderr).contains("TS2322"));
    let permissive = run(true);
    assert!(
        permissive.status.success(),
        "{}",
        String::from_utf8_lossy(&permissive.stderr)
    );
}
