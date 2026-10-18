//! A host whose modules are not files package.path finds (a game's virtual scripts, named
//! case-sensitively) supplies its own module map: each source's modules by their exact names.
//! With it, a module is what the map says or unresolved.
use std::{fs, path::{Path, PathBuf}, process::Command, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn fixture(files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("luanalysis module map {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&root).unwrap();
    for (path, text) in files {
        fs::write(root.join(path), text).unwrap();
    }
    fs::canonicalize(root).unwrap()
}

fn check(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(root)
        .args(["check", "--lua-version", "5.2", "--stdlib", "require", "--dump-types"]).args(arguments).output().unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn modules_are_what_the_map_names_and_nothing_else() {
    let root = fixture(&[
        ("main.lua", "local upper = require('Case')\nlocal lower = require('case')\nlocal unmapped = require('other')\nlocal x, y = upper.x, lower.y\n"),
        ("m1.lua", "return { x = 1 }\n"),
        ("m2.lua", "return { y = 'two' }\n"),
        // package.path would find this one; the map does not name it.
        ("other.lua", "return 3\n"),
        ("map.json", r#"{"version": 1, "sources": [{"file": "main.lua", "imports": {"Case": "m1.lua", "case": "m2.lua"}}]}"#),
    ]);
    let report = check(&root, &["--module-map", "map.json", "main.lua"]);
    // Names differing only in case are different modules.
    assert!(report.contains(r#"upper: { ["x"]: 1 }"#) && report.contains(r#"lower: { ["y"]: "two" }"#), "{report}");
    // An unmapped module is unresolved, though a file of its name exists.
    assert!(report.contains(r#"main.lua:3:26: error[TS2307]: module "other" is not in the module map's imports of this file"#), "{report}");
    assert!(!report.contains("unmapped: number"), "{report}");
    // Without the map, package.path finds it.
    let report = check(&root, &["main.lua"]);
    assert!(report.contains("unmapped: number"), "{report}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_invalid_manifest_is_a_host_error() {
    let root = fixture(&[
        ("main.lua", "local value = require('a')\n"),
        ("twice.json", r#"{"version": 1, "sources": [{"file": "main.lua", "imports": {"a": "main.lua", "a": "main.lua"}}]}"#),
        ("missing.json", r#"{"version": 1, "sources": [{"file": "main.lua", "imports": {"a": "absent.lua"}}]}"#),
        ("version.json", r#"{"version": 2, "sources": []}"#),
    ]);
    for manifest in ["twice.json", "missing.json", "version.json", "nowhere.json"] {
        let report = check(&root, &["--module-map", manifest, "main.lua"]);
        assert!(report.contains("LUA0001"), "{manifest}: {report}");
    }
    fs::remove_dir_all(root).unwrap();
}
