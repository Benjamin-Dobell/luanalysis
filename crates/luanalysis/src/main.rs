use luanalysis::incremental::ProjectWorkspace;
use luanalysis::project_check::ProjectCheckOptions;
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_engine::NativeLoaderPolicy;
use luanalysis_project::{
    DependencyOutcome, Project, ProjectConfig, ProjectInput, ProjectLoadPlan, ResolveError,
    Resolver, SourceOrigin, SourceRole,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
mod cli_output;
use cli_output::{CliReport, HostDiagnostic, OutputFormat};

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
    if command != "check" {
        eprintln!("luanalysis: unknown command {:?}", command);
        help();
        return 2;
    }
    let remaining: Vec<_> = args.collect();
    let format = OutputFormat::requested(&remaining);
    let mut args = remaining.into_iter();
    let mut dump = false;
    let mut watch = false;
    let mut jobs = 0;
    let mut files = Vec::new();
    let mut libraries = Vec::new();
    let mut module_roots = Vec::new();
    let mut package_path = None;
    let mut environment = StandardEnvironmentProfile::LegacyMinimal;
    let mut overload_mode = luanalysis::OverloadMode::default();
    let mut no_implicit_any = false;
    let mut assume_unknown_standard_effects = true;
    let mut positional = false;
    while let Some(arg) = args.next() {
        if !positional && arg == "--" {
            positional = true;
            continue;
        }
        if !positional && arg == "--dump-types" {
            dump = true;
            continue;
        }
        if !positional && arg == "--watch" {
            watch = true;
            continue;
        }
        if !positional && arg == "--jobs" {
            let Some(count) = args.next().and_then(|value| {
                let value = value.to_str()?;
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return None;
                }
                value.parse::<usize>().ok().filter(|count| *count != 0)
            }) else {
                return usage_error(format, "--jobs requires a positive integer");
            };
            jobs = count;
            continue;
        }
        if !positional && arg == "--legacy-overloads" {
            overload_mode = luanalysis::OverloadMode::LegacyExposeImplementation;
            continue;
        }
        if !positional && arg == "--no-implicit-any" {
            no_implicit_any = true;
            continue;
        }
        if !positional && arg == "--strict-standard-effects" {
            assume_unknown_standard_effects = false;
            continue;
        }
        if !positional && arg == "--format" {
            if !matches!(
                args.next().as_deref().and_then(|value| value.to_str()),
                Some("text" | "json")
            ) {
                return usage_error(format, "--format requires text or json");
            }
            continue;
        }
        if !positional && arg == "--library" {
            let Some(path) = args.next() else {
                return usage_error(format, "--library requires a file or directory path");
            };
            libraries.push(PathBuf::from(path));
            continue;
        }
        if !positional && arg == "--no-stdlib" {
            environment = StandardEnvironmentProfile::None;
            continue;
        }
        if !positional && arg == "--lua-version" {
            let Some(version) = args.next() else {
                return usage_error(
                    format,
                    "--lua-version requires a supported version (5.0, 5.1, 5.2, 5.3 or 5.4)",
                );
            };
            environment = match version.to_str() {
                Some("5.0") => StandardEnvironmentProfile::Lua50,
                Some("5.1") => StandardEnvironmentProfile::Lua51,
                Some("5.2") => StandardEnvironmentProfile::Lua52,
                Some("5.3") => StandardEnvironmentProfile::Lua53,
                Some("5.4") => StandardEnvironmentProfile::Lua54,
                _ => {
                    return usage_error(
                        format,
                        &format!(
                            "Lua standard environment {:?} is not implemented; supported versions: 5.0, 5.1, 5.2, 5.3, 5.4",
                            version
                        ),
                    );
                }
            };
            continue;
        }
        if !positional && arg == "--module-root" {
            let Some(path) = args.next() else {
                return usage_error(format, "--module-root requires a directory path");
            };
            module_roots.push(PathBuf::from(path));
            continue;
        }
        if !positional && arg == "--package-path" {
            let Some(path) = args.next().and_then(|value| value.into_string().ok()) else {
                return usage_error(
                    format,
                    "--package-path requires UTF-8 package.path templates",
                );
            };
            package_path = Some(path);
            continue;
        }
        if !positional && (arg == "--help" || arg == "-h") {
            help();
            return 0;
        }
        if !positional && arg.to_string_lossy().starts_with('-') {
            return usage_error(format, &format!("unknown option {:?}", arg));
        }
        files.push(PathBuf::from(arg));
    }
    if files.is_empty() && libraries.is_empty() {
        return usage_error(format, "check requires at least one Lua file");
    }

    let inputs = CheckInputs {
        files,
        libraries,
        module_roots,
        package_path,
    };
    let loader_policy = match environment {
        StandardEnvironmentProfile::Lua50 => NativeLoaderPolicy::Lua50StandardFile,
        StandardEnvironmentProfile::Lua51 => NativeLoaderPolicy::Lua51StandardFile,
        StandardEnvironmentProfile::Lua52 => NativeLoaderPolicy::Lua52StandardFile,
        StandardEnvironmentProfile::Lua53 => NativeLoaderPolicy::Lua53StandardFile,
        StandardEnvironmentProfile::Lua54
        | StandardEnvironmentProfile::LegacyMinimal
        | StandardEnvironmentProfile::None => NativeLoaderPolicy::Lua54StandardFile,
    };
    let options = ProjectCheckOptions {
        environment,
        overload_mode,
        no_implicit_any,
        loader_policy,
        assume_unknown_standard_effects,
        jobs,
    };
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
        let result = workspace.check_project(project, options);
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

