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
