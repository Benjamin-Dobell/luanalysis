//! LuaLS's project configuration: a checked directory's `.luarc.json`, or `.luarc.jsonc`, in
//! LuaLS's flat (`"runtime.version"`) or nested (`{ "runtime": { "version": … } }`) form, with
//! or without its `Lua.` prefix. Luanalysis reads `runtime.version`, `runtime.path`,
//! `runtime.pathStrict`, `workspace.library`, `workspace.ignoreDir` and
//! `workspace.useGitIgnore`; LuaLS's other keys are its own. Options a check is given override
//! what a configuration says.
use crate::cli::CheckInputs;
use crate::json::Json;
use crate::project_check::ProjectCheckOptions;
use crate::report::HostDiagnostic;
use crate::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::IgnoreRules;
use std::path::{Path, PathBuf};

/// The configuration's file names, in the order LuaLS looks for them.
pub(crate) const FILE_NAMES: [&str; 2] = [".luarc.json", ".luarc.jsonc"];

/// What one configuration says. Relative paths are its directory's.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct LuaRc {
    pub version: Option<StandardEnvironmentProfile>,
    pub templates: Option<Vec<String>>,
    pub path_strict: Option<bool>,
    pub libraries: Vec<PathBuf>,
    pub ignore_dir: Option<Vec<String>>,
    pub use_git_ignore: Option<bool>,
    /// What is wrong with it: what it could not read is left unsaid.
    pub errors: Vec<String>,
}

/// What a directory's discovery skips without a configuration that says otherwise, as LuaLS
/// skips it: `.vscode`, and what Git ignores.
pub(crate) fn default_ignore() -> IgnoreRules {
    IgnoreRules { patterns: vec![".vscode".into()], git_ignore: true }
}

impl LuaRc {
    fn ignore(&self) -> IgnoreRules {
        let defaults = default_ignore();
        IgnoreRules {
            patterns: self.ignore_dir.clone().unwrap_or(defaults.patterns),
            git_ignore: self.use_git_ignore.unwrap_or(defaults.git_ignore),
        }
    }
}

/// Applies each checked directory's configuration where the options do not decide: the first
/// directory whose configuration gives a Lua version, `package.path` templates or path
/// strictness gives them; every configuration's libraries are read, and each directory and
/// each library a configuration names skips what that configuration ignores. Problems are
/// `inputs.config_errors`.
pub(crate) fn apply(inputs: &mut CheckInputs, options: &mut ProjectCheckOptions) {
    let mut version = None;
    let mut templates = None;
    let mut path_strict = None;
    let directories: Vec<PathBuf> = inputs.files.iter().filter(|path| path.is_dir()).cloned().collect();
    for directory in directories {
        let Some((path, config)) = read(&directory) else { continue };
        for error in &config.errors {
            inputs.config_errors.push(HostDiagnostic::new(&path, "LUA0001", error));
        }
        let ignore = config.ignore();
        for library in &config.libraries {
            if !inputs.libraries.contains(library) {
                inputs.libraries.push(library.clone());
                inputs.ignore.push((library.clone(), ignore.clone()));
            }
        }
        inputs.ignore.push((directory, ignore));
        version = version.or(config.version);
        templates = templates.or(config.templates);
        path_strict = path_strict.or(config.path_strict);
    }
    if !inputs.lua_version_chosen && let Some(version) = version {
        options.environment = version;
        options.loader_policy = crate::cli::loader_policy(version);
        if let Some(list) = &options.standard_library
            && let Err(error) = list.validate(version)
        {
            inputs.config_errors.push(HostDiagnostic::general("LUA0001", format!("runtime.version: --stdlib: {error}")));
        }
    }
    if inputs.package_path.is_none() {
        inputs.package_path = templates.map(|templates| templates.join(";"));
    }
    if inputs.path_strict.is_none() {
        inputs.path_strict = path_strict;
    }
}

/// The configuration in `directory`, if it has one, and its path.
pub(crate) fn read(directory: &Path) -> Option<(PathBuf, LuaRc)> {
    let path = FILE_NAMES.iter().map(|name| directory.join(name)).find(|path| path.is_file())?;
    let config = match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text, directory),
        Err(error) => LuaRc { errors: vec![error.to_string()], ..LuaRc::default() },
    };
    Some((path, config))
}

