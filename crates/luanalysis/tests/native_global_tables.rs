//! Original global allocations retain their later stores and publish complete schemas.
use luanalysis::incremental::ProjectWorkspace;
use luanalysis::language_service::{complete, completion_site};
use luanalysis::project_check::{ProjectCheckOptions, ProjectCheckReport, check_project, check_project_with_options};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis::{CheckReport, check};
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{fs, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

const GLOBAL: &str = r#"GlobalTable={initial=1}
---@type number
GlobalTable.later=2
---@param value number
---@return number
function GlobalTable.read(value) return value end
"#;

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    report
}

fn diagnostic(source: &str, code: &str, expression: &str) {
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{source}\n{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert_eq!(error.code, code, "{error:?}");
    assert_eq!(&source[error.span.start..error.span.end], expression);
}

#[test]
fn global_constructor_stores_keep_their_written_contracts_and_function_signatures() {
    let source = format!("{GLOBAL}\n---@type number\nlocal later=GlobalTable.later\n---@type number\nlocal result=GlobalTable.read(42)\n");
    clean(&source);
    diagnostic(&source.replace("read(42)", "read('wrong')"), "TS2345", "'wrong'");
    diagnostic(&format!("{source}GlobalTable.later='wrong'\n"), "TS2322", "'wrong'");
    diagnostic(&format!("{source}local absent=GlobalTable.missing\n"), "TS2339", "missing");
    clean(&source.replace("read(42)", "read(7)"));
}

#[test]
fn a_rebound_global_owns_the_new_constructor_without_mutating_the_original_alias() {
    let source = "GlobalTable={}\nlocal original=GlobalTable\nGlobalTable={}\nGlobalTable.later=1\nlocal later=GlobalTable.later\n";
    clean(source);
    diagnostic(&format!("{source}local absent=original.later\n"), "TS2339", "later");
}

#[test]
fn a_global_allocation_schema_does_not_make_a_future_store_present_earlier() {
    let source = "GlobalTable={}\nlocal before=GlobalTable.later\n---@type nil\nlocal absent=before\nGlobalTable.later=1\n---@type number\nlocal after=GlobalTable.later\n";
    clean(source);
    diagnostic(&source.replace("---@type nil", "---@type number"), "TS2322", "before");
}

#[test]
fn own_schema_initialization_keeps_written_values_and_other_allocations_checked() {
    let source = "GlobalTable={\n---@type number\nvalue=1,\n}\nGlobalTable.later=1\n";
    clean(source);
    diagnostic(&source.replace("value=1", "value='wrong'"), "TS2322", "'wrong'");
    diagnostic(&format!("{source}GlobalTable.value='wrong'\n"), "TS2322", "'wrong'");
    diagnostic(&format!("{source}GlobalTable={{}}\n"), "TS2322", "{}");
}

#[test]
fn a_lexically_shadowed_environment_cannot_attach_stores_to_the_physical_global() {
    let source = "GlobalTable={}\ndo\nlocal _ENV={GlobalTable={}}\nGlobalTable.only_here=1\nlocal present=GlobalTable.only_here\nend\n";
    clean(source);
    diagnostic(&format!("{source}local absent=GlobalTable.only_here\n"), "TS2339", "only_here");
}

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("luanalysis global allocation {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn load(&self, sources: &[(&str, &str)]) -> Project {
        let paths = sources.iter().map(|(name, source)| {
            let path = self.0.join(name);
            fs::write(&path, source).unwrap();
            path
        }).collect::<Vec<_>>();
        let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
        Project::load_plan(&resolver, ProjectLoadPlan::new(paths), |_| vec![])
    }
    fn check(&self, sources: &[(&str, &str)]) -> ProjectCheckReport {
        check_project(self.load(sources))
    }
    fn check_standard(&self, sources: &[(&str, &str)]) -> ProjectCheckReport {
        check_project_with_options(self.load(sources), ProjectCheckOptions {
            environment: StandardEnvironmentProfile::Lua54,
            ..Default::default()
        })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}
fn clean_project(report: &ProjectCheckReport) {
    for file in &report.files {
        assert!(file.report.diagnostics.is_empty(), "{}: {:#?}", report.source(file.file).unwrap().path.display(), file.report.diagnostics);
    }
}

#[test]
fn global_and_nested_allocation_members_publish_to_other_files_with_argument_contracts() {
    let fixture = Fixture::new();
    let producer = format!("{GLOBAL}\n---@class Socket\nlocal Socket={{}}\nSocket.AnonymousThread={{}}\n---@param timeout number\nfunction Socket.AnonymousThread.execute(timeout) end\n");
    let consumer = "---@type number\nlocal later=GlobalTable.later\n---@type number\nlocal result=GlobalTable.read(42)\n---@type Socket\nlocal socket\nsocket.AnonymousThread.execute(1)\n";
    clean_project(&fixture.check(&[("producer.lua", &producer), ("consumer.lua", consumer)]));
    for (original, replacement) in [("GlobalTable.read(42)", "GlobalTable.read('wrong')"), ("execute(1)", "execute('wrong')")] {
        let wrong = consumer.replace(original, replacement);
        let report = fixture.check(&[("producer.lua", &producer), ("consumer.lua", &wrong)]);
        let diagnostics = report.files.iter().flat_map(|file| &file.report.diagnostics).collect::<Vec<_>>();
        assert_eq!(diagnostics.len(), 1, "{report:#?}");
        assert_eq!(diagnostics[0].code, "TS2345", "{report:#?}");
        assert_eq!(&wrong[diagnostics[0].span.start..diagnostics[0].span.end], "'wrong'");
    }
}

#[test]
fn global_member_completion_includes_original_and_later_constructor_fields() {
    let source = format!("{GLOBAL}\nGlobalTable.\n");
    let offset = source.rfind('.').unwrap() + 1;
    let site = completion_site(&source, offset).unwrap();
    let fixture = Fixture::new();
    let project = fixture.load(&[("main.lua", &site.text)]);
    let file = project.entries[0];
    let labels = std::sync::Mutex::new(Vec::new());
    complete(project, ProjectCheckOptions::default(), &mut ProjectWorkspace::default(), file, &site, |items| {
        *labels.lock().unwrap() = items.into_iter().map(|item| item.label).collect();
    });
    let labels = labels.into_inner().unwrap();
    for expected in ["initial", "later", "read"] { assert!(labels.iter().any(|label| label == expected), "{labels:?}"); }
}

#[test]
fn a_standard_namespace_extension_publishes_its_original_signature_to_other_files() {
    let fixture = Fixture::new();
    let producer = "---@param value number\n---@return number\nfunction math.extension(value) return value end\n";
    let consumer = "---@type number\nlocal result=math.extension(42)\n---@type number\nlocal pi=math.pi\n";
    clean_project(&fixture.check_standard(&[("producer.lua", producer), ("consumer.lua", consumer)]));
    for (wrong, code, expression) in [
        (consumer.replace("extension(42)", "extension('wrong')"), "TS2345", "'wrong'"),
        (consumer.replacen("---@type number", "---@type string", 1), "TS2322", "math.extension(42)"),
    ] {
        let report = fixture.check_standard(&[("producer.lua", producer), ("consumer.lua", &wrong)]);
        let diagnostics = report.files.iter().flat_map(|file| &file.report.diagnostics).collect::<Vec<_>>();
        assert_eq!(diagnostics.len(), 1, "{report:#?}");
        assert_eq!(diagnostics[0].code, code, "{report:#?}");
        assert_eq!(&wrong[diagnostics[0].span.start..diagnostics[0].span.end], expression);
    }
    clean_project(&fixture.check_standard(&[("producer.lua", producer), ("consumer.lua", consumer)]));
}

#[test]
fn local_and_environment_namespace_shadows_cannot_augment_the_standard_class() {
    let fixture = Fixture::new();
    let consumer = "local absent=math.private\n";
    for shadow in ["local math={}\n", "local _ENV={math={}}\n"] {
        let producer = format!("{shadow}---@param value number\nfunction math.private(value) return value end\nmath.private(42)\n");
        let report = fixture.check_standard(&[("producer.lua", &producer), ("consumer.lua", consumer)]);
        let diagnostics = report.files.iter().flat_map(|file| &file.report.diagnostics).collect::<Vec<_>>();
        assert_eq!(diagnostics.len(), 1, "{producer}\n{report:#?}");
        assert_eq!(diagnostics[0].code, "TS2339", "{report:#?}");
        assert_eq!(&consumer[diagnostics[0].span.start..diagnostics[0].span.end], "private");
    }
}

#[test]
fn replacing_a_standard_member_still_checks_its_original_declared_contract() {
    let fixture = Fixture::new();
    let producer = "---@param value string\n---@return string\nfunction math.abs(value) return value end\n";
    let consumer = "---@type number\nlocal result=math.abs(42)\n";
    let report = fixture.check_standard(&[("producer.lua", producer), ("consumer.lua", consumer)]);
    let diagnostics = report.files.iter().flat_map(|file| &file.report.diagnostics).collect::<Vec<_>>();
    assert!(!diagnostics.is_empty(), "{report:#?}");
    assert!(diagnostics.iter().any(|error| error.code == "TS2322"), "{report:#?}");
    for file in &report.files {
        if report.source(file.file).unwrap().path.file_name().unwrap() == "consumer.lua" {
            assert!(file.report.diagnostics.is_empty(), "{report:#?}");
        }
    }
    let corrected = producer.replace("string", "number");
    clean_project(&fixture.check_standard(&[("producer.lua", &corrected), ("consumer.lua", consumer)]));
}

#[test]
fn a_repeated_namespace_extension_checks_the_first_signature_instead_of_adding_an_overload() {
    let fixture = Fixture::new();
    let producer = "---@param value number\n---@return number\nfunction math.extension(value) return value end\n---@param value string\n---@return string\nfunction math.extension(value) return value end\n";
    let consumer = "---@type number\nlocal result=math.extension(42)\n";
    let report = fixture.check_standard(&[("producer.lua", producer), ("consumer.lua", consumer)]);
    let producer_report = report.files.iter().find(|file| report.source(file.file).unwrap().path.file_name().unwrap() == "producer.lua").unwrap();
    assert!(producer_report.report.diagnostics.iter().any(|error| error.code == "TS2322"), "{report:#?}");
    let consumer_report = report.files.iter().find(|file| report.source(file.file).unwrap().path.file_name().unwrap() == "consumer.lua").unwrap();
    assert!(consumer_report.report.diagnostics.is_empty(), "{report:#?}");
    let wrong = consumer.replace("extension(42)", "extension('wrong')");
    let report = fixture.check_standard(&[("producer.lua", producer), ("consumer.lua", &wrong)]);
    let consumer_report = report.files.iter().find(|file| report.source(file.file).unwrap().path.file_name().unwrap() == "consumer.lua").unwrap();
    assert_eq!(consumer_report.report.diagnostics.len(), 1, "{report:#?}");
    assert_eq!(consumer_report.report.diagnostics[0].code, "TS2345", "{report:#?}");
}

#[test]
fn original_cross_file_global_completion_keeps_all_three_number_fields() {
    const GLOBALS: &str = include_str!("../../luanalysis-fixtures/tests/legacy/resources/completion/globals.lua");
    const MARKED: &str = include_str!("../../luanalysis-fixtures/tests/legacy/embedded/TestCompletion/testGlobalCompletion1/main.lua");
    let source = MARKED.replace("<caret>", "");
    let offset = MARKED.find("<caret>").unwrap();
    let site = completion_site(&source, offset).unwrap();
    let fixture = Fixture::new();
    for strict_nil in [false, true] {
        let project = fixture.load(&[("globals.lua", GLOBALS), ("main.lua", &site.text)]);
        let file = project.entries[1];
        let completions = std::sync::Mutex::new(Vec::new());
        complete(project, ProjectCheckOptions {
            environment: StandardEnvironmentProfile::Lua53,
            strict_nil,
            ..Default::default()
        }, &mut ProjectWorkspace::default(), file, &site, |items| {
            *completions.lock().unwrap() = items;
        });
        let completions = completions.into_inner().unwrap();
        let labels = completions.iter().map(|item| item.label.as_str()).collect::<std::collections::BTreeSet<_>>();
        assert_eq!(labels, ["aaa", "bbb", "ccc"].into_iter().collect(), "{completions:?}");
        for item in &completions {
            let literal = match item.label.as_str() {
                "aaa" => "2",
                "bbb" => "3",
                "ccc" => "4",
                _ => unreachable!("exact labels checked above"),
            };
            assert!(item.detail.as_deref().is_some_and(|detail| detail == "number" || detail == literal), "{item:?}");
        }
    }
    let consumer = "---@type number\nlocal aaa=gVar2.aaa\n---@type number\nlocal bbb=gVar2.bbb\n---@type number\nlocal ccc=gVar2.ccc\n";
    clean_project(&fixture.check(&[("globals.lua", GLOBALS), ("main.lua", consumer)]));
    let wrong = consumer.replace("---@type number", "---@type string");
    let report = fixture.check(&[("globals.lua", GLOBALS), ("main.lua", &wrong)]);
    let diagnostics = report.files.iter().flat_map(|file| &file.report.diagnostics).collect::<Vec<_>>();
    assert_eq!(diagnostics.len(), 3, "{report:#?}");
    assert!(diagnostics.iter().all(|error| error.code == "TS2322"), "{report:#?}");
}
