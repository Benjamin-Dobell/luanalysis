//! Explicit predicate return contracts use the ordinary boolean and flow checker paths.
use luanalysis::project_check::{
    ProjectCheckOptions, check_project_with_options, static_require_requests,
};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis::{CheckReport, check};
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

const IS_STRING: &str = r#"---@param value unknown
---@return value is string
local function isString(value)
    return type(value) == "string"
end
"#;

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

fn one_error(source: &str, code: &str) -> CheckReport {
    let report = check(source);
    assert_eq!(
        report.diagnostics.len(),
        1,
        "{source}\n{:#?}",
        report.diagnostics
    );
    assert_eq!(
        report.diagnostics[0].code, code,
        "{:#?}",
        report.diagnostics
    );
    report
}

fn expression_type(report: &CheckReport, source: &str, declaration: &str, expected: &str) {
    let start = source.find(declaration).unwrap() + declaration.find('=').unwrap() + 1;
    let start = start + source[start..].len() - source[start..].trim_start().len();
    assert_eq!(
        report.type_at(start),
        Some(expected),
        "{declaration}: {report:#?}"
    );
}

fn binding_type(report: &CheckReport, name: &str, expected: &str) {
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == name)
            .map(|binding| binding.ty.as_str()),
        Some(expected),
        "{name}: {report:#?}"
    );
}

#[test]
fn predicates_return_booleans_and_narrow_both_branches() {
    let source = format!(
        r#"{IS_STRING}
---@param value string | number
local function inspect(value)
    local answer = isString(value)
    if isString(value) then
        local positive = value
        ---@type string
        local checked = value
    else
        local negative = value
        ---@type number
        local checked = value
    end
end
"#
    );
    let report = clean(&source);
    binding_type(&report, "answer", "boolean");
    expression_type(&report, &source, "local positive = value", "string");
    expression_type(&report, &source, "local negative = value", "number");

    let wrong = source.replace(
        "---@type string\n        local checked",
        "---@type number\n        local checked",
    );
    one_error(&wrong, "TS2322");
}

#[test]
fn negated_predicates_and_early_returns_use_the_opposite_flow_fact() {
    let source = format!(
        r#"{IS_STRING}
---@param value string | number
local function inspect(value)
    if not isString(value) then
        local negative = value
        return value + 1
    end
    local positive = value
    ---@type string
    local checked = value
    return checked
end
"#
    );
    let report = clean(&source);
    expression_type(&report, &source, "local negative = value", "number");
    expression_type(&report, &source, "local positive = value", "string");
}

#[test]
fn asserting_a_predicate_narrows_the_original_argument_after_the_assertion() {
    let source = format!(
        r#"{IS_STRING}
---@param value string | number
local function inspect(value)
    assert(isString(value))
    local positive = value
    return value:upper()
end
"#
    );
    let report = Fixture::new().check_source_with_lua54(&source);
    expression_type(&report, &source, "local positive = value", "string");
}

#[test]
fn comparing_predicates_with_booleans_narrows_both_branches() {
    for (condition, positive, negative) in [
        ("isString(value) == true", "string", "number"),
        ("isString(value) == false", "number", "string"),
        ("isString(value) ~= true", "number", "string"),
        ("true == isString(value)", "string", "number"),
    ] {
        let source = format!(
            r#"{IS_STRING}
---@param value string | number
local function inspect(value)
    if {condition} then
        local positive = value
        ---@type {positive}
        local checked = value
    else
        local negative = value
        ---@type {negative}
        local checked = value
    end
end
"#
        );
        let report = clean(&source);
        expression_type(&report, &source, "local positive = value", positive);
        expression_type(&report, &source, "local negative = value", negative);
    }
}