/// A configuration's text, in `directory`.
pub(crate) fn parse(text: &str, directory: &Path) -> LuaRc {
    let mut config = LuaRc::default();
    let value = match Json::parse_with_comments(text) {
        Ok(value @ Json::Object(_)) => value,
        Ok(_) => {
            config.errors.push("a configuration is a JSON object".into());
            return config;
        }
        Err(error) => {
            config.errors.push(error);
            return config;
        }
    };
    let mut settings = Vec::new();
    flatten("", &value, &mut settings);
    // A key given twice, flat and nested, has its last value, as LuaLS reads it.
    let setting = |key: &str| settings.iter().rev().find(|(name, _)| name == key).map(|(_, value)| *value);
    let mut error = |key: &str, expected: &str| config.errors.push(format!("{key} must be {expected}"));
    let boolean = |key: &str, error: &mut dyn FnMut(&str, &str)| match setting(key) {
        Some(Json::Bool(value)) => Some(*value),
        Some(_) => {
            error(key, "true or false");
            None
        }
        None => None,
    };
    let strings = |key: &str, error: &mut dyn FnMut(&str, &str)| match setting(key) {
        Some(Json::Array(values)) if values.iter().all(|value| value.as_str().is_some()) => {
            Some(values.iter().filter_map(Json::as_str).map(str::to_owned).collect::<Vec<_>>())
        }
        Some(Json::String(value)) => Some(value.split(';').filter(|part| !part.is_empty()).map(str::to_owned).collect()),
        Some(_) => {
            error(key, "an array of strings");
            None
        }
        None => None,
    };
    let version = match setting("runtime.version") {
        Some(Json::String(name)) => match lua_version(name) {
            Some(version) => Some(version),
            None => {
                error("runtime.version", "\"Lua 5.1\", \"Lua 5.2\", \"Lua 5.3\", \"Lua 5.4\", \"Lua 5.5\" or \"LuaJIT\"");
                None
            }
        },
        Some(_) => {
            error("runtime.version", "a string");
            None
        }
        None => None,
    };
    let templates = strings("runtime.path", &mut error).map(|templates| with_lua_extensions(&templates));
    let path_strict = boolean("runtime.pathStrict", &mut error);
    let libraries = match setting("workspace.library") {
        // LuaLS's older form: each path, and whether it is a library.
        Some(Json::Object(entries)) => Some(entries.iter().filter(|(_, value)| *value == Json::Bool(true)).map(|(path, _)| path.clone()).collect()),
        _ => strings("workspace.library", &mut error),
    };
    let ignore_dir = strings("workspace.ignoreDir", &mut error);
    let use_git_ignore = boolean("workspace.useGitIgnore", &mut error);
    config.version = version;
    config.templates = templates;
    config.path_strict = path_strict;
    config.libraries = libraries.unwrap_or_default().iter().filter_map(|library| library_path(library, directory)).collect();
    config.ignore_dir = ignore_dir;
    config.use_git_ignore = use_git_ignore;
    config
}

/// Each setting by its flat name, without LuaLS's `Lua.` prefix. A library's older object form
/// is its value, not more settings.
fn flatten<'a>(prefix: &str, value: &'a Json, settings: &mut Vec<(String, &'a Json)>) {
    let Json::Object(entries) = value else { return };
    for (key, value) in entries {
        let key = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
        let name = key.strip_prefix("Lua.").unwrap_or(&key);
        if matches!(value, Json::Object(_)) && name != "workspace.library" {
            flatten(&key, value, settings);
        } else {
            settings.push((name.to_owned(), value));
        }
    }
}

/// The standard environment of a LuaLS `runtime.version`. Lua 5.5 has Lua 5.4's library
/// declarations, and LuaJIT Lua 5.1's.
fn lua_version(name: &str) -> Option<StandardEnvironmentProfile> {
    Some(match name {
        "Lua 5.1" | "LuaJIT" => StandardEnvironmentProfile::Lua51,
        "Lua 5.2" => StandardEnvironmentProfile::Lua52,
        "Lua 5.3" => StandardEnvironmentProfile::Lua53,
        "Lua 5.4" | "Lua 5.5" => StandardEnvironmentProfile::Lua54,
        _ => return None,
    })
}

