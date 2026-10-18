//! A host declares the global callbacks a script may implement, as `_G.name` or bare `name`
//! values of an optional function type in a declaration library. A script's implementation, in
//! any of Lua's spellings, is checked with that contract: its parameters (whatever their names)
//! have the declared types by position, and its body and results are checked against them.
use std::{fs, process::Command, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);

const TYPES: &str = r#"---@alias tts__PlayerColor "Red" | "Blue"

---@class tts__Vector
---@field x number

---@class tts__Object
---@field tag string

---@function tts__Object.getPosition(): tts__Vector
"#;

const CALLBACKS: &str = r#"---@type nil | fun(player_color: tts__PlayerColor, object: tts__Object): void
_G.onObjectDrop = nil

---@type nil | fun(container: tts__Object, object: tts__Object): boolean
_G.tryObjectEnterContainer = nil

---@type nil | fun(): string | nil
_G.onSave = nil
"#;

fn check(callbacks: &str, source: &str) -> String {
    check_with(callbacks, source, &[])
}

fn check_with(callbacks: &str, source: &str, options: &[&str]) -> String {
    let directory = std::env::temp_dir().join(format!("luanalysis ambient callbacks {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("api.def.lua"), format!("{TYPES}\n{callbacks}")).unwrap();
    fs::write(directory.join("main.lua"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&directory)
        .args(["check", "--lua-version", "5.2", "--library", "api.def.lua", "--dump-types", "main.lua"]).args(options).output().unwrap();
    fs::remove_dir_all(directory).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn every_implementation_spelling_is_checked_with_the_declared_contract() {
    for (callbacks, form) in [
        (CALLBACKS, "function onObjectDrop(colour, dropped)"),
        (CALLBACKS, "onObjectDrop = function(colour, dropped)"),
        (CALLBACKS, "function _G.onObjectDrop(colour, dropped)"),
        (CALLBACKS, "_G.onObjectDrop = function(colour, dropped)"),
        (&CALLBACKS.replace("_G.", ""), "function onObjectDrop(colour, dropped)"),
    ] {
        let report = check(callbacks, &format!("{form}\n    local position = dropped.getPosition()\n    print(position.x)\n    dropped.getPosition(123)\n    dropped.doesNotExist()\nend\n"));
        assert!(!report.contains("LUA2038"), "{form}\n{report}");
        assert!(report.contains(r#"colour: "Blue" | "Red""#) && report.contains("dropped: tts__Object"), "{form}\n{report}");
        assert!(report.contains("position: tts__Vector"), "{form}\n{report}");
        assert!(report.contains("main.lua:4:5: error[TS2554]: Expected 0 arguments, but got 1."), "{form}\n{report}");
        assert!(report.contains("Property 'doesNotExist' does not exist on type 'tts__Object'"), "{form}\n{report}");
        // The contract types the parameters, so they are not implicitly `any`, even where the
        // body's calls check their arguments.
        let strict = check_with(callbacks, &format!("{form}\n    print(colour, dropped.tag)\nend\n"), &["--no-implicit-any"]);
        assert!(strict.contains("No type errors"), "{form}\n{strict}");
    }
}

#[test]
fn results_are_checked_against_the_contract() {
    let valid = check(CALLBACKS, "function tryObjectEnterContainer(container, object) return object.tag == 'x' end\nfunction onSave() if true then return nil end return 'state' end\n");
    assert!(!valid.contains("error["), "{valid}");
    let invalid = check(CALLBACKS, "function tryObjectEnterContainer(container, object) return 'nope' end\nfunction onSave() return 1 end\n");
    assert!(invalid.contains("main.lua:1:1: error[TS2322]") && invalid.contains("main.lua:2:1: error[TS2322]"), "{invalid}");
}

#[test]
fn absent_nil_and_wrongly_typed_handlers() {
    assert!(!check(CALLBACKS, "onSave = nil\n").contains("error["));
    let report = check(CALLBACKS, "tryObjectEnterContainer = 5\n");
    assert!(report.contains("error[TS2322]: Type '5' is not assignable"), "{report}");
    // An explicit annotation is kept, and its conflict reported.
    let report = check(CALLBACKS, "---@param colour number\nfunction onObjectDrop(colour, dropped) end\n");
    assert!(report.contains("colour: number") && report.contains("error[TS2322]"), "{report}");
}

#[test]
fn locals_and_other_tables_do_not_borrow_a_global_contract() {
    let report = check(CALLBACKS, "local function onObjectDrop(a, b) return a end\nlocal _G = {}\nfunction _G.tryObjectEnterContainer(c, d) return c end\nlocal other = {}\nfunction other.onSave(e) return e end\n");
    for parameter in ["a: any", "b: any", "c: any", "d: any", "e: any"] {
        assert!(report.contains(parameter), "{parameter}\n{report}");
    }
    assert!(!report.contains("error["), "{report}");
}
