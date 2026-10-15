use luanalysis::project_check::{ProjectCheckOptions, check_project_with_options, static_require_requests};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{
    Project, ProjectConfig, ProjectInput, ProjectLoadPlan, Resolver, SourceRole,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
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
    let mut files = Vec::new();
    let mut libraries = Vec::new();
    let mut module_roots = Vec::new();
    let mut package_path = None;
    let mut environment = StandardEnvironmentProfile::LegacyMinimal;
    let mut overload_mode = luanalysis::OverloadMode::default();
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
        if !positional && arg == "--legacy-overloads" {
            overload_mode = luanalysis::OverloadMode::LegacyExposeImplementation;
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
                return usage_error(format, "--lua-version requires a supported version (5.4)");
            };
            if version != "5.4" {
                return usage_error(format, &format!("Lua standard environment {:?} is not implemented; supported version: 5.4", version));
            }
            environment = StandardEnvironmentProfile::Lua54;
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

    let mut host_errors = Vec::new();
    let mut display_paths = HashMap::new();
    let mut library_files = Vec::new();
    let mut library_seen = HashSet::new();
    for path in libraries {
        for file in expand(&path, &mut host_errors) {
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
    for path in files {
        let directory = path.is_dir();
        if directory {
            roots.push(path.clone());
        }
        for file in expand(&path, &mut host_errors) {
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
    let roots = if !module_roots.is_empty() {
        module_roots
    } else if !roots.is_empty() {
        roots
    } else {
        match std::env::current_dir() {
            Ok(root) => vec![root],
            Err(error) => {
                return host_error(format, error);
            }
        }
    };
    let mut config = ProjectConfig::new(roots);
    config.set_package_path(
        package_path
            .as_deref()
            .unwrap_or("?.lua;?/init.lua;?.ttslua;?/init.ttslua"),
    );
    let resolver = match Resolver::new(config) {
        Ok(resolver) => resolver,
        Err(error) => {
            return host_error(format, error);
        }
    };
    let project = Project::load_plan(
        &resolver,
        ProjectLoadPlan {
            entries,
            libraries: library_files,
        },
        static_require_requests,
    );
    let result = check_project_with_options(project, ProjectCheckOptions { environment, overload_mode, ..ProjectCheckOptions::default() });
    let report = CliReport::from_project(&result, &display_paths, host_errors, dump);
    report.print(format);
    report.exit_code
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
        "luanalysis check [--format text|json] [--dump-types] [--legacy-overloads] [--lua-version 5.4 | --no-stdlib] [--library <file|directory>]... [--module-root <directory>]... [--package-path <templates>] <file.lua|directory>...\n\nStandalone Lua/Luanalysis type checking in one project session.\nDirectories discover .lua and .ttslua files recursively.\n--lua-version 5.4 selects the complete maintained Lua 5.4 declaration set.\n--no-stdlib disables standard declarations and the legacy minimal type builtin.\nThe standard profile is opt-in; default checking retains legacy compatibility.\n--legacy-overloads exposes implementation signatures alongside explicit overloads (v1 compatibility).\n--library includes original declaration files independently of module search roots.\n--module-root sets ordered require roots; --package-path sets package.path templates.\nDefault require policy is standard Lua 5.4.\nReports diagnostics at original Lua source locations.\n--format json writes one structured report to stdout, including original UTF-8 byte ranges."
    );
}
