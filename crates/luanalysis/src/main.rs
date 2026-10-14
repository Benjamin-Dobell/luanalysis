use luanalysis::project_check::check_project;
use luanalysis_project::{
    Project, ProjectConfig, ProjectInput, ProjectLoadPlan, Resolver, SourceRole,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

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
    let mut dump = false;
    let mut files = Vec::new();
    let mut libraries = Vec::new();
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
        if !positional && arg == "--library" {
            let Some(path) = args.next() else {
                eprintln!("luanalysis: --library requires a file or directory path");
                return 2;
            };
            libraries.push(PathBuf::from(path));
            continue;
        }
        if !positional && (arg == "--help" || arg == "-h") {
            help();
            return 0;
        }
        if !positional && arg.to_string_lossy().starts_with('-') {
            eprintln!("luanalysis: unknown option {:?}", arg);
            return 2;
        }
        files.push(PathBuf::from(arg));
    }
    if files.is_empty() && libraries.is_empty() {
        eprintln!("luanalysis: check requires at least one Lua file");
        return 2;
    }

    let mut io_errors = false;
    let mut display_paths = HashMap::new();
    let mut library_files = Vec::new();
    let mut library_seen = HashSet::new();
    for path in libraries {
        for file in expand(&path, &mut io_errors) {
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
        for file in expand(&path, &mut io_errors) {
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

    // Library membership and module search policy are independent. This slice
    // connects named project checking; binder-proven requires and guarded native
    // module/global queries remain their own explicit integration capability.
    let resolver = match Resolver::new(ProjectConfig::new(roots)) {
        Ok(resolver) => resolver,
        Err(error) => {
            eprintln!("luanalysis: error[LUA0001]: {error}");
            return 2;
        }
    };
    let project = Project::load_plan(
        &resolver,
        ProjectLoadPlan {
            entries,
            libraries: library_files,
        },
        |_| Vec::new(),
    );
    let result = check_project(project);
    for error in result
        .project
        .entry_errors
        .iter()
        .chain(&result.project.library_errors)
    {
        eprintln!(
            "{}: error[LUA0001]: {}",
            error.path.display(),
            error.message
        );
        io_errors = true;
    }
    for conflict in &result.project.role_conflicts {
        eprintln!(
            "{}: error[LUA0002]: {conflict}",
            conflict.requested_path.display()
        );
        io_errors = true;
    }
    for file in &result.files {
        let source = result
            .source(file.file)
            .expect("report source belongs to this project");
        let path = display_paths.get(&source.path).unwrap_or(&source.path);
        for error in &file.report.diagnostics {
            let (line, column) = luanalysis::location(&source.source, error.span);
            eprintln!(
                "{}:{line}:{column}: error[{}]: {}",
                path.display(),
                error.code,
                error.message
            );
        }
        if dump {
            for binding in &file.report.bindings {
                let (line, column) = luanalysis::location(&source.source, binding.span);
                println!(
                    "{}:{line}:{column}: {}: {}",
                    path.display(),
                    binding.name,
                    binding.ty
                );
            }
        }
    }
    if io_errors {
        2
    } else if result.has_errors() {
        1
    } else {
        println!("No type errors in {} file(s).", result.project.files.len());
        0
    }
}

fn identity(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn expand(path: &Path, io_errors: &mut bool) -> Vec<PathBuf> {
    if !path.is_dir() {
        return vec![path.to_path_buf()];
    }
    let mut config = ProjectConfig::new([path.to_path_buf()]);
    config.discovery_extensions.push("ttslua".into());
    let resolver = match Resolver::new(config) {
        Ok(resolver) => resolver,
        Err(error) => {
            eprintln!("{}: error[LUA0001]: {error}", path.display());
            *io_errors = true;
            return Vec::new();
        }
    };
    let discovery = resolver.discover();
    for error in discovery.errors {
        eprintln!(
            "{}: error[LUA0001]: {}",
            error.path.display(),
            error.message
        );
        *io_errors = true;
    }
    discovery.files
}

fn help() {
    println!(
        "luanalysis check [--dump-types] [--library <file|directory>]... <file.lua|directory>...\n\nStandalone Lua/Luanalysis type checking in one project session.\nDirectories discover .lua and .ttslua files recursively.\n--library includes original declaration files independently of module search roots.\nReports diagnostics at original Lua source locations."
    );
}
