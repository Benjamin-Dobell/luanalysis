//! `---@type` above an assignment types what it assigns. On a field of a table it declares the
//! field, as JSDoc's `@type` on an expando assignment does; on a local it is a checked
//! annotation of the value, as LuaLS reads it. A written key type may include nil, as in v1, so
//! lookups with a key that may be nil check against it.
use std::{fs, path::PathBuf, process::{Command, Output}, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn check(source: &str) -> String {
    let root = std::env::temp_dir().join(format!("luanalysis assignment annotations {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("main.lua"), source).unwrap();
    let output: Output = Command::new(env!("CARGO_BIN_EXE_luanalysis")).current_dir(&root)
        .args(["check", "--lua-version", "5.4", "--dump-types", "main.lua"]).output().unwrap();
    let _ = fs::remove_dir_all(PathBuf::from(&root));
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn a_typed_store_declares_a_field_of_a_plain_table() {
    let report = check("local M = {}\nM.Zone = { A = 'a' }\nM.Category = {}\n---@type table<nil | string, nil | true>\nM.Category.Battle = {\n    [M.Zone.A] = true,\n}\n---@type number\nM.count = 'wrong'\n---@param zone nil | string\nlocal function battle(zone)\n    local found = M.Category.Battle[zone]\n    return found\nend\nreturn M\n");
    assert!(report.contains("main.lua:9:11: error[TS2322]: Type '\"wrong\"' is not assignable to type 'number'."), "{report}");
    assert!(report.contains("main.lua:12:11: found: nil | true"), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}

#[test]
fn a_typed_assignment_to_a_local_is_checked_and_types_the_local() {
    let report = check("---@param text string\nlocal function f(text)\n    local value\n    ---@type integer?\n    value = tonumber(text)\n    local after = value\n    ---@type string\n    value = 1\nend\n");
    assert!(report.contains("main.lua:6:11: after: nil | number"), "{report}");
    assert!(report.contains("main.lua:8:13: error[TS2322]: Type '1' is not assignable to type 'string'."), "{report}");
    assert!(!report.contains("LUA2004"), "{report}");
}

#[test]
fn only_a_written_key_type_may_include_nil() {
    let report = check("---@type table<nil | string, true>\nlocal set = { a = true }\n---@type table<string, true>\nlocal strict = { a = true }\n---@param key nil | string\nlocal function f(key)\n    local found = set[key]\n    local missing = strict[key]\n    set[nil] = true\n    local constructed = { [key] = 1 }\nend\n");
    assert!(report.contains("main.lua:7:11: found: nil | true"), "{report}");
    assert!(report.contains("main.lua:8:28: error[TS2538]"), "{report}");
    assert!(report.contains("main.lua:9:9: error[TS2538]: nil cannot be used as a Lua table key."), "{report}");
    assert!(report.contains("main.lua:10:28: error[TS2538]"), "{report}");
    assert_eq!(report.matches("error[").count(), 3, "{report}");
}
