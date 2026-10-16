use luanalysis::project_check::{
    ProjectCheckOptions, ProjectCheckReport, check_project_with_options, static_require_requests,
};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const PROFILES: [(StandardEnvironmentProfile, &str); 2] = [
    (StandardEnvironmentProfile::Lua52, "5.2"),
    (StandardEnvironmentProfile::Lua54, "5.4"),
];
const CONSUMERS: &str = "---@function takeArray(values:number[]):void\n---@function takeDictionary(values:table<number,number>):void\n\n";
const CHOOSE: &str =
    "---@function choose<T>(first:T[],...:T[]):T[]\n---@function choose<T>(first:T,...:T):T\n\n";
const CONTAINERS: &str = r#"---@shape Options
---@field limit number
---@field label string

---@shape Packed<T>
---@field [number] T
---@field n number

"#;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis array form {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, source).unwrap();
        path
    }
    fn check(&self, source: &str, environment: StandardEnvironmentProfile) -> ProjectCheckReport {
        let main = self.file("main.lua", source);
        let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
        let project = Project::load_plan(
            &resolver,
            ProjectLoadPlan::new([main]),
            static_require_requests,
        );
        check_project_with_options(
            project,
            ProjectCheckOptions {
                environment,
                ..Default::default()
            },
        )
    }
    fn cli(&self, source: &str, version: &str) -> Output {
        self.file("main.lua", source);
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args([
                "check",
                "--format",
                "json",
                "--lua-version",
                version,
                "main.lua",
            ])
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn clean(report: &ProjectCheckReport) {
    assert!(!report.has_errors(), "{:#?}", report.files);
}
fn span(source: &str, statement: &str, expression: &str) -> (usize, usize) {
    let start = source.find(statement).unwrap() + statement.rfind(expression).unwrap();
    (start, start + expression.len())
}
fn error_at(
    report: &ProjectCheckReport,
    source: &str,
    code: &str,
    statement: &str,
    expression: &str,
) {
    let (start, end) = span(source, statement, expression);
    let diagnostics = &report
        .file(report.project.entries[0])
        .unwrap()
        .report
        .diagnostics;
    assert_eq!(diagnostics.len(), 1, "{source}\n{diagnostics:#?}");
    let error = &diagnostics[0];
    assert_eq!(error.code, code, "{source}\n{error:#?}");
    assert_eq!(error.span.start, start, "{source}\n{error:#?}");
    assert_eq!(error.span.end, end, "{source}\n{error:#?}");
}
fn cli_report(output: Output, exit: i32) -> String {
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(exit), "{stdout}\n{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    assert!(stdout.starts_with("{\"formatVersion\":1,"), "{stdout}");
    stdout
}

#[test]
fn array_contracts_accept_arrays_dense_positive_tuples_and_empty_structural_tables() {
    let fixture = Fixture::new();
    let source = format!(
        r#"{CONSUMERS}---@shape Empty

---@shape Dense
---@field [1] number
---@field [2] number

---@param array number[]
---@param dense Dense
---@param empty Empty
local function inspect(array,dense,empty)
    takeArray(array)
    takeArray(dense)
    takeArray(empty)
    takeArray({{}})
    takeArray({{1,2,3}})
    takeArray({{[2]=2,[1]=1}})
    ---@type number[]
    local denseArray=dense
    ---@type number[]
    local emptyArray=empty
    ---@type number[]
    local freshArray={{}}
end
"#
    );
    for (profile, _) in PROFILES {
        clean(&fixture.check(&source, profile));
    }
}

#[test]
fn numeric_dictionaries_and_numeric_indexer_shapes_do_not_satisfy_array_contracts() {
    let fixture = Fixture::new();
    for (declaration, annotation) in [
        ("", "table<number,number>"),
        (
            "---@shape NumericIndexer\n---@field [number] number\n\n",
            "NumericIndexer",
        ),
    ] {
        let source = format!(
            "{declaration}{CONSUMERS}---@param value {annotation}\nlocal function inspect(value)\n    takeArray(value)\nend\n"
        );
        let corrected = source.replace("takeArray(value)", "takeDictionary(value)");
        for (profile, _) in PROFILES {
            error_at(
                &fixture.check(&source, profile),
                &source,
                "TS2345",
                "takeArray(value)",
                "value",
            );
            clean(&fixture.check(&corrected, profile));
        }
    }
}

