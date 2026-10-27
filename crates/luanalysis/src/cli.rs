//! The command line's options and input discovery, shared by the `luanalysis` binary, the
//! language server and the C library (`luanalysis-ffi`).
use crate::incremental::ProjectWorkspace;
use crate::project_check::ProjectCheckOptions;
use crate::report::{CliReport, HostDiagnostic};
use crate::standard_environment::{StandardEnvironmentProfile, StandardLibraryAllowList};
use luanalysis_engine::NativeLoaderPolicy;
use luanalysis_project::{IgnoreRules, Project, ProjectConfig, ProjectInput, ProjectLoadPlan, Resolver, SourceRole};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The options of `check` and `lsp`.
pub struct Arguments {
    pub dump: bool,
    pub watch: bool,
    pub inputs: CheckInputs,
    pub options: ProjectCheckOptions,
}

pub enum Parsed {
    Run(Arguments),
    Help,
}

pub fn parse_arguments(remaining: Vec<OsString>) -> Result<Parsed, String> {
    let mut args = remaining.into_iter();
    let mut dump = false;
    let mut watch = false;
    let mut jobs = 0;
    let mut files = Vec::new();
    let mut libraries = Vec::new();
    let mut module_roots = Vec::new();
    let mut package_path = None;
    let mut path_strict = None;
    let mut module_map = None;
    let mut lua_version_chosen = false;
    let mut environment = StandardEnvironmentProfile::LegacyMinimal;
    let mut standard_library: Option<StandardLibraryAllowList> = None;
    let mut no_stdlib = false;
    let mut overload_mode = crate::OverloadMode::default();
    let mut no_implicit_any = false;
    let mut strict_nil = true;
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
                return Err("--jobs requires a positive integer".into());
            };
            jobs = count;
            continue;
        }
        if !positional && arg == "--legacy-overloads" {
            overload_mode = crate::OverloadMode::LegacyExposeImplementation;
            continue;
        }
        if !positional && arg == "--no-implicit-any" {
            no_implicit_any = true;
            continue;
        }
        if !positional && arg == "--no-strict-nil" {
            strict_nil = false;
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
                return Err("--format requires text or json".into());
            }
            continue;
        }
        if !positional && arg == "--library" {
            let Some(path) = args.next() else {
                return Err("--library requires a file or directory path".into());
            };
            libraries.push(PathBuf::from(path));
            continue;
        }
        if !positional && arg == "--no-stdlib" {
            no_stdlib = true;
            continue;
        }
        if !positional && arg == "--stdlib" {
            let Some(list) = args.next().and_then(|value| value.into_string().ok()) else {
                return Err("--stdlib requires a comma-separated list of standard libraries and members".into());
            };
            let mut entries = standard_library.take().map(|list| list.entries().to_vec()).unwrap_or_default();
            entries.extend(StandardLibraryAllowList::parse(&list).entries().iter().cloned());
            standard_library = Some(StandardLibraryAllowList::new(entries));
            continue;
        }
        if !positional && arg == "--lua-version" {
            let Some(version) = args.next() else {
                return Err("--lua-version requires a supported version (5.0, 5.1, 5.2, 5.3 or 5.4)".into());
            };
            environment = version.to_str().and_then(lua_version).ok_or_else(|| format!(
                "Lua standard environment {:?} is not implemented; supported versions: 5.0, 5.1, 5.2, 5.3, 5.4",
                version
            ))?;
            lua_version_chosen = true;
            continue;
        }
        if !positional && arg == "--module-root" {
            let Some(path) = args.next() else {
                return Err("--module-root requires a directory path".into());
            };
            module_roots.push(PathBuf::from(path));
            continue;
        }
        if !positional && arg == "--module-map" {
            let Some(path) = args.next() else {
                return Err("--module-map requires a JSON manifest path".into());
            };
            if module_map.replace(PathBuf::from(path)).is_some() {
                return Err("--module-map may only be given once".into());
            }
            continue;
        }
        if !positional && arg == "--path-strict" {
            path_strict = Some(true);
            continue;
        }
        if !positional && arg == "--package-path" {
            let Some(path) = args.next().and_then(|value| value.into_string().ok()) else {
                return Err("--package-path requires UTF-8 package.path templates".into());
            };
            package_path = Some(path);
            continue;
        }
        if !positional && (arg == "--help" || arg == "-h") {
            return Ok(Parsed::Help);
        }
        if !positional && arg.to_string_lossy().starts_with('-') {
            return Err(format!("unknown option {:?}", arg));
        }
        files.push(PathBuf::from(arg));
    }

    // Without the standard library, a host declares its own; `--stdlib` adds parts back.
    if no_stdlib && standard_library.is_none() {
        standard_library = Some(StandardLibraryAllowList::default());
    }
    if standard_library.is_some() && !environment.is_full_lua() {
        environment = StandardEnvironmentProfile::Lua54;
    }
    if let Some(list) = &standard_library {
        list.validate(environment).map_err(|error| format!("--stdlib: {error}"))?;
    }
    let inputs = CheckInputs {
        files,
        libraries,
        module_roots,
        package_path,
        path_strict,
        module_map,
        lua_version_chosen,
        ..CheckInputs::default()
    };
    let loader_policy = loader_policy(environment);
    let options = ProjectCheckOptions {
        environment,
        standard_library,
        overload_mode,
        no_implicit_any,
        strict_nil,
        loader_policy,
        assume_unknown_standard_effects,
        jobs,
        describe_types: dump,
        inspections: false,
        cancellation: Default::default(),
    };
    Ok(Parsed::Run(Arguments { dump, watch, inputs, options }))
}

