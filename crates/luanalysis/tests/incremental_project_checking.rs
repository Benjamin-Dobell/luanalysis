//! Repeated project checks reuse immutable source preparation, never an old
//! semantic result. Every warm result is compared with a separately loaded,
//! cold project, including source diagnostics, rendered types and binding maps.
use luanalysis::incremental::{PreparationStats, ProjectWorkspace};
use luanalysis::project_check::{
    ProjectCheckOptions, ProjectCheckReport, check_project_with_options, static_require_requests,
};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis::{BindingInfo, Diagnostic, OverloadMode, TypeInfo, bind::BoundFile};
use luanalysis_engine::NativeLoaderPolicy;
use luanalysis_project::{
    DeclarationSource, DependencyOutcome, FileId, Project, ProjectConfig, ProjectInput,
    ProjectLoadPlan, Resolver, SourceRole,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis incremental project {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
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

    fn plan(&self, entries: &[&str], libraries: &[(&str, SourceRole)]) -> ProjectLoadPlan {
        let mut plan = ProjectLoadPlan::new(entries.iter().map(|name| self.0.join(name)));
        plan.libraries = libraries
            .iter()
            .map(|(name, role)| ProjectInput::new(self.0.join(name), *role))
            .collect();
        plan
    }

    fn resolver(&self) -> Resolver {
        Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap()
    }

    fn compare(
        &self,
        workspace: &mut ProjectWorkspace,
        plan: ProjectLoadPlan,
        options: ProjectCheckOptions,
    ) -> ProjectCheckReport {
        self.compare_with_resolver(workspace, &self.resolver(), plan, options)
    }

    fn compare_with_resolver(
        &self,
        workspace: &mut ProjectWorkspace,
        resolver: &Resolver,
        plan: ProjectLoadPlan,
        options: ProjectCheckOptions,
    ) -> ProjectCheckReport {
        let warm = Project::load_plan(resolver, plan.clone(), |source| {
            workspace.static_require_requests(source)
        });
        let cold = Project::load_plan(resolver, plan, static_require_requests);
        assert_ne!(warm.id, cold.id);
        let warm = workspace.check_project(warm, options);
        let cold = check_project_with_options(cold, options);
        assert_eq!(normalized(&warm), normalized(&cold));
        assert_eq!(workspace.last_preparation_stats().semantic_reused, 0);
        warm
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[derive(Debug, PartialEq, Eq)]
struct NormalizedFileReport {
    path: PathBuf,
    diagnostics: Vec<Diagnostic>,
    bindings: Vec<BindingInfo>,
    types: Vec<TypeInfo>,
    bound: Option<BoundFile>,
}

#[derive(Debug, PartialEq, Eq)]
struct NormalizedReport {
    sources: Vec<(PathBuf, String, SourceRole, String)>,
    files: Vec<NormalizedFileReport>,
    entries: Vec<PathBuf>,
    libraries: Vec<PathBuf>,
    standard_libraries: Vec<PathBuf>,
    dependencies: Vec<(PathBuf, String, usize, usize, String)>,
    cycles: Vec<Vec<(PathBuf, PathBuf)>>,
    entry_errors: Vec<String>,
    library_errors: Vec<String>,
    role_conflicts: Vec<String>,
    environment_errors: Vec<String>,
    checked_file_count: usize,
    has_errors: bool,
}

fn paths(report: &ProjectCheckReport, ids: &[FileId]) -> Vec<PathBuf> {
    let mut paths: Vec<_> = ids
        .iter()
        .map(|&id| report.source(id).unwrap().path.clone())
        .collect();
    paths.sort();
    paths
}

fn normalized(report: &ProjectCheckReport) -> NormalizedReport {
    let path = |id| report.source(id).unwrap().path.clone();
    let mut sources: Vec<_> = report
        .project
        .files
        .iter()
        .map(|source| {
            (
                source.path.clone(),
                source.source.clone(),
                source.role,
                format!("{:?}", source.origin),
            )
        })
        .collect();
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    let mut files: Vec<_> = report
        .files
        .iter()
        .map(|file| {
            let mut diagnostics = file.report.diagnostics.clone();
            diagnostics.sort_by(|a, b| {
                (a.span.start, a.span.end, a.code, &a.message).cmp(&(
                    b.span.start,
                    b.span.end,
                    b.code,
                    &b.message,
                ))
            });
            let mut bindings = file.report.bindings.clone();
            bindings.sort_by(|a, b| {
                (a.span.start, a.span.end, &a.name, &a.ty).cmp(&(
                    b.span.start,
                    b.span.end,
                    &b.name,
                    &b.ty,
                ))
            });
            let mut types = file.report.types.clone();
            types.sort_by(|a, b| {
                (a.span.start, a.span.end, &a.ty).cmp(&(b.span.start, b.span.end, &b.ty))
            });
            NormalizedFileReport {
                path: path(file.file),
                diagnostics,
                bindings,
                types,
                bound: file.report.bound.clone(),
            }
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let mut dependencies: Vec<_> = report
        .project
        .dependencies
        .iter()
        .map(|dependency| {
            let outcome = match &dependency.outcome {
                DependencyOutcome::Resolved(id) => format!("resolved:{}", path(*id).display()),
                other => format!("{other:?}"),
            };
            (
                path(dependency.from),
                dependency.request.module.clone(),
                dependency.request.span.start,
                dependency.request.span.end,
                outcome,
            )
        })
        .collect();
    dependencies.sort();
    let mut cycles: Vec<_> = report
        .project
        .cycles
        .iter()
        .map(|cycle| {
            let mut edges: Vec<_> = cycle
                .files
                .windows(2)
                .map(|edge| (path(edge[0]), path(edge[1])))
                .collect();
            edges.sort();
            edges
        })
        .collect();
    cycles.sort();
    let mut role_conflicts: Vec<_> = report
        .project
        .role_conflicts
        .iter()
        .map(|conflict| conflict.to_string())
        .collect();
    role_conflicts.sort();
    let mut environment_errors: Vec<_> = report
        .environment_errors
        .iter()
        .map(ToString::to_string)
        .collect();
    environment_errors.sort();
    NormalizedReport {
        sources,
        files,
        entries: paths(report, &report.project.entries),
        libraries: paths(report, &report.project.libraries),
        standard_libraries: paths(report, &report.standard_library_files),
        dependencies,
        cycles,
        entry_errors: report
            .project
            .entry_errors
            .iter()
            .map(ToString::to_string)
            .collect(),
        library_errors: report
            .project
            .library_errors
            .iter()
            .map(ToString::to_string)
            .collect(),
        role_conflicts,
        environment_errors,
        checked_file_count: report.checked_file_count(),
        has_errors: report.has_errors(),
    }
}

fn options() -> ProjectCheckOptions {
    ProjectCheckOptions {
        jobs: 1,
        environment: StandardEnvironmentProfile::None,
        ..Default::default()
    }
}

fn modules() -> ProjectCheckOptions {
    ProjectCheckOptions {
        environment: StandardEnvironmentProfile::LegacyMinimal,
        ..options()
    }
}

fn clean(report: &ProjectCheckReport) {
    assert!(!report.has_errors(), "{:#?}", normalized(report));
}

fn diagnostic_at(report: &ProjectCheckReport, path: &Path, code: &str, token: &str) {
    let source = report.project.file_by_path(path).unwrap();
    let file = report.file(source.id).unwrap();
    assert!(
        file.report.diagnostics.iter().any(|error| {
            error.code == code && &source.source[error.span.start..error.span.end] == token
        }),
        "expected {code} at {token:?} in {}: {:#?}",
        path.display(),
        file.report.diagnostics,
    );
}

fn preparation(workspace: &ProjectWorkspace, parsed: usize, reused: usize, bound: usize) {
    let stats: PreparationStats = workspace.last_preparation_stats();
    assert_eq!(stats.parsed, parsed, "{stats:?}");
    assert_eq!(stats.parse_reused, reused, "{stats:?}");
    assert_eq!(stats.files_seen, parsed + reused, "{stats:?}");
    assert_eq!(stats.bound, bound, "{stats:?}");
    assert_eq!(stats.semantic_reused, 0, "{stats:?}");
}

#[test]
fn same_length_source_edit_rechecks_semantics_and_preserves_previous_report() {
    let fixture = Fixture::new();
    let wrong = "-- café\r\n---@type number\r\nlocal value='bad'\r\n";
    let corrected = wrong.replace("'bad'", "42   ");
    assert_eq!(wrong.len(), corrected.len());
    let main = fixture.file("main.lua", wrong);
    let mut workspace = ProjectWorkspace::new();
    let first = fixture.compare(&mut workspace, fixture.plan(&["main.lua"], &[]), options());
    diagnostic_at(&first, &main, "TS2322", "'bad'");
    preparation(&workspace, 1, 0, 1);
    let old_id = first.project.entries[0];
    fs::write(&main, &corrected).unwrap();
    let second = fixture.compare(&mut workspace, fixture.plan(&["main.lua"], &[]), options());
    clean(&second);
    preparation(&workspace, 1, 0, 1);
    assert!(second.source(old_id).is_none());
    assert!(second.file(old_id).is_none());
    assert_eq!(first.source(old_id).unwrap().source, wrong);
    diagnostic_at(&first, &main, "TS2322", "'bad'");
    let unchanged = fixture.compare(&mut workspace, fixture.plan(&["main.lua"], &[]), options());
    clean(&unchanged);
    preparation(&workspace, 0, 1, 0);
    assert_eq!(workspace.last_preparation_stats().bind_reused, 1);
}

#[test]
fn unchanged_importer_observes_an_edited_module_return_type() {
    let fixture = Fixture::new();
    let main = fixture.file(
        "main.lua",
        "local value=require('value')\n---@type number\nlocal checked=value\n",
    );
    let value = fixture.file("value.lua", "return 42\n");
    let mut workspace = ProjectWorkspace::default();
    clean(&fixture.compare(&mut workspace, fixture.plan(&["main.lua"], &[]), modules()));
    preparation(&workspace, 3, 0, 3);
    fs::write(&value, "return 'wrong'\n").unwrap();
    let changed = fixture.compare(&mut workspace, fixture.plan(&["main.lua"], &[]), modules());
    diagnostic_at(&changed, &main, "TS2322", "value");
    preparation(&workspace, 1, 2, 1);
    assert_eq!(workspace.last_preparation_stats().bind_reused, 2);
    fs::write(value, "return 42\n").unwrap();
    clean(&fixture.compare(&mut workspace, fixture.plan(&["main.lua"], &[]), modules()));
    preparation(&workspace, 1, 2, 1);
}

#[test]
fn unchanged_legacy_global_consumers_observe_alias_addition_change_and_conflict() {
    let fixture = Fixture::new();
    let main = fixture.file("main.lua", "---@type Future\nlocal checked=42\n");
    let first = fixture.file("first.def.lua", "-- no declaration yet\n");
    let second = fixture.file("second.def.lua", "-- no duplicate yet\n");
    let plan = || {
        fixture.plan(
            &["main.lua"],
            &[
                ("first.def.lua", SourceRole::Declaration),
                ("second.def.lua", SourceRole::Declaration),
            ],
        )
    };
    let mut workspace = ProjectWorkspace::new();
    let missing = fixture.compare(&mut workspace, plan(), options());
    diagnostic_at(&missing, &main, "TS2304", "Future");
    fs::write(&first, "---@alias Future number\n").unwrap();
    clean(&fixture.compare(&mut workspace, plan(), options()));
    preparation(&workspace, 1, 2, 1);
    fs::write(&first, "---@alias Future string\n").unwrap();
    let changed = fixture.compare(&mut workspace, plan(), options());
    diagnostic_at(&changed, &main, "TS2322", "42");
    fs::write(&first, "---@alias Future number\n").unwrap();
    fs::write(&second, "---@alias Future string\n").unwrap();
    let duplicate = fixture.compare(&mut workspace, plan(), options());
    for path in [&main, &first, &second] {
        diagnostic_at(&duplicate, path, "TS2300", "Future");
    }
    preparation(&workspace, 2, 1, 2);
    fs::write(&second, "-- no duplicate yet\n").unwrap();
    clean(&fixture.compare(&mut workspace, plan(), options()));
    fs::write(&first, "---@alias Future Missing\n").unwrap();
    let unresolved = fixture.compare(&mut workspace, plan(), options());
    diagnostic_at(&unresolved, &first, "TS2304", "Missing");
    fs::write(first, "---@alias Future number\n").unwrap();
    clean(&fixture.compare(&mut workspace, plan(), options()));
}

#[test]
fn identical_bytes_with_a_different_source_role_do_not_reuse_the_previous_role() {
    let fixture = Fixture::new();
    let stub = fixture.file(
        "api.lua",
        "---@return number\nfunction external() return 'wrong' end\n",
    );
    fixture.file("main.lua", "local checked=42\n");
    let mut workspace = ProjectWorkspace::new();
    let plan = |role| fixture.plan(&["main.lua"], &[("api.lua", role)]);
    clean(&fixture.compare(&mut workspace, plan(SourceRole::Declaration), options()));
    let implementation =
        fixture.compare(&mut workspace, plan(SourceRole::Implementation), options());
    diagnostic_at(&implementation, &stub, "TS2322", "'wrong'");
    preparation(&workspace, 1, 1, 1);
    assert_eq!(workspace.last_preparation_stats().evicted, 1);
    clean(&fixture.compare(&mut workspace, plan(SourceRole::Declaration), options()));
    preparation(&workspace, 1, 1, 1);
}

#[test]
fn missing_removed_and_recreated_require_targets_are_resolved_on_every_check() {
    let fixture = Fixture::new();
    fixture.file(
        "main.lua",
        "local value=require('value')\n---@type number\nlocal checked=value\n",
    );
    let mut workspace = ProjectWorkspace::new();
    let plan = || fixture.plan(&["main.lua"], &[]);
    let missing = fixture.compare(&mut workspace, plan(), modules());
    assert!(missing.has_errors());
    assert!(matches!(
        missing.project.dependencies[0].outcome,
        DependencyOutcome::Unresolved(_)
    ));
    let value = fixture.file("value.lua", "return 42\n");
    clean(&fixture.compare(&mut workspace, plan(), modules()));
    preparation(&workspace, 1, 2, 1);
    fs::remove_file(&value).unwrap();
    let removed = fixture.compare(&mut workspace, plan(), modules());
    assert!(removed.has_errors());
    assert!(matches!(
        removed.project.dependencies[0].outcome,
        DependencyOutcome::Unresolved(_)
    ));
    preparation(&workspace, 0, 2, 0);
    assert_eq!(workspace.last_preparation_stats().evicted, 1);
    fixture.file("value.lua", "return 42\n");
    clean(&fixture.compare(&mut workspace, plan(), modules()));
    preparation(&workspace, 1, 2, 1);
}

#[test]
fn a_new_higher_priority_module_candidate_changes_an_unchanged_require() {
    let fixture = Fixture::new();
    let main = fixture.file(
        "main.lua",
        "local value=require('value')\n---@type number\nlocal checked=value\n",
    );
    fixture.file("value/init.lua", "return 42\n");
    let mut workspace = ProjectWorkspace::new();
    let plan = || fixture.plan(&["main.lua"], &[]);
    clean(&fixture.compare(&mut workspace, plan(), modules()));
    let high_priority = fixture.file("value.lua", "return 'wrong'\n");
    let shadowed = fixture.compare(&mut workspace, plan(), modules());
    diagnostic_at(&shadowed, &main, "TS2322", "value");
    assert!(shadowed.project.file_by_path(&high_priority).is_some());
    assert!(
        shadowed
            .project
            .file_by_path(&fixture.0.join("value/init.lua"))
            .is_none()
    );
    preparation(&workspace, 1, 2, 1);
    assert_eq!(workspace.last_preparation_stats().evicted, 1);
    fs::remove_file(high_priority).unwrap();
    clean(&fixture.compare(&mut workspace, plan(), modules()));
    preparation(&workspace, 1, 2, 1);
}

#[test]
fn overload_option_changes_reuse_source_inputs_but_recompute_the_public_call_contract() {
    let fixture = Fixture::new();
    let main = fixture.file("main.lua", "---@overload fun(value:number):number\n---@generic T\n---@param value T\n---@return T\nlocal function identity(value) return value end\n---@type string\nlocal result=identity('word')\n");
    let mut workspace = ProjectWorkspace::new();
    let plan = || fixture.plan(&["main.lua"], &[]);
    diagnostic_at(
        &fixture.compare(&mut workspace, plan(), options()),
        &main,
        "TS2345",
        "'word'",
    );
    let legacy = ProjectCheckOptions {
        overload_mode: OverloadMode::LegacyExposeImplementation,
        ..options()
    };
    clean(&fixture.compare(&mut workspace, plan(), legacy));
    preparation(&workspace, 0, 1, 0);
    assert_eq!(workspace.last_preparation_stats().bind_reused, 1);
    diagnostic_at(
        &fixture.compare(&mut workspace, plan(), options()),
        &main,
        "TS2345",
        "'word'",
    );
    preparation(&workspace, 0, 1, 0);
}

#[test]
fn standard_profile_changes_refresh_membership_and_semantics_with_unchanged_user_source() {
    let fixture = Fixture::new();
    fixture.file("main.lua", "---@type number\nlocal checked=math.abs(-2)\n");
    let mut workspace = ProjectWorkspace::new();
    let plan = || fixture.plan(&["main.lua"], &[]);
    let absent = fixture.compare(&mut workspace, plan(), options());
    assert!(absent.has_errors());
    for environment in [
        StandardEnvironmentProfile::Lua52,
        StandardEnvironmentProfile::Lua54,
    ] {
        let report = fixture.compare(
            &mut workspace,
            plan(),
            ProjectCheckOptions {
                environment,
                ..options()
            },
        );
        clean(&report);
        let stats = workspace.last_preparation_stats();
        assert_eq!(stats.files_seen, report.project.files.len());
        assert_eq!(
            stats.parse_reused, 1,
            "only the unchanged user file belongs to both profiles: {stats:?}"
        );
        assert_eq!(stats.bind_reused, 1, "{stats:?}");
        assert_eq!(
            stats.parsed,
            report.standard_library_files.len(),
            "{stats:?}"
        );
        assert_eq!(stats.bound, stats.parsed, "{stats:?}");
    }
    let absent_again = fixture.compare(&mut workspace, plan(), options());
    assert!(absent_again.has_errors());
    assert!(absent_again.standard_library_files.is_empty());
    preparation(&workspace, 0, 1, 0);
    assert_eq!(workspace.retained_source_count(), 1);
}

#[test]
fn loader_policy_changes_recompute_result_packs_from_reused_module_sources() {
    let fixture = Fixture::new();
    let main = fixture.file(
        "main.lua",
        "local value,loader=require('value')\n---@type nil\nlocal checked=loader\n",
    );
    fixture.file("value.lua", "return 42\n");
    let mut workspace = ProjectWorkspace::new();
    let plan = || fixture.plan(&["main.lua"], &[]);
    let old_loader = ProjectCheckOptions {
        loader_policy: NativeLoaderPolicy::Lua51StandardFile,
        ..modules()
    };
    clean(&fixture.compare(&mut workspace, plan(), old_loader));
    let current = fixture.compare(&mut workspace, plan(), modules());
    diagnostic_at(&current, &main, "TS2322", "loader");
    preparation(&workspace, 0, 3, 0);
    clean(&fixture.compare(&mut workspace, plan(), old_loader));
    preparation(&workspace, 0, 3, 0);
}

#[test]
fn syntax_error_correction_replaces_the_failed_parse_and_only_then_binds_it() {
    let fixture = Fixture::new();
    fixture.file("main.lua", "local = ?\n");
    let mut workspace = ProjectWorkspace::new();
    let plan = || fixture.plan(&["main.lua"], &[]);
    let invalid = fixture.compare(&mut workspace, plan(), options());
    assert!(invalid.has_errors());
    assert!(invalid.files[0].report.bound.is_none());
    assert!(
        invalid.files[0]
            .report
            .diagnostics
            .iter()
            .all(|error| error.code == "LUA1001")
    );
    preparation(&workspace, 1, 0, 0);
    let repeated = fixture.compare(&mut workspace, plan(), options());
    assert_eq!(normalized(&invalid), normalized(&repeated));
    preparation(&workspace, 0, 1, 0);
    assert_eq!(workspace.last_preparation_stats().bind_reused, 0);
    fixture.file("main.lua", "local checked=42\n");
    clean(&fixture.compare(&mut workspace, plan(), options()));
    preparation(&workspace, 1, 0, 1);
}

#[test]
fn project_identity_entry_order_and_worker_count_do_not_change_rendered_results_or_binding_maps() {
    let fixture = Fixture::new();
    fixture.file("factory.lua", "---@generic T\n---@param captured T\nlocal function factory(captured) return function() return captured,captured end end\nreturn factory\n");
    fixture.file("numeric.lua", "local factory=require('factory')\nlocal closure=factory(42)\nlocal first,second=closure()\n---@type number\nlocal checked=second\n");
    let textual = fixture.file("textual.lua", "local factory=require('factory')\nlocal closure=factory('word')\nlocal first,second=closure()\n---@type string\nlocal checked=second\n");
    let mut workspace = ProjectWorkspace::new();
    let first = fixture.compare(
        &mut workspace,
        fixture.plan(&["numeric.lua", "textual.lua"], &[]),
        modules(),
    );
    clean(&first);
    let baseline = normalized(&first);
    for jobs in [2, 4, 1, 4] {
        let report = fixture.compare(
            &mut workspace,
            fixture.plan(&["textual.lua", "numeric.lua"], &[]),
            ProjectCheckOptions { jobs, ..modules() },
        );
        clean(&report);
        assert_eq!(normalized(&report), baseline);
        assert_ne!(first.project.id, report.project.id);
        preparation(&workspace, 0, 4, 0);
        assert_eq!(workspace.last_preparation_stats().bind_reused, 4);
    }
    let wrong = fs::read_to_string(&textual)
        .unwrap()
        .replace("---@type string", "---@type number");
    fs::write(&textual, &wrong).unwrap();
    for jobs in [2, 4] {
        let report = fixture.compare(
            &mut workspace,
            fixture.plan(&["textual.lua", "numeric.lua"], &[]),
            ProjectCheckOptions { jobs, ..modules() },
        );
        diagnostic_at(&report, &textual, "TS2322", "second");
    }
    fs::write(
        &textual,
        wrong.replace("---@type number", "---@type string"),
    )
    .unwrap();
    clean(&fixture.compare(
        &mut workspace,
        fixture.plan(&["textual.lua", "numeric.lua"], &[]),
        ProjectCheckOptions {
            jobs: 4,
            ..modules()
        },
    ));
}

#[test]
fn declaration_resource_identity_is_part_of_the_preparation_key() {
    let fixture = Fixture::new();
    let main = fixture.file("main.lua", "---@type Count\nlocal checked=42\n");
    let mut workspace = ProjectWorkspace::new();
    let mut check = |identity: &str, source: &str| {
        let warm = Project::load(&fixture.resolver(), [main.clone()], |source| {
            workspace.static_require_requests(source)
        });
        let cold = Project::load(&fixture.resolver(), [main.clone()], static_require_requests);
        let add_resource = |mut project: Project| {
            project
                .add_declaration_source(DeclarationSource {
                    identity: identity.into(),
                    path: fixture.0.join("virtual-count.def.lua"),
                    source: source.into(),
                })
                .unwrap();
            project
        };
        let report = workspace.check_project(add_resource(warm), options());
        let cold = check_project_with_options(add_resource(cold), options());
        assert_eq!(normalized(&report), normalized(&cold));
        (report, workspace.last_preparation_stats())
    };
    clean(&check("resource-one", "---@alias Count number\n").0);
    let (same_bytes_new_origin, stats) = check("resource-two", "---@alias Count number\n");
    clean(&same_bytes_new_origin);
    assert_eq!(
        (stats.parsed, stats.parse_reused, stats.bound, stats.evicted),
        (1, 1, 1, 1),
        "{stats:?}"
    );
    let (wrong, stats) = check("resource-two", "---@alias Count string\n");
    diagnostic_at(&wrong, &main, "TS2322", "42");
    assert_eq!(
        (stats.parsed, stats.parse_reused, stats.semantic_reused),
        (1, 1, 0),
        "{stats:?}"
    );
    clean(&check("resource-two", "---@alias Count number\n").0);
}
