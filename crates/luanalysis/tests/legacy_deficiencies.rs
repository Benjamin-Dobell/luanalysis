//! Rules the legacy audit found missing, as Luanalysis v1 reads them unless TypeScript or Lua
//! argue otherwise (see README's Compatibility section for the deliberate differences).
use luanalysis::project_check::{ProjectCheckOptions, check_project_with_options, static_require_requests};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{fs, process::Command, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn report(source: &str, extra: &[&str]) -> String {
    let directory = std::env::temp_dir().join(format!("luanalysis legacy deficiencies {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("main.lua"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&directory)
        .args(["check", "--lua-version", "5.3"]).args(extra).arg("main.lua").output().unwrap();
    fs::remove_dir_all(directory).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

fn errors(report: &str) -> Vec<&str> {
    report.lines().filter(|line| line.contains(": error[")).collect()
}

#[test]
fn an_unannotated_override_takes_a_written_inherited_type_and_keeps_its_own_beside_an_inferred_one() {
    let source = "---@type number\nlocal anyNumber\n---@type 1\nlocal one\n\
---@class Written\n---@field value number\n---@field method fun(): number\nlocal Written = {}\n\
---@class Inferred\nlocal Inferred = { value = anyNumber }\n\
---@class FromWritten : Written\nlocal FromWritten = { value = 1, method = function() return one end }\n\
---@class AssignedFromWritten : Written\nlocal AssignedFromWritten = {}\nAssignedFromWritten.value = one\n\
---@class DeclaredFromWritten : Written\nlocal DeclaredFromWritten = {}\nfunction DeclaredFromWritten.method() return one end\n\
---@class FromInferred : Inferred\nlocal FromInferred = { value = 1 }\n\
---@class AssignedFromInferred : Inferred\nlocal AssignedFromInferred = {}\nAssignedFromInferred.value = one\n\
---@type FromWritten\nlocal a\n---@type DeclaredFromWritten\nlocal c\n---@type FromInferred\nlocal d\n---@type AssignedFromInferred\nlocal e\n\
one = a.value\none = a.method()\none = c.method()\na.value = anyNumber\none = d.value\ne.value = anyNumber\n";
    let report = report(source, &[]);
    let errors = errors(&report);
    // Over a written member: the inherited `number`, for reads and writes.
    for line in [32, 33, 34] { assert!(errors.iter().any(|error| error.starts_with(&format!("main.lua:{line}:7: error[TS2322]"))), "{line}\n{report}"); }
    // Over an inferred member: the child's own `1`.
    assert!(errors.iter().any(|error| error.starts_with("main.lua:37:11: error[TS2322]")), "{report}");
    assert_eq!(errors.len(), 4, "{report}");
}

#[test]
fn a_subclass_overriding_inferred_defaults_is_still_its_parent() {
    let source = "---@class Animal\nlocal Animal = { sound = \"...\", legs = 4 }\nfunction Animal.speak() return \"...\" end\n\
---@class Bird : Animal\nlocal Bird = { sound = \"tweet\", legs = 2 }\nfunction Bird.speak() return \"tweet\" end\n\
---@type Bird\nlocal bird\n---@type Animal\nlocal animal = bird\n";
    let report = report(source, &[]);
    assert!(errors(&report).is_empty(), "{report}");
}

#[test]
fn every_incompatible_override_is_reported_and_an_override_keeps_inherited_optionality() {
    let source = "---@class P\n---@field a number\n---@field b number\n---@field foo fun(p?: string)\nlocal P = {}\n\
---@class C : P\nlocal C = { a = \"x\", b = \"y\" }\nfunction C.foo(p) end\n---@type C\nlocal c\nc.foo()\n";
    let report = report(source, &[]);
    let errors = errors(&report);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:7:17: error[TS2416]")), "{report}");
    assert!(errors.iter().any(|error| error.starts_with("main.lua:7:26: error[TS2416]")), "{report}");
    // The table with wrong fields is still the class's definition: its later members and their
    // inherited optional parameter are not errors too.
    assert!(!errors.iter().any(|error| error.contains("TS2339") || error.contains("TS2554")), "{report}");
}

#[test]
fn a_written_variadic_tail_is_the_contract_and_lua_drops_a_tail_past_the_target() {
    let source = "---@return number, string, boolean...\nlocal function producer() return 1, \"a\", true end\n\
---@param a number\n---@param b string\nlocal function two(a, b) end\ntwo(producer())\n\
---@return number, string, boolean\nlocal function forward() return producer() end\n\
---@type any[]\nlocal values\n---@return any\nlocal function first() return table.unpack(values) end\n\
---@return number\nlocal function excess() return 1, 2 end\n";
    let report = report(source, &[]);
    let errors = errors(&report);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:8:33: error[TS2322]")), "{report}");
    assert!(errors.iter().any(|error| error.starts_with("main.lua:14:35: error[TS2322]")), "{report}");
    assert_eq!(errors.len(), 2, "{report}");
}

#[test]
fn a_call_no_overload_accepts_has_the_longest_candidates_result() {
    let source = "---@param a number\n---@param b number\n---@param c number\n---@return string | number | boolean\n\
---@overload fun(a: number): boolean\n---@overload fun(a: number, b: number): number\n---@overload fun(a: number, b: number, c: number): string\n\
local function f(a, b, c) return \"\" end\n---@type number\nlocal n = f(1, 2, 3, 4)\n";
    let report = report(source, &[]);
    assert!(report.contains("main.lua:10:11: error[TS2554]: Expected 1-3 arguments, but got 4."), "{report}");
    assert!(report.contains("main.lua:10:11: error[TS2322]: Type 'string' is not assignable to type 'number'."), "{report}");
}

#[test]
fn an_expansion_reads_as_its_element_and_a_store_stays_sparse() {
    let source = "---@vararg number\nlocal function pack(...) return {...} end\n---@type number\nlocal first = pack(1)[1]\n\
---@param i number\n---@param j number\nlocal function fill(i, j)\n    local list = {}\n    list[i] = \"word\"\n    ---@type string\n    local read = list[j]\nend\n";
    let report = report(source, &[]);
    let errors = errors(&report);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:11:18: error[TS2322]")), "{report}");
    assert_eq!(errors.len(), 1, "{report}");
}

#[test]
fn result_lists_of_other_lengths_infer_shared_positions_and_failed_calls_do_not_cascade() {
    let source = "---@type fun<A, B>(func: fun(): A, B): A, B\nlocal both\nlocal a, b = both(function() return 1 end)\n---@type string\nlocal s = a\n\
---@generic S, T\n---@param arg { a: S, b: (fun(): T)[] }\n---@return S, T\nlocal function nested(arg) return arg.a, arg.b() end\n";
    let report = report(source, &[]);
    let errors = errors(&report);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:5:11: error[TS2322]: Type 'number'")), "{report}");
    assert!(errors.iter().any(|error| error.contains("error[TS2349]")), "{report}");
    assert!(!errors.iter().any(|error| error.contains("result pack")), "{report}");
}

/// `code text` for each diagnostic, with or without inspections.
fn diagnostics(source: &str, inspections: bool) -> Vec<String> {
    let root = std::env::temp_dir().join(format!("luanalysis legacy inspections {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.lua");
    fs::write(&main, source).unwrap();
    let project = Project::load_plan(&Resolver::new(ProjectConfig::new([root.clone()])).unwrap(), ProjectLoadPlan::new([main]), static_require_requests);
    let report = check_project_with_options(project, ProjectCheckOptions {
        environment: StandardEnvironmentProfile::Lua54,
        inspections,
        ..ProjectCheckOptions::default()
    });
    fs::remove_dir_all(&root).unwrap();
    let main = report.file(report.project.entries[0]).unwrap();
    main.report.diagnostics.iter().map(|diagnostic| format!("{} {}", diagnostic.code, &source[diagnostic.span.start..diagnostic.span.end])).collect()
}

#[test]
fn a_shadowed_generic_parameter_is_a_warning_inspection() {
    let source = "---@generic T : boolean\nlocal function outer()\n    ---@generic T : string\n    local function inner(arg) end\nend\n\
---@class Box<T>\nlocal Box = {}\n---@generic T\n---@param arg T\nfunction Box:shadow(arg) end\n";
    assert!(!diagnostics(source, false).iter().any(|diagnostic| diagnostic.starts_with("LUA4005")));
    let inspected = diagnostics(source, true);
    let shadowed: Vec<_> = inspected.iter().filter(|diagnostic| diagnostic.starts_with("LUA4005")).collect();
    assert_eq!(shadowed, ["LUA4005 T : string", "LUA4005 T"], "{inspected:#?}");
}

#[test]
fn no_unsafe_any_reports_members_indexes_and_calls_of_any_once() {
    let source = "---@type any\nlocal u\nu.field = 1\nu.method()\nlocal x = u[1]\nu:other()\nu()\n---@type number\nlocal n = u\nlocal function f(param) return param.value end\n";
    assert!(errors(&report(source, &[])).is_empty());
    let report = report(source, &["--no-unsafe-any"]);
    let errors = errors(&report);
    for expected in [
        "main.lua:3:3: error[TS2339]: Property 'field' does not exist on type 'any'.",
        "main.lua:4:3: error[TS2339]: Property 'method' does not exist on type 'any'.",
        "main.lua:5:13: error[TS7053]",
        "main.lua:6:3: error[TS2339]: Property 'other' does not exist on type 'any'.",
        "main.lua:7:1: error[TS2349]: This expression is not callable: its type is 'any'.",
        "main.lua:10:38: error[TS2339]: Property 'value' does not exist on type 'any'.",
    ] {
        assert!(errors.iter().any(|error| error.starts_with(expected)), "{expected}\n{report}");
    }
    // Each use is reported once: the member read, not the call of its result too; `any` itself
    // stays assignable.
    assert_eq!(errors.len(), 6, "{report}");
}