#[test]
fn ordinary_literal_return_calls_keep_their_contract_and_conservative_branch_reachability() {
    for literal in ["true", "false"] {
        let source = format!(
            r#"---@return {literal}
local function answer() return {literal} end
local literalResult = answer()
---@type {literal}
local literalChecked = literalResult
local function choose()
    if answer() then
        return 42
    else
        return 'other'
    end
end
local result = choose()
---@type number | string
local checked = result
"#
        );
        let report = clean(&source);
        binding_type(&report, "literalResult", "boolean");
        binding_type(&report, "literalChecked", literal);
        // A call's result contract does not make its source recipe a literal condition.
        // Preserve the preexisting conservative reachability of ordinary calls.
        let result = &report
            .bindings
            .iter()
            .find(|binding| binding.name == "result")
            .unwrap()
            .ty;
        assert!(
            result.contains("number") && result.contains("string"),
            "{result}"
        );
        one_error(
            &source.replace(
                "---@type number | string\nlocal checked",
                "---@type number\nlocal checked",
            ),
            "TS2322",
        );
    }
}

#[test]
fn boolean_comparisons_of_ordinary_calls_do_not_erase_correlated_alternatives() {
    let source = "---@return string\nlocal function ordinary() return 'text' end\n---@return (true, number) | (false, string)\nlocal function pair() return true, 1 end\nlocal ok, value = pair()\nif ok and ordinary() == true then\nelse\n    ---@type string\n    local stale = value\nend\n";
    for condition in [
        "ordinary() == true",
        "ordinary() == false",
        "ordinary() ~= true",
        "ordinary() ~= false",
        "not ordinary() == true",
    ] {
        let source = source.replace("ordinary() == true", condition);
        one_error(&source, "TS2322");
        clean(&source.replace(
            "---@type string\n    local stale",
            "---@type number | string\n    local stale",
        ));
    }
}

#[test]
fn predicate_bodies_are_checked_as_boolean_returns() {
    let wrong = IS_STRING.replace("type(value) == \"string\"", "42");
    let report = one_error(&wrong, "TS2322");
    let diagnostic = &report.diagnostics[0];
    assert_eq!(&wrong[diagnostic.span.start..diagnostic.span.end], "42");
    clean(IS_STRING);
}

#[test]
fn predicate_subjects_must_be_parameters_and_targets_must_fit_the_parameter() {
    one_error(
        &IS_STRING.replace("value is string", "missing is string"),
        "TS1225",
    );
    one_error(
        &IS_STRING.replace("value unknown", "value number"),
        "TS2677",
    );
    clean(IS_STRING);
}

#[test]
fn function_type_predicates_narrow_higher_order_callback_arguments() {
    let source = format!(
        r#"{IS_STRING}
---@param value string | number
---@param guard fun(candidate: string | number): candidate is string
local function inspect(value, guard)
    if guard(value) then
        local positive = value
        ---@type string
        local checked = value
    else
        local negative = value
        ---@type number
        local checked = value
    end
end
inspect("word", isString)
"#
    );
    let report = clean(&source);
    expression_type(&report, &source, "local positive = value", "string");
    expression_type(&report, &source, "local negative = value", "number");
}

#[test]
fn generic_higher_order_calls_infer_the_predicates_target_type() {
    let source = format!(
        r#"{IS_STRING}
---@generic T
---@param guard fun(value: unknown): value is T
---@param value unknown
---@return T | nil
local function select(guard, value)
    if guard(value) then
        return value
    end
    return nil
end
local selected = select(isString, 'word')
---@type string | nil
local checked = selected
"#
    );
    let report = clean(&source);
    let selected = &report
        .bindings
        .iter()
        .find(|binding| binding.name == "selected")
        .unwrap()
        .ty;
    assert!(
        selected.contains("string") && selected.contains("nil"),
        "{selected}"
    );
    assert!(
        !selected.contains("any") && !selected.contains("unknown"),
        "{selected}"
    );
    one_error(
        &source.replace(
            "---@type string | nil\nlocal checked",
            "---@type number\nlocal checked",
        ),
        "TS2322",
    );
}

