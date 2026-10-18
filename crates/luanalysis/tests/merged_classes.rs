//! A class declared in several places is one class, as LuaLS reads repeated `---@class X` and
//! TypeScript merges interface declarations. A module that reopens a class on another module's
//! class table defines members of that class for every importer.
use std::{fs, path::PathBuf, process::{Command, Output}, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);
impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("luanalysis merged classes {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        for (path, source) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source).unwrap();
        }
        Self(root)
    }
    fn check(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&self.0)
            .args(["check", "--lua-version", "5.4", "--module-root", ".", "--dump-types", "main.lua"]).output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn text(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn repeated_class_declarations_merge_their_fields() {
    let fixture = Fixture::new(&[
        ("object.lua", "---@class parser.object\n---@field type string\n\nreturn {}\n"),
        ("compiler.lua", "---@class parser.object\n---@field start integer\n\nreturn {}\n"),
        ("main.lua", "require 'object'\nrequire 'compiler'\n---@param source parser.object\n---@return string, integer\nlocal function read(source) return source.type, source.start end\n---@param source parser.object\nlocal function wrong(source) return source.finish end\n"),
    ]);
    let output = fixture.check();
    let report = text(&output);
    assert!(!report.contains("TS2300"), "{report}");
    assert!(report.contains("main.lua:7:44: error[TS2339]: Property 'finish' does not exist on type 'parser.object'"), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}

#[test]
fn a_reopened_class_defines_members_for_every_importer() {
    let fixture = Fixture::new(&[
        ("vm/vm.lua", "---@class vm\nlocal m = {}\nfunction m.compile() return 1 end\nreturn m\n"),
        ("vm/global.lua", "---@class vm\nlocal vm = require 'vm.vm'\nfunction vm.getGlobal() return 'global' end\nlocal own = vm.compile()\nreturn vm\n"),
        ("main.lua", "local vm = require 'vm.vm'\nrequire 'vm.global'\nlocal compiled = vm.compile()\nlocal global = vm.getGlobal()\n"),
    ]);
    let output = fixture.check();
    let report = text(&output);
    assert!(output.status.success(), "{report}");
    assert!(report.contains("main.lua:1:7: vm: vm"), "{report}");
    assert!(report.contains("main.lua:3:7: compiled: number"), "{report}");
    assert!(report.contains("main.lua:4:7: global: string"), "{report}");
}

#[test]
fn an_imported_class_table_is_of_its_class() {
    // As in v1: its declared members, assigned or not, read as the class declares them.
    let fixture = Fixture::new(&[
        ("point.lua", "---@class Point\n---@field x number\nlocal point = {}\nreturn point\n"),
        ("main.lua", "local point = require 'point'\n---@param value Point\nlocal function use(value) end\nuse(point)\nlocal x = point.x\n"),
    ]);
    let output = fixture.check();
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("main.lua:5:7: x: number"), "{}", text(&output));
}

#[test]
fn appending_with_the_length_operator_builds_a_sequence() {
    let fixture = Fixture::new(&[
        ("main.lua", "local list = {}\nfor i = 1, 3 do list[#list + 1] = { start = i } end\nfor _, item in ipairs(list) do\n    local start = item.start\nend\nlocal sparse = {}\nfor i = 1, 3 do sparse[i * 2] = i end\n---@type number[]\nlocal evolved = sparse\nlocal named = { n = 0 }\nfor i = 1, 3 do named[i] = i end\n---@type number[]\nlocal wrong = named\n"),
    ]);
    let output = fixture.check();
    let report = text(&output);
    assert!(report.contains("main.lua:4:11: start: number"), "{report}");
    // As TypeScript evolves an array, a store at another numeric key keeps a sequence one; a
    // table with a named key becomes a numeric dictionary.
    assert!(report.contains("main.lua:13:15: error[TS2322]"), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}

#[test]
fn a_class_table_stored_in_its_own_field_is_the_class() {
    let fixture = Fixture::new(&[
        ("main.lua", "---@class Node\nlocal mt = {}\nmt.__index = mt\nmt.self = mt\nfunction mt:name() return 'node' end\n---@type Node\nlocal index = mt.__index\n"),
    ]);
    let output = fixture.check();
    assert!(output.status.success(), "{}", text(&output));
}

#[test]
fn a_class_field_stored_from_another_field_is_inferred() {
    // Building the class's definition table for the read of `m.y` once requested `x` again,
    // which was reported as excessively deep, and the class was unusable.
    let fixture = Fixture::new(&[
        ("main.lua", "---@class Pair\nlocal m = {}\nm.y = 1\nm.x = m.y\nfunction m.f()\n    m.z = m.missing\nend\nlocal x = m.x\nreturn m\n"),
    ]);
    let output = fixture.check();
    let report = text(&output);
    assert!(report.contains("main.lua:8:7: x: number"), "{report}");
    assert!(report.contains("main.lua:6:13: error[TS2339]: Property 'missing' does not exist on type 'Pair'"), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}

#[test]
fn a_metamethod_stored_from_a_method_of_its_own_class_is_not_too_deep() {
    let fixture = Fixture::new(&[
        ("main.lua", "---@class Hash\nlocal mt = {}\nmt.__index = mt\n---@param text string\n---@return integer\nfunction mt:hash(text) return #text end\nmt.__call = mt.hash\nreturn mt\n"),
    ]);
    let report = text(&fixture.check());
    assert!(!report.contains("TS2589") && !report.contains("LUA2031"), "{report}");
}

#[test]
fn a_class_declared_as_string_extends_the_string_library() {
    // LuaLS adds operators to strings this way. The standard library's own `string` stays the
    // class that the declaration extends.
    let fixture = Fixture::new(&[
        ("format.lua", "---@class string\n---@operator mod(table): string\n\nreturn {}\n"),
        ("main.lua", "require 'format'\nlocal byte = string.byte('x')\nlocal upper = ('x'):upper()\nlocal formatted = ('x') % {}\nlocal wrong = ('x') % 1\n"),
    ]);
    let output = fixture.check();
    let report = text(&output);
    assert!(report.contains("main.lua:3:7: upper: string"), "{report}");
    assert!(report.contains("main.lua:4:7: formatted: string"), "{report}");
    assert!(report.contains("main.lua:5:21: error[TS2365]: Operator '%' cannot be applied to types '\"x\"' and '1'"), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}

#[test]
fn a_methods_self_stores_assign_fields_of_its_class() {
    // As `this.x = v` in a JavaScript class method declares a property, and as Luanalysis for
    // IntelliJ indexed `self` assignments as members.
    let fixture = Fixture::new(&[
        ("main.lua", "---@class Lens\nlocal mt = {}\nmt.__index = mt\n---@param uri string\nfunction mt:init(uri)\n    self.uri = uri\n    self.results = {}\nend\n---@return string\nfunction mt:read() return self.uri end\nreturn mt\n"),
    ]);
    let output = fixture.check();
    assert!(output.status.success(), "{}", text(&output));
}

#[test]
fn a_field_declared_twice_alike_is_one_field() {
    // As TypeScript merges declarations: an identical redeclaration is the same member, and a
    // different one is reported once, at the declaration, while uses read the first.
    let fixture = Fixture::new(&[
        ("main.lua", "---@shape Controller\n---@field reinterpret fun(color: string): void\n---@field reinterpret fun(color: string): void\n\n---@shape Card\n---@field color string\n---@field color number\n\n---@class Impl\n---@field reinterpret fun(color: string): void\n\n---@param controller Controller\nlocal function use(controller) controller.reinterpret('Red') end\n---@param impl Impl\n---@param card Card\nlocal function f(impl, card)\n    use(impl)\n    local color = card.color\nend\n"),
    ]);
    let report = text(&fixture.check());
    assert!(report.contains("main.lua:5:11: error[LUA2032]: Duplicate declared property \"color\"."), "{report}");
    assert!(report.contains("main.lua:18:11: color: string"), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}

#[test]
fn a_written_field_types_another_partial_declarations_value() {
    let fixture = Fixture::new(&[
        ("comp.lua", "local m = {}\nfunction m.compile(src)\n    ---@class Pst\n    ---@field uri string\n    local state = { lua = src, ast = {} }\n    return state\nend\nreturn m\n"),
        ("main.lua", "local comp = require 'comp'\n---@class Pst\n---@field lua? string\n\n---@param s Pst\nlocal function f(s)\n    s.uri = 'x'\n    s.ast = {}\n    local text = s.lua\nend\n"),
    ]);
    let output = fixture.check();
    let report = text(&output);
    assert!(output.status.success(), "{report}");
    assert!(report.contains("main.lua:9:11: text: nil | string"), "{report}");
}

#[test]
fn a_computed_key_of_one_string_literal_type_names_a_field() {
    let fixture = Fixture::new(&[
        ("main.lua", "local K = { Self = 'self', Target = 'target' }\nlocal Computed = { [K.Self] = { Owner = 'owner' } }\nlocal Stored = {}\nStored[K.Target] = { Owner = 'x' }\n---@param index number\nlocal function f(index)\n    local named = { slot = false }\n    named[index] = 'word'\n    local kept = named.slot\nend\nlocal owner = Computed.self.Owner\nlocal stored = Stored.target\n"),
    ]);
    let output = fixture.check();
    let report = text(&output);
    assert!(output.status.success(), "{report}");
    assert!(report.contains("main.lua:11:7: owner: string"), "{report}");
    assert!(report.contains("main.lua:12:7: stored: { [\"Owner\"]: \"x\" }"), "{report}");
    // A numeric key cannot write a named field.
    assert!(report.contains("main.lua:9:11: kept: boolean"), "{report}");
}

#[test]
fn a_store_below_a_modules_member_is_part_of_its_export() {
    for class in ["", "---@class Types\n"] {
        let fixture = Fixture::new(&[
            ("types.lua", &format!("{class}local Types = {{}}\nTypes.Zone = {{}}\nTypes.Zone.Building = 'Building Zone'\nreturn Types\n")),
            ("main.lua", "local Types = require 'types'\n---@type string\nlocal zone = Types.Zone.Building\nlocal missing = Types.Zone.Missing\n"),
        ]);
        let output = fixture.check();
        let report = text(&output);
        assert!(report.contains("main.lua:3:7: zone: string"), "{report}");
        assert!(report.contains("main.lua:4:28: error[TS2339]: Property 'Missing' does not exist"), "{report}");
        assert_eq!(report.matches("error[").count(), 1, "{report}");
    }
}

#[test]
fn a_cast_store_through_a_subclass_constructors_self_overrides_an_inherited_member() {
    // As in v1, the store declares the subclass's member; it is checked as an override.
    let source = "---@shape Target\n---@field kind string\n\n---@shape DeadTarget : Target\n---@field kind 'dead'\n\n---@class Zone\n---@field getTarget fun(): Target\n\n---@class DeadZone : Zone\n\n---@param zone Zone\n---@return DeadZone\nlocal function newDeadZone(zone)\n    local self = --[[---@type DeadZone]] zone\n    self.getTarget = --[[---@type fun(): DeadTarget]] self.getTarget\n    return self\nend\n\n---@param dead DeadZone\n---@return DeadTarget\nlocal function deadTarget(dead)\n    return dead.getTarget()\nend\n";
    let fixture = Fixture::new(&[("main.lua", source)]);
    let output = fixture.check();
    assert!(output.status.success(), "{}", text(&output));
    let wrong = source.replace("--[[---@type fun(): DeadTarget]] self.getTarget", "--[[---@type fun(): number]] function() return 1 end");
    let fixture = Fixture::new(&[("main.lua", &wrong)]);
    let report = text(&fixture.check());
    assert!(report.contains("error[TS2416]: Declared member type 'fun(): number' is not assignable to the inherited member type 'fun(): Target'."), "{report}");
    // As in TypeScript, the class keeps the member it declares; its uses are not errors too.
    assert!(!report.contains("TS7053") && !report.contains("TS2339"), "{report}");
}

#[test]
fn a_type_argument_of_a_base_prints_as_itself_not_as_the_derived_type() {
    let fixture = Fixture::new(&[
        ("main.lua", "---@alias Names string[]\n\n---@shape BaseCard<A>\n---@field abilities nil | A\n\n---@shape BuildingCard : BaseCard<string[]>\n\n---@param card BuildingCard\n---@param names Names\nlocal function f(card, names)\n    local abilities = card.abilities\n    local aliased = names\nend\n"),
    ]);
    let report = text(&fixture.check());
    assert!(report.contains("main.lua:11:11: abilities: nil | string[]"), "{report}");
    assert!(report.contains("main.lua:12:11: aliased: Names"), "{report}");
}

#[test]
fn a_methods_self_is_its_table_with_every_stored_member() {
    // A method runs after its table is built, so it sees methods and fields stored after it.
    let fixture = Fixture::new(&[
        ("main.lua", "local mt = {}\nmt.__index = mt\nfunction mt:a(x)\n    self.count = 1\n    return self:b(x)\nend\nfunction mt:b(x)\n    return x + self.count\nend\nreturn mt\n"),
    ]);
    let output = fixture.check();
    assert!(output.status.success(), "{}", text(&output));
}

#[test]
fn an_optional_array_table_or_record_field_may_be_nil() {
    let fixture = Fixture::new(&[
        ("main.lua", "---@class Node\n---@field children? Node[]\n---@field index? table<number, string>\n---@field range? { start: number }\n\n---@param node Node | nil\nlocal function f(node)\n    if not node then return end\n    local children = node.children\n    local index = node.index\n    local range = node.range\n    if node.children == nil then\n        node.children = {}\n    end\n    local after = node\nend\n"),
    ]);
    let output = fixture.check();
    let report = text(&output);
    assert!(output.status.success(), "{report}");
    for expected in ["children: nil | Node[]", "index: nil | table<number, string>", "range: nil | { [\"start\"]: number }", "after: Node"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn a_member_no_alternative_has_does_not_narrow_its_table_to_never() {
    // It may be stored through another reference to the table: its absence proves nothing.
    let fixture = Fixture::new(&[
        ("main.lua", "local function push(chunk) chunk.locals = {} end\n---@param cond boolean\nlocal function f(cond)\n    local block = cond and { type = 'if' } or { type = 'else' }\n    push(block)\n    if block.locals then\n        local inside = block\n    end\n    local u = cond and { kind = 'a', x = 1 } or { kind = 'b' }\n    if u.x then local onlyA = u end\nend\n"),
    ]);
    let report = text(&fixture.check());
    assert!(!report.contains("'never'"), "{report}");
    assert!(report.contains("main.lua:7:15: inside: { [\"type\"]: \"if\" } | { [\"type\"]: \"else\" }"), "{report}");
    // A member one alternative has still selects it.
    assert!(report.contains("main.lua:10:23: onlyA: { [\"kind\"]: \"a\"; [\"x\"]: 1 }"), "{report}");
}

#[test]
fn a_class_chain_declared_in_two_files_reads_inherited_fields_once_per_base() {
    // Each file names every class's base; a read through the merged chain followed both
    // listings at every level, doubling with each class.
    let mut chain = String::from("---@class C0\n---@field x number\nlocal C0 = {}\n");
    for i in 1..=24 {
        chain += &format!("---@class C{i} : C{}\nlocal C{i} = {{}}\n", i - 1);
    }
    let main = format!("require 'copy'\n{chain}---@param c C24\nlocal function read(c) local x = c.x; local y = c.y end\n");
    let fixture = Fixture::new(&[("copy.lua", &format!("{chain}return {{}}\n")), ("main.lua", &main)]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&fixture.0)
        .args(["check", "--lua-version", "5.4", "--module-root", ".", "--dump-types", "main.lua"])
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
    let started = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > std::time::Duration::from_secs(20) {
            child.kill().unwrap();
            panic!("checking the merged chain did not finish in 20 s");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let report = text(&child.wait_with_output().unwrap());
    assert!(report.contains(": x: number"), "{report}");
    assert!(report.contains("error[TS2339]: Property 'y' does not exist on type 'C24'"), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}

#[test]
fn a_class_declared_again_in_one_file_is_one_class_with_its_setmetatable_tables_fields() {
    // As LuaLS reads it: the class table, then an object of the class made by setmetatable.
    let fixture = Fixture::new(&[
        ("main.lua", "---@class Builder\nlocal builder = { kind = 'b' }\n\nfunction builder:get()\n    return self.cache\nend\n\nlocal function make()\n    ---@class Builder\n    local b = setmetatable({ cache = {} }, { __index = builder })\n    return b:get(), b.kind, b.missing\nend\n"),
    ]);
    let output = fixture.check();
    let report = text(&output);
    assert!(!report.contains("LUA2031"), "{report}");
    // `b` is the class's second definition.
    assert!(report.contains("main.lua:11:31: error[TS2339]: Property 'missing' does not exist on type 'def Builder'"), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}