/// The standard environment of a `--lua-version`.
pub(crate) fn lua_version(version: &str) -> Option<StandardEnvironmentProfile> {
    Some(match version {
        "5.0" => StandardEnvironmentProfile::Lua50,
        "5.1" => StandardEnvironmentProfile::Lua51,
        "5.2" => StandardEnvironmentProfile::Lua52,
        "5.3" => StandardEnvironmentProfile::Lua53,
        "5.4" => StandardEnvironmentProfile::Lua54,
        _ => return None,
    })
}

/// How `require` loads files in a standard environment.
pub(crate) fn loader_policy(environment: StandardEnvironmentProfile) -> NativeLoaderPolicy {
    match environment {
        StandardEnvironmentProfile::Lua50 => NativeLoaderPolicy::Lua50StandardFile,
        StandardEnvironmentProfile::Lua51 => NativeLoaderPolicy::Lua51StandardFile,
        StandardEnvironmentProfile::Lua52 => NativeLoaderPolicy::Lua52StandardFile,
        StandardEnvironmentProfile::Lua53 => NativeLoaderPolicy::Lua53StandardFile,
        StandardEnvironmentProfile::Lua54
        | StandardEnvironmentProfile::LegacyMinimal
        | StandardEnvironmentProfile::None => NativeLoaderPolicy::Lua54StandardFile,
    }
}

/// The paths and module search settings of a check.
#[derive(Clone, Default)]
pub struct CheckInputs {
    pub files: Vec<PathBuf>,
    pub libraries: Vec<PathBuf>,
    pub module_roots: Vec<PathBuf>,
    pub package_path: Option<String>,
    /// Leaves a module that no module root provides unresolved, instead of finding it among the
    /// checked and library files by its path's suffix (LuaLS's `runtime.pathStrict`); None
    /// leaves it to the project configuration, which by default finds it.
    pub path_strict: Option<bool>,
    /// A host's module map manifest (`--module-map`), read again by each discovery.
    pub module_map: Option<PathBuf>,
    /// Whether the options chose the Lua version, which a project configuration then leaves.
    pub lua_version_chosen: bool,
    /// What discovery skips in a directory input, as its project configuration says. Other
    /// directories skip what LuaLS does by default.
    pub ignore: Vec<(PathBuf, IgnoreRules)>,
    /// Problems with project configurations, reported with discovery's.
    pub config_errors: Vec<HostDiagnostic>,
}