#[test]
fn nominal_instances_definitions_and_named_or_mixed_fields_are_not_arrays() {
    let fixture = Fixture::new();
    for declaration in [
        "---@class NotArray\n\n",
        "---@shape NotArray\n---@field label string\n\n",
        "---@shape NotArray\n---@field [1] number\n---@field label string\n\n",
        "---@shape NotArray\n---@field [number] number\n---@field n number\n\n",
    ] {
        let source = format!(
            "{declaration}{CONSUMERS}---@param value NotArray\nlocal function inspect(value)\n    takeArray(value)\nend\n"
        );
        let corrected = source.replace("takeArray(value)", "takeArray({})");
        for (profile, _) in PROFILES {
            error_at(
                &fixture.check(&source, profile),
                &source,
                "TS2345",
                "takeArray(value)",
                "value",
            );
            clean(&fixture.check(&corrected, profile));
        }
    }
    let definition =
        format!("---@class Nominal\nlocal Nominal={{}}\n\n{CONSUMERS}takeArray(Nominal)\n");
    for (profile, _) in PROFILES {
        error_at(
            &fixture.check(&definition, profile),
            &definition,
            "TS2345",
            "takeArray(Nominal)",
            "Nominal",
        );
        clean(&fixture.check(
            &definition.replace("takeArray(Nominal)", "takeArray({})"),
            profile,
        ));
    }
}

#[test]
fn concrete_array_candidates_reject_holes_nonpositive_and_fractional_keys() {
    let fixture = Fixture::new();
    for literal in [
        "{[1]=1,[3]=3}",
        "{[2]=2}",
        "{[0]=0,[1]=1}",
        "{[-1]=1,[1]=1}",
        "{[1.5]=1}",
        "{label='text'}",
        "{[1]=1,label='text'}",
    ] {
        let call = format!("takeArray({literal})");
        let source = format!("{CONSUMERS}{call}\n");
        let corrected = format!("{CONSUMERS}takeArray({{1,2}})\n");
        for (profile, _) in PROFILES {
            error_at(
                &fixture.check(&source, profile),
                &source,
                "TS2345",
                &call,
                literal,
            );
            clean(&fixture.check(&corrected, profile));
        }
    }
}

#[test]
fn arrays_remain_assignable_to_numeric_dictionaries_but_not_the_reverse() {
    let fixture = Fixture::new();
    let source = format!(
        r#"{CONSUMERS}---@param array number[]
---@param dictionary table<number,number>
local function inspect(array,dictionary)
    takeDictionary(array)
    ---@type table<number,number>
    local accepted=array
    ---@type number[]
    local rejected=dictionary
end
"#
    );
    let corrected = source.replace("local rejected=dictionary", "local rejected=array");
    for (profile, _) in PROFILES {
        error_at(
            &fixture.check(&source, profile),
            &source,
            "TS2322",
            "local rejected=dictionary",
            "dictionary",
        );
        clean(&fixture.check(&corrected, profile));
    }
}

#[test]
fn sequence_admission_still_checks_array_elements_and_accepts_corrections() {
    let fixture = Fixture::new();
    for (prefix, call, expression) in [
        ("", "takeArray({'wrong'})", "{'wrong'}"),
        (
            "---@param value string[]\nlocal function inspect(value)\n",
            "takeArray(value)",
            "value",
        ),
    ] {
        let suffix = if prefix.is_empty() { "" } else { "end\n" };
        let source = format!("{CONSUMERS}{prefix}{call}\n{suffix}");
        let corrected = source.replace(call, "takeArray({1,2})");
        for (profile, _) in PROFILES {
            error_at(
                &fixture.check(&source, profile),
                &source,
                "TS2345",
                call,
                expression,
            );
            clean(&fixture.check(&corrected, profile));
        }
    }
}

fn chosen_containers() -> String {
    format!(
        r#"{CONTAINERS}{CHOOSE}---@param array number[]
---@param options Options
---@param packed Packed<string>
---@param dictionary table<number,string>
local function inspect(array,options,packed,dictionary)
    local selectedArray=choose(array,array)
    ---@type number[]
    local checkedArray=selectedArray
    ---@type number
    local arrayValue=selectedArray[1]
    local selectedLiteral=choose({{1,2}},{{3,4}})
    ---@type number[]
    local checkedLiteral=selectedLiteral
    ---@type number
    local literalValue=selectedLiteral[1]
    local selectedOptions=choose(options,options)
    ---@type Options
    local checkedOptions=selectedOptions
    ---@type number
    local limit=selectedOptions.limit
    ---@type string
    local label=selectedOptions.label
    selectedOptions.limit=42
    local selectedPacked=choose(packed,packed)
    ---@type Packed<string>
    local checkedPacked=selectedPacked
    ---@type number
    local count=selectedPacked.n
    ---@type string
    local packedValue=selectedPacked[1]
    local selectedDictionary=choose(dictionary,dictionary)
    ---@type table<number,string>
    local checkedDictionary=selectedDictionary
    ---@type string
    local dictionaryValue=selectedDictionary[1]
end
"#
    )
}

