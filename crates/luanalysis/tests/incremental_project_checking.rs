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

    /// An incremental check of `plan` after the edits on disk matches a cold check; the number of
    /// files it checked.
    fn compare_incrementally(&self, workspace: &mut ProjectWorkspace, plan: ProjectLoadPlan, options: ProjectCheckOptions) -> usize {
        let resolver = self.resolver();
        let warm = Project::load_plan(&resolver, plan.clone(), |source| workspace.static_require_requests(source));
        let cold = Project::load_plan(&resolver, plan, static_require_requests);
        let warm = workspace.check_project_incrementally(warm, options.clone());
        let cold = check_project_with_options(cold, options);
        assert_eq!(normalized(&warm), normalized(&cold));
        workspace.last_incremental_checked()
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
        let warm = workspace.check_project(warm, options.clone());
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
    bound: Option<std::sync::Arc<BoundFile>>,
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
                source.source.to_string(),
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
    assert_eq!(&*first.source(old_id).unwrap().source, wrong);
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
    clean(&fixture.compare(&mut workspace, plan(), old_loader.clone()));
    let current = fixture.compare(&mut workspace, plan(), modules());
    diagnostic_at(&current, &main, "TS2322", "loader");
    preparation(&workspace, 0, 3, 0);
    clean(&fixture.compare(&mut workspace, plan(), old_loader));
    preparation(&workspace, 0, 3, 0);
}

