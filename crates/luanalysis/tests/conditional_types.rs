//! Conditional types, EmmyLua Analyzer's `T extends U and X or Y` (TypeScript's
//! `T extends U ? X : Y`), with `infer`, as TypeScript gives them.
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
    let root = std::env::temp_dir().join(format!("luanalysis conditional types {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
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

#[test]
fn a_conditional_type_is_its_true_branch_where_the_check_type_extends_the_constraint() {
    let checked = check(r#"---@alias IsString<T> T extends string and true or false

---@type IsString<"a">
local yes = true
---@type IsString<1>
local no = false
---@type IsString<1>
local wrong = true
"#);
    assert_eq!(checked.ty("yes"), "true");
    assert_eq!(checked.ty("no"), "false");
    assert_eq!(checked.diagnostics, ["TS2322 true | Type 'true' is not assignable to type 'false'."]);
}

#[test]
fn a_naked_type_parameter_distributes_over_a_union() {
    let checked = check(r#"---@alias NonNil<T> T extends nil and never or T
---@alias Without<T, U> T extends U and never or T

---@type NonNil<string | nil>
local present = nil
---@type Without<"a" | "b" | "c", "a">
local remaining = "a"
"#);
    assert_eq!(checked.diagnostics, [
        "TS2322 nil | Type 'nil' is not assignable to type 'string'.",
        "TS2322 \"a\" | Type '\"a\"' is not assignable to type '\"b\" | \"c\"'.",
    ]);
}

#[test]
fn infer_declares_a_type_parameter_that_the_true_branch_sees() {
    let checked = check(r#"---@alias ElementOf<T> T extends (infer U)[] and U or never
---@alias ValueOf<T> T extends { value: infer V } and V or nil
---@alias FirstParameter<F> F extends fun(x: infer A, ...) and A or nil
---@alias ReturnOf<F> F extends fun(...): infer R and R or nil
---@alias Unwrap<T> T extends (infer U)[] and U or T

---@type ElementOf<string[]>
local element = 1
---@type ValueOf<{ value: number }>
local value = "x"
---@type FirstParameter<fun(x: string)>
local parameter = 1
---@type ReturnOf<fun(): number>
local result = "x"
---@type Unwrap<string[] | number>
local unwrapped = true
"#);
    assert_eq!(checked.diagnostics, [
        "TS2322 1 | Type '1' is not assignable to type 'string'.",
        "TS2322 \"x\" | Type '\"x\"' is not assignable to type 'number'.",
        "TS2322 1 | Type '1' is not assignable to type 'string'.",
        "TS2322 \"x\" | Type '\"x\"' is not assignable to type 'number'.",
        "TS2322 true | Type 'true' is not assignable to type 'string | number'.",
    ]);
}

#[test]
fn a_conditional_type_in_the_false_branch_chains() {
    let checked = check(r#"---@alias Kind<T> T extends string and "s" or T extends number and "n" or "o"

---@type Kind<1>
local number = "n"
---@type Kind<true>
local other = "s"
"#);
    assert_eq!(checked.diagnostics, ["TS2322 \"s\" | Type '\"s\"' is not assignable to type '\"o\"'."]);
}

#[test]
fn a_generic_conditional_type_is_deferred_until_its_check_type_is_known() {
    let checked = check(r#"---@generic T
---@param x T
---@return T extends string and number or boolean
local function pick(x) return nil --[[@as any]] end
local n = pick("a")
local b = pick(1)

---@generic T
---@param x T
---@return T extends string and "s" or "n"
local function kind(x)
  return "s"
end

---@generic T
---@param x T
local function use(x)
  ---@type "s" | "n"
  local either = kind(x)
  ---@type "s"
  local only = kind(x)
end
"#);
    assert_eq!(checked.ty("n"), "number");
    assert_eq!(checked.ty("b"), "boolean");
    assert_eq!(checked.diagnostics, [
        "TS2322 \"s\" | Lua result pack is not assignable to the declared result pack. Actual: \"s\". Expected: T extends string and \"s\" or \"n\".",
        "TS2322 kind(x) | Type 'T extends string and \"s\" or \"n\"' is not assignable to type '\"s\"'.",
    ]);
}

#[test]
fn a_branch_may_refer_to_the_alias_it_is_the_body_of() {
    let checked = check(r#"---@alias Flatten<T> T extends (infer U)[] and Flatten<U> or T
---@alias Forever<T> T extends string and Forever<T> or nil

---@type Flatten<string[][][]>
local flat = 1
---@type Forever<"a">
local forever

---@generic T
---@param x T
---@return Forever<T>
local function loop(x) return nil --[[@as any]] end
local function run()
  local looped = loop("c")
  return looped
end
"#);
    assert_eq!(checked.diagnostics, [
        "TS2322 1 | Type '1' is not assignable to type 'string'.",
        "TS2589 Forever<\"a\"> | Type instantiation is excessively deep and possibly infinite.",
        "TS2589 loop(\"c\") | Type instantiation is excessively deep and possibly infinite.",
    ]);
}

#[test]
fn infer_is_only_seen_in_the_extends_type_and_true_branch() {
    let checked = check(r#"---@alias BadScope<T> T extends (infer U)[] and U or U
---@alias Unused<T> T extends string and Missing or nil

---@type BadScope<number>
local bad
---@type infer X
local outside
"#);
    assert_eq!(checked.diagnostics, [
        "TS2304 U | Cannot find name 'U'.",
        "TS2304 Missing | Cannot find name 'Missing'.",
        "TS1338 infer X | 'infer' declarations are only permitted in the 'extends' clause of a conditional type.",
    ]);
}

#[test]
fn infer_may_be_constrained() {
    let checked = check(r#"---@alias FirstString<T> T extends (infer S extends string)[] and S or nil
---@alias Direct<T> T extends infer U extends string and U or nil
---@alias Nested<T> T extends (infer U extends string and 1 or 2) and T or 3
---@alias Circular<T> T extends (infer U extends U)[] and U or nil

---@type FirstString<"a"[]>
local first = "b"
---@type FirstString<number[]>
local none = 1
---@type Direct<"x">
local direct = "y"
---@type Nested<4>
local nested = 5
---@type Circular<string[]>
local circular
"#);
    assert_eq!(checked.diagnostics, [
        "TS2313 U | Type parameter 'U' has a circular constraint.",
        "TS2322 \"b\" | Type '\"b\"' is not assignable to type '\"a\"'.",
        "TS2322 1 | Type '1' is not assignable to type 'nil'.",
        "TS2322 \"y\" | Type '\"y\"' is not assignable to type '\"x\"'.",
        "TS2322 5 | Type '5' is not assignable to type '3'.",
    ]);
}

#[test]
fn a_generic_function_type_in_the_constraint_is_compared_as_typescript_compares_it() {
    let checked = check(r#"---@alias Applied<F> F extends fun<A>(a: A): infer R and R or nil
---@alias ReturnOf<F> F extends fun(...): infer R and R or nil

---@type Applied<fun(a: number): string>
local specific = 1
---@type Applied<fun<B>(b: B): B>
local generic = 1
---@type ReturnOf<fun<B>(b: B): B[]>
local array = 1
"#);
    assert_eq!(checked.diagnostics, [
        "TS2322 1 | Type '1' is not assignable to type 'nil'.",
        "TS2322 1 | Type '1' is not assignable to type 'unknown[]'.",
    ]);
}

#[test]
fn a_class_field_may_be_conditional_on_the_class_type_parameter() {
    let checked = check(r#"---@class Box<T>
---@field value T extends string and "str" or "other"
local Box = {}

---@type Box<string>
local box
---@type "other"
local value = box.value

---@class List<T>
local List = {}
---@param value T extends number and T or nil
function List:push(value) end

---@type List<string>
local list
list:push(1)
"#);
    assert_eq!(checked.diagnostics, [
        "TS2322 box.value | Type '\"str\"' is not assignable to type '\"other\"'.",
        "TS2345 1 | Argument of type '1' is not assignable to parameter of type 'nil'.",
    ]);
}

#[test]
fn extends_without_a_conditional_after_it_stays_prose() {
    let checked = check(r#"---@return string extends the base name
local function name() return "a" end
local n = name()
"#);
    assert_eq!(checked.ty("n"), "string");
    assert_eq!(checked.diagnostics, Vec::<String>::new());
}

#[test]
fn every_task_reads_a_branch_where_its_alias_is_declared() {
    // Each of these files is checked by a task of its own. A task that reads the alias's
    // conditional type, rather than lowering it, still lowers its branches in the alias,
    // where `T` is its parameter.
    let root = std::env::temp_dir().join(format!("luanalysis conditional types {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&root).unwrap();
    let aliases = root.join("aliases.lua");
    fs::write(&aliases, "---@alias Listed<T> T extends string and T[] or { value: T }\n").unwrap();
    let users: Vec<PathBuf> = (0..8).map(|index| {
        let path = root.join(format!("user{index}.lua"));
        fs::write(&path, "---@type Listed<\"a\">\nlocal list = { \"b\" }\n---@type Listed<number>\nlocal boxed = { value = \"c\" }\n").unwrap();
        path
    }).collect();
    let project = Project::load_plan(&Resolver::new(ProjectConfig::new([root.clone()])).unwrap(), ProjectLoadPlan::new(std::iter::once(aliases).chain(users)), static_require_requests);
    let report = check_project_with_options(project, ProjectCheckOptions {
        environment: StandardEnvironmentProfile::Lua54,
        jobs: 4,
        ..ProjectCheckOptions::default()
    });
    fs::remove_dir_all(&root).unwrap();
    assert_eq!(report.project.entries.len(), 9);
    for &entry in &report.project.entries[1..] {
        let file = report.file(entry).unwrap();
        let messages: Vec<_> = file.report.diagnostics.iter().map(|diagnostic| (diagnostic.code, diagnostic.message.as_str())).collect();
        assert_eq!(messages, [
            ("TS2322", "Type '{ [1]: \"b\" }' is not assignable to type '\"a\"[]'."),
            ("TS2322", "Type '{ value: \"c\" }' is not assignable to type '{ value: number }'."),
        ]);
    }
}
