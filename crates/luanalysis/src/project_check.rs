//! Native checker tasks over an immutable collection of original Lua files.
//! Files retain their own syntax, lexical identities, graphs and source reports.
use crate::native_functions::project_provider::{ProjectRegistrySnapshot, RegistryInput};
use crate::standard_environment::{StandardEnvironmentProfile, StandardEnvironmentSelection};
use crate::{CheckReport, Diagnostic};
use luanalysis_engine::{
    Engine, NativeLoaderPolicy, NativePlanOptions, NativeTaskRunStats,
};
use luanalysis_project::{
    DeclarationSourceError, FileId, Project, RequireRequest, SourceFile, SourceRole,
};
use crate::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct ProjectCheckOptions {
    /// Maximum simultaneous native checker tasks. Zero selects the host's
    /// available parallelism; one requests deterministic serial execution.
    pub jobs: usize,
    pub loader_policy: NativeLoaderPolicy,
    pub environment: StandardEnvironmentProfile,
    pub overload_mode: crate::OverloadMode,
    /// Opt-in missing parameter/rest contract diagnostics; ordinary analysis
    /// preserves v1's acceptance of unannotated runtime functions.
    pub no_implicit_any: bool,
    /// Ordinary analysis preserves reached standard bindings across unknown
    /// effects. False requests the stricter runtime-effect certificates.
    pub assume_unknown_standard_effects: bool,
    /// Describe binding and expression types in the report. Only `--dump-types`
    /// and library callers that query types need them.
    pub describe_types: bool,
    /// Report inspections, as an editor shows them: warnings for conditions that are always
    /// true or false and comparisons that cannot be equal, and suggestions for unreachable code,
    /// unused locals and parameters without a type. Off for `check`; none is an error.
    pub inspections: bool,
}
impl Default for ProjectCheckOptions {
    fn default() -> Self {
        Self {
            jobs: 0,
            loader_policy: NativeLoaderPolicy::Lua54StandardFile,
            environment: StandardEnvironmentProfile::LegacyMinimal,
            overload_mode: crate::OverloadMode::default(),
            no_implicit_any: false,
            assume_unknown_standard_effects: true,
            describe_types: true,
            inspections: false,
        }
    }
}