#[test]
fn generic_array_overloads_fall_through_for_named_packed_and_dictionary_arguments() {
    let fixture = Fixture::new();
    let source = chosen_containers();
    let wrong = source.replace("selectedOptions.limit=42", "selectedOptions.limit='wrong'");
    for (profile, _) in PROFILES {
        clean(&fixture.check(&source, profile));
        error_at(
            &fixture.check(&wrong, profile),
            &wrong,
            "TS2322",
            "selectedOptions.limit='wrong'",
            "'wrong'",
        );
        clean(&fixture.check(&source, profile));
    }
}

#[test]
fn generic_array_form_survives_substitution_and_module_export_without_promoting_dictionaries() {
    let fixture = Fixture::new();
    fixture.file(
        "containers.lua",
        r#"---@generic T
---@param values T[]
---@return T[]
local function array(values) return values end

---@generic T
---@param values table<number,T>
---@return table<number,T>
local function dictionary(values) return values end

return {array=array,dictionary=dictionary}
"#,
    );
    let source = format!(
        r#"{CONSUMERS}local containers=require('containers')
---@param array number[]
---@param dictionary table<number,number>
local function inspect(array,dictionary)
    local mappedArray=containers.array(array)
    takeArray(mappedArray)
    takeDictionary(mappedArray)
    local freshArray=containers.array({{1,2}})
    takeArray(freshArray)
    local mappedDictionary=containers.dictionary(dictionary)
    takeDictionary(mappedDictionary)
end
"#
    );
    for (profile, _) in PROFILES {
        clean(&fixture.check(&source, profile));
        let wrong_argument = source.replace(
            "takeDictionary(mappedDictionary)",
            "takeDictionary(mappedDictionary)\n    containers.array(dictionary)",
        );
        error_at(
            &fixture.check(&wrong_argument, profile),
            &wrong_argument,
            "TS2345",
            "containers.array(dictionary)",
            "dictionary",
        );
        clean(&fixture.check(&source, profile));
        let wrong_result = source.replace(
            "takeDictionary(mappedDictionary)",
            "takeArray(mappedDictionary)",
        );
        error_at(
            &fixture.check(&wrong_result, profile),
            &wrong_result,
            "TS2345",
            "takeArray(mappedDictionary)",
            "mappedDictionary",
        );
        clean(&fixture.check(&source, profile));
    }
}

#[test]
fn cli_preserves_original_array_argument_ranges_and_accepts_dictionary_corrections() {
    let fixture = Fixture::new();
    let source = format!(
        "-- café\n{CONSUMERS}---@param dictionary table<number,number>\nlocal function inspect(dictionary)\n    takeArray(dictionary)\nend\n"
    );
    let corrected = source.replace("takeArray(dictionary)", "takeDictionary(dictionary)");
    let (start, end) = span(&source, "takeArray(dictionary)", "dictionary");
    let range = format!("\"range\":{{\"start\":{start},\"end\":{end}}}");
    for (profile, version) in PROFILES {
        error_at(
            &fixture.check(&source, profile),
            &source,
            "TS2345",
            "takeArray(dictionary)",
            "dictionary",
        );
        let rejected = cli_report(fixture.cli(&source, version), 1);
        assert!(
            rejected
                .split("},{\"path\":")
                .any(|diagnostic| diagnostic.contains("\"code\":\"TS2345\"")
                    && diagnostic.contains(&range)),
            "{rejected}"
        );
        assert!(!rejected.contains("contextual type"), "{rejected}");
        assert!(
            rejected.contains("parameter of type 'number[]'"),
            "{rejected}"
        );
        clean(&fixture.check(&corrected, profile));
        let corrected = cli_report(fixture.cli(&corrected, version), 0);
        assert!(corrected.contains("\"diagnostics\":[]"), "{corrected}");
    }
}