#[test]
fn plain_boolean_functions_do_not_satisfy_predicate_callback_contracts() {
    let boolean_only = r#"---@param value unknown
---@return boolean
local function booleanOnly(value)
    return type(value) == 'string'
end
"#;
    one_error(
        &format!(
            "{boolean_only}---@type fun(value: unknown): value is string\nlocal guard = booleanOnly\n"
        ),
        "TS2322",
    );
    one_error(
        &format!(
            r#"{boolean_only}---@param guard fun(value: unknown): value is string
local function consume(guard)
    return guard('word')
end
consume(booleanOnly)
"#
        ),
        "TS2345",
    );
}

#[test]
fn nominal_subclass_predicates_narrow_instances_without_inventing_a_discriminated_union() {
    let source = r#"---@class Object
---@field type string

---@class Bag : Object
---@field capacity number
---@field getQuantity fun(self: Bag): number

---@param obj Bag
---@return number
local function acceptsBag(obj)
    return obj:getQuantity()
end

---@param obj Object
---@return obj is Bag
local function isBag(obj)
    return obj.type == 'Bag'
end

---@param obj Object
local function inspect(obj)
    if isBag(obj) then
        local positive = obj
        ---@type number
        local capacity = obj.capacity
        ---@type number
        local quantity = obj:getQuantity()
        acceptsBag(obj)
    else
        local negative = obj
    end
end
"#;
    let report = clean(source);
    expression_type(&report, source, "local positive = obj", "Bag");
    expression_type(&report, source, "local negative = obj", "Object");

    let wrong_false_branch = source.replace("local negative = obj", "acceptsBag(obj)");
    one_error(&wrong_false_branch, "TS2345");
    let direct_comparison = source.replace("if isBag(obj) then", "if obj.type == 'Bag' then");
    let report = check(&direct_comparison);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "TS2345"),
        "a string field comparison cannot establish nominal Bag identity: {:#?}",
        report.diagnostics
    );
    expression_type(
        &report,
        &direct_comparison,
        "local positive = obj",
        "Object",
    );
}

#[test]
fn generic_predicate_targets_use_the_selected_calls_type_arguments() {
    let source = r#"---@generic T
---@param value T | nil
---@return value is T
local function isPresent(value)
    return value ~= nil
end
---@param text string | nil
---@param count number | nil
local function inspect(text, count)
    if isPresent(text) then
        local presentText = text
    else
        local absentText = text
    end
    if isPresent(count) then
        local presentCount = count
    end
end
"#;
    let report = clean(source);
    expression_type(&report, source, "local presentText = text", "string");
    expression_type(&report, source, "local absentText = text", "nil");
    expression_type(&report, source, "local presentCount = count", "number");
}

#[test]
fn overloaded_predicates_use_the_resolved_head() {
    let source = r#"---@overload fun(value: string | number, mode: 'text'): value is string
---@overload fun(value: string | number, mode: 'number'): value is number
---@param value string | number
---@param mode 'text' | 'number'
---@return boolean
local function matches(value, mode)
    if mode == 'text' then
        return type(value) == 'string'
    end
    return type(value) == 'number'
end
---@param value string | number
local function inspect(value)
    if matches(value, 'text') then
        local text = value
    else
        local nonText = value
    end
    if matches(value, 'number') then
        local numeric = value
    end
end
"#;
    let report = clean(source);
    expression_type(&report, source, "local text = value", "string");
    expression_type(&report, source, "local nonText = value", "number");
    expression_type(&report, source, "local numeric = value", "number");
}

#[test]
fn method_predicates_account_for_the_implicit_receiver() {
    let source = r#"local guards = {}
---@param value string | number
---@return value is string
function guards:isString(value)
    return type(value) == 'string'
end
---@param value string | number
local function inspect(value)
    if guards:isString(value) then
        local colon = value
    end
    if guards.isString(guards, value) then
        local dot = value
    end
end
"#;
    let report = clean(source);
    expression_type(&report, source, "local colon = value", "string");
    expression_type(&report, source, "local dot = value", "string");
}

