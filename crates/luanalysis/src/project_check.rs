//! Native checker tasks over an immutable collection of original Lua files.
//! Files retain their own syntax, lexical identities, graphs and source reports.
use crate::native_functions::project_provider::{ProjectRegistrySnapshot, RegistryInput};
use crate::standard_environment::{StandardEnvironmentProfile, StandardEnvironmentSelection};
use crate::{CheckReport, Diagnostic};
use luanalysis_engine::{
    Engine, NativeCheckPlan, NativeLoaderPolicy, NativePlanOptions, NativeTaskRunStats,
};
use luanalysis_project::{
    DeclarationSourceError, FileId, Project, RequireRequest, SourceFile, SourceRole,
};
use crate::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;

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
        }
    }
}

/// Collects original static require candidates for source-host discovery.
/// Native callee and loader provenance must still be proven before a candidate
/// schedules module checking; comments and strings are never scanned for imports.
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
                .any(|file| !file.report.diagnostics.is_empty())
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

pub(crate) fn check_project_with_workspace(
    mut project: Project,
    options: ProjectCheckOptions,
    workspace: &mut crate::incremental::ProjectWorkspace,
) -> ProjectCheckReport {
    let (selection, environment_errors) =
        match StandardEnvironmentSelection::register(&mut project, options.environment) {
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
    for source in workspace.prepare_registered_sources(&project).sources {
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
    let dependencies = project.dependencies.clone();
    let roots: Vec<_> = project
        .entries
        .iter()
        .chain(&project.libraries)
        .copied()
        .collect();
    let snapshot = ProjectRegistrySnapshot::new(
        inputs,
        dependencies,
        options.loader_policy,
        selection.profile,
        options.overload_mode,
    ).with_no_implicit_any(options.no_implicit_any);
    let make_registry = |_| {
        let mut registry = snapshot.worker_registry();
        registry.set_standard_environment_assumptions(options.assume_unknown_standard_effects);
        registry
    };
    let requested = NonZeroUsize::new(options.jobs)
        .unwrap_or_else(|| std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN));
    // Use the extracted Bun driver policy: sampled light-file warmups and
    // source-cost chunks. Module edges do not impose publication barriers.
    // Lua has no TS declaration-statement range task adapter, so ranges are empty.
    let plan = NativeCheckPlan::new(
        roots.len(),
        &|index| project.file(roots[index]).unwrap().source.len(),
        &|index| source_costs[&roots[index]],
        &|_, _| Vec::new(),
        &plan_options,
    );
    let capacity = plan.steps.iter().map(Vec::len).max().unwrap_or(1);
    let workers = requested.get().min(capacity).max(1);
    let (checked, required, execution) = if workers == 1 {
        let (checked, required) = Engine::new().with_native_provider(
            || make_registry(0),
            |registry, queries| {
                let reports = registry.check_roots(queries, &roots);
                (reports, registry.required_files())
            },
        );
        (checked, required, None)
    } else {
        let scheduled: HashSet<_> = roots.iter().copied().collect();
        let mut batches = Vec::new();
        let mut steps = Vec::new();
        let mut batch_costs = Vec::new();
        for step in plan.steps {
            let mut jobs = Vec::new();
            for task in step {
                jobs.push(batches.len());
                batch_costs.push(
                    task.iter()
                        .map(|&index| source_costs[&roots[index]])
                        .sum::<usize>(),
                );
                batches.push(
                    task.into_iter()
                        .map(|index| roots[index])
                        .collect::<Vec<_>>(),
                );
            }
            steps.push(jobs);
        }
        let run = Engine::new().with_native_task_plan_by_cost_report(
            NonZeroUsize::new(workers).unwrap(),
            &steps,
            |job| batch_costs[job],
            make_registry,
            |job, registry, queries| {
                let reports = registry.check_scheduled_roots(queries, &batches[job], &scheduled);
                (reports, registry.required_files())
            },
        );
        let mut required = HashSet::default();
        let mut checked = Vec::new();
        for (reports, targets) in run.results {
            checked.extend(reports);
            required.extend(targets);
        }
        (merge_reports(checked), required, Some(run.stats))
    };
    files.retain(|report| required.contains(&report.file));
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