#[test]
fn nullable_arrays_accept_original_dense_nil_slots_with_and_without_context() {
    let fixture = Fixture::new();
    let source = r#"---@function takeNullable(values:(number|nil)[]):void

---@type (number|nil)[]
local contextual={nil,1}
takeNullable(contextual)

---@class Holder
local Holder={}
---@type (number|nil)[]
Holder.values={nil,1}
takeNullable(Holder.values)

local values={nil,1}
takeNullable(values)
takeNullable({nil,1})
---@type number
local retainedSecond=values[2]
"#;
    for (profile, _) in PROFILES {
        clean(&fixture.check(source, profile));
    }
}

#[test]
fn generic_array_element_inference_keeps_nil_from_original_constructor_rows() {
    let fixture = Fixture::new();
    let source = r#"---@function elements<T>(values:T[]):T
local values={nil,1}
local element=elements(values)
---@type number|nil
local checked=element
local direct=elements({nil,1})
---@type number|nil
local checkedDirect=direct
"#;
    for (profile, _) in PROFILES {
        clean(&fixture.check(source, profile));
        for (annotation, statement, expression) in [
            (
                "---@type number|nil\nlocal checked=element",
                "local checked=element",
                "element",
            ),
            (
                "---@type number|nil\nlocal checkedDirect=direct",
                "local checkedDirect=direct",
                "direct",
            ),
        ] {
            let wrong = source.replace(annotation, &annotation.replace("number|nil", "number"));
            error_at(
                &fixture.check(&wrong, profile),
                &wrong,
                "TS2322",
                statement,
                expression,
            );
            clean(&fixture.check(source, profile));
        }
    }
}

#[test]
fn generic_array_identity_and_exported_returns_preserve_original_nullable_elements() {
    let fixture = Fixture::new();
    fixture.file(
        "nullable_arrays.lua",
        r#"---@generic T
---@param values T[]
---@return T[]
local function identity(values) return values end

local values={nil,1}
return {identity=identity,values=values}
"#,
    );
    let source = r#"---@function arrayIdentity<T>(values:T[]):T[]
---@function takeNullable(values:(number|nil)[]):void
local arrays=require('nullable_arrays')
local values={nil,1}
local mapped=arrayIdentity(values)
takeNullable(mapped)
---@type number|nil
local localElement=mapped[1]
local imported=arrays.identity(values)
takeNullable(imported)
---@type number|nil
local importedElement=imported[1]
local exported=arrays.identity(arrays.values)
takeNullable(exported)
---@type number|nil
local exportedElement=exported[1]
"#;
    for (profile, _) in PROFILES {
        clean(&fixture.check(source, profile));
        for (annotation, statement, expression) in [
            (
                "---@type number|nil\nlocal localElement=mapped[1]",
                "local localElement=mapped[1]",
                "mapped[1]",
            ),
            (
                "---@type number|nil\nlocal importedElement=imported[1]",
                "local importedElement=imported[1]",
                "imported[1]",
            ),
            (
                "---@type number|nil\nlocal exportedElement=exported[1]",
                "local exportedElement=exported[1]",
                "exported[1]",
            ),
        ] {
            let wrong = source.replace(annotation, &annotation.replace("number|nil", "number"));
            error_at(
                &fixture.check(&wrong, profile),
                &wrong,
                "TS2322",
                statement,
                expression,
            );
            clean(&fixture.check(source, profile));
        }
    }
}

#[test]
fn constructor_rows_still_reject_nil_for_nonnullable_arrays_and_wrong_element_types() {
    let fixture = Fixture::new();
    for (source, corrected, statement, expression) in [
        (
            "---@type number[]\nlocal values={nil,1}\n",
            "---@type (number|nil)[]\nlocal values={nil,1}\n",
            "local values={nil,1}",
            "{nil,1}",
        ),
        (
            "---@class Holder\nlocal Holder={}\n---@type number[]\nHolder.values={nil,1}\n",
            "---@class Holder\nlocal Holder={}\n---@type (number|nil)[]\nHolder.values={nil,1}\n",
            "Holder.values={nil,1}",
            "{nil,1}",
        ),
        (
            "---@type (number|nil)[]\nlocal values={nil,'wrong'}\n",
            "---@type (number|nil)[]\nlocal values={nil,1}\n",
            "local values={nil,'wrong'}",
            "{nil,'wrong'}",
        ),
        (
            "---@class Holder\nlocal Holder={}\n---@type (number|nil)[]\nHolder.values={nil,'wrong'}\n",
            "---@class Holder\nlocal Holder={}\n---@type (number|nil)[]\nHolder.values={nil,1}\n",
            "Holder.values={nil,'wrong'}",
            "{nil,'wrong'}",
        ),
    ] {
        for (profile, _) in PROFILES {
            error_at(
                &fixture.check(source, profile),
                source,
                "TS2322",
                statement,
                expression,
            );
            clean(&fixture.check(corrected, profile));
        }
    }
    for (annotation, literal, correction) in [
        ("number[]", "{nil,1}", "{0,1}"),
        ("(number|nil)[]", "{nil,'wrong'}", "{nil,1}"),
    ] {
        let call = format!("take({literal})");
        let source = format!("---@function take(values:{annotation}):void\n{call}\n");
        let corrected = source.replace(&call, &format!("take({correction})"));
        for (profile, _) in PROFILES {
            error_at(
                &fixture.check(&source, profile),
                &source,
                "TS2345",
                &call,
                literal,
            );
            clean(&fixture.check(&corrected, profile));
        }
    }
}