/// The `package.path` templates of a check that sets none.
const DEFAULT_PACKAGE_PATH: &str = "?.lua;?/init.lua;?.ttslua;?/init.ttslua";

/// The files `CheckInputs` name, and how their modules resolve.
#[derive(Clone, PartialEq, Eq)]
pub struct DiscoveredInputs {
    pub plan: ProjectLoadPlan,
    pub display_paths: HashMap<PathBuf, PathBuf>,
    pub roots: Vec<PathBuf>,
    pub errors: Vec<HostDiagnostic>,
    /// The host's module map, as its manifest says at discovery.
    pub module_map: Option<luanalysis_project::ModuleMap>,
}

impl CheckInputs {
    /// Applies each checked directory's LuaLS project configuration (`.luarc.json`) where these
    /// inputs and `options` do not decide.
    pub fn configure(&mut self, options: &mut ProjectCheckOptions) {
        crate::luarc::apply(self, options);
    }

    /// What discovery skips in `directory`.
    fn ignore_rules(&self, directory: &Path) -> IgnoreRules {
        self.ignore.iter().find(|(path, _)| path == directory).map_or_else(crate::luarc::default_ignore, |(_, rules)| rules.clone())
    }

    pub fn discover(&self) -> io::Result<DiscoveredInputs> {
        let mut host_errors = self.config_errors.clone();
        let mut display_paths = HashMap::new();
        let mut library_files = Vec::new();
        let mut library_seen = HashSet::new();
        for path in &self.libraries {
            for file in expand(path, self.ignore_rules(path), &mut host_errors) {
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
            for file in expand(path, self.ignore_rules(path), &mut host_errors) {
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
            module_map: self.module_map.as_deref().map(crate::module_map::read).transpose()?,
        })
    }

    pub fn resolver(&self, discovery: &DiscoveredInputs) -> io::Result<Resolver> {
        let mut config = ProjectConfig::new(discovery.roots.clone());
        config.set_package_path(self.package_path.as_deref().unwrap_or(DEFAULT_PACKAGE_PATH));
        config.module_map = discovery.module_map.clone();
        // As Luanalysis v1 and LuaLS find a module, one that no root provides is the checked or
        // library file whose path ends with its own.
        if !self.path_strict.unwrap_or(false) {
            config.suffix_files = discovery.display_paths.keys().cloned().collect();
        }
        Resolver::new(config)
    }
}

/// Checks once, as `luanalysis check --format json <arguments>` does, and returns the JSON
/// report (its `exitCode` is the command's). `--watch` is not available here.
pub fn check_json(arguments: Vec<OsString>) -> String {
    let report = match parse_arguments(arguments) {
        Ok(Parsed::Run(arguments)) if arguments.watch => CliReport::failure(HostDiagnostic::general("LUA0000", "--watch needs the command line")),
        Ok(Parsed::Run(arguments)) if arguments.inputs.files.is_empty() && arguments.inputs.libraries.is_empty() => {
            CliReport::failure(HostDiagnostic::general("LUA0000", "check requires at least one Lua file"))
        }
        Ok(Parsed::Run(mut arguments)) => {
            arguments.inputs.configure(&mut arguments.options);
            match check_once(&arguments, &mut ProjectWorkspace::new()) {
                Ok(report) => report,
                Err(error) => CliReport::failure(HostDiagnostic::general("LUA0001", error)),
            }
        }
        Ok(Parsed::Help) => CliReport::failure(HostDiagnostic::general("LUA0000", "--help needs the command line")),
        Err(message) => CliReport::failure(HostDiagnostic::general("LUA0000", message)),
    };
    report.to_json()
}

/// One check of `arguments`' inputs.
pub fn check_once(arguments: &Arguments, workspace: &mut ProjectWorkspace) -> io::Result<CliReport> {
    let discovery = arguments.inputs.discover()?;
    let resolver = arguments.inputs.resolver(&discovery)?;
    let project = Project::load_plan(&resolver, discovery.plan.clone(), |source| workspace.static_require_requests(source));
    let result = workspace.check_project(project, arguments.options.clone());
    Ok(CliReport::from_project(&result, &discovery.display_paths, discovery.errors.clone(), arguments.dump))
}

pub fn identity(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn expand(path: &Path, ignore: IgnoreRules, errors: &mut Vec<HostDiagnostic>) -> Vec<PathBuf> {
    if !path.is_dir() {
        return vec![path.to_path_buf()];
    }
    let mut config = ProjectConfig::new([path.to_path_buf()]);
    config.discovery_extensions.push("ttslua".into());
    config.ignore = ignore;
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

/// The command line's help text.
pub fn help() {
    println!(
        "luanalysis check [--format text|json] [--dump-types] [--watch] [--jobs <count>] [--legacy-overloads] [--no-implicit-any] [--lua-version 5.0|5.1|5.2|5.3|5.4] [--stdlib <libraries and members>]... [--no-stdlib] [--library <file|directory>]... [--module-root <directory>]... [--package-path <templates>] [--path-strict] [--module-map <manifest.json>] <file.lua|directory>...\n\nStandalone Lua/Luanalysis type checking in one project session.\nDirectories discover .lua and .ttslua files recursively, skipping .vscode and what .gitignore files ignore.\nA directory's LuaLS .luarc.json (or .luarc.jsonc) supplies runtime.version, runtime.path, runtime.pathStrict, workspace.library, workspace.ignoreDir and workspace.useGitIgnore where options do not.\n--watch checks on source, declaration, discovery or module-resolution changes (250 ms polling).\n--watch --format json streams one complete report per revision as NDJSON.\n--jobs selects a positive worker count; omitted uses available parallelism.\n--lua-version selects the matching Lua standard declaration set (5.0 through 5.4).\n--stdlib declares only the named standard libraries and members, as a sandbox provides them, e.g. basic,string,table,math,os.time (repeatable).\n--no-stdlib declares none of the standard library, for a host that declares its own, except type, assert, setmetatable and the builtin types; --stdlib adds parts back.\nWithout --lua-version, --stdlib and --no-stdlib use Lua 5.4.\nThe standard profile is opt-in; default checking retains legacy compatibility.\n--legacy-overloads exposes implementation signatures alongside explicit overloads (v1 compatibility).\n--no-implicit-any requires written or contextual parameter/rest types; ordinary checking permits unannotated functions.\n--library includes original declaration files independently of module search roots.\n--module-root sets ordered require roots; --package-path sets package.path templates.\nA module no root provides is the checked or library file whose path ends with a template's path for it, the shortest first; --path-strict leaves it unresolved.\n--module-map supplies a host's closed module namespace: each source's modules by their exact names (version 1 JSON; paths relative to the manifest). An unmapped module is unresolved, never found by package.path.\nDefault require policy is standard Lua 5.4.\nReports diagnostics at original Lua source locations.\n--format json writes one structured report to stdout, including original UTF-8 byte ranges."
    );
    println!(
        "Selected standard profiles preserve reached builtin/environment bindings across unknown calls and property effects; known assignments still apply.\n--strict-standard-effects requires runtime-effect preservation proofs instead."
    );
    println!("--no-strict-nil permits nil assignments as in Luanalysis v1; nil remains represented for runtime flow and table presence.");
    println!(
        "\nluanalysis lsp [check options] [<file.lua|directory>...]\n\nA language server over stdio: diagnostics, hover and go-to-definition. It accepts the options of check; without paths it checks the editor's workspace root. Each revision of the editor's buffers is checked as one project."
    );
}
