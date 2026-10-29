//! Regression tests for native Lua type and diagnostic semantics.
#[allow(dead_code)] // The shared full-suite host also exposes fields unused by these controls.
#[path = "support/legacy.rs"]
mod legacy;
use luanalysis::project_check::{
    ProjectCheckOptions, check_project_with_options, static_require_requests,
};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis::{CheckReport, OverloadMode};
use luanalysis_project::{Project, ProjectConfig, Resolver};
use std::fs;

fn check(source: &str) -> CheckReport {
    let temp = legacy::Temp::new();
    let path = temp.0.join("main.lua");
    fs::write(&path, source).unwrap();
    let resolver = Resolver::new(ProjectConfig::new([temp.0.clone()])).unwrap();
    let project = Project::load(&resolver, [path], static_require_requests);
    let entry = project.entries[0];
    let report = check_project_with_options(
        project,
        ProjectCheckOptions {
            jobs: 1,
            environment: StandardEnvironmentProfile::Lua53,
            overload_mode: OverloadMode::LegacyExposeImplementation,
            describe_types: false,
            ..Default::default()
        },
    );
    report.file(entry).unwrap().report.clone()
}
fn errors(source: &str) -> Vec<(String, String)> {
    let report = check(source);
    report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.to_owned(),
                source[d.span.start..d.span.end].to_owned(),
            )
        })
        .collect()
}
#[track_caller]
fn exact(source: &str, expected: &[(&str, &str)]) {
    assert_eq!(
        errors(source),
        expected
            .iter()
            .map(|(code, text)| (code.to_string(), text.to_string()))
            .collect::<Vec<_>>(),
        "source:\n{source}"
    );
}