#[test]
fn explicit_self_predicate_targets_the_parameter_shadowing_the_implicit_receiver() {
    let source = r#"local guards = {}
---@param self unknown
---@return self is string
function guards:isString(self)
    return type(self) == 'string'
end
---@param value string | number
local function inspect(value)
    if guards:isString(value) then
        local explicit = value
    else
        local negative = value
    end
    if guards.isString(guards, value) then
        local dot = value
    end
end
"#;
    let report = clean(source);
    expression_type(&report, source, "local explicit = value", "string");
    expression_type(&report, source, "local negative = value", "number");
    expression_type(&report, source, "local dot = value", "string");
}

#[test]
fn writes_invalidate_predicate_facts_and_boolean_views_do_not_invent_them() {
    let source = format!(
        r#"{IS_STRING}
---@param value string | number
local function inspect(value)
    if isString(value) then
        value = 42
        ---@type string
        local stale = value
    end
end
"#
    );
    one_error(&source, "TS2322");

    let erased = format!(
        r#"{IS_STRING}
---@type fun(value: unknown): boolean
local ordinary = isString
---@param value string | number
local function inspect(value)
    if ordinary(value) then
        ---@type string
        local invented = value
    end
end
"#
    );
    one_error(&erased, "TS2322");
}

#[test]
fn later_call_arguments_cannot_apply_a_predicate_to_a_value_replaced_during_the_call() {
    let source = r#"---@param value unknown
---@param ignored number
---@return value is string
local function isStringWithIgnored(value, ignored)
    return type(value) == 'string'
end
---@param value string | number
local function inspect(value)
    ---@return number
    local function replace()
        value = 42
        return 0
    end
    if isStringWithIgnored(value, replace()) then
        ---@type string
        local stale = value
    end
end
"#;
    one_error(source, "TS2322");
    let unary = source
        .replace("    ---@return number\n    local function replace()", "    ---@param ignored number\n    ---@return number\n    local function replace(ignored)")
        .replace("replace()) then", "replace(1)) then");
    one_error(&unary, "TS2322");
    clean(&source.replace(
        "isStringWithIgnored(value, replace())",
        "isStringWithIgnored(value, 0)",
    ));
}

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis type predicates {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, source: &str) {
        fs::write(self.0.join(name), source).unwrap();
    }
    fn check_source_with_lua54(&self, source: &str) -> CheckReport {
        self.file("main.lua", source);
        let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
        let project = Project::load_plan(
            &resolver,
            ProjectLoadPlan::new([self.0.join("main.lua")]),
            static_require_requests,
        );
        let report = check_project_with_options(
            project,
            ProjectCheckOptions {
                environment: StandardEnvironmentProfile::Lua54,
                ..Default::default()
            },
        );
        assert!(!report.has_errors(), "{report:#?}");
        report
            .file(report.project.entries[0])
            .unwrap()
            .report
            .clone()
    }
    fn check(&self, expected_exit: i32) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .current_dir(&self.0)
            .args(["check", "--format", "json", "main.lua"])
            .output()
            .unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            output.status.code(),
            Some(expected_exit),
            "{stdout}\n{stderr}"
        );
        assert!(stderr.is_empty(), "{stderr}");
        stdout
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn required_guards_preserve_their_predicate_across_module_boundaries() {
    let fixture = Fixture::new();
    fixture.file("guard.lua", &format!("{IS_STRING}return isString\n"));
    let source = r#"local isString = require('guard')
---@param value string | number
local function inspect(value)
    if isString(value) then
        ---@type string
        local checked = value
    else
        ---@type number
        local checked = value
    end
end
"#;
    fixture.file("main.lua", source);
    assert!(fixture.check(0).contains("\"diagnostics\":[]"));
    fixture.file(
        "main.lua",
        &source.replace("---@type string", "---@type boolean"),
    );
    assert!(fixture.check(1).contains("\"code\":\"TS2322\""));
    fixture.file("main.lua", source);
    fixture.check(0);
}
