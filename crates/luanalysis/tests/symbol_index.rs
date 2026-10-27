//! Library callers find a name's definitions and occurrences, and rename it, from a checked
//! project's `SymbolIndex`: locals in their own file, globals in every file.
use luanalysis::{
    project_check::{ProjectCheckOptions, check_project_with_options, static_require_requests},
    standard_environment::StandardEnvironmentProfile,
    symbols::{RenameError, Symbol, SymbolIndex},
};
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{fs, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);
impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("luanalysis symbol index {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        for (path, source) in files {
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join(path), source).unwrap();
        }
        Self(fs::canonicalize(root).unwrap())
    }
    fn index(&self, entries: &[&str]) -> SymbolIndex {
        let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
        let plan = ProjectLoadPlan::new(entries.iter().map(|entry| self.0.join(entry)));
        let report = check_project_with_options(Project::load_plan(&resolver, plan, static_require_requests), ProjectCheckOptions {
            environment: StandardEnvironmentProfile::Lua54,
            ..ProjectCheckOptions::default()
        });
        SymbolIndex::new(&report)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

const DEFINE: &str = "Config = {}\nfunction _G.reset() Config = {} end\n_G['Mode'] = 'fast'\n";
const USE: &str = "require('define')\nreset()\nlocal mode = Mode\nlocal function shadow()\n    local Config = 1\n    return Config\nend\ndo\n    local _ENV = { Config = 2 }\n    local hidden = Config\nend\nreturn _ENV.Config\n";

#[test]
fn a_global_is_every_files_and_a_local_its_own() {
    let fixture = Fixture::new(&[("define.lua", DEFINE), ("use.lua", USE)]);
    let index = fixture.index(&["use.lua"]);
    let define = index.file(&fixture.0.join("define.lua")).unwrap();
    let used = index.file(&fixture.0.join("use.lua")).unwrap();

    let (symbol, span) = index.symbol_at(define, 2).unwrap();
    assert_eq!(symbol, Symbol::Global { name: "Config".into() });
    assert_eq!(&DEFINE[span.start..span.end], "Config");
    // Both writes define it; `_ENV.Config` reads it; the shadowing local and the read through a
    // local `_ENV` are other symbols.
    let occurrences = index.occurrences(&symbol);
    let places: Vec<(bool, usize, bool)> = occurrences.iter().map(|occurrence| (occurrence.file == define, occurrence.span.start, occurrence.definition)).collect();
    let define_writes: Vec<usize> = DEFINE.match_indices("Config").map(|(at, _)| at).collect();
    let env_read = USE.rfind("Config").unwrap();
    assert_eq!(places, [(true, define_writes[0], true), (true, define_writes[1], true), (false, env_read, false)]);
    assert_eq!(index.definitions(&symbol).len(), 2);

    // `function _G.reset()` and `_G['Mode'] =` define globals too.
    let reset = USE.find("reset").unwrap();
    let (symbol, _) = index.symbol_at(used, reset + 2).unwrap();
    let definitions = index.definitions(&symbol);
    assert_eq!(definitions.len(), 1);
    assert_eq!(&DEFINE[definitions[0].span.start..definitions[0].span.end], "reset");
    let mode = USE.find("Mode").unwrap();
    let (symbol, _) = index.symbol_at(used, mode + 4).unwrap();
    assert_eq!(index.definitions(&symbol)[0].span.start, DEFINE.find("Mode").unwrap());

    // A local's occurrences are its declaration and uses.
    let shadow = USE.find("Config = 1").unwrap();
    let (symbol, _) = index.symbol_at(used, shadow).unwrap();
    assert!(matches!(symbol, Symbol::Local { .. }));
    assert_eq!(index.occurrences(&symbol).len(), 2);

    // Renaming edits the global in both files and reports what cannot be renamed.
    let edits = index.rename(define, 0, "Settings").unwrap();
    assert_eq!(edits.len(), 3);
    assert_eq!(index.rename(define, 0, "1st"), Err(RenameError::InvalidName("1st".into())));
    let receiver = Fixture::new(&[("method.lua", "local Point = {}\nfunction Point:move() return self end\nprint(Point)\n")]);
    let index = receiver.index(&["method.lua"]);
    let file = index.file(&receiver.0.join("method.lua")).unwrap();
    let text = "local Point = {}\nfunction Point:move() return self end\nprint(Point)\n";
    assert_eq!(index.prepare_rename(file, text.find("self").unwrap()).map(|_| ()), Err(RenameError::ImplicitReceiver));
    assert_eq!(index.prepare_rename(file, text.find("print").unwrap()).map(|_| ()), Err(RenameError::Library("print".into())));
}

#[test]
fn a_rename_keeps_every_name_referring_to_what_it_did() {
    let text = "local first = 1\nlocal second = 2\nprint(first, second)\nlocal later = 3\nprint(later)\nlocal later2 = 4\nprint(later2)\nGlobal = 1\nlocal function f()\n    local inner = 1\n    return Global + inner\nend\n";
    let fixture = Fixture::new(&[("main.lua", text)]);
    let index = fixture.index(&["main.lua"]);
    let file = index.file(&fixture.0.join("main.lua")).unwrap();
    let at = |needle: &str| text.find(needle).unwrap();
    let captured = |result: Result<Vec<_>, RenameError>| matches!(result, Err(RenameError::Capture { .. }));
    // `print(first, second)` would read the second declaration.
    assert!(captured(index.rename(file, at("first"), "second")));
    // A local renamed to a global its scope reads would capture the global.
    assert!(captured(index.rename(file, at("inner"), "Global")));
    // A global renamed to a local's name would be captured where the local is in scope.
    assert!(captured(index.rename(file, at("Global"), "inner")));
    // A global renamed to another's name would merge them.
    assert_eq!(index.rename(file, at("Global"), "print"), Err(RenameError::Exists("print".into())));
    // Shadowing that every name survives is a valid rename: each `later` still refers to its
    // own declaration.
    assert_eq!(index.rename(file, at("later2"), "later").unwrap().len(), 2);
    assert_eq!(index.rename(file, at("first"), "initial").unwrap().len(), 2);
}

#[test]
fn a_rename_edits_the_variables_that_typeof_names() {
    let text = "---@alias Color typeof Colors[keyof typeof Colors]\nlocal Colors = { Red = 1 }\n---@type typeof Colors\nlocal copy = Colors\nShared = { on = true }\n---@type typeof Shared\nlocal state = Shared\nreturn copy, state\n";
    let fixture = Fixture::new(&[("main.lua", text)]);
    let index = fixture.index(&["main.lua"]);
    let file = index.file(&fixture.0.join("main.lua")).unwrap();
    // Each `typeof Colors` refers to the local, the alias's included: it describes that statement.
    let (symbol, _) = index.symbol_at(file, text.find("Colors").unwrap()).unwrap();
    assert!(matches!(symbol, Symbol::Local { .. }));
    let edits = index.rename(file, text.find("Colors").unwrap(), "Palette").unwrap();
    let mut renamed: Vec<usize> = edits.iter().map(|edit| edit.span.start).collect();
    renamed.sort();
    assert_eq!(renamed, text.match_indices("Colors").map(|(at, _)| at).collect::<Vec<_>>());
    let (symbol, _) = index.symbol_at(file, text.find("typeof Shared").unwrap() + 7).unwrap();
    assert_eq!(symbol, Symbol::Global { name: "Shared".into() });
    assert_eq!(index.occurrences(&symbol).len(), 3);
}