/// LuaLS's `runtime.path` templates, then, for those that name `.lua` files, the same names as
/// `.ttslua` files, which Luanalysis also checks as Lua.
fn with_lua_extensions(templates: &[String]) -> Vec<String> {
    let mut all = templates.to_vec();
    for template in templates {
        if let Some(stem) = template.strip_suffix(".lua") {
            let tts = format!("{stem}.ttslua");
            if !all.contains(&tts) {
                all.push(tts);
            }
        }
    }
    all
}

/// A `workspace.library` entry's directory or file: `~` is the home directory, and a relative
/// path is the configuration's. LuaLS's own `${…}` placeholders and paths that do not exist
/// name nothing here.
fn library_path(library: &str, directory: &Path) -> Option<PathBuf> {
    if library.contains("${") {
        return None;
    }
    let path = match library.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?).join(rest.trim_start_matches(['/', '\\']))
        }
        _ => directory.join(library),
    };
    path.exists().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_nested_and_prefixed_keys_are_the_same_settings() {
        let directory = std::env::temp_dir();
        let flat = parse(r#"{ "runtime.version": "Lua 5.2", "runtime.path": ["src/?.lua"], "runtime.pathStrict": true,
            "workspace.ignoreDir": ["/build/"], "workspace.useGitIgnore": false }"#, &directory);
        let nested = parse(r#"{ "runtime": { "version": "Lua 5.2", "path": ["src/?.lua"], "pathStrict": true },
            "workspace": { "ignoreDir": ["/build/"], "useGitIgnore": false } }"#, &directory);
        let prefixed = parse(r#"// LuaLS
            { "Lua": { "runtime.version": "Lua 5.2", "runtime": { "path": "src/?.lua", "pathStrict": true, }, },
              "Lua.workspace.ignoreDir": ["/build/"], "Lua.workspace": { "useGitIgnore": false } }"#, &directory);
        assert_eq!(flat, nested);
        assert_eq!(flat, prefixed);
        assert_eq!(flat.version, Some(StandardEnvironmentProfile::Lua52));
        assert_eq!(flat.templates, Some(vec!["src/?.lua".to_owned(), "src/?.ttslua".to_owned()]));
        assert_eq!(flat.path_strict, Some(true));
        assert_eq!(flat.ignore(), IgnoreRules { patterns: vec!["/build/".into()], git_ignore: false });
        assert!(flat.errors.is_empty(), "{:?}", flat.errors);
    }

    #[test]
    fn defaults_unknown_keys_and_wrong_values() {
        let directory = std::env::temp_dir();
        let empty = parse(r#"{ "$schema": "x", "diagnostics": { "disable": ["a"] }, "runtime.special": { "import": "require" } }"#, &directory);
        assert_eq!(empty, LuaRc::default());
        assert_eq!(empty.ignore(), default_ignore());
        let wrong = parse(r#"{ "runtime.version": "Lua 6", "runtime.path": [1], "runtime.pathStrict": "yes", "workspace.useGitIgnore": 0 }"#, &directory);
        assert_eq!(wrong.errors, [
            "runtime.version must be \"Lua 5.1\", \"Lua 5.2\", \"Lua 5.3\", \"Lua 5.4\", \"Lua 5.5\" or \"LuaJIT\"",
            "runtime.path must be an array of strings",
            "runtime.pathStrict must be true or false",
            "workspace.useGitIgnore must be true or false",
        ]);
        assert_eq!(parse(r#"{ "runtime.version": "LuaJIT" }"#, &directory).version, Some(StandardEnvironmentProfile::Lua51));
        assert_eq!(parse(r#"{ "runtime.version": "Lua 5.5" }"#, &directory).version, Some(StandardEnvironmentProfile::Lua54));
        assert_eq!(parse("[]", &directory).errors, ["a configuration is a JSON object"]);
        assert_eq!(parse("{ \"a\": }", &directory).errors.len(), 1);
    }

    #[test]
    fn libraries_are_existing_paths_of_the_configuration_or_home() {
        let directory = std::env::temp_dir().join(format!("luanalysis luarc {}", std::process::id()));
        std::fs::create_dir_all(directory.join("types")).unwrap();
        let config = parse(r#"{ "workspace.library": ["types", "missing", "${3rd}/love2d/library"] }"#, &directory);
        assert_eq!(config.libraries, [directory.join("types")]);
        let older = parse(r#"{ "workspace": { "library": { "types": true, "other": false } } }"#, &directory);
        assert_eq!(older.libraries, [directory.join("types")]);
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
