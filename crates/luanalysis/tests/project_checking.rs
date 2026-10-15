use luanalysis::project_check::{ProjectCheckReport, check_project};
use luanalysis_project::{
    Project, ProjectConfig, ProjectInput, ProjectLoadPlan, Resolver, SourceRole,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis project checking {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        path
    }
    fn load(&self, entries: Vec<PathBuf>, declarations: Vec<PathBuf>) -> Project {
        let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
        let mut plan = ProjectLoadPlan::new(entries);
        plan.libraries = declarations
            .into_iter()
            .map(|path| ProjectInput::new(path, SourceRole::Declaration))
            .collect();
        Project::load_plan(&resolver, plan, |_| vec![])
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn binding<'a>(report: &'a ProjectCheckReport, name: &str) -> &'a str {
    &report
        .files
        .iter()
        .flat_map(|file| &file.report.bindings)
        .find(|binding| binding.name == name)
        .unwrap()
        .ty
}
fn assert_clean(report: &ProjectCheckReport) {
    for file in &report.files {
        assert!(
            file.report.diagnostics.is_empty(),
            "{}: {:#?}",
            report.source(file.file).unwrap().path.display(),
            file.report.diagnostics
        );
    }
}

#[test]
fn separate_source_snapshots_share_forward_alias_and_generic_declaration_queries() {
    let fixture = Fixture::new();
    let early = fixture.file(
        "early.def.lua",
        "---@alias Early Later\n---@alias Box<T> {value:T}\n",
    );
    let later = fixture.file("later.def.lua", "---@alias Later number\n");
    let main = fixture.file("main.lua", "---@generic T\n---@param value Box<T>\n---@return T\nlocal function read(value) return value.value end\n---@type Early\nlocal original=42\nlocal numeric=read({value=original})\nlocal text=read({value='word'})\n---@type number\nlocal good=numeric\n---@type string\nlocal also_good=text\n");
    let report = check_project(fixture.load(vec![main], vec![early, later]));
    assert_clean(&report);
    assert_eq!(binding(&report, "original"), "number");
    assert_eq!(binding(&report, "numeric"), "number");
    assert_eq!(binding(&report, "text"), "string");
    assert_eq!(report.checked_file_count(), 3);
    assert_eq!(report.project.entries.len(), 1);
}

#[test]
fn cross_file_recursive_record_heads_publish_without_a_source_cycle_solver() {
    let fixture = Fixture::new();
    let left = fixture.file(
        "left.def.lua",
        "---@alias Left {value:number,right:Right|nil}\n",
    );
    let right = fixture.file(
        "right.def.lua",
        "---@alias Right {value:string,left:Left|nil}\n",
    );
    let main = fixture.file("main.lua", "---@param node Left\nlocal function read(node)\nif node.right then\n---@type string\nlocal found=node.right.value\nend\nreturn node.value\nend\n");
    let report = check_project(fixture.load(vec![main], vec![left, right]));
    assert_clean(&report);
    assert_eq!(binding(&report, "found"), "string");
}

#[test]
fn diagnostics_retain_the_importing_original_utf8_crlf_file_and_source_correction() {
    let fixture = Fixture::new();
    let types = fixture.file("types.def.lua", "---@alias Count number\n");
    let wrong =
        "-- café\r\n---@type Count\r\nlocal value=42\r\n---@type string\r\nlocal invalid=value\r\n";
    let main = fixture.file("main.lua", wrong);
    let first = check_project(fixture.load(vec![main.clone()], vec![types.clone()]));
    let id = first.project.entries[0];
    let report = &first.file(id).unwrap().report;
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    let source = &first.source(id).unwrap().source;
    let span = report.diagnostics[0].span;
    assert_eq!(&source[span.start..span.end], "value");
    assert_eq!(luanalysis::location(source, span), (5, 15));
    fs::write(&main, wrong.replace("---@type string", "---@type number")).unwrap();
    let corrected = check_project(fixture.load(vec![main], vec![types]));
    assert_clean(&corrected);
    assert_eq!(first.source(id).unwrap().source, wrong);
    assert!(corrected.file(id).is_none());
    assert!(corrected.source(id).is_none());
}

#[test]
fn cross_file_alias_cycles_and_unused_invalid_members_report_the_owner_source() {
    let fixture = Fixture::new();
    let first = fixture.file(
        "first.def.lua",
        "---@alias First Second\n---@shape Bad\n---@field value Missing\n",
    );
    let second = fixture.file("second.def.lua", "---@alias Second First\n");
    let report = check_project(fixture.load(vec![], vec![first, second]));
    assert!(report.files.iter().any(|file| {
        file.report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2456")
    }));
    for file in &report.files {
        let source = &report.source(file.file).unwrap().source;
        for error in &file.report.diagnostics {
            let origin = &source[error.span.start..error.span.end];
            match error.code {
                "TS2456" => assert!(matches!(origin, "First" | "Second")),
                "TS2304" => assert_eq!(origin, "Missing"),
                other => panic!("unexpected {other}: {error:?}"),
            }
        }
    }
    assert!(report.files.iter().any(|file| {
        file.report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2304")
    }));
}

#[test]
fn duplicate_project_named_types_keep_each_origin_and_never_select_a_first_declaration() {
    let fixture = Fixture::new();
    let first = fixture.file("first.def.lua", "---@alias Same number\n");
    let second = fixture.file("second.def.lua", "---@alias Same string\n");
    let main = fixture.file("main.lua", "---@type Same\nlocal value=42\n");
    let report = check_project(fixture.load(vec![main], vec![first, second]));
    assert_eq!(report.checked_file_count(), 3);
    for file in &report.files {
        let source = &report.source(file.file).unwrap().source;
        assert_eq!(
            file.report.diagnostics.len(),
            1,
            "{:#?}",
            file.report.diagnostics
        );
        let error = &file.report.diagnostics[0];
        assert_eq!(error.code, "TS2300");
        assert_eq!(&source[error.span.start..error.span.end], "Same");
    }
}

