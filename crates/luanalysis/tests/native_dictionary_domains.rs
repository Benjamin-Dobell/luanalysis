use luanalysis::project_check::{
    ProjectCheckOptions, ProjectCheckReport, check_project_with_options, static_require_requests,
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

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

fn assignment_error(source: &str, expression: &str) {
    let report = check(source);
    assert_eq!(
        report.diagnostics.len(),
        1,
        "{source}\n{:#?}",
        report.diagnostics
    );
    let error = &report.diagnostics[0];
    assert_eq!(error.code, "TS2322", "{error:#?}");
    assert_eq!(&source[error.span.start..error.span.end], expression);
}

#[test]
fn a_nonempty_disjoint_source_cannot_satisfy_an_unrelated_dictionary_domain() {
    for (annotation, wrong, corrected) in [
        ("table<number,number>", "{word=1}", "{[1]=1}"),
        ("table<'a',1>", "{b=1}", "{a=1}"),
        ("table<number,number>", "{[true]=1}", "{[1]=1}"),
        ("table<string,number>", "{[1]=1}", "{one=1}"),
    ] {
        let source = format!("---@type {annotation}\nlocal values={wrong}\n");
        assignment_error(&source, wrong);
        clean(&source.replace(wrong, corrected));

        let source =
            format!("local original={wrong}\n---@type {annotation}\nlocal values=original\n");
        assignment_error(&source, "original");
        clean(&source.replace(wrong, corrected));
    }
}

#[test]
fn dictionary_views_preserve_unrelated_domains_and_check_every_shared_value() {
    for parameter in ["table<string|number,number>", "WithBooleanIndexer"] {
        let declarations = "---@shape WithBooleanIndexer\n---@field [number] number\n---@field [boolean] number\n\n";
        let source = format!(
            "{declarations}---@param original {parameter}\nlocal function inspect(original)\n---@type table<number,number>\nlocal values=original\nend\n"
        );
        clean(&source);
        assignment_error(
            &source.replace(
                "---@type table<number,number>",
                "---@type table<number,string>",
            ),
            "original",
        );
        clean(&source.replace(
            "---@type table<number,number>",
            "---@type table<string|number|boolean,number>",
        ));
    }
    let broad = "---@param original table<string,number>\nlocal function inspect(original)\n---@type table<'a'|'b',number>\nlocal values=original\nend\n";
    clean(broad);
    clean(&broad.replace(
        "---@type table<'a'|'b',number>",
        "---@type table<string,number>",
    ));
    for (annotation, value) in [
        ("table<number,number>", "{[1]=1,word='metadata'}"),
        ("table<string,string>", "{word='text',[1]=false}"),
    ] {
        let source =
            format!("local original={value}\n---@type {annotation}\nlocal projected=original\n");
        clean(&source);
        assignment_error(
            &source
                .replace("table<number,number>", "table<number,string>")
                .replace("table<string,string>", "table<string,number>"),
            "original",
        );
    }
    let inferred = "---@generic V\n---@param values table<string,V>\n---@return V\nlocal function word(values) return values.word end\nlocal mixed={word='text',[1]=false}\n---@type string\nlocal result=word(mixed)\n";
    clean(inferred);
    assignment_error(
        &inferred.replace("---@type string", "---@type boolean"),
        "word(mixed)",
    );
}

#[test]
fn a_written_indexer_projects_its_domain_without_erasing_finite_declared_metadata() {
    let declarations = "---@shape Measured<T>\n---@field [number] T\n---@field label string\n---@field count number\n\n";
    let source = format!(
        "{declarations}---@param original Measured<number>\nlocal function inspect(original)\n---@type table<number,number>\nlocal values=original\n---@type number\nlocal item=values[1]\n---@type string\nlocal metadata=original.label\nend\n"
    );
    clean(&source);
    assignment_error(
        &source.replace(
            "---@param original Measured<number>",
            "---@param original Measured<string>",
        ),
        "original",
    );
    assignment_error(
        &source.replace(
            "---@type number\nlocal item",
            "---@type string\nlocal item",
        ),
        "values[1]",
    );
    assignment_error(
        &source.replace(
            "---@type string\nlocal metadata",
            "---@type number\nlocal metadata",
        ),
        "original.label",
    );
    let exception = source.replace(
        "---@field count number",
        "---@field count number\n---@field [1] string",
    );
    assignment_error(&exception, "original");
    clean(
        "---@shape Measured\n---@field [number] number\n---@field [string] number\n---@field [false] boolean\n\n---@param original Measured\nlocal function inspect(original)\n---@type table<number|string,number>\nlocal values=original\nend\n",
    );
}

#[test]
fn multiple_indexers_keep_their_distinct_value_contracts() {
    let prefix =
        "---@shape Mixed\n---@field [number] number\n---@field [string] string\n\n---@type Mixed\n";
    clean(&format!("{prefix}local values={{[1]=1,label='text'}}\n"));
    for wrong in ["{[1]='text',label='text'}", "{[1]=1,label=2}", "{[true]=1}"] {
        assignment_error(&format!("{prefix}local values={wrong}\n"), wrong);
    }
    clean("---@type table<'a'|'b',number>\nlocal values={a=1,b=2}\n");
    clean("---@type table<string|number,number>\nlocal values={[1]=1,label=2}\n");
}

#[test]
fn partially_overlapping_union_indexers_check_values_in_every_shared_key_domain() {
    let declarations = "---@shape Source\n---@field [string|number] number\n---@field [boolean] string\n\n---@shape Target\n---@field [string|boolean] string\n---@field [number] number\n\n";
    let source = format!(
        "{declarations}---@param original Source\nlocal function inspect(original)\n---@type number\nlocal stored=original.word\n---@type Target\nlocal dictionary=original\nend\n"
    );
    assignment_error(&source, "original");
    clean(&source.replace(
        "---@field [string|boolean] string",
        "---@field [string|boolean] number|string",
    ));

    clean(
        "---@shape Target\n---@field [string|boolean] number\n---@field [number|boolean] number\n\n---@param original table<string|number,number>\nlocal function inspect(original)\n---@type Target\nlocal dictionary=original\n---@type number\nlocal named=dictionary.word\n---@type number\nlocal indexed=dictionary[1]\nend\n",
    );
    for annotation in ["table<string|number,number>", "BothDomains"] {
        let source = format!(
            "---@shape BothDomains\n---@field [string] number\n---@field [number] number\n\n---@param original table<number,number>\nlocal function inspect(original)\n---@type {annotation}\nlocal dictionary=original\nend\n"
        );
        clean(&source);
    }
}

#[test]
fn an_explicit_open_any_indexer_controls_its_view_without_weakening_written_fields() {
    let source = "---@shape Open\n---@field [any] any\n---@field answer number\n\n---@param original Open\nlocal function inspect(original)\n---@type table<string,function|nil>\nlocal handlers=original\nend\n";
    clean(source);
    assignment_error(
        &source.replace("---@field [any] any", "---@field [string] number"),
        "original",
    );
    assignment_error(&source.replace("---@field [any] any\n", ""), "original");
    let required = source.replace(
        "---@type table<string,function|nil>",
        "---@type {answer:string}",
    );
    assignment_error(&required, "original");
}

#[test]
fn an_optional_exact_field_also_constrains_the_sources_possible_indexer_entry() {
    for field in ["special? string", "special string|nil"] {
        for (generic, key) in [("", "string"), ("---@generic K:string\n", "K")] {
            let source = format!(
                "---@shape Target\n---@field {field}\n---@field [string] number\n\n{generic}---@param original table<{key},number>\nlocal function inspect(original)\n---@type Target\nlocal dictionary=original\nend\n"
            );
            assignment_error(&source, "original");
            clean(&source.replace(field, &field.replace("string", "number")));
        }
    }
    clean(
        "---@shape Source\n---@field special string\n---@field [string] number\n\n---@shape Target\n---@field special? string\n---@field [string] number\n\n---@param original Source\nlocal function inspect(original)\n---@type Target\nlocal dictionary=original\nend\n---@type Target\nlocal empty={}\n---@type Target\nlocal known={special='text',other=1}\n",
    );
    assignment_error(
        "---@shape Target\n---@field special string\n---@field [string] string\n\n---@param original table<string,string>\nlocal function inspect(original)\n---@type Target\nlocal dictionary=original\nend\n",
        "original",
    );
}

#[test]
fn dictionary_coverage_preserves_record_width_named_indexer_members_and_empty_tables() {
    clean(
        "---@shape Record\n---@field value number\n\n---@shape Packed\n---@field [number] number\n---@field n number\n\n---@type Record\nlocal record={value=1,extra='text'}\n---@type Packed\nlocal packed={[1]=1,[3]=3,n=2}\n---@type table<number,number>\nlocal empty={}\n---@type table<number,number>\nlocal sparse={[1]=1,[3]=3}\n---@type table<number,number>\nlocal removed={word=nil}\n",
    );
    clean(
        "---@generic K,V\n---@param values table<K,V>\n---@return table<K,V>\nlocal function identity(values) return values end\nlocal numeric=identity({[1]=1})\nlocal named=identity({word='text'})\n---@type number\nlocal first=numeric[1]\n---@type string\nlocal text=named.word\n",
    );
}

#[test]
fn dictionary_domain_validation_preserves_lua_deletion_and_array_value_checks() {
    let source = "---@type number[]\nlocal array={1,2}\narray[1]=nil\n---@type table<number,number>\nlocal dictionary=array\ndictionary[2]=nil\ndictionary[3]=3\n";
    clean(source);
    assignment_error(&format!("{source}dictionary[3]='wrong'\n"), "'wrong'");
    assignment_error("---@type number[]\nlocal array={1,nil,3}\n", "{1,nil,3}");
}

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis dictionary domains {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn source(&self, source: &str) -> PathBuf {
        let path = self.0.join("main.lua");
        fs::write(&path, source).unwrap();
        path
    }
    fn check(&self, source: &str, environment: StandardEnvironmentProfile) -> ProjectCheckReport {
        let main = self.source(source);
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
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const JSON: &str = "---@alias JSONObject table<string,JSONValue>\n---@alias JSONArray JSONValue[]\n---@alias JSONValue JSONObject|JSONArray|number|string|boolean|nil\n\n";
const PROFILES: [StandardEnvironmentProfile; 2] = [
    StandardEnvironmentProfile::Lua52,
    StandardEnvironmentProfile::Lua54,
];

#[test]
fn packed_numeric_views_work_in_iterator_function_variance_and_direct_calls() {
    let fixture = Fixture::new();
    let source = r#"---@param length number
---@return fun(arr:table<number,unknown>,i:number):nil|(number,unknown)
local function fixedLengthIterator(length)
    return function(arr,i)
        i=i+1
        if i<=length then return i,arr[i] end
    end
end

---@param values table<number,any>
local function takeNumeric(values) end

---@param packed std__Packed<unknown>
---@return fun(arr:std__Packed<unknown>,i:number):nil|(number,unknown),std__Packed<unknown>,0
local function iteratorTriple(packed)
    return fixedLengthIterator(packed.n),packed,0
end

---@generic V
---@param values std__Packed<V>|V[]
local function iterate(values)
    takeNumeric(values)
    for index,value in fixedLengthIterator(3),values,0 do end
end
"#;
    for profile in PROFILES {
        let report = fixture.check(source, profile);
        assert!(!report.has_errors(), "{:#?}", report.files);
        let wrong = format!(
            "{source}\n---@param packed std__Packed<string>\nlocal function rejectWrongElements(packed)\n---@type table<number,number>\nlocal rejected=packed\nend\n"
        );
        let report = fixture.check(&wrong, profile);
        let errors = &report
            .file(report.project.entries[0])
            .unwrap()
            .report
            .diagnostics;
        assert_eq!(errors.len(), 1, "{errors:#?}");
        assert_eq!(errors[0].code, "TS2322");
        assert_eq!(&wrong[errors[0].span.start..errors[0].span.end], "packed");
        let corrected = wrong.replace(
            "---@type table<number,number>",
            "---@type table<number,string>",
        );
        let report = fixture.check(&corrected, profile);
        assert!(!report.has_errors(), "{:#?}", report.files);
    }
}

#[test]
fn recursive_containers_reject_sparse_and_opaque_values_and_accept_nested_valid_values() {
    let fixture = Fixture::new();
    for wrong in [
        "{[3]='sparse'}",
        "coroutine.create(function() end)",
        "{nested={[3]='sparse'}}",
    ] {
        let source = format!("{JSON}---@type JSONValue\nlocal value={wrong}\n");
        for profile in PROFILES {
            let report = fixture.check(&source, profile);
            let errors = &report
                .file(report.project.entries[0])
                .unwrap()
                .report
                .diagnostics;
            assert_eq!(errors.len(), 1, "{source}\n{errors:#?}");
            assert_eq!(errors[0].code, "TS2322", "{errors:#?}");
            assert_eq!(&source[errors[0].span.start..errors[0].span.end], wrong);
            let corrected = source.replace(wrong, "{nested={1,true,{label='text'}}}");
            let report = fixture.check(&corrected, profile);
            assert!(!report.has_errors(), "{corrected}\n{:#?}", report.files);
        }
    }
}

#[test]
fn opaque_nominal_inheritance_and_record_members_do_not_imply_dictionary_compatibility() {
    let fixture = Fixture::new();
    let prefix = "---@class Host:userdata\n---@field value number\n\n---@class Routine:thread\n\n---@shape View\n---@field value number\n\n---@param host Host\n---@param routine Routine\nlocal function inspect(host,routine)\n---@type userdata\nlocal opaque=host\n---@type thread\nlocal parent=routine\n---@type View\nlocal view=host\n---@type number\nlocal value=view.value\n";
    for profile in PROFILES {
        let valid = format!("{prefix}end\n");
        let report = fixture.check(&valid, profile);
        assert!(!report.has_errors(), "{:#?}", report.files);
        for (annotation, original) in [
            ("table<string,number>", "host"),
            ("table<string,unknown>", "routine"),
        ] {
            let source =
                format!("{prefix}---@type {annotation}\nlocal dictionary={original}\nend\n");
            let report = fixture.check(&source, profile);
            let errors = &report
                .file(report.project.entries[0])
                .unwrap()
                .report
                .diagnostics;
            assert_eq!(errors.len(), 1, "{source}\n{errors:#?}");
            assert_eq!(errors[0].code, "TS2322");
            assert_eq!(&source[errors[0].span.start..errors[0].span.end], original);
        }
    }
}

#[test]
fn cli_rejects_a_dictionary_that_would_give_pairs_the_wrong_key_type() {
    let fixture = Fixture::new();
    let source = "---@type table<number,number>\nlocal dictionary={word=1}\nfor key,value in pairs(dictionary) do\nlocal incremented=key+1\nend\n";
    let run = |source: &str| {
        let path = fixture.source(source);
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .args(["check", "--format", "json", "--lua-version", "5.4"])
            .arg(path)
            .output()
            .unwrap()
    };
    let rejected = run(source);
    let stdout = String::from_utf8(rejected.stdout).unwrap();
    assert_eq!(rejected.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("\"code\":\"TS2322\""), "{stdout}");
    assert!(rejected.stderr.is_empty(), "{:?}", rejected.stderr);
    let corrected = run(&source.replace("word=1", "[1]=1"));
    let stdout = String::from_utf8(corrected.stdout).unwrap();
    assert_eq!(corrected.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("\"diagnostics\":[]"), "{stdout}");
    assert!(corrected.stderr.is_empty(), "{:?}", corrected.stderr);
}
