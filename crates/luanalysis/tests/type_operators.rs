//! `keyof T`, `T[K]` and `typeof value`: types derived from tables and other values.
use luanalysis::project_check::{ProjectCheckOptions, check_project_with_options, static_require_requests};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{fs, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Checked {
    diagnostics: Vec<String>,
    bindings: Vec<(String, String)>,
}

impl Checked {
    fn ty(&self, name: &str) -> &str {
        self.bindings.iter().find(|(binding, _)| binding == name).map(|(_, ty)| ty.as_str()).unwrap_or_else(|| panic!("{name}: {:#?}", self.bindings))
    }
}

/// `code text | message` for each diagnostic, and each binding's type.
fn check(source: &str, inspections: bool) -> Checked {
    let root = std::env::temp_dir().join(format!("luanalysis type operators {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&root).unwrap();
    let main: PathBuf = root.join("main.lua");
    fs::write(&main, source).unwrap();
    let project = Project::load_plan(
        &Resolver::new(ProjectConfig::new([root.clone()])).unwrap(),
        ProjectLoadPlan::new([main]),
        static_require_requests,
    );
    let report = check_project_with_options(project, ProjectCheckOptions {
        environment: StandardEnvironmentProfile::Lua54,
        describe_types: true,
        inspections,
        ..ProjectCheckOptions::default()
    });
    fs::remove_dir_all(&root).unwrap();
    let main = report.file(report.project.entries[0]).unwrap();
    Checked {
        diagnostics: main.report.diagnostics.iter().map(|diagnostic| format!("{} {} | {}", diagnostic.code, &source[diagnostic.span.start..diagnostic.span.end], diagnostic.message)).collect(),
        bindings: main.report.bindings.iter().map(|binding| (binding.name.clone(), binding.ty.clone())).collect(),
    }
}

#[test]
fn a_tables_keys_and_values_are_types() {
    let source = r#"---@alias Color typeof Colors[keyof typeof Colors]
local Colors = { Red = 1, Green = 2 }

---@alias ColorName keyof typeof Colors

---@param color Color
---@param name ColorName
local function paint(color, name) return color, name end

paint(Colors.Red, "Green")
paint(3, "Red")
paint(1, "Blue")
---@type Color
local chosen = 2
"#;
    let checked = check(source, false);
    assert_eq!(checked.diagnostics, [
        "TS2345 3 | Argument of type '3' is not assignable to parameter of type '1 | 2'.",
        "TS2345 \"Blue\" | Argument of type '\"Blue\"' is not assignable to parameter of type '\"Green\" | \"Red\"'.",
    ]);
    assert_eq!(checked.ty("chosen"), "1 | 2");
}

#[test]
fn keyof_gives_each_key_as_lua_indexes_it() {
    let source = r#"local mixed = { Red = 1, [10] = "ten", [true] = false }
---@type keyof typeof mixed
local mixed_key = 10
---@type keyof string[]
local index = 1
---@type keyof table<string, boolean>
local name = "anything"
---@type keyof { [1]: string, [integer]: number }
local position = 2

---@class Animal
---@field name string
local Animal = {}
function Animal:speak() return self.name end

---@type keyof Animal
local member = "speak"
---@type keyof Animal
local missing = "fly"
"#;
    let checked = check(source, false);
    assert_eq!(checked.ty("mixed_key"), "\"Red\" | 10 | true");
    assert_eq!(checked.ty("index"), "number");
    assert_eq!(checked.ty("name"), "string");
    assert_eq!(checked.ty("position"), "number");
    assert_eq!(checked.ty("member"), "\"name\" | \"speak\"");
    assert_eq!(checked.diagnostics, ["TS2322 \"fly\" | Type '\"fly\"' is not assignable to type '\"name\" | \"speak\"'."]);
}

#[test]
fn an_indexed_access_reads_a_field_type_as_lua_reads_the_field() {
    let source = r#"---@shape Point
---@field x number
---@field label? string
---@field [1] boolean

---@type Point["x"]
local x = 1
---@type Point["label"]
local label = nil
---@type Point[1]
local first = true
---@type Point["x" | "label"]
local either = "a"
---@type Point["z"]
local z
---@type Point[boolean]
local flag
---@type string[][number]
local element = "a"
"#;
    let checked = check(source, false);
    assert_eq!(checked.ty("x"), "number");
    assert_eq!(checked.ty("label"), "nil | string");
    assert_eq!(checked.ty("first"), "boolean");
    assert_eq!(checked.ty("either"), "nil | string | number");
    assert_eq!(checked.ty("element"), "string");
    assert_eq!(checked.diagnostics, [
        "TS2339 \"z\" | Property 'z' does not exist on type 'Point'.",
        "TS2536 boolean | Type 'boolean' cannot be used to index type 'Point'.",
    ]);
}

#[test]
fn generic_accessors_index_a_type_parameter_by_its_keys() {
    let source = r#"---@generic T, K: keyof T
---@param object T
---@param key K
---@return T[K]
local function get(object, key) return object[key] end

---@type { x: number, label: string }
local point = { x = 1, label = "p" }
local x = get(point, "x")
local label = get(point, "label")
get(point, "y")
"#;
    let checked = check(source, false);
    assert_eq!(checked.ty("x"), "number");
    assert_eq!(checked.ty("label"), "string");
    assert_eq!(checked.diagnostics, ["TS2345 \"y\" | Argument of type '\"y\"' is not assignable to parameter of type '\"label\" | \"x\"'."]);
}

#[test]
fn typeof_is_the_declared_type_of_a_value_in_scope() {
    let source = r#"local M = { config = { width = 10, title = "t" } }
---@type typeof M.config.width
local width = 10
---@type typeof M.config
local config = { width = 10, title = "t" }

---@class Shop
local Shop = {}
---@type typeof Shop
local same = Shop

Global = { enabled = true }
---@type typeof Global
local global = { enabled = true }

---@param text string | nil
local function narrowed(text)
    if text then
        -- The declared type, not the narrowed one.
        ---@type typeof text
        local copy = nil
        return copy
    end
end

---@type typeof M.missing
local missing
---@type typeof Nowhere
local nowhere
return width, config, same, global, narrowed, missing, nowhere
"#;
    let checked = check(source, false);
    assert_eq!(checked.ty("width"), "10");
    assert_eq!(checked.ty("same"), "def Shop");
    assert_eq!(checked.ty("global"), "{ enabled: true }");
    assert_eq!(checked.ty("copy"), "string | nil");
    assert_eq!(checked.diagnostics, [
        "TS2339 missing | Property 'missing' does not exist on type '{ config: { title: \"t\", width: 10 } }'.",
        "TS2304 Nowhere | Cannot find name 'Nowhere'.",
    ]);
}

#[test]
fn typeof_names_the_variable_in_scope_where_it_is_written() {
    let source = r#"---@type typeof later
local before = nil
local later = 1

local shadowed = "outer"
local function f()
    local shadowed = 2
    ---@type typeof shadowed
    local inner = 3
    return shadowed, inner
end
---@type typeof shadowed
local outer = "o"
return before, later, f, outer
"#;
    let checked = check(source, false);
    assert_eq!(checked.ty("inner"), "number");
    assert_eq!(checked.ty("outer"), "string");
    assert_eq!(checked.diagnostics, ["TS2304 later | Cannot find name 'later'."]);
}

#[test]
fn a_variable_that_only_typeof_names_is_used() {
    let source = r#"local template = { size = 1 }
---@type typeof template
local sized = { size = 1 }
return sized
"#;
    assert!(check(source, true).diagnostics.is_empty());
}

#[test]
fn the_readme_examples_check_as_described() {
    let colors = r#"---@alias Color typeof Colors[keyof typeof Colors]
local Colors = { Red = 1, Green = 2, Blue = 3 }

---@param color Color
local function paint(color) end

paint(Colors.Red)
paint(4) -- error: '4' is not assignable to '1 | 2 | 3'
"#;
    assert_eq!(check(colors, false).diagnostics, ["TS2345 4 | Argument of type '4' is not assignable to parameter of type '1 | 2 | 3'."]);
    let keys = r#"---@type keyof { [1]: string, active: boolean }
local key = 1
"#;
    assert_eq!(check(keys, false).ty("key"), "\"active\" | 1");
    let get = r#"---@shape Point
---@field x number
---@field y number

---@generic T, K: keyof T
---@param object T
---@param key K
---@return T[K]
local function get(object, key) return object[key] end

---@type Point
local point = { x = 3, y = 4 }
local x = get(point, "x") -- number
get(point, "z")           -- error: '"z"' is not assignable to '"x" | "y"'
"#;
    let checked = check(get, false);
    assert_eq!(checked.ty("x"), "number");
    assert_eq!(checked.diagnostics, ["TS2345 \"z\" | Argument of type '\"z\"' is not assignable to parameter of type '\"x\" | \"y\"'."]);
}

#[test]
fn a_type_that_refers_to_itself_is_reported() {
    let source = r#"---@type typeof Counter
Counter = 1

---@alias Color typeof Colors[keyof typeof Colors]
local Colors = { Red = --[[---@type Color]] 1, Green = 2 }

---@alias Keys keyof Keys
return Colors
"#;
    assert_eq!(check(source, false).diagnostics, [
        "TS2502 Counter | 'Counter' is referenced directly or indirectly in its own type annotation.",
        "TS2456 Color | Type alias circularly references itself.",
        "TS2456 Keys | Type alias circularly references itself.",
    ]);
}

#[test]
fn a_luals_enum_is_its_tables_values_or_keys() {
    let source = r#"---@enum Mode
local Mode = { Read = 1, Write = 2 }

---@enum (key) Side
local Side = { left = true, right = true }

local M = {}
---@enum Level
M.Level = { Low = "low", High = "high" }

---@param mode Mode
---@param side Side
---@param level Level
local function use(mode, side, level) return mode, side, level end

use(Mode.Read, "left", M.Level.High)
use(3, "left", "low")
use(1, "up", "low")
use(1, "left", "medium")
"#;
    assert_eq!(check(source, false).diagnostics, [
        "TS2345 3 | Argument of type '3' is not assignable to parameter of type '1 | 2'.",
        "TS2345 \"up\" | Argument of type '\"up\"' is not assignable to parameter of type '\"left\" | \"right\"'.",
        "TS2345 \"medium\" | Argument of type '\"medium\"' is not assignable to parameter of type '\"high\" | \"low\"'.",
    ]);
}

#[test]
fn an_enum_used_by_a_function_its_table_is_passed_in_is_not_circular() {
    // lua-language-server's script/core/color.lua: the table escapes to `pairs` inside a function
    // whose result is the enum, and that view must not need the function's signature.
    let source = r#"---@enum (key) ColorMode
local colorPattern = { argb8 = "a", hexrgb6 = "b" }

---@param text string
---@return ColorMode | false
local function getColorMode(text)
    for k, v in pairs(colorPattern) do
        if text:match(v) then return k end
    end
    print(colorPattern)
    return false
end
return getColorMode
"#;
    assert!(check(source, false).diagnostics.is_empty(), "{:#?}", check(source, false).diagnostics);
}