#[test]
fn nullable_array_elements_do_not_make_explicit_constructor_gaps_dense() {
    let fixture = Fixture::new();
    for (source, code, statement) in [
        (
            "---@function take(values:(number|nil)[]):void\ntake({[2]=1})\n",
            "TS2345",
            "take({[2]=1})",
        ),
        (
            "---@type (number|nil)[]\nlocal values={[2]=1}\n",
            "TS2322",
            "local values={[2]=1}",
        ),
    ] {
        let corrected = source.replace("{[2]=1}", "{nil,1}");
        for (profile, _) in PROFILES {
            error_at(
                &fixture.check(source, profile),
                source,
                code,
                statement,
                "{[2]=1}",
            );
            clean(&fixture.check(&corrected, profile));
        }
    }
}

#[test]
fn cli_rejects_original_nil_array_elements_at_original_ranges_and_accepts_correction() {
    let fixture = Fixture::new();
    let source = "-- café\n---@function take(values:number[]):void\ntake({nil,1})\n";
    let corrected = source.replace("{nil,1}", "{0,1}");
    let (start, end) = span(source, "take({nil,1})", "{nil,1}");
    let range = format!("\"range\":{{\"start\":{start},\"end\":{end}}}");
    for (profile, version) in PROFILES {
        error_at(
            &fixture.check(source, profile),
            source,
            "TS2345",
            "take({nil,1})",
            "{nil,1}",
        );
        let rejected = cli_report(fixture.cli(source, version), 1);
        assert!(
            rejected
                .split("},{\"path\":")
                .any(|diagnostic| diagnostic.contains("\"code\":\"TS2345\"")
                    && diagnostic.contains(&range)),
            "{rejected}"
        );
        clean(&fixture.check(&corrected, profile));
        let corrected = cli_report(fixture.cli(&corrected, version), 0);
        assert!(corrected.contains("\"diagnostics\":[]"), "{corrected}");
    }
}

#[test]
fn proven_writes_replace_original_nil_sequence_types_for_relation_and_inference() {
    let fixture = Fixture::new();
    let source = "---@function take(values:number[]):void\n---@function elements<T>(values:T[]):T\nlocal values={nil,1}\nvalues[1]=2\ntake(values)\n---@type number\nlocal current=elements(values)\n";
    let wrong = source.replace("values[1]=2", "values[1]='wrong'").replace(
        "---@type number\nlocal current",
        "---@type string|number\nlocal current",
    );
    for (profile, _) in PROFILES {
        clean(&fixture.check(source, profile));
        error_at(
            &fixture.check(&wrong, profile),
            &wrong,
            "TS2345",
            "take(values)",
            "values",
        );
        clean(&fixture.check(source, profile));
    }
}

#[test]
fn proven_deletion_uses_current_nil_instead_of_the_preceding_numeric_store() {
    let fixture = Fixture::new();
    let source = "---@function takeNullable(values:(number|nil)[]):void\n---@function takeNumber(values:number[]):void\n---@function elements<T>(values:T[]):T\nlocal values={nil,1}\nvalues[1]=2\nvalues[1]=nil\ntakeNullable(values)\n---@type number|nil\nlocal current=elements(values)\n";
    let wrong_argument = source.replace("takeNullable(values)", "takeNumber(values)");
    let wrong_result = source.replace(
        "---@type number|nil\nlocal current",
        "---@type number\nlocal current",
    );
    let restored = source
        .replace("values[1]=nil", "values[1]=0")
        .replace("takeNullable(values)", "takeNumber(values)");
    for (profile, _) in PROFILES {
        clean(&fixture.check(source, profile));
        error_at(
            &fixture.check(&wrong_argument, profile),
            &wrong_argument,
            "TS2345",
            "takeNumber(values)",
            "values",
        );
        error_at(
            &fixture.check(&wrong_result, profile),
            &wrong_result,
            "TS2322",
            "local current=elements(values)",
            "elements(values)",
        );
        clean(&fixture.check(&restored, profile));
    }
}