struct CheckInputs {
    files: Vec<PathBuf>,
    libraries: Vec<PathBuf>,
    module_roots: Vec<PathBuf>,
    package_path: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
struct DiscoveredInputs {
    plan: ProjectLoadPlan,
    display_paths: HashMap<PathBuf, PathBuf>,
    roots: Vec<PathBuf>,
    errors: Vec<HostDiagnostic>,
}

impl CheckInputs {
    fn discover(&self) -> io::Result<DiscoveredInputs> {
        let mut host_errors = Vec::new();
        let mut display_paths = HashMap::new();
        let mut library_files = Vec::new();
        let mut library_seen = HashSet::new();
        for path in &self.libraries {
            for file in expand(path, &mut host_errors) {
                let identity = identity(&file);
                if library_seen.insert(identity.clone()) {
                    display_paths
                        .entry(identity)
                        .or_insert_with(|| file.clone());
                    library_files.push(ProjectInput::new(file, SourceRole::Declaration));
                }
            }
        }

        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        let mut roots = Vec::new();
        for path in &self.files {
            let directory = path.is_dir();
            if directory {
                roots.push(path.clone());
            }
            for file in expand(path, &mut host_errors) {
                let identity = identity(&file);
                // A declaration directory nested in positional discovery has an
                // explicit role. Direct file arguments still request an explicit
                // implementation role and receive a precise host role conflict.
                if directory && library_seen.contains(&identity) {
                    continue;
                }
                if seen.insert(identity.clone()) {
                    display_paths
                        .entry(identity)
                        .or_insert_with(|| file.clone());
                    entries.push(file);
                }
            }
        }

        // Explicit module roots override positional directory roots. A file-only
        // invocation follows Lua's current-directory package-path convention;
        // declaration libraries never add implicit module search roots.
        let roots = if !self.module_roots.is_empty() {
            self.module_roots.clone()
        } else if !roots.is_empty() {
            roots
        } else {
            vec![std::env::current_dir()?]
        };
        Ok(DiscoveredInputs {
            plan: ProjectLoadPlan {
                entries,
                libraries: library_files,
            },
            display_paths,
            roots,
            errors: host_errors,
        })
    }

    fn resolver(&self, discovery: &DiscoveredInputs) -> io::Result<Resolver> {
        let mut config = ProjectConfig::new(discovery.roots.clone());
        config.set_package_path(
            self.package_path
                .as_deref()
                .unwrap_or("?.lua;?/init.lua;?.ttslua;?/init.ttslua"),
        );
        Resolver::new(config)
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

fn identity(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn expand(path: &Path, errors: &mut Vec<HostDiagnostic>) -> Vec<PathBuf> {
    if !path.is_dir() {
        return vec![path.to_path_buf()];
    }
    let mut config = ProjectConfig::new([path.to_path_buf()]);
    config.discovery_extensions.push("ttslua".into());
    let resolver = match Resolver::new(config) {
        Ok(resolver) => resolver,
        Err(error) => {
            errors.push(HostDiagnostic::new(path, "LUA0001", error));
            return Vec::new();
        }
    };
    let discovery = resolver.discover();
    for error in discovery.errors {
        errors.push(HostDiagnostic::new(error.path, "LUA0001", error.message));
    }
    discovery.files
}

fn help() {
    println!(
        "luanalysis check [--format text|json] [--dump-types] [--watch] [--jobs <count>] [--legacy-overloads] [--no-implicit-any] [--lua-version 5.0|5.1|5.2|5.3|5.4 | --no-stdlib] [--library <file|directory>]... [--module-root <directory>]... [--package-path <templates>] <file.lua|directory>...\n\nStandalone Lua/Luanalysis type checking in one project session.\nDirectories discover .lua and .ttslua files recursively.\n--watch checks on source, declaration, discovery or module-resolution changes (250 ms polling).\n--watch --format json streams one complete report per revision as NDJSON.\n--jobs selects a positive worker count; omitted uses available parallelism.\n--lua-version selects the matching Lua standard declaration set (5.0 through 5.4).\n--no-stdlib disables standard declarations and the legacy minimal type builtin.\nThe standard profile is opt-in; default checking retains legacy compatibility.\n--legacy-overloads exposes implementation signatures alongside explicit overloads (v1 compatibility).\n--no-implicit-any requires written or contextual parameter/rest types; ordinary checking permits unannotated functions.\n--library includes original declaration files independently of module search roots.\n--module-root sets ordered require roots; --package-path sets package.path templates.\nDefault require policy is standard Lua 5.4.\nReports diagnostics at original Lua source locations.\n--format json writes one structured report to stdout, including original UTF-8 byte ranges."
    );
    println!(
        "Selected standard profiles preserve reached builtin/environment bindings across unknown calls and property effects; known assignments still apply.\n--strict-standard-effects requires runtime-effect preservation proofs instead."
    );
}