#[test]
fn cross_file_augmentations_supply_candidates_to_canonical_native_owner_resolution() {
    let fixture = Fixture::new();
    let original = fixture.file("original.def.lua", "---@alias Original {x:number}\n");
    let extension = fixture.file(
        "extension.def.lua",
        "---@alias Owner Original\n---@field <Owner> y string\n",
    );
    let main = fixture.file(
        "main.lua",
        "---@type Original\nlocal value={x=1,y='yes'}\nlocal read=value.y\n",
    );
    let report = check_project(fixture.load(vec![main], vec![original, extension]));
    assert_clean(&report);
    assert_eq!(binding(&report, "read"), "string");
}

#[test]
fn syntax_invalid_sources_remain_owned_and_do_not_poison_other_files() {
    let fixture = Fixture::new();
    let invalid = fixture.file("invalid.lua", "local = ?");
    let valid = fixture.file("valid.lua", "local value=42");
    let report = check_project(fixture.load(vec![invalid, valid], vec![]));
    assert_eq!(report.checked_file_count(), 2);
    assert!(report.files[0].report.bound.is_none());
    assert!(
        report.files[0]
            .report
            .diagnostics
            .iter()
            .all(|error| error.code == "LUA1001")
    );
    assert!(report.files[1].report.diagnostics.is_empty());
    assert_eq!(binding(&report, "value"), "number");
    assert_eq!(
        report.source(report.files[0].file).unwrap().source,
        "local = ?"
    );
}

#[test]
fn declaration_value_stubs_stay_explicitly_incomplete_without_false_runtime_body_errors() {
    let fixture = Fixture::new();
    let stub = fixture.file(
        "value.def.lua",
        "---@return number\nlocal function external() end\n",
    );
    let implementation = fixture.file(
        "main.lua",
        "---@return number\nlocal function actual() end\n",
    );
    let report = check_project(fixture.load(vec![implementation], vec![stub]));
    let declarations = &report.file(report.project.libraries[0]).unwrap().report;
    assert_eq!(
        declarations.diagnostics.len(),
        1,
        "{:#?}",
        declarations.diagnostics
    );
    assert_eq!(declarations.diagnostics[0].code, "LUA2034");
    let actual = &report.file(report.project.entries[0]).unwrap().report;
    assert!(
        actual
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2355"),
        "{:#?}",
        actual.diagnostics
    );
}

#[test]
fn independent_files_reusing_binding_and_function_indices_keep_their_own_reports() {
    let fixture = Fixture::new();
    let numeric = fixture.file(
        "numeric.lua",
        "---@generic T\n---@param seed T\nlocal function outer(seed) return function() return seed,seed end end\nlocal closure=outer(42)\nlocal first,second=closure()\n---@type number\nlocal good=second\n",
    );
    let textual = fixture.file(
        "textual.lua",
        "---@generic T\n---@param seed T\nlocal function outer(seed) return function() return seed,seed end end\nlocal closure=outer('word')\nlocal first,second=closure()\n---@type string\nlocal good=second\n",
    );
    let report = check_project(fixture.load(vec![numeric, textual], vec![]));
    assert_clean(&report);
    for (file, expected) in report.files.iter().zip(["number", "string"]) {
        let second = file
            .report
            .bindings
            .iter()
            .find(|binding| binding.name == "second")
            .unwrap();
        assert_eq!(second.ty, expected);
        let source = report.source(file.file).unwrap();
        assert_eq!(&source.source[second.span.start..second.span.end], "second");
        let binding = file
            .report
            .bound
            .as_ref()
            .unwrap()
            .binding_at(second.span.start)
            .unwrap();
        assert_eq!(binding, second.binding);
    }
    let first_binding = report.files[0]
        .report
        .bindings
        .iter()
        .find(|binding| binding.name == "second")
        .unwrap()
        .binding;
    let second_binding = report.files[1]
        .report
        .bindings
        .iter()
        .find(|binding| binding.name == "second")
        .unwrap()
        .binding;
    assert_eq!(first_binding, second_binding);
}

#[test]
fn project_host_failures_and_empty_snapshots_remain_available_to_the_caller() {
    let fixture = Fixture::new();
    let resolver = Resolver::new(ProjectConfig::new([fixture.0.clone()])).unwrap();
    let missing = fixture.0.join("missing.lua");
    let library = fixture.file("types.def.lua", "---@alias Count number\n");
    let project = Project::load_plan(
        &resolver,
        ProjectLoadPlan {
            entries: vec![missing, library.clone()],
            libraries: vec![ProjectInput::new(library, SourceRole::Declaration)],
        },
        |_| vec![],
    );
    let report = check_project(project);
    assert_eq!(report.project.entry_errors.len(), 1);
    assert_eq!(report.project.role_conflicts.len(), 1);
    assert!(report.has_errors());
    assert_eq!(report.checked_file_count(), 1);
    assert_clean(&report);
    let empty = check_project(fixture.load(vec![], vec![]));
    assert_eq!(empty.checked_file_count(), 0);
    // The default profile supplies its original loader declaration even when
    // the host supplies no source. It is retained as a standard-library file.
    assert_eq!(empty.standard_library_files.len(), 1);
    assert!(empty.project.files.iter().all(|source| empty.standard_library_files.contains(&source.id)));
    assert!(!empty.has_errors());
}