/// Collects the static require calls whose targets join the program, as Bun's
/// load collects imports. Whether a call returns its module is still decided
/// by checking; comments and strings are never scanned for imports.
pub fn static_require_requests(source: &SourceFile) -> Vec<RequireRequest> {
    if source.role == SourceRole::Declaration {
        return Vec::new();
    }
    let file = luanalysis_syntax::parse(&source.source);
    if file.has_lua_syntax_errors() {
        return Vec::new();
    }
    let bound = crate::bind::bind(&file);
    crate::module_requests::require_calls(&file, &bound)
        .into_iter()
        .filter_map(|call| {
            let crate::module_requests::RequireTarget::Static(module) = call.target else {
                return None;
            };
            Some(RequireRequest::new(
                module,
                call.argument_span.start..call.argument_span.end,
            ))
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct FileCheckReport {
    pub file: FileId,
    pub report: CheckReport,
}

/// Retains original source contents and host failures after the native arena ends.
#[derive(Clone, Debug)]
pub struct ProjectCheckReport {
    pub project: Project,
    pub files: Vec<FileCheckReport>,
    pub standard_library_files: Vec<FileId>,
    pub environment_errors: Vec<DeclarationSourceError>,
    /// Actual shared-Program task/barrier execution, when multiple roots were
    /// scheduled. A single worker uses the original one-session check path.
    pub execution: Option<NativeTaskRunStats>,
    /// Maximum task count in a step of Bun's source/cost plan.
    pub planned_parallelism: usize,
}
impl ProjectCheckReport {
    pub fn file(&self, file: FileId) -> Option<&FileCheckReport> {
        self.project.file(file)?;
        self.files.iter().find(|report| report.file == file)
    }

    pub fn source(&self, file: FileId) -> Option<&SourceFile> {
        self.project.file(file)
    }

    pub fn checked_file_count(&self) -> usize {
        self.files
            .iter()
            .filter(|file| !self.standard_library_files.contains(&file.file))
            .count()
    }

    /// Host failures and source diagnostics affect success. A static dependency
    /// cycle alone is a graph fact rather than a type-checking failure.
    pub fn has_errors(&self) -> bool {
        !self.environment_errors.is_empty()
            || self
                .files
                .iter()
                .any(|file| file.report.diagnostics.iter().any(|diagnostic| diagnostic.category() == crate::DiagnosticCategory::Error))
            || !self.project.entry_errors.is_empty()
            || !self.project.library_errors.is_empty()
            || !self.project.role_conflicts.is_empty()
    }
}

/// Checks registered project sources together without manufacturing module
/// exports or merging their independent lexical/flow identities.
pub fn check_project(project: Project) -> ProjectCheckReport {
    check_project_with_options(project, ProjectCheckOptions::default())
}

pub fn check_project_with_options(
    project: Project,
    options: ProjectCheckOptions,
) -> ProjectCheckReport {
    crate::incremental::ProjectWorkspace::new().check_project(project, options)
}

/// The sources of a program, prepared for native sessions as `check` prepares them.
struct PreparedProgram {
    snapshot: ProjectRegistrySnapshot,
    /// Reports for sources that could not be prepared (syntax errors).
    files: Vec<FileCheckReport>,
    source_costs: HashMap<FileId, usize>,
    standard_library_files: Vec<FileId>,
    environment_errors: Vec<DeclarationSourceError>,
}

fn prepare_program(
    project: &mut Project,
    options: ProjectCheckOptions,
    workspace: &mut crate::incremental::ProjectWorkspace,
) -> PreparedProgram {
    let (selection, environment_errors) =
        match StandardEnvironmentSelection::register(project, options.environment) {
            Ok(selection) => (selection, Vec::new()),
            Err(error) => (
                StandardEnvironmentSelection::empty(StandardEnvironmentProfile::None),
                vec![error],
            ),
        };
    let standard_library_files = selection.files();
    let mut inputs = Vec::new();
    let mut files = Vec::new();
    let plan_options = NativePlanOptions::default();
    let mut source_costs = HashMap::default();
    for source in workspace.prepare_registered_sources(project).sources {
        let parsed = source.snapshot.syntax();
        if let Some(bound) = source.snapshot.bound() {
            source_costs.insert(
                source.id,
                crate::project_cost::checking_cost(parsed, bound, plan_options.type_node_cost),
            );
            inputs.push(RegistryInput {
                id: source.id,
                role: source.role,
                standard_library: selection.kind(source.id),
                syntax: parsed.clone(),
                bound: bound.clone(),
            });
        } else {
            source_costs.insert(source.id, 1);
            files.push(FileCheckReport {
                file: source.id,
                report: CheckReport {
                    diagnostics: parsed
                        .diagnostics
                        .iter()
                        .map(|error| Diagnostic {
                            code: "LUA1001",
                            message: error.message.clone(),
                            span: error.span,
                        })
                        .collect(),
                    ..CheckReport::default()
                },
            });
        }
    }
    let snapshot = ProjectRegistrySnapshot::new(
        inputs,
        project.dependencies.clone(),
        options.loader_policy,
        selection.profile,
        options.overload_mode,
    ).with_no_implicit_any(options.no_implicit_any);
    PreparedProgram { snapshot, files, source_costs, standard_library_files, environment_errors }
}

/// Runs `query` in one native session over the project, as one checking task runs, for the
/// language service: like tsgo's language service, it asks the checker only what an answer needs.
pub(crate) fn with_project_session<R: Send>(
    mut project: Project,
    options: ProjectCheckOptions,
    workspace: &mut crate::incremental::ProjectWorkspace,
    query: impl FnOnce(&Project, &crate::native_functions::project_provider::ProjectRegistry, &mut dyn luanalysis_engine::TypeQueries) -> R + Send,
) -> R {
    let prepared = prepare_program(&mut project, options, workspace);
    let snapshot = &prepared.snapshot;
    let project = &project;
    Engine::new().with_native_provider(
        || {
            let mut registry = snapshot.worker_registry();
            registry.set_standard_environment_assumptions(options.assume_unknown_standard_effects);
            registry.set_describe_types(options.describe_types);
            registry.set_inspections(options.inspections);
            registry
        },
        |registry, queries| query(project, registry, queries),
    )
}

pub(crate) fn check_project_with_workspace(
    mut project: Project,
    options: ProjectCheckOptions,
    workspace: &mut crate::incremental::ProjectWorkspace,
) -> ProjectCheckReport {
    let profile = std::env::var_os("LUANALYSIS_PROFILE").is_some();
    let started = Instant::now();
    let PreparedProgram { mut snapshot, mut files, source_costs, standard_library_files, environment_errors } =
        prepare_program(&mut project, options, workspace);
    // As in Bun, the program is every file the load reached: explicit inputs and
    // the targets of their syntactic require calls. Each is checked once, by one
    // task, whether or not the checker later proves the call's loader.
    let mut roots: Vec<_> = project
        .entries
        .iter()
        .chain(&project.libraries)
        .copied()
        .collect();
    let explicit: HashSet<FileId> = roots.iter().copied().collect();
    roots.extend(project.files.iter().map(|file| file.id).filter(|file| !explicit.contains(file)));
    let requested = NonZeroUsize::new(options.jobs)
        .unwrap_or_else(|| std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN));
    // One task per worker, a measured deviation from Bun's warm-up and chunk plan
    // (docs/BUN-CHECK-COMPARISON.md, item 9). Bun's plan relies on cheap tasks and
    // on publishing what a task derives; here a task builds the whole project
    // registry and derives the native results it needs itself, so every extra
    // task repeats that work. On Prototype Inertia Bun's plan took 4.1 to 4.6 s
    // wall at 4 and 10 workers, against 2.2 s for this plan and 3.7 s serially.
    let capacity = requested.get().min(roots.len()).max(1);
    let tasks = pool_tasks(roots.len(), &|index| source_costs[&roots[index]], capacity);
    let workers = tasks.len().max(1);
    let source_prepare = started.elapsed();
    let flow_started = Instant::now();
    if workers > 1 { snapshot.prepare_shared_flows(); }
    let flow_prepare = flow_started.elapsed();
    let factory_nanos = AtomicU64::new(0);
    let factories = AtomicU64::new(0);
    let make_registry = |_| {
        let started = profile.then(Instant::now);
        let mut registry = snapshot.worker_registry();
        registry.set_standard_environment_assumptions(options.assume_unknown_standard_effects);
        registry.set_describe_types(options.describe_types);
        registry.set_inspections(options.inspections);
        if let Some(started) = started {
            factory_nanos.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
            factories.fetch_add(1, Ordering::Relaxed);
        }
        registry
    };
    let checking_started = Instant::now();
    let (checked, execution) = if workers == 1 {
        let checked = Engine::new().with_native_provider(
            || make_registry(0),
            |registry, queries| registry.check_roots(queries, &roots),
        );
        (checked, None)
    } else {
        // Each task checks only the files it owns, as a Bun task checks its
        // files: every file is planned, so each body is checked by one task.
        let batches: Vec<Vec<FileId>> = tasks.into_iter()
            .map(|task| task.into_iter().map(|index| roots[index]).collect())
            .collect();
        let batch_costs: Vec<usize> = batches.iter()
            .map(|batch| batch.iter().map(|file| source_costs[file]).sum())
            .collect();
        let steps = vec![(0..batches.len()).collect::<Vec<_>>()];
        let run = Engine::new().with_native_task_plan_by_cost_report(
            NonZeroUsize::new(workers).unwrap(),
            &steps,
            |job| batch_costs[job],
            make_registry,
            |job, registry, queries| {
                let started = profile.then(Instant::now);
                let reports = registry.check_owned_roots(queries, &batches[job]);
                if let Some(started) = started {
                    let names: Vec<_> = batches[job].iter().map(|&file| {
                        project.file(file).unwrap().path.file_name().unwrap().to_string_lossy()
                    }).collect();
                    eprintln!("luanalysis profile: job={job} elapsed={:.6}s files={names:?}", started.elapsed().as_secs_f64());
                }
                reports
            },
        );
        (merge_reports(run.results.into_iter().flatten().collect()), Some(run.stats))
    };
    if profile {
        eprintln!("luanalysis profile: source_prepare={:.6}s flow_prepare={:.6}s checking={:.6}s registry_factory_sum={:.6}s factories={} execution={execution:?}",
            source_prepare.as_secs_f64(), flow_prepare.as_secs_f64(), checking_started.elapsed().as_secs_f64(),
            factory_nanos.load(Ordering::Relaxed) as f64 / 1_000_000_000.0, factories.load(Ordering::Relaxed));
    }
    files.extend(checked);
    files.sort_by_key(|file| file.file.index);
    ProjectCheckReport {
        project,
        files,
        standard_library_files,
        environment_errors,
        execution,
        planned_parallelism: capacity,
    }
}

/// Roots assigned heaviest first to the least loaded of `workers` tasks, each task's roots in
/// program order.
fn pool_tasks(count: usize, cost: &dyn Fn(usize) -> usize, workers: usize) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by_key(|&index| (std::cmp::Reverse(cost(index)), index));
    let mut tasks: Vec<(usize, Vec<usize>)> = (0..workers.max(1)).map(|_| (0, Vec::new())).collect();
    for index in order {
        let lightest = (0..tasks.len()).min_by_key(|&task| (tasks[task].0, task)).unwrap();
        tasks[lightest].0 += cost(index);
        tasks[lightest].1.push(index);
    }
    let mut tasks: Vec<Vec<usize>> = tasks.into_iter()
        .map(|(_, mut task)| { task.sort_unstable(); task })
        .filter(|task| !task.is_empty())
        .collect();
    tasks.sort_unstable();
    tasks
}

fn merge_reports(reports: Vec<FileCheckReport>) -> Vec<FileCheckReport> {
    let mut merged = Vec::<FileCheckReport>::new();
    let mut indices = HashMap::<FileId, usize>::default();
    for file in reports {
        assert!(
            file.report.pending_bindings.is_empty()
                && file.report.pending_types.is_empty()
                && file.report.pending_diagnostics.is_empty(),
            "project task reports must render every native operand before ending"
        );
        if let Some(&index) = indices.get(&file.file) {
            let target = &mut merged[index];
            debug_assert_eq!(target.report.bound, file.report.bound);
            target.report.append(file.report);
        } else {
            indices.insert(file.file, merged.len());
            merged.push(file);
        }
    }
    for file in &mut merged {
        let report = &mut file.report;
        report.diagnostics.sort_by(|left, right| {
            (left.span.start, left.span.end, left.code, &left.message).cmp(&(
                right.span.start,
                right.span.end,
                right.code,
                &right.message,
            ))
        });
        report.diagnostics.dedup();
        report.bindings.sort_by(|left, right| {
            (left.span.start, left.span.end, left.binding.0, &left.ty).cmp(&(
                right.span.start,
                right.span.end,
                right.binding.0,
                &right.ty,
            ))
        });
        report.bindings.dedup();
        report.types.sort_by(|left, right| {
            (left.span.start, left.span.end, &left.ty).cmp(&(
                right.span.start,
                right.span.end,
                &right.ty,
            ))
        });
        report.types.dedup();
    }
    merged
}
