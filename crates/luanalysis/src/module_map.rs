//! A host's module map (`--module-map`): its own, closed module namespace, for a host whose Lua
//! modules are not files that package.path finds, such as a game's virtual scripts. It names
//! each importing source's modules by their exact names; parsing, binding, loader recognition
//! and source positions are unchanged.
//!
//! ```json
//! {"version": 1, "sources": [{"file": "main.lua", "imports": {"Case": "a.lua", "case": "b.lua"}}]}
//! ```
//!
//! Paths are relative to the manifest's directory, or absolute, and must name files. A module's
//! name is its exact, case-sensitive Lua string. Duplicates, unknown fields and other versions
//! are refused.
use crate::json::Json;
use luanalysis_project::ModuleMap;
use std::{collections::{HashMap, HashSet}, fs, io, path::{Path, PathBuf}};

/// The module map in the manifest at `path`.
pub(crate) fn read(path: &Path) -> io::Result<ModuleMap> {
    let text = fs::read_to_string(path)?;
    parse(path, &text).map_err(|message| io::Error::new(io::ErrorKind::InvalidData, format!("module map {}: {message}", path.display())))
}

fn parse(path: &Path, text: &str) -> Result<ModuleMap, String> {
    let value = Json::parse(text).map_err(|error| error.to_string())?;
    fields(&value, &["version", "sources"])?;
    if value.get("version").and_then(Json::as_u64) != Some(1) {
        return Err("version must be 1".into());
    }
    let sources = value.get("sources").and_then(Json::as_array).ok_or("sources must be an array")?;
    let manifest = std::path::absolute(path).map_err(|error| error.to_string())?;
    let directory = manifest.parent().ok_or("the manifest has no directory")?;
    let mut map = ModuleMap::new();
    for source in sources {
        fields(source, &["file", "imports"])?;
        let file = file_path(directory, source.get("file").and_then(Json::as_str).ok_or("a source's file must be a string")?)?;
        let Some(Json::Object(imports)) = source.get("imports") else { return Err("a source's imports must be an object".into()) };
        let mut targets = HashMap::new();
        for (module, target) in imports {
            if module.is_empty() || module.contains('\0') {
                return Err("module names must be nonempty and contain no NUL".into());
            }
            let target = file_path(directory, target.as_str().ok_or("a module's file must be a string")?)?;
            if targets.insert(module.clone(), target).is_some() {
                return Err(format!("{} imports {module:?} twice", file.display()));
            }
        }
        if map.insert(file.clone(), targets).is_some() {
            return Err(format!("{} is listed twice", file.display()));
        }
    }
    Ok(map)
}

fn file_path(directory: &Path, value: &str) -> Result<PathBuf, String> {
    if value.is_empty() || value.contains('\0') {
        return Err("file paths must be nonempty and contain no NUL".into());
    }
    Ok(directory.join(value))
}

/// Whether `value` is an object with exactly the fields `allowed`, each once.
fn fields(value: &Json, allowed: &[&str]) -> Result<(), String> {
    let Json::Object(fields) = value else { return Err("expected an object".into()) };
    let mut seen = HashSet::new();
    for (name, _) in fields {
        if !allowed.contains(&name.as_str()) { return Err(format!("unknown field {name:?}")); }
        if !seen.insert(name.as_str()) { return Err(format!("duplicate field {name:?}")); }
    }
    match allowed.iter().find(|name| !seen.contains(*name)) {
        Some(name) => Err(format!("missing field {name:?}")),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_maps_exact_names_and_refuses_what_is_ambiguous() {
        let path = Path::new("/project/map.json");
        let map = parse(path, r#"{"version":1,"sources":[{"file":"main.lua","imports":{"Case":"a.lua","case":"b.lua","CON":"/abs/c.lua"}}]}"#).unwrap();
        // Absolute as the platform makes it: on Windows, on the current drive.
        let project = std::path::absolute("/project").unwrap();
        let imports = &map[&project.join("main.lua")];
        assert_eq!(imports["Case"], project.join("a.lua"));
        assert_eq!(imports["case"], project.join("b.lua"));
        assert_eq!(imports["CON"], std::path::absolute("/abs/c.lua").unwrap());
        for text in [
            r#"{}"#,
            r#"{"version":2,"sources":[]}"#,
            r#"{"version":1,"version":1,"sources":[]}"#,
            r#"{"version":1,"sources":[],"extra":[]}"#,
            r#"{"version":1,"sources":[{"file":"main.lua","imports":{"x":"a.lua","x":"b.lua"}}]}"#,
            r#"{"version":1,"sources":[{"file":"main.lua","imports":{}},{"file":"main.lua","imports":{}}]}"#,
            r#"{"version":1,"sources":[{"file":"main.lua","imports":{"":"a.lua"}}]}"#,
            r#"{"version":1,"sources":[{"file":"","imports":{}}]}"#,
        ] {
            assert!(parse(path, text).is_err(), "{text}");
        }
    }
}
