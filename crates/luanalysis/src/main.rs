use luanalysis::cli::{Arguments, DiscoveredInputs, Parsed, help, identity, parse_arguments};
use luanalysis::incremental::ProjectWorkspace;
use luanalysis::lsp;
use luanalysis::report::{CliReport, HostDiagnostic, OutputFormat};
use luanalysis_project::{DependencyOutcome, Project, ResolveError, Resolver, SourceOrigin};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let mut args = std::env::args_os().skip(1);
    let Some(command) = args.next() else {
        help();
        return 2;
    };
    if command == "--help" || command == "-h" {
        help();
        return 0;
    }
    if command == "--version" || command == "-V" {
        println!(
            "luanalysis {} (Bun core {})",
            env!("CARGO_PKG_VERSION"),
            luanalysis_engine::BUN_REVISION
        );
        return 0;
    }
    if command == "lsp" {
        return match parse_arguments(args.collect()) {
            Ok(Parsed::Help) => { help(); 0 }
            Ok(Parsed::Run(arguments)) => lsp::run_stdio(arguments.inputs, arguments.options),
            Err(message) => { eprintln!("luanalysis: {message}"); 2 }
        };
    }
    if command != "check" {
        eprintln!("luanalysis: unknown command {:?}", command);
        help();
        return 2;
    }
    let remaining: Vec<_> = args.collect();
    let format = OutputFormat::requested(&remaining);
    let Arguments { dump, watch, inputs, options } = match parse_arguments(remaining) {
        Ok(Parsed::Run(arguments)) => arguments,
        Ok(Parsed::Help) => { help(); return 0; }
        Err(message) => return usage_error(format, &message),
    };
    if inputs.files.is_empty() && inputs.libraries.is_empty() {
        return usage_error(format, "check requires at least one Lua file");
    }
    let mut workspace = ProjectWorkspace::new();
    let mut discovery = match inputs.discover() {
        Ok(discovery) => discovery,
        Err(error) => return host_error(format, error),
    };
    loop {
        let resolver = match inputs.resolver(&discovery) {
            Ok(resolver) => resolver,
            Err(error) => return host_error(format, error),
        };
        // Reload source files and resolution outcomes for every revision. Only
        // owned syntax/binding snapshots survive in the workspace; checker
        // handles and diagnostics always belong to this new checking epoch.
        let project = Project::load_plan(&resolver, discovery.plan.clone(), |source| {
            workspace.static_require_requests(source)
        });
        let watched = watch.then(|| WatchSnapshot::loaded(&discovery, &project));
        let result = workspace.check_project(project, options.clone());
        let report = CliReport::from_project(
            &result,
            &discovery.display_paths,
            discovery.errors.clone(),
            dump,
        );
        report.print(format);
        let Some(watched) = watched else {
            return report.exit_code;
        };
        // A complete JSON object is one watch revision. Explicit flushing also
        // delivers a revision when stdout is a pipe to an editor or build tool.
        if let Err(error) = io::stdout().flush() {
            eprintln!("luanalysis: cannot write watch report: {error}");
            return 2;
        }
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let next = match inputs.discover() {
                Ok(discovery) => discovery,
                Err(error) => return host_error(format, error),
            };
            let resolver = match inputs.resolver(&next) {
                Ok(resolver) => resolver,
                Err(error) => return host_error(format, error),
            };
            if watched.changed(&next, &resolver) {
                discovery = next;
                break;
            }
        }
    }
}

#[derive(PartialEq, Eq)]
enum FileContents {
    Bytes(Vec<u8>),
    Error(io::ErrorKind, String),
}

fn file_contents(path: &Path) -> FileContents {
    match fs::read(path) {
        Ok(bytes) => FileContents::Bytes(bytes),
        Err(error) => FileContents::Error(error.kind(), error.to_string()),
    }
}

struct WatchSnapshot {
    discovery: DiscoveredInputs,
    files: BTreeMap<PathBuf, FileContents>,
    modules: BTreeMap<String, Result<PathBuf, ResolveError>>,
}

impl WatchSnapshot {
    fn loaded(discovery: &DiscoveredInputs, project: &Project) -> Self {
        let mut paths: BTreeSet<_> = discovery
            .plan
            .entries
            .iter()
            .cloned()
            .chain(
                discovery
                    .plan
                    .libraries
                    .iter()
                    .map(|input| input.path.clone()),
            )
            .collect();
        let mut loaded = HashMap::new();
        let mut failed = HashMap::new();
        for error in project.entry_errors.iter().chain(&project.library_errors) {
            paths.insert(error.path.clone());
            failed.insert(error.path.clone(), error);
        }
        for source in &project.files {
            if source.origin == SourceOrigin::Filesystem {
                paths.insert(source.path.clone());
                loaded.insert(source.path.clone(), source.source.as_bytes());
            }
        }
        let mut modules = BTreeMap::new();
        for dependency in &project.dependencies {
            // A module provided by name follows its provider's source, which is already watched.
            if project.provider(&dependency.request.module).is_some() {
                continue;
            }
            let resolution = match &dependency.outcome {
                DependencyOutcome::Resolved(file) => {
                    project.file(*file).map(|source| Ok(source.path.clone()))
                }
                DependencyOutcome::Unresolved(error) => Some(Err(error.clone())),
                DependencyOutcome::Unreadable(error) => {
                    paths.insert(error.path.clone());
                    failed.insert(error.path.clone(), error);
                    Some(Ok(error.path.clone()))
                }
                DependencyOutcome::InvalidSpan => None,
            };
            if let Some(resolution) = resolution {
                modules.insert(dependency.request.module.clone(), resolution);
            }
        }
        let files = paths
            .into_iter()
            .map(|path| {
                // Seed readable files with the bytes actually checked. If an editor
                // changes them during checking, the next poll sees that change.
                let contents = loaded.get(&identity(&path)).map_or_else(
                    || {
                        let current = file_contents(&path);
                        if let Some(error) = failed.get(&path) {
                            // Keep the failure that was actually reported if a
                            // file recovered between loading and this snapshot.
                            // Invalid UTF-8 still compares its exact bytes while
                            // that same decoding failure remains present.
                            if let FileContents::Bytes(bytes) = &current {
                                if error.kind == io::ErrorKind::InvalidData
                                    && std::str::from_utf8(bytes).is_err()
                                {
                                    return current;
                                }
                            }
                            return FileContents::Error(error.kind, error.message.clone());
                        }
                        current
                    },
                    |bytes| FileContents::Bytes(bytes.to_vec()),
                );
                (path, contents)
            })
            .collect();
        Self {
            discovery: discovery.clone(),
            files,
            modules,
        }
    }

    fn changed(&self, discovery: &DiscoveredInputs, resolver: &Resolver) -> bool {
        self.discovery != *discovery
            || self.files.iter().any(|(path, contents)| file_contents(path) != *contents)
            // Resolve again even for failed requests. A newly created candidate
            // or a higher-priority package.path candidate changes the closure.
            || self.modules.iter().any(|(module, previous)| {
                resolver.resolve(module).map(|resolved| resolved.path) != *previous
            })
    }
}

fn usage_error(format: OutputFormat, message: &str) -> i32 {
    let report = CliReport::failure(HostDiagnostic::general("LUA0000", message));
    report.print(format);
    report.exit_code
}

fn host_error(format: OutputFormat, message: impl ToString) -> i32 {
    let report = CliReport::failure(HostDiagnostic::general("LUA0001", message));
    report.print(format);
    report.exit_code
}

