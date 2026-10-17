//! Lua's `setmetatable` returns the table it is given, so a table built through it, Lua's
//! module and object idiom, is extended by the stores after the call as the table itself is.
use std::{fs, path::PathBuf, process::{Command, Output}, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn check(source: &str) -> (Output, String) {
    let root = std::env::temp_dir().join(format!("luanalysis setmetatable tables {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&root).unwrap();
    let path: PathBuf = root.join("main.lua");
    fs::write(&path, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--lua-version", "5.4", "--dump-types"]).arg(&path).output().unwrap();
    let _ = fs::remove_dir_all(&root);
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    (output, text)
}

#[test]
fn stores_extend_a_table_given_to_setmetatable() {
    let (output, report) = check("local m = setmetatable({}, {})\nm.count = 1\nfunction m.next() return m.count + 1 end\nlocal read = m.count\nreturn m\n");
    assert!(output.status.success(), "{report}");
    assert!(report.contains("main.lua:4:7: read: number"), "{report}");
}

#[test]
fn a_field_holding_a_table_given_to_setmetatable_is_extended() {
    let (output, report) = check("local export = {}\nexport.make = setmetatable({}, {})\nexport.make.field = 1\nexport.make['doc.name'] = function() end\nlocal read = export.make.field\nreturn export\n");
    assert!(output.status.success(), "{report}");
    assert!(report.contains("main.lua:5:7: read: number"), "{report}");
}

#[test]
fn a_class_definition_through_setmetatable_is_still_the_class() {
    let (output, report) = check("---@class Null\nlocal NULL = setmetatable({}, { __metatable = false })\n\n---@return Null\nlocal function null() return NULL end\n");
    assert!(output.status.success(), "{report}");
}

#[test]
fn a_class_definitions_declared_indexer_answers_for_every_key() {
    // LuaLS's config template: an `__index` function provides each unit, which the class
    // declares with an indexer. Reading one while defining the class is not nil.
    let (output, report) = check("---@class Unit\n---@field name string\n\n---@class Units\n---@field [string] Unit\nlocal Type = setmetatable({}, { __index = function(_, name) return { name = name } end })\nlocal name = Type.Array.name\n");
    assert!(output.status.success(), "{report}");
    assert!(report.contains("main.lua:7:7: name: string"), "{report}");
}

#[test]
fn a_table_given_a_class_as_its_metatable_is_an_instance_of_the_class() {
    // LuaLS's markdown module returns `setmetatable({}, mt)` for its `---@class markdown`.
    let (output, report) = check("---@class Markdown\nlocal mt = {}\nmt.__index = mt\n---@param text string\n---@return Markdown\nfunction mt:add(text) return self end\nlocal function markdown() return setmetatable({}, mt) end\nlocal md = markdown()\nlocal same = md:add('x')\n");
    assert!(output.status.success(), "{report}");
    assert!(report.contains("main.lua:8:7: md: Markdown"), "{report}");
}