#[test]
fn syntax_error_correction_replaces_the_failed_parse_and_its_recovered_binding() {
    let fixture = Fixture::new();
    fixture.file("main.lua", "local = ?\n");
    let mut workspace = ProjectWorkspace::new();
    let plan = || fixture.plan(&["main.lua"], &[]);
    let invalid = fixture.compare(&mut workspace, plan(), options());
    assert!(invalid.has_errors());
    assert!(invalid.files[0].report.bound.is_some());
    assert!(
        invalid.files[0]
            .report
            .diagnostics
            .iter()
            .all(|error| error.code == "LUA1001")
    );
    preparation(&workspace, 1, 0, 1);
    let repeated = fixture.compare(&mut workspace, plan(), options());
    assert_eq!(normalized(&invalid), normalized(&repeated));
    preparation(&workspace, 0, 1, 0);
    assert_eq!(workspace.last_preparation_stats().bind_reused, 1);
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

#[test]
fn incremental_checks_redo_only_what_an_edit_can_affect_and_match_cold_checks() {
    let fixture = Fixture::new();
    fixture.file("m.lua", "local M = {}\n---@return number\nfunction M.size() return 1 end\nreturn M\n");
    fixture.file("a.lua", "local m = require('m')\n---@type number\nlocal n = m.size()\nlocal function private() return n end\nreturn private\n");
    fixture.file("g.lua", "Shared = { count = 1 }\n");
    fixture.file("b.lua", "local c = Shared.count\n");
    fixture.file("c.lua", "---@class Point\n---@field x number\nlocal Point = {}\nreturn Point\n");
    fixture.file("d.lua", "---@type Point\nlocal p = { x = 1 }\nlocal y = p.y\n");
    let entries = ["a.lua", "b.lua", "c.lua", "d.lua", "g.lua", "m.lua"];
    let plan = || fixture.plan(&entries, &[]);
    let mut workspace = ProjectWorkspace::new();
    // The first check checks every file, the standard declarations' included.
    let all = fixture.compare_incrementally(&mut workspace, plan(), modules());
    assert!(all >= entries.len(), "{all}");
    // Nothing changed: nothing is checked again.
    assert_eq!(fixture.compare_incrementally(&mut workspace, plan(), modules()), 0);
    // An edit no other file can see checks only its file.
    fixture.file("a.lua", "local m = require('m')\n---@type number\nlocal n = m.size()\nlocal function private() return n + 1 end\nreturn private\n");
    assert_eq!(fixture.compare_incrementally(&mut workspace, plan(), modules()), 1);
    // A module's export changes for the file that requires it.
    fixture.file("m.lua", "local M = {}\n---@return string\nfunction M.size() return '1' end\nreturn M\n");
    assert_eq!(fixture.compare_incrementally(&mut workspace, plan(), modules()), 2);
    // A global's removal reaches the file that reads it, and its return.
    fixture.file("g.lua", "local unshared = 1\n");
    assert!(fixture.compare_incrementally(&mut workspace, plan(), modules()) >= 2);
    fixture.file("g.lua", "Shared = { count = 1 }\n");
    assert!(fixture.compare_incrementally(&mut workspace, plan(), modules()) >= 2);
    // A class's new field reaches the file that uses the class.
    fixture.file("c.lua", "---@class Point\n---@field x number\n---@field y number\nlocal Point = {}\nreturn Point\n");
    assert!(fixture.compare_incrementally(&mut workspace, plan(), modules()) >= 2);
    // A body edit in the class's file checks only that file.
    fixture.file("c.lua", "---@class Point\n---@field x number\n---@field y number\nlocal Point = {}\nlocal unused = 2\nreturn Point\n");
    assert_eq!(fixture.compare_incrementally(&mut workspace, plan(), modules()), 1);
    // A field made private reaches the file that reads it, where it is no longer accessible.
    fixture.file("c.lua", "---@class Point\n---@field x number\n---@field private y number\nlocal Point = {}\nlocal unused = 2\nreturn Point\n");
    assert!(fixture.compare_incrementally(&mut workspace, plan(), modules()) >= 2);
    // Other options check everything again.
    assert_eq!(fixture.compare_incrementally(&mut workspace, plan(), ProjectCheckOptions { no_unsafe_any: true, ..modules() }), all);
}

#[test]
fn an_edit_of_a_type_parameter_reaches_the_files_that_read_it() {
    let fixture = Fixture::new();
    fixture.file("a.lua", "---@class Box<T: string>\n---@field first T\n\nreturn {}\n");
    fixture.file("b.lua", "---@class Box<T: string>\n---@field second T\n\nreturn {}\n");
    fixture.file("single.lua", "---@class Single<T: string, U = number>\n---@field value T\n\nreturn {}\n");
    fixture.file("use.lua", "---@type Single<string>\nlocal single\nlocal value = single.value\n");
    let plan = || fixture.plan(&["a.lua", "b.lua", "single.lua", "use.lua"], &[]);
    let mut workspace = ProjectWorkspace::new();
    fixture.compare_incrementally(&mut workspace, plan(), modules());
    // A merged class's declarations are reported together when one writes another constraint...
    fixture.file("b.lua", "---@class Box<T: number>\n---@field second T\n\nreturn {}\n");
    assert!(fixture.compare_incrementally(&mut workspace, plan(), modules()) >= 2);
    // ...and no more when it writes the same one again.
    fixture.file("b.lua", "---@class Box<T: string>\n---@field second T\n\nreturn {}\n");
    assert!(fixture.compare_incrementally(&mut workspace, plan(), modules()) >= 2);
    // A constraint or a default that files read in a reference.
    fixture.file("single.lua", "---@class Single<T: number, U = number>\n---@field value T\n\nreturn {}\n");
    assert!(fixture.compare_incrementally(&mut workspace, plan(), modules()) >= 2);
    fixture.file("single.lua", "---@class Single<T: string, U = boolean>\n---@field value T\n\nreturn {}\n");
    assert!(fixture.compare_incrementally(&mut workspace, plan(), modules()) >= 2);
}

#[test]
fn an_edit_beside_a_global_that_reads_itself_checks_only_its_file() {
    let fixture = Fixture::new();
    let main = "LOGPATH = LOGPATH or 'log'\nMETAPATH = METAPATH or LOGPATH\nlocal function helper() end\nhelper()\n";
    fixture.file("main.lua", main);
    fixture.file("other.lua", "LOGPATH = LOGPATH or 'other'\nMETAPATH = METAPATH or LOGPATH\n");
    // Large enough that checking the edited file is not half of the program.
    fixture.file("user.lua", &"local path = LOGPATH\nlocal meta = METAPATH\n".repeat(40));
    let plan = fixture.plan(&["main.lua", "other.lua", "user.lua"], &[]);
    let mut workspace = ProjectWorkspace::new();
    fixture.compare_incrementally(&mut workspace, plan.clone(), options());
    fixture.file("main.lua", &main.replace("helper() end", "helper() local probe = 1 return probe end"));
    assert_eq!(fixture.compare_incrementally(&mut workspace, plan.clone(), options()), 1);
    fixture.file("main.lua", main);
    assert_eq!(fixture.compare_incrementally(&mut workspace, plan, options()), 1);
}

#[test]
fn a_function_capturing_a_module_and_an_automatic_local_reads_both() {
    let fixture = Fixture::new();
    fixture.file("m.lua", "return { value = 1 }\n");
    let main = fixture.file(
        "main.lua",
        "local m = require('m')\nlocal count\nlocal function read()\n  count = 1\n  return m.value\nend\n---@type string\nlocal wrong = read()\nreturn wrong, count\n",
    );
    let mut workspace = ProjectWorkspace::new();
    let report = fixture.compare(&mut workspace, fixture.plan(&["main.lua"], &[]), modules());
    diagnostic_at(&report, &main, "TS2322", "read()");
}

#[test]
fn one_global_declaration_of_another_type_conflicts_with_all_of_them() {
    let fixture = Fixture::new();
    let sources = [("a.lua", "---@type number\nSHARED = 1\n"), ("b.lua", "---@type number\nSHARED = 2\n"), ("c.lua", "---@type string\nSHARED = ''\n")];
    let paths: Vec<_> = sources.iter().map(|(name, source)| fixture.file(name, source)).collect();
    let mut workspace = ProjectWorkspace::new();
    let report = fixture.compare(&mut workspace, fixture.plan(&["a.lua", "b.lua", "c.lua"], &[]), options());
    for path in &paths {
        diagnostic_at(&report, path, "TS2403", "SHARED");
    }
}

/// Each edit of `file` is checked as a cold check would, and the last of `edits` reads what the
/// first published when `reads` says it can.
fn edits_match_cold_checks(fixture: &Fixture, entries: &[&str], file: &str, edits: &[&str], reads: bool) {
    // An edit costing half the program is a whole check, so the program is much larger than `file`.
    fixture.file("rest.lua", &"do\n---@type number\nlocal rest = 1\nlocal function more() return rest end\nend\n".repeat(500));
    let entries: Vec<_> = entries.iter().copied().chain(["rest.lua"]).collect();
    let plan = || fixture.plan(&entries, &[]);
    let mut workspace = ProjectWorkspace::new();
    fixture.compare_incrementally(&mut workspace, plan(), modules());
    for edit in edits {
        fixture.file(file, edit);
        fixture.compare_incrementally(&mut workspace, plan(), modules());
    }
    assert_eq!(workspace.last_incremental_read_published(), reads);
}

#[test]
fn a_second_edit_reads_what_the_first_published() {
    let fixture = Fixture::new();
    fixture.file("m.lua", "local M = {}\nfunction M.size() return 1 end\nreturn M\n");
    let edited = |extra: &str| format!("local m = require('m')\nlocal n = m.size()\n---@type string\nlocal bad = n\n{extra}\nreturn bad\n");
    fixture.file("a.lua", &edited(""));
    edits_match_cold_checks(&fixture, &["a.lua", "m.lua"], "a.lua", &[&edited("local x = 1"), &edited("local x = 2"), &edited("local y = 3")], true);
}

#[test]
fn a_class_of_the_edited_file_that_another_file_returns_stays_that_class() {
    let fixture = Fixture::new();
    let class = |extra: &str| format!("---@class Point\n---@field x number\nlocal Point = {{}}\n---@return Point\nfunction Point.make() return {{ x = 1 }} end\nlocal made = require('y').point()\n---@type Point\nlocal same = made\nlocal bad = made.y\n{extra}\nreturn Point\n");
    fixture.file("x.lua", &class(""));
    fixture.file("y.lua", "local M = {}\nfunction M.point() return require('x').make() end\nreturn M\n");
    edits_match_cold_checks(&fixture, &["x.lua", "y.lua"], "x.lua", &[&class("local a = 1"), &class("local a = 2"), &class("local a = 3")], true);
}

#[test]
fn an_edit_that_changes_what_another_file_inferred_from_it_checks_again() {
    let fixture = Fixture::new();
    let source = |value: &str| format!("local M = {{}}\nfunction M.value() return {value} end\n---@type string\nlocal echoed = require('y').echo()\nreturn M\n");
    fixture.file("x.lua", &source("1"));
    fixture.file("y.lua", "local M = {}\nfunction M.echo() return require('x').value() end\nreturn M\n");
    // The first edit publishes, the second reads, and the third reshapes the file.
    edits_match_cold_checks(&fixture, &["x.lua", "y.lua"], "x.lua", &[&source("1 + 0"), &source("1 + 1"), &source("'one'")], false);
}

#[test]
fn a_method_one_file_adds_to_another_files_class_is_found_when_it_changes() {
    let fixture = Fixture::new();
    fixture.file("y.lua", "---@class Foo\nFoo = {}\n");
    let adds = |methods: &str| format!("{methods}\n---@type string\nlocal text = Foo:m()\n");
    fixture.file("x.lua", &adds("function Foo:m() return 1 end"));
    fixture.file("z.lua", "---@type string\nlocal s = Foo:m()\nlocal t = Foo:n()\n");
    let entries = ["x.lua", "y.lua", "z.lua"];
    fixture.file("rest.lua", &"do\n---@type number\nlocal rest = 1\nlocal function more() return rest end\nend\n".repeat(500));
    let entries: Vec<_> = entries.iter().copied().chain(["rest.lua"]).collect();
    let plan = || fixture.plan(&entries, &[]);
    let mut workspace = ProjectWorkspace::new();
    fixture.compare_incrementally(&mut workspace, plan(), modules());
    // The return type changes, then a method is added and another removed. Each is seen by the
    // file that adds it, and by the file that calls it.
    for (edit, reads) in [
        (adds("function Foo:m() return 2 end"), false),
        (adds("function Foo:m() return 2 end\nlocal a = 1"), true),
        (adds("function Foo:m() return 'two' end\nlocal a = 1"), false),
        (adds("function Foo:m() return 'two' end\nfunction Foo:n() return 3 end\nlocal a = 1"), false),
        (adds("function Foo:n() return 3 end\nlocal a = 1"), false),
        (adds("function Foo:n() return 3 end\nlocal a = 2"), true),
    ] {
        fixture.file("x.lua", &edit);
        fixture.compare_incrementally(&mut workspace, plan(), modules());
        assert_eq!(workspace.last_incremental_read_published(), reads, "{edit}");
    }
}

#[test]
fn a_class_declared_in_two_files_keeps_the_edited_files_errors() {
    let fixture = Fixture::new();
    fixture.file("base.lua", "---@class Base\n---@field size number\nlocal Base = {}\nreturn Base\n");
    fixture.file("y.lua", "---@class Shared : Base\n---@field a number\nlocal Shared = {}\nreturn Shared\n");
    let declares = |extra: &str| format!("---@class Shared\n---@field a string\n---@field size string\nlocal Shared = {{}}\n{extra}\nreturn Shared\n");
    fixture.file("x.lua", &declares(""));
    edits_match_cold_checks(&fixture, &["base.lua", "x.lua", "y.lua"], "x.lua", &[&declares("local a = 1"), &declares("local a = 2"), &declares("local a = 3")], true);
}

#[test]
fn a_new_global_of_the_edited_file_renumbers_the_rest() {
    let fixture = Fixture::new();
    fixture.file("g.lua", "Beta = 1\nGamma = 'g'\n");
    let uses = |extra: &str| format!("{extra}\nlocal b = Beta\n---@type string\nlocal bad = b\nlocal g = Gamma\n");
    fixture.file("x.lua", &uses(""));
    edits_match_cold_checks(&fixture, &["g.lua", "x.lua"], "x.lua", &[&uses("local a = 1"), &uses("local a = 2"), &uses("Alpha = 1")], false);
}

#[test]
fn a_global_declared_in_two_files_keeps_the_edited_files_conflict() {
    let fixture = Fixture::new();
    fixture.file("y.lua", "Both = 1\n");
    let declares = |extra: &str| format!("Both = 'text'\n{extra}\n");
    fixture.file("x.lua", &declares(""));
    edits_match_cold_checks(&fixture, &["x.lua", "y.lua"], "x.lua", &[&declares("local a = 1"), &declares("local a = 2"), &declares("local a = 3")], true);
}

#[test]
fn a_file_published_for_before_its_first_edit_reads_it_then() {
    let fixture = Fixture::new();
    fixture.file("rest.lua", &"do\n---@type number\nlocal rest = 1\nlocal function more() return rest end\nend\n".repeat(500));
    fixture.file("m.lua", "local M = {}\nfunction M.size() return 1 end\nreturn M\n");
    let edited = |extra: &str| format!("local m = require('m')\nlocal n = m.size()\n---@type string\nlocal bad = n\n{extra}\nreturn bad\n");
    fixture.file("a.lua", &edited(""));
    let entries = ["a.lua", "m.lua", "rest.lua"];
    let plan = || fixture.plan(&entries, &[]);
    let mut workspace = ProjectWorkspace::new();
    fixture.compare_incrementally(&mut workspace, plan(), modules());
    let resolver = fixture.resolver();
    let project = Project::load_plan(&resolver, plan(), |source| workspace.static_require_requests(source));
    workspace.publish_for_edits(project, modules(), &fs::canonicalize(fixture.0.join("a.lua")).unwrap());
    fixture.file("a.lua", &edited("local x = 1"));
    fixture.compare_incrementally(&mut workspace, plan(), modules());
    assert!(workspace.last_incremental_read_published());
}

#[test]
fn a_base_for_another_open_file_keeps_the_edited_files() {
    let fixture = Fixture::new();
    fixture.file("rest.lua", &"do\n---@type number\nlocal rest = 1\nlocal function more() return rest end\nend\n".repeat(500));
    fixture.file("m.lua", "local M = {}\nfunction M.size() return 1 end\nreturn M\n");
    let edited = |extra: &str| format!("local m = require('m')\nlocal n = m.size()\n---@type string\nlocal bad = n\n{extra}\nreturn bad\n");
    fixture.file("a.lua", &edited(""));
    fixture.file("b.lua", "local m = require('m')\n---@type string\nlocal also = m.size()\nreturn also\n");
    let entries = ["a.lua", "b.lua", "m.lua", "rest.lua"];
    let plan = || fixture.plan(&entries, &[]);
    let resolver = fixture.resolver();
    let mut workspace = ProjectWorkspace::new();
    fixture.compare_incrementally(&mut workspace, plan(), modules());
    fixture.file("a.lua", &edited("local x = 1"));
    fixture.compare_incrementally(&mut workspace, plan(), modules());
    // The editor shows b.lua, with nothing changed since: its next edit has a base of its own.
    let project = Project::load_plan(&resolver, plan(), |source| workspace.static_require_requests(source));
    workspace.publish_for_edits(project, modules(), &fs::canonicalize(fixture.0.join("b.lua")).unwrap());
    // Back in a.lua, whose base still holds.
    fixture.file("a.lua", &edited("local x = 2"));
    fixture.compare_incrementally(&mut workspace, plan(), modules());
    assert!(workspace.last_incremental_read_published());
    // And in b.lua, whose base a.lua's edit replaced.
    fixture.file("b.lua", "local m = require('m')\n---@type string\nlocal also = m.size()\nlocal y = 1\nreturn also\n");
    fixture.compare_incrementally(&mut workspace, plan(), modules());
    assert!(!workspace.last_incremental_read_published());
}

#[test]
fn a_check_of_the_function_around_an_edit_finds_what_the_whole_check_finds_there() {
    let fixture = Fixture::new();
    fixture.file("rest.lua", &"do\n---@type number\nlocal rest = 1\nlocal function more() return rest end\nend\n".repeat(500));
    fixture.file("m.lua", "local M = {}\nfunction M.size() return 1 end\nreturn M\n");
    // A method that calls another and passes a callback, whose parameter its call site types.
    let edited = |inside: &str| format!(
        "local m = require('m')\nlocal M = {{}}\n---@type string\nlocal early = 1\n---@param n number\nfunction M.half(n) return n / 2 end\n---@param each fun(n: number): number\nlocal function apply(each) return each(1) end\nfunction M.method()\n  local n = m.size()\n{inside}  return apply(function(x) return x + M.half(n) end)\nend\n---@type string\nlocal late = 2\nreturn M\n");
    fixture.file("a.lua", &edited(""));
    let entries = ["a.lua", "m.lua", "rest.lua"];
    let plan = || fixture.plan(&entries, &[]);
    let resolver = fixture.resolver();
    let mut workspace = ProjectWorkspace::new();
    fixture.compare_incrementally(&mut workspace, plan(), modules());
    fixture.file("a.lua", &edited("  local x = 1\n"));
    fixture.compare_incrementally(&mut workspace, plan(), modules());
    let inside = "  ---@type string\n  local bad = n\n  local wrong = M.half('two')\n";
    let text = edited(inside);
    fixture.file("a.lua", &text);
    let path = fs::canonicalize(fixture.0.join("a.lua")).unwrap();
    let at = text.find(inside).unwrap();
    let project = Project::load_plan(&resolver, plan(), |source| workspace.static_require_requests(source));
    let (region, found) = workspace.check_region(project, modules(), &path, luanalysis_syntax::Span { start: at, end: at + inside.len() }).expect("a base for a.lua");
    assert_eq!(&text[region.start..region.start + 17], "function M.method");
    let report = Project::load_plan(&resolver, plan(), |source| workspace.static_require_requests(source));
    let report = workspace.check_project_incrementally(report, modules());
    let key = |diagnostic: &Diagnostic| (diagnostic.span.start, diagnostic.span.end, diagnostic.code, diagnostic.message.clone());
    let mut whole: Vec<_> = report.files.iter().filter(|file| report.source(file.file).unwrap().path == path)
        .flat_map(|file| file.report.diagnostics.iter())
        .filter(|diagnostic| region.start <= diagnostic.span.start && diagnostic.span.end <= region.end)
        .map(key).collect();
    let mut found: Vec<_> = found.iter().map(key).collect();
    whole.sort();
    found.sort();
    assert_eq!(found, whole);
    assert_eq!(found.len(), 2, "{found:?}");
}

#[test]
fn a_description_over_what_was_published_matches_a_cold_one() {
    let fixture = Fixture::new();
    fixture.file("rest.lua", &"do\n---@type number\nlocal rest = 1\nlocal function more() return rest end\nend\n".repeat(500));
    fixture.file("m.lua", "local M = {}\nfunction M.size() return 1 end\nfunction M.name() return 'm' end\nreturn M\n");
    let edited = |extra: &str| format!("local m = require('m')\nlocal n = m.size()\nlocal s = m.name()\nlocal both = {{ n = n, s = s }}\n{extra}\nreturn both\n");
    fixture.file("a.lua", &edited(""));
    let entries = ["a.lua", "m.lua", "rest.lua"];
    let plan = || fixture.plan(&entries, &[]);
    // As a language server checks: diagnostics first, and types when a hover asks for them.
    let unwritten = || ProjectCheckOptions { describe_types: false, ..modules() };
    let resolver = fixture.resolver();
    let mut workspace = ProjectWorkspace::new();
    let project = Project::load_plan(&resolver, plan(), |source| workspace.static_require_requests(source));
    workspace.check_project_incrementally(project, unwritten());
    for extra in ["local x = 1", "local x = 2"] {
        fixture.file("a.lua", &edited(extra));
        let project = Project::load_plan(&resolver, plan(), |source| workspace.static_require_requests(source));
        workspace.check_project_incrementally(project, unwritten());
    }
    assert!(workspace.last_incremental_read_published());
    let path = fs::canonicalize(fixture.0.join("a.lua")).unwrap();
    let project = Project::load_plan(&resolver, plan(), |source| workspace.static_require_requests(source));
    let described = ProjectCheckOptions { describe_types: true, ..modules() };
    let warm = workspace.describe_incrementally(project, described.clone(), std::slice::from_ref(&path));
    let cold = check_project_with_options(Project::load_plan(&resolver, plan(), static_require_requests), described);
    let of = |report: &ProjectCheckReport| {
        let file = report.project.file_by_path(&path).unwrap().id;
        let report = &report.file(file).unwrap().report;
        (report.bindings.clone(), report.types.clone())
    };
    assert_eq!(of(&warm), of(&cold));
    assert!(!of(&warm).1.is_empty());
}

#[test]
fn an_edit_of_what_a_file_exposes_stands_when_nothing_it_reads_depends_on_it() {
    let fixture = Fixture::new();
    fixture.file("rest.lua", &"do\n---@type number\nlocal rest = 1\nlocal function more() return rest end\nend\n".repeat(500));
    fixture.file("m.lua", "local M = {}\nfunction M.size() return 1 end\nreturn M\n");
    let exposes = |value: &str| format!("local m = require('m')\nlocal A = {{}}\nfunction A.value() return {value} end\n---@type string\nlocal bad = m.size()\nreturn A\n");
    fixture.file("a.lua", &exposes("1"));
    fixture.file("b.lua", "---@type string\nlocal s = require('a').value()\n");
    let entries = ["a.lua", "b.lua", "m.lua", "rest.lua"];
    let plan = || fixture.plan(&entries, &[]);
    let mut workspace = ProjectWorkspace::new();
    fixture.compare_incrementally(&mut workspace, plan(), modules());
    // The first edit publishes. Each after it reads that, and the file that requires it follows
    // when what it returns changes.
    for (value, checked) in [("2", 1), ("3", 1), ("'three'", 2), ("'four'", 1), ("4", 2)] {
        fixture.file("a.lua", &exposes(value));
        let found = fixture.compare_incrementally(&mut workspace, plan(), modules());
        assert_eq!((value, found), (value, checked));
    }
    assert!(workspace.last_incremental_read_published());
}

#[test]
fn a_global_the_edited_file_redeclares_keeps_its_conflict_with_a_library() {
    let fixture = Fixture::new();
    fixture.file("rest.lua", &"do\n---@type number\nlocal rest = 1\nlocal function more() return rest end\nend\n".repeat(500));
    fixture.file("events.def.lua", "---@meta\n---@type nil | fun(): void | nil | string\nonSave = nil\n");
    let redeclares = |extra: &str| format!("{extra}\nlocal original = onSave\n---@return string\nfunction onSave()\n    return 'saved'\nend\n");
    fixture.file("x.lua", &redeclares(""));
    let plan = || fixture.plan(&["rest.lua", "x.lua"], &[("events.def.lua", SourceRole::Declaration)]);
    let mut workspace = ProjectWorkspace::new();
    fixture.compare_incrementally(&mut workspace, plan(), modules());
    for extra in ["local a = 1", "local a = 2", "local a = 3"] {
        fixture.file("x.lua", &redeclares(extra));
        fixture.compare_incrementally(&mut workspace, plan(), modules());
    }
    assert!(workspace.last_incremental_read_published());
    let report = fixture.compare(&mut ProjectWorkspace::new(), plan(), modules());
    diagnostic_at(&report, &fixture.0.join("x.lua"), "TS2403", "onSave");
}