#[test]
fn replacing_or_deleting_a_written_number_preserves_its_contract_and_reached_view() {
    let fixture = Fixture::new();
    for (value, annotation, statement, expression) in [
        ("'wrong'", "number", "take(values)", "values"),
        (
            "nil",
            "number",
            "local current=elements(values)",
            "elements(values)",
        ),
    ] {
        let source = format!(
            "---@function take(values:number[]):void\n---@function elements<T>(values:T[]):T\nlocal values={{nil,--[[---@type number]]1}}\nvalues[1]=2\nvalues[2]={value}\n---@type {annotation}\nlocal current=elements(values)\n{}",
            if value == "nil" { "" } else { "take(values)\n" }
        );
        let corrected = source.replace(&format!("values[2]={value}"), "values[2]=3");
        for (profile, _) in PROFILES {
            let report = fixture.check(&source, profile);
            let diagnostics = &report
                .file(report.project.entries[0])
                .unwrap()
                .report
                .diagnostics;
            // The written broad element contract rejects the store without
            // freezing it to the inferred literal 1 under v1 field inference.
            // The current view must still retain its string/nil value.
            let expected_count = if value == "nil" { 2 } else { 3 };
            assert_eq!(
                diagnostics.len(),
                expected_count,
                "{source}\n{diagnostics:#?}"
            );
            let assignment = format!("values[2]={value}");
            let (store_start, store_end) = span(&source, &assignment, value);
            assert!(
                diagnostics.iter().any(|error| error.code == "TS2322"
                    && error.span.start == store_start
                    && error.span.end == store_end),
                "{source}\n{diagnostics:#?}"
            );
            let (start, end) = span(&source, statement, expression);
            assert!(
                diagnostics.iter().any(|error| error.span.start == start
                    && error.span.end == end
                    && error.code == if value == "nil" { "TS2322" } else { "TS2345" }),
                "{source}\n{diagnostics:#?}"
            );
            clean(&fixture.check(&corrected, profile));
        }
    }
}

#[test]
fn contextual_nullable_literal_elements_keep_their_literal_and_reject_other_numbers() {
    let fixture = Fixture::new();
    let source = "---@function take(values:(1|nil)[]):void\n\n---@type (1|nil)[]\nlocal values={nil,1}\ntake(values)\ntake({nil,1})\n";
    let wrong_local = source.replace("local values={nil,1}", "local values={nil,2}");
    let wrong_argument = source.replace("take({nil,1})", "take({nil,2})");
    let inferred =
        "---@function take(values:(1|nil)[]):void\n\nlocal values={nil,1}\ntake(values)\n";
    let wrong_inferred = inferred.replace("{nil,1}", "{nil,2}");
    let wrong_store = format!("{inferred}values[2]=2\n");
    let corrected_store = wrong_store.replace("values[2]=2", "values[2]=1");
    for (profile, _) in PROFILES {
        clean(&fixture.check(source, profile));
        error_at(
            &fixture.check(&wrong_local, profile),
            &wrong_local,
            "TS2322",
            "local values={nil,2}",
            "{nil,2}",
        );
        error_at(
            &fixture.check(&wrong_argument, profile),
            &wrong_argument,
            "TS2345",
            "take({nil,2})",
            "{nil,2}",
        );
        clean(&fixture.check(source, profile));
        clean(&fixture.check(inferred, profile));
        error_at(
            &fixture.check(&wrong_inferred, profile),
            &wrong_inferred,
            "TS2345",
            "take(values)",
            "values",
        );
        error_at(
            &fixture.check(&wrong_store, profile),
            &wrong_store,
            "TS2322",
            "values[2]=2",
            "2",
        );
        clean(&fixture.check(&corrected_store, profile));
    }
}

fn asserted_arrays_and_packed() -> String {
    format!(
        r#"{CONTAINERS}---@param array number[]
---@param packed Packed<number>
local function inspect(array,packed)
    local assertedPacked=--[[---@type Packed<number>]] array
    ---@type number
    local count=assertedPacked.n
    ---@type number
    local packedElement=assertedPacked[1]
    assertedPacked.n=2
    assertedPacked[1]=3
    local assertedArray=--[[---@type number[] ]] packed
    ---@type number
    local arrayElement=assertedArray[1]
    assertedArray[1]=4
end
"#
    )
}

