//! Mapped types, `{ [P in K as N]?: T }`, TypeScript's utility types and `NoInfer`, as
//! TypeScript gives them.
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
fn check(source: &str) -> Checked {
    let root = std::env::temp_dir().join(format!("luanalysis mapped types {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&root).unwrap();
    let main: PathBuf = root.join("main.lua");
    fs::write(&main, source).unwrap();
    let project = Project::load_plan(&Resolver::new(ProjectConfig::new([root.clone()])).unwrap(), ProjectLoadPlan::new([main]), static_require_requests);
    let report = check_project_with_options(project, ProjectCheckOptions {
        environment: StandardEnvironmentProfile::Lua54,
        describe_types: true,
        ..ProjectCheckOptions::default()
    });
    fs::remove_dir_all(&root).unwrap();
    let main = report.file(report.project.entries[0]).unwrap();
    Checked {
        diagnostics: main.report.diagnostics.iter().map(|diagnostic| format!("{} {} | {}", diagnostic.code, &source[diagnostic.span.start..diagnostic.span.end], diagnostic.message)).collect(),
        bindings: main.report.bindings.iter().map(|binding| (binding.name.clone(), binding.ty.clone())).collect(),
    }
}

const POINT: &str = "---@class Point\n---@field x number\n---@field y number\nlocal Point = {}\n\n";

#[test]
fn a_mapped_type_over_keyof_keeps_or_changes_each_property() {
    let checked = check(&format!(r#"{POINT}---@alias Optional<T> {{ [P in keyof T]?: T[P] }}
---@alias Frozen<T> {{ readonly [P in keyof T]: T[P] }}
---@alias Thawed<T> {{ -readonly [P in keyof T]: T[P] }}
---@alias Complete<T> {{ [P in keyof T]-?: T[P] }}

---@type Optional<Point>
local some = {{ x = 1 }}
---@type Optional<Point>
local wrong = {{ x = "a" }}
---@type Frozen<Point>
local frozen = {{ x = 1, y = 2 }}
frozen.x = 3
---@type Thawed<Frozen<Point>>
local thawed = {{ x = 1, y = 2 }}
thawed.x = 3
---@type Complete<Optional<Point>>
local complete = {{ x = 1 }}
"#));
    assert_eq!(checked.diagnostics, [
        "TS2322 {{ x = \"a\" }} | Type '{ x: \"a\" }' is not assignable to type '{ x?: number, y?: number }'.",
        "TS2540 x | Cannot assign to readonly property \"x\".",
        "TS2322 {{ x = 1 }} | Type '{ x: 1 }' is not assignable to type '{ x: number, y: number }'.",
    ].map(|line| line.replace("{{", "{").replace("}}", "}")));
}

#[test]
fn a_mapped_type_has_a_property_for_each_key_and_may_rename_it() {
    let checked = check(&format!(r#"{POINT}---@alias Table<K, V> {{ [P in K]: V }}
---@alias Getters<T> {{ [P in keyof T as `get_${{P & string}}`]: fun(): T[P] }}

---@type Table<"a" | "b", number>
local counts = {{ a = 1 }}
---@type Getters<Point>
local getters
local x = getters.get_x()
---@type string
local wrong = getters.get_y()
"#));
    assert_eq!(checked.ty("x"), "number");
    assert_eq!(checked.diagnostics, [
        "TS2322 { a = 1 } | Type '{ a: 1 }' is not assignable to type '{ a: number, b: number }'.",
        "TS2322 getters.get_y() | Type 'number' is not assignable to type 'string'.",
    ]);
}

#[test]
fn a_lua_table_has_an_absent_property_as_nil() {
    let checked = check(&format!(r#"{POINT}---@alias Nullable<T> {{ [P in keyof T]: T[P] | nil }}

---@type Nullable<Point>
local half = {{ y = 1 }}
"#));
    assert_eq!(checked.diagnostics, Vec::<String>::new());
}

#[test]
fn a_mapped_type_may_refer_to_the_alias_whose_body_it_is() {
    let checked = check(r#"---@alias DeepPartial<T> { [P in keyof T]?: DeepPartial<T[P]> }
---@alias Settings { window: { width: number, height: number }, title: string }

---@type DeepPartial<Settings>
local overrides = { window = { width = 640 } }
---@type DeepPartial<Settings>
local wrong = { window = { width = "wide" } }
"#);
    assert_eq!(checked.diagnostics.len(), 1, "{:#?}", checked.diagnostics);
    assert!(checked.diagnostics[0].starts_with("TS2322 { window = { width = \"wide\" } } |"), "{:#?}", checked.diagnostics);
}

#[test]
fn a_generic_mapped_type_is_inferred_through_and_instantiated() {
    let checked = check(r#"---@generic T
---@param value T
---@return Readonly<T>
local function freeze(value) return value end
local frozen = freeze({ x = 1 })
frozen.x = 2

---@generic T
---@param defaults Partial<T>
---@return T
local function fill(defaults) return defaults --[[@as any]] end
local filled = fill({ name = "ada" })
local name = filled.name
"#);
    // As TypeScript infers through a reverse mapped type, the literal widens.
    assert_eq!(checked.ty("name"), "string");
    assert_eq!(checked.diagnostics, ["TS2540 x | Cannot assign to readonly property \"x\"."]);
}

#[test]
fn utility_types_are_typescripts() {
    let checked = check(&format!(r#"{POINT}---@type Pick<Point, "x">
local picked = {{ x = "a" }}
---@type Omit<Point, "x">
local omitted = {{ y = 1 }}
---@type Record<"a" | "b", boolean>
local flags = {{ a = true }}
---@type Exclude<"a" | "b" | "c", "a">
local excluded = "a"
---@type Extract<string | number, number>
local extracted = "a"
---@type NonNullable<string | nil>
local present = nil
---@type ReturnType<fun(): number>
local returned = "a"
---@type Pick<Point, "z">
local missing
---@type keyof Partial<Point>
local key = "z"
"#));
    assert_eq!(checked.diagnostics, [
        "TS2322 { x = \"a\" } | Type '{ x: \"a\" }' is not assignable to type '{ x: number }'.",
        "TS2322 { a = true } | Type '{ a: true }' is not assignable to type '{ a: boolean, b: boolean }'.",
        "TS2322 \"a\" | Type '\"a\"' is not assignable to type '\"b\" | \"c\"'.",
        "TS2322 \"a\" | Type '\"a\"' is not assignable to type 'number'.",
        "TS2322 nil | Type 'nil' is not assignable to type 'string'.",
        "TS2322 \"a\" | Type '\"a\"' is not assignable to type 'number'.",
        "TS2344 \"z\" | Type '\"z\"' does not satisfy constraint '\"x\" | \"y\"'.",
        "TS2322 \"z\" | Type '\"z\"' is not assignable to type '\"x\" | \"y\"'.",
    ]);
}

#[test]
fn a_project_type_of_a_utility_types_name_is_the_projects() {
    let checked = check(r#"---@class Record
---@field id number
local Record = {}

---@type Record
local record = { id = "x" }
---@alias Partial<T> string
---@type Partial<number>
local partial = 1
"#);
    assert_eq!(checked.diagnostics, [
        "TS2322 { id = \"x\" } | Type '{ id: \"x\" }' is not assignable to type 'Record'.",
        "TS2322 1 | Type '1' is not assignable to type 'string'.",
    ]);
}

#[test]
fn no_infer_keeps_a_parameter_from_inferring_its_type_parameter() {
    let checked = check(r#"---@generic T: string
---@param values T[]
---@param default NoInfer<T>
---@return T
local function pick(values, default) return default end
local ok = pick({ "a", "b" }, "a")
local wrong = pick({ "a", "b" }, "c")
"#);
    assert_eq!(checked.diagnostics, ["TS2345 \"c\" | Argument of type '\"c\"' is not assignable to parameter of type '\"a\" | \"b\"'."]);
}

#[test]
fn a_constraint_may_follow_extends() {
    let checked = check(r#"---@alias Result<F extends function> F extends fun(...): infer R and R or nil
---@type Result<number>
local bad
---@generic K extends string
---@param key K
---@return K
local function same(key) return key end
same(1)
"#);
    assert_eq!(checked.diagnostics, [
        "TS2344 number | Type 'number' does not satisfy constraint 'function'.",
        "TS2345 1 | Argument of type '1' is not assignable to parameter of type 'string'.",
    ]);
}

#[test]
fn a_record_field_may_be_optional() {
    let checked = check(r#"---@type { x?: number, y: string }
local a = { y = "a" }
---@type { x?: number, y: string }
local b = { x = "s", y = "a" }
---@alias Named { name?: string }
---@type Named
local c = {}
local d = c.name
"#);
    assert_eq!(checked.ty("d"), "nil | string");
    assert_eq!(checked.diagnostics, ["TS2322 { x = \"s\", y = \"a\" } | Type '{ x: \"s\", y: \"a\" }' is not assignable to type '{ x?: number, y: string }'."]);
}
