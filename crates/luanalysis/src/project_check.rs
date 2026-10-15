//! One native checker session for an immutable collection of original Lua files.
//! Files retain their own syntax, lexical identities, graphs and source reports.
use crate::native_functions::project_provider::{ProjectRegistry, RegistryInput};
use crate::{CheckReport, Diagnostic};
use crate::standard_environment::{StandardEnvironmentProfile, StandardEnvironmentSelection};
use luanalysis_engine::{Engine, NativeLoaderPolicy};
use luanalysis_project::{
    DeclarationSourceError, DependencyOutcome, FileId, Project, RequireRequest, SourceFile, SourceRole,
};

#[derive(Clone, Copy, Debug)]
pub struct ProjectCheckOptions {
    pub loader_policy: NativeLoaderPolicy,
    pub environment: StandardEnvironmentProfile,
}
impl Default for ProjectCheckOptions {
    fn default() -> Self {
        Self {
            loader_policy: NativeLoaderPolicy::Lua54StandardFile,
            environment: StandardEnvironmentProfile::LegacyMinimal,
        }
    }
}

/// Extracts only calls resolved by the binder to the standard require
/// intrinsic. Source roles, lexical shadowing and loader mutation remain
/// explicit; comments and strings are never scanned for imports.
pub fn static_require_requests(source: &SourceFile) -> Vec<RequireRequest> {
    if source.role == SourceRole::Declaration {
        return Vec::new();
    }
    let file = luanalysis_syntax::parse(&source.source);
    if !file.diagnostics.is_empty() {
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
        self.files.iter().filter(|file| !self.standard_library_files.contains(&file.file)).count()
    }

    /// Host failures and source diagnostics affect success. A static dependency
    /// cycle alone is a graph fact rather than a type-checking failure.
    pub fn has_errors(&self) -> bool {
        !self.environment_errors.is_empty() || self.files
            .iter()
            .any(|file| !file.report.diagnostics.is_empty())
            || !self.project.entry_errors.is_empty()
            || !self.project.library_errors.is_empty()
            || !self.project.role_conflicts.is_empty()
            || self
                .project
                .dependencies
                .iter()
                .any(|dependency| !matches!(dependency.outcome, DependencyOutcome::Resolved(_)))
    }
}

/// Checks registered project sources together without manufacturing module
/// exports or merging their independent lexical/flow identities.
pub fn check_project(project: Project) -> ProjectCheckReport {
    check_project_with_options(project, ProjectCheckOptions::default())
}

pub fn check_project_with_options(
    mut project: Project,
    options: ProjectCheckOptions,
) -> ProjectCheckReport {
    let (selection, environment_errors) = match StandardEnvironmentSelection::register(&mut project, options.environment) {
        Ok(selection) => (selection, Vec::new()),
        Err(error) => (StandardEnvironmentSelection::empty(StandardEnvironmentProfile::None), vec![error]),
    };
    let standard_library_files = selection.files();
    let mut inputs = Vec::new();
    let mut files = Vec::new();
    for source in &project.files {
        let parsed = luanalysis_syntax::parse(&source.source);
        if parsed.diagnostics.is_empty() {
            let bound = crate::bind::bind(&parsed);
            inputs.push(RegistryInput {
                id: source.id,
                role: source.role,
                standard_library: selection.kind(source.id),
                syntax: parsed,
                bound,
            });
        } else {
            files.push(FileCheckReport {
                file: source.id,
                report: CheckReport {
                    diagnostics: parsed
                        .diagnostics
                        .into_iter()
                        .map(|error| Diagnostic {
                            code: "LUA1001",
                            message: error.message,
                            span: error.span,
                        })
                        .collect(),
                    ..CheckReport::default()
                },
            });
        }
    }
    let dependencies = project.dependencies.clone();
    files.extend(Engine::new().with_native_provider(
        move || ProjectRegistry::new_with_environment(inputs, dependencies, options.loader_policy, selection.profile),
        |registry, queries| registry.check(queries),
    ));
    files.sort_by_key(|file| file.file.index);
    ProjectCheckReport { project, files, standard_library_files, environment_errors }
}