#[test]
fn array_and_packed_assertions_preserve_numeric_elements_and_the_packed_count_contract() {
    let fixture = Fixture::new();
    let source = asserted_arrays_and_packed();
    for (profile, _) in PROFILES {
        clean(&fixture.check(&source, profile));
        for (original, wrong) in [
            ("assertedPacked.n=2", "assertedPacked.n='wrong'"),
            ("assertedPacked[1]=3", "assertedPacked[1]='wrong'"),
            ("assertedArray[1]=4", "assertedArray[1]='wrong'"),
        ] {
            let rejected = source.replace(original, wrong);
            error_at(
                &fixture.check(&rejected, profile),
                &rejected,
                "TS2322",
                wrong,
                "'wrong'",
            );
            clean(&fixture.check(&source, profile));
        }
    }
}

#[test]
fn generic_array_to_packed_assertions_keep_the_original_element_parameter() {
    let fixture = Fixture::new();
    let source = format!(
        r#"{CONTAINERS}---@generic V
---@param values V[]
---@param value V
---@return Packed<V>
local function asPacked(values,value)
    local packed=--[[---@type Packed<V>]] values
    packed[1]=value
    packed.n=1
    return packed
end
local packed=asPacked({{1,2}},3)
---@type number
local element=packed[1]
---@type number
local count=packed.n
"#
    );
    let wrong = source.replace("packed[1]=value", "packed[1]='wrong'");
    for (profile, _) in PROFILES {
        clean(&fixture.check(&source, profile));
        error_at(
            &fixture.check(&wrong, profile),
            &wrong,
            "TS2322",
            "packed[1]='wrong'",
            "'wrong'",
        );
        clean(&fixture.check(&source, profile));
    }
}

#[test]
fn packed_assertions_do_not_make_plain_array_assignment_valid() {
    let fixture = Fixture::new();
    let source = format!(
        "{CONTAINERS}---@param packed Packed<number>\nlocal function inspect(packed)\n    ---@type number[]\n    local values=packed\nend\n"
    );
    let corrected = source.replace(
        "local values=packed",
        "local values=--[[---@type number[] ]] packed",
    );
    for (profile, _) in PROFILES {
        error_at(
            &fixture.check(&source, profile),
            &source,
            "TS2322",
            "local values=packed",
            "packed",
        );
        clean(&fixture.check(&corrected, profile));
    }
}

#[test]
fn array_packed_assertions_take_the_written_element_type_in_both_directions() {
    let fixture = Fixture::new();
    for (parameter, assertion, element, wrong_element) in [
        (
            "number[]",
            "--[[---@type Packed<string>]] values",
            "string",
            "number",
        ),
        (
            "Packed<string>",
            "--[[---@type number[] ]] values",
            "number",
            "string",
        ),
    ] {
        let statement = "local rejected=asserted[1]";
        let source = format!(
            "{CONTAINERS}---@param values {parameter}\nlocal function inspect(values)\n    local asserted={assertion}\n    ---@type {element}\n    local accepted=asserted[1]\n    ---@type {wrong_element}\n    {statement}\nend\n"
        );
        let corrected = source.replace(
            &format!("---@type {wrong_element}\n    local rejected"),
            &format!("---@type {element}\n    local rejected"),
        );
        for (profile, _) in PROFILES {
            error_at(
                &fixture.check(&source, profile),
                &source,
                "TS2322",
                statement,
                "asserted[1]",
            );
            clean(&fixture.check(&corrected, profile));
        }
    }
}

#[test]
fn array_packed_assertions_do_not_weaken_the_written_nominal_element_class() {
    let fixture = Fixture::new();
    let classes =
        "---@class First\n---@field value number\n\n---@class Second\n---@field value number\n\n";
    for (parameter, assertion, element, wrong_element) in [
        (
            "First[]",
            "--[[---@type Packed<Second>]] values",
            "Second",
            "First",
        ),
        (
            "Packed<Second>",
            "--[[---@type First[] ]] values",
            "First",
            "Second",
        ),
    ] {
        let statement = "local rejected=asserted[1]";
        let source = format!(
            "{CONTAINERS}{classes}---@param values {parameter}\nlocal function inspect(values)\n    local asserted={assertion}\n    ---@type {element}\n    local accepted=asserted[1]\n    ---@type {wrong_element}\n    {statement}\nend\n"
        );
        let corrected = source.replace(
            &format!("---@type {wrong_element}\n    local rejected"),
            &format!("---@type {element}\n    local rejected"),
        );
        for (profile, _) in PROFILES {
            error_at(
                &fixture.check(&source, profile),
                &source,
                "TS2322",
                statement,
                "asserted[1]",
            );
            clean(&fixture.check(&corrected, profile));
        }
    }
}