#[test]
fn optional_parameter_may_be_absent_but_takes_no_nil_it_does_not_declare() {
    // As v1's function_optional_parameters.lua expects: "Required: 'string' Found: 'nil | string'".
    // Within its function the parameter may be nil, so it is no argument for itself.
    exact(
        r#"---@param value? string
---@return string
local function optional(value) return optional(value) end
---@type string|nil
local nullable
optional()
optional(nil)
optional(nullable)
optional(42)
"#,
        &[("TS2345", "value"), ("TS2345", "nil"), ("TS2345", "nullable"), ("TS2345", "42")],
    );
}
#[test]
fn untyped_callback_extra_parameters_are_optional_but_written_contracts_are_checked() {
    exact(
        r#"---@param callback fun(value:number)
local function each(callback) end
each(function(value, extra) return true end)
---@param value number
---@param extra boolean
local function written(value, extra) return true end
each(written)
"#,
        &[("TS2345", "written")],
    );
}
#[test]
fn alias_and_boolean_flow_narrowing_do_not_weaken_arbitrary_parameter_contracts() {
    exact(
        r#"---@class Item
---@field a string

---@alias Choice string|Item
---@type string
local text
---@type Choice
local choice
choice=text
---@type string
local accepted=choice
---@param value Choice
local function unknown(value)
---@type string
local rejected=value
end
"#,
        &[("TS2322", "value")],
    );
    exact(
        r#"---@type boolean
local flag
flag=false
---@type false
local accepted=flag
---@param value boolean
local function unknown(value)
---@type false
local rejected=value
end
"#,
        &[("TS2322", "value")],
    );
    exact(
        r#"---@alias Text {[1]:0,[2]:string}
---@alias Numeric {[1]:1,[2]:number}
---@type Text|Numeric
local selected={1,42}
---@type 1
local tag=selected[1]
selected[1]=1
---@param value Text|Numeric
local function unknown(value) value[1]=1 end
"#,
        &[("TS2322", "1")],
    );
}
#[test]
fn aggregate_record_errors_independently_reject_every_wrong_and_missing_field() {
    exact(
        r#"---@shape Pair
---@field a number
---@field b string
---@type Pair
local both={a='bad',b=42}
---@type Pair
local first={a='bad',b='ok'}
---@type Pair
local second={a=1,b=42}
---@type Pair
local absent={a=1}
---@type Pair
local valid={a=1,b='ok'}
"#,
        &[
            ("TS2322", "{a='bad',b=42}"),
            ("TS2322", "{a='bad',b='ok'}"),
            ("TS2322", "{a=1,b=42}"),
            ("TS2322", "{a=1}"),
        ],
    );
    exact(
        r#"---@alias Choice {tag:'text',value:string}|{tag:'number',value:number}
---@type Choice
local invalid={tag='number',value={}}
---@type Choice
local valid={tag='number',value=1}
"#,
        &[("TS2322", "{tag='number',value={}}")],
    );
}
#[test]
fn inheritance_cycles_are_rejected_once_per_component_and_acyclic_bases_are_valid() {
    exact(
        r#"---@class A:B
---@class B:C
---@class C:A
"#,
        &[("TS2310", "B")],
    );
    exact(
        r#"---@class A:B
---@class B:C
---@class C
"#,
        &[],
    );
}
#[test]
fn name_errors_do_not_need_a_duplicate_call_error_but_real_noncallables_are_checked() {
    exact(
        r#"missing()
local numeric=1
numeric()
"#,
        &[("TS2304", "missing"), ("TS2349", "numeric")],
    );
}
#[test]
fn overload_omission_must_admit_physical_nil_in_the_private_implementation() {
    exact(
        r#"---@overload fun(value:number):boolean
---@param value number
---@param extra number
---@return boolean
local function invalid(value,extra) return true end
---@overload fun(value:number):boolean
---@param value number
---@param extra number|nil
---@return boolean
local function valid(value,extra) return true end
"#,
        &[("TS2394", "@overload fun(value:number):boolean")],
    );
}
#[test]
fn generic_shadowing_has_fresh_constraints_and_circular_bounds_are_rejected_at_declaration() {
    exact(
        r#"---@generic T:number
local function outer()
---@generic T:string
---@param value T
---@return T
local function inner(value) return value end
inner('ok')
inner(42)
end
"#,
        &[("TS2345", "42")],
    );
    exact(
        r#"---@generic A:B,B:A
---@param a A
---@param b B
local function invalid(a,b) end
invalid(1,1)
"#,
        &[("TS2313", "A"), ("TS2313", "B")],
    );
    exact(
        r#"---@generic T
---@param first T
---@param second T
---@return T
local function same(first,second) return first end
same(1,'text')
"#,
        &[("TS2345", "'text'")],
    );
}
#[test]
fn lua_assignment_adjustment_and_rhs_error_locations_are_preserved() {
    exact(
        r#"---@return number,string
local function pair() return 1,'ok' end
---@type string,number
local text,count=pair()
"#,
        &[("TS2322", "pair()"), ("TS2322", "pair()")],
    );
    exact(
        r#"---@return number,string
local function pair() return 1,'ok' end
local first=pair()
local a,b,c=pair()
"#,
        &[],
    );
    exact(
        r#"---@type boolean|nil
local first
---@type boolean|nil
local second
---@vararg number
local function write(...) first,second=... end
"#,
        &[("TS2322", "...")],
    );
}
#[test]
fn explicit_call_arity_and_a_possibly_empty_variadic_argument_pack_remain_checked() {
    exact(
        r#"---@return number,string,boolean
local function three() return 1,'ok',true end
---@param first number
---@param second string
local function two(first,second) end
two(three())
"#,
        &[("TS2554", "two(three())")],
    );
    exact(
        r#"---@param value number|nil
local function need(value) end
---@vararg number
local function forward(...) need(...) end
"#,
        &[("TS2554", "need(...)")],
    );
}
#[test]
fn a_callback_result_generic_takes_the_whole_result_list_in_every_row_that_names_it() {
    // v1's `TyMultipleResults.substitute` splices `T`'s result list into each row, as v1's std
    // declares `pcall`: `---@return (true, T) | (false, any)`.
    exact(
        r#"---@return number, string
local function two() return 1, "x" end
---@generic T
---@param f fun: T
---@return (true, T)
local function forward(f) return true, f() end
local forwarded, c, d = forward(two)
---@type number
local third = c
---@type string
local fourth = d
---@generic T
---@param f fun: T
---@return (true, T) | (false, string)
local function try(f) if f then return true, f() end return false, "no" end
local ok, a, b = try(two)
---@type number | string
local first = a
---@type string | nil
local second = b
---@type number
local wrongFirst = a
---@type string
local wrongSecond = b
"#,
        &[("TS2322", "a"), ("TS2322", "b")],
    );
    exact(
        r#"---@return number, string
local function two() return 1, "x" end
local ok, e, g = pcall(two)
---@type string | nil
local second = g
---@type string
local present = g
---@type nil
local absent = g
"#,
        &[("TS2322", "g"), ("TS2322", "g")],
    );
}
#[test]
fn a_callback_result_generic_in_a_union_of_one_result_is_its_whole_result_list() {
    // v1 substitutes `string | T` as `string` or T's whole result list, so the body may return the
    // callback's results and a caller receives each of them.
    exact(
        r#"---@return number, boolean
local function two() return 1, true end
---@generic T
---@param f fun: T
---@return string | T
local function orString(f) if f then return f() end return "x" end
---@generic T
---@param f fun: T
---@return T | nil
local function orNil(f) if f then return f() end return nil end
local first, second = orString(two)
---@type string | number
local one = first
---@type boolean | nil
local other = second
---@type boolean
local present = second
local value, flag = orNil(two)
---@type number | nil
local maybe = value
---@type boolean | nil
local maybeFlag = flag
"#,
        &[("TS2322", "second")],
    );
}
#[test]
fn a_void_callback_overload_does_not_take_a_callback_that_returns_values() {
    // v1's function_multiple_returns.lua, lines 112-124: a function returning values is
    // assignable to `fun: void`, as in TypeScript, but the call prefers the generic signature that
    // takes its results, as v1 does. A callback with no results still takes the void overload.
    exact(
        r#"---@return 1, 2, 3
local function returns123() return 1, 2, 3 end
---@type string
local aString = "s"
---@overload fun(f: (fun: void), ...: any): nil|string
---@generic T
---@param f fun: T
---@return string | T
local function returnStringOrGeneric(f, ...) return aString end
local inferredNumberOrString = returnStringOrGeneric(returns123)
---@type 1 | string
local numberOrString = inferredNumberOrString
---@type number
local aNumber = inferredNumberOrString
local none = returnStringOrGeneric(function() end)
---@type nil | string
local nilOrString = none
---@type string
local notNil = none
"#,
        &[("TS2322", "inferredNumberOrString"), ("TS2322", "none")],
    );
}
#[test]
fn array_deletion_and_commented_out_documentation_have_explicit_native_policies() {
    exact(
        r#"---@type number[]
local values={1,2,3}
values[1]=nil
values[1]='bad'
"#,
        &[("TS2322", "'bad'")],
    );
    exact(
        r#"----@type fun<T>(index:number,...:T):T
local broken
broken(1,1)
---@type fun<T>(index:number,...:T):T
local corrected
corrected(1,1)
"#,
        &[("TS2349", "broken")],
    );
}
#[test]
fn correcting_the_original_dictionary_completion_typo_exercises_all_original_member_assertions() {
    let case = legacy::cases()
        .into_iter()
        .find(|case| case.id == "TestCompletion/testAnnotationDict")
        .unwrap();
    let directory = legacy::Temp::new();
    let result = legacy::run_with_sources(&case, &directory.0, |input, source| {
        if input.ends_with("testAnnotationDict.lua") {
            assert_eq!(source.matches("pair(").count(), 1);
            source.replace("pair(", "pairs(")
        } else {
            source
        }
    });
    assert!(result.passed, "{}", result.detail);
}
