//! `---@return asserts value is T` and `asserts value`: functions that return only when an
//! argument has a type, or is truthy, and narrow it wherever they return.
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
fn check_with(source: &str, inspections: bool) -> Checked {
    let root = std::env::temp_dir().join(format!("luanalysis assertion functions {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
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

/// With inspections.
fn check(source: &str) -> Checked {
    check_with(source, true)
}

const ASSERTIONS: &str = r#"---@param value any
---@return asserts value is string
local function check_string(value)
    if type(value) ~= "string" then error("expected a string") end
end

---@param value any
---@param message? string
---@return asserts value
local function expect(value, message)
    if not value then error(message or "expected a value") end
end
"#;

#[test]
fn an_assertion_narrows_its_argument_after_the_call() {
    let source = format!("{ASSERTIONS}{}", r#"
---@param a string | number | nil
---@param b string | nil
---@param c boolean | number
local function use(a, b, c)
    check_string(a)
    expect(b, "b")
    expect(c)
    local text, present, truthy = a, b, c
    return text, present, truthy, check_string, expect
end
return use
"#);
    let checked = check(&source);
    assert!(checked.diagnostics.is_empty(), "{:#?}", checked.diagnostics);
    assert_eq!(checked.ty("text"), "string");
    assert_eq!(checked.ty("present"), "string");
    assert_eq!(checked.ty("truthy"), "number | true");
}

#[test]
fn function_types_methods_and_branches_assert_too() {
    let source = r#"---@type fun(value: any): asserts value is number
local check_number = function(value) assert(type(value) == "number") end

---@class Validator
local Validator = {}
---@param value any
---@return asserts value is boolean
function Validator:flag(value) assert(type(value) == "boolean") end

-- The method's receiver has a written type, as a call's effects need.
---@param validator Validator
---@param d string | number
---@param e string | boolean
---@param f string | number
local function use(validator, d, e, f)
    check_number(d)
    validator:flag(e)
    if f ~= "" then check_number(f) end
    local number, boolean, either = d, e, f
    return number, boolean, either
end
return use
"#;
    let checked = check(source);
    assert!(checked.diagnostics.is_empty(), "{:#?}", checked.diagnostics);
    assert_eq!(checked.ty("number"), "number");
    assert_eq!(checked.ty("boolean"), "boolean");
    assert_eq!(checked.ty("either"), "string | number");
}

#[test]
fn an_assertion_returns_nothing_and_what_follows_it_is_reachable() {
    let source = format!("{ASSERTIONS}{}", r#"
---@param n number
local function impossible(n)
    check_string(n)
    local after = n
    return after, expect
end
return impossible
"#);
    let checked = check(&source);
    assert!(checked.diagnostics.is_empty(), "{:#?}", checked.diagnostics);
    assert_eq!(checked.ty("after"), "never");
}

#[test]
fn an_ordinary_predicate_called_as_a_statement_does_not_narrow() {
    let source = r#"---@param value any
---@return value is string
local function is_string(value) return type(value) == "string" end

---@param a string | number
local function use(a)
    is_string(a)
    local unchanged = a
    return unchanged
end
return use
"#;
    let checked = check(source);
    assert_eq!(checked.ty("unchanged"), "string | number");
}

#[test]
fn an_assertion_names_a_parameter_its_type_fits() {
    let source = r#"---@param value any
---@return asserts other is string
local function wrong_name(value) end

---@param value number
---@return asserts value is string
local function wrong_type(value) end

---@param value any
---@return asserts value is string
local function returns(value) return true end
return wrong_name, wrong_type, returns
"#;
    assert_eq!(check_with(source, false).diagnostics, [
        "TS1225 other | Cannot find parameter 'other'.",
        "TS2677 string | A type predicate's type 'string' must be assignable to its parameter's type 'number'.",
        "TS2322 true | Lua result pack is not assignable to the declared result pack. Actual: true. Expected: void.",
    ]);
}

#[test]
fn the_readme_example_checks_as_described() {
    let source = r#"---@param value any
---@return asserts value is string
local function checkString(value)
    if type(value) ~= "string" then error("expected a string") end
end

---@param value string | number
local function shout(value)
    checkString(value)
    return value:upper() -- string
end
return shout
"#;
    assert!(check(source).diagnostics.is_empty(), "{:#?}", check(source).diagnostics);
}