#[test]
fn authoritative_array_assertions_do_not_change_ordinary_key_domain_assignment() {
    let fixture = Fixture::new();
    for key in ["string", "boolean"] {
        for (parameter, assertion, annotation) in [
            (
                format!("table<{key},number>"),
                "--[[---@type number[] ]] values".to_owned(),
                "number[]".to_owned(),
            ),
            (
                "number[]".to_owned(),
                format!("--[[---@type table<{key},number>]] values"),
                format!("table<{key},number>"),
            ),
        ] {
            let statement = "local rejected=values";
            let source = format!(
                "---@param values {parameter}\nlocal function inspect(values)\n    local asserted={assertion}\n    ---@type {annotation}\n    {statement}\nend\n"
            );
            let corrected = source.replace(statement, "local rejected=asserted");
            for (profile, _) in PROFILES {
                error_at(
                    &fixture.check(&source, profile),
                    &source,
                    "TS2322",
                    statement,
                    "values",
                );
                clean(&fixture.check(&corrected, profile));
            }
        }
    }
    let separate_domains = "---@shape SeparateDomains\n---@field [number] number\n---@field [string] number\n---@field n number\n\n";
    for (parameter, assertion, annotation) in [
        (
            "SeparateDomains",
            "--[[---@type number[] ]] values",
            "number[]",
        ),
        (
            "number[]",
            "--[[---@type SeparateDomains]] values",
            "SeparateDomains",
        ),
    ] {
        let statement = "local rejected=values";
        let source = format!(
            "{separate_domains}---@param values {parameter}\nlocal function inspect(values)\n    local asserted={assertion}\n    ---@type {annotation}\n    {statement}\nend\n"
        );
        let corrected = source.replace(statement, "local rejected=asserted");
        for (profile, _) in PROFILES {
            error_at(
                &fixture.check(&source, profile),
                &source,
                "TS2322",
                statement,
                "values",
            );
            clean(&fixture.check(&corrected, profile));
        }
    }
}

#[test]
fn numeric_assertions_do_not_change_original_required_concrete_element_values() {
    let fixture = Fixture::new();
    let assertion = "--[[---@type number[] ]] values";
    let statement = "local rejected=values[1]";
    let source = format!(
        "---@shape NumericWithRequiredString\n---@field [number] number|string\n---@field [1] string\n---@field n number\n\n---@param values NumericWithRequiredString\nlocal function inspect(values)\n    local asserted={assertion}\n    ---@type number\n    local accepted=asserted[1]\n    ---@type number\n    {statement}\nend\n"
    );
    let corrected = source.replace(
        "---@type number\n    local rejected",
        "---@type string\n    local rejected",
    );
    for (profile, _) in PROFILES {
        error_at(
            &fixture.check(&source, profile),
            &source,
            "TS2322",
            statement,
            "values[1]",
        );
        clean(&fixture.check(&corrected, profile));
    }
}

#[test]
fn cli_reports_invalid_uses_of_array_packed_assertions_at_the_original_expression() {
    let fixture = Fixture::new();
    let assertion = "--[[---@type Packed<string>]] values";
    let statement = "local rejected=packed[1]";
    let source = format!(
        "-- café\n{CONTAINERS}---@param values number[]\nlocal function inspect(values)\n    local packed={assertion}\n    ---@type number\n    {statement}\nend\n"
    );
    let corrected = source.replace(
        "---@type number\n    local rejected",
        "---@type string\n    local rejected",
    );
    let (start, end) = span(&source, statement, "packed[1]");
    let range = format!("\"range\":{{\"start\":{start},\"end\":{end}}}");
    for (profile, version) in PROFILES {
        error_at(
            &fixture.check(&source, profile),
            &source,
            "TS2322",
            statement,
            "packed[1]",
        );
        let rejected = cli_report(fixture.cli(&source, version), 1);
        assert!(
            rejected
                .split("},{\"path\":")
                .any(|diagnostic| diagnostic.contains("\"code\":\"TS2322\"")
                    && diagnostic.contains(&range)),
            "{rejected}"
        );
        clean(&fixture.check(&corrected, profile));
        let corrected = cli_report(fixture.cli(&corrected, version), 0);
        assert!(corrected.contains("\"diagnostics\":[]"), "{corrected}");
    }
}
