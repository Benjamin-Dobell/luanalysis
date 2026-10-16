//! Project checking must preserve original-source results when Bun schedules
//! native checker tasks on different workers. Only source-generation FileIds and
//! incidental output ordering are removed from these comparisons.
use luanalysis::{
    BindingInfo, Diagnostic, TypeInfo,
    bind::BoundFile,
    project_check::{
        ProjectCheckOptions, ProjectCheckReport, check_project_with_options,
        static_require_requests,
    },
    standard_environment::StandardEnvironmentProfile,
};
use luanalysis_project::{
    DependencyOutcome, FileId, Project, ProjectConfig, ProjectInput, ProjectLoadPlan, Resolver,
    SourceOrigin, SourceRole,
};
use std::{
    fs,
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis parallel project checking {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn file(&self, name: &str, source: &str) {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }

    fn check(
        &self,
        entries: &[&str],
        declarations: &[&str],
        environment: StandardEnvironmentProfile,
        jobs: usize,
    ) -> ProjectCheckReport {
        let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
        let mut plan = ProjectLoadPlan::new(entries.iter().map(|name| self.0.join(name)));
        plan.libraries = declarations
            .iter()
            .map(|name| ProjectInput::new(self.0.join(name), SourceRole::Declaration))
            .collect();
        check_project_with_options(
            Project::load_plan(&resolver, plan, static_require_requests),
            ProjectCheckOptions {
                jobs,
                environment,
                ..ProjectCheckOptions::default()
            },
        )
    }

    /// Every run reloads the source host too: generation-local IDs must never
    /// accidentally make two semantically identical reports appear different.
    fn parity(
        &self,
        entries: &[&str],
        declarations: &[&str],
        environment: StandardEnvironmentProfile,
    ) -> ProjectCheckReport {
        let serial = self.check(entries, declarations, environment, 1);
        let expected = snapshot(&serial, &self.0);
        for jobs in [2, 4, 4, 2, 0] {
            let candidate = self.check(entries, declarations, environment, jobs);
            assert_eq!(
                snapshot(&candidate, &self.0),
                expected,
                "project results changed with jobs={jobs}"
            );
        }
        serial
    }

    fn minimal(&self, entries: &[&str], declarations: &[&str]) -> ProjectCheckReport {
        self.parity(
            entries,
            declarations,
            StandardEnvironmentProfile::LegacyMinimal,
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[derive(Debug, PartialEq, Eq)]
struct SourceSnapshot {
    path: PathBuf,
    source: String,
    role: SourceRole,
    origin: SourceOrigin,
}

#[derive(Debug, PartialEq, Eq)]
struct FileSnapshot {
    path: PathBuf,
    diagnostics: Vec<Diagnostic>,
    bindings: Vec<BindingInfo>,
    types: Vec<TypeInfo>,
    bound: Option<BoundFile>,
}

#[derive(Debug, PartialEq, Eq)]
enum DependencySnapshot {
    Resolved(PathBuf),
    Failed(String),
    InvalidSpan,
}

#[derive(Debug, PartialEq, Eq)]
struct ProjectSnapshot {
    sources: Vec<SourceSnapshot>,
    reports: Vec<FileSnapshot>,
    entries: Vec<PathBuf>,
    libraries: Vec<PathBuf>,
    standard_library_files: Vec<PathBuf>,
    dependencies: Vec<(PathBuf, String, Range<usize>, DependencySnapshot)>,
    cycles: Vec<Vec<PathBuf>>,
    host_errors: Vec<String>,
    checked_file_count: usize,
    has_errors: bool,
}

fn snapshot(report: &ProjectCheckReport, root: &Path) -> ProjectSnapshot {
    let normalize = |path: &Path| path.strip_prefix(root).unwrap_or(path).to_path_buf();
    let path = |id: FileId| normalize(&report.source(id).expect("owned original source").path);
    let mut sources: Vec<_> = report
        .project
        .files
        .iter()
        .map(|source| SourceSnapshot {
            path: normalize(&source.path),
            source: source.source.clone(),
            role: source.role,
            origin: source.origin.clone(),
        })
        .collect();
    sources.sort_by(|a, b| a.path.cmp(&b.path));
    let mut reports: Vec<_> = report
        .files
        .iter()
        .map(|file| {
            let mut diagnostics = file.report.diagnostics.clone();
            diagnostics.sort_by(|a, b| {
                (a.span.start, a.span.end, a.code, &a.message).cmp(&(
                    b.span.start,
                    b.span.end,
                    b.code,
                    &b.message,
                ))
            });
            let mut bindings = file.report.bindings.clone();
            bindings.sort_by(|a, b| {
                (a.binding.0, a.span.start, a.span.end, &a.name, &a.ty).cmp(&(
                    b.binding.0,
                    b.span.start,
                    b.span.end,
                    &b.name,
                    &b.ty,
                ))
            });
            let mut types = file.report.types.clone();
            types.sort_by(|a, b| {
                (a.span.start, a.span.end, &a.ty).cmp(&(b.span.start, b.span.end, &b.ty))
            });
            FileSnapshot {
                path: path(file.file),
                diagnostics,
                bindings,
                types,
                bound: file.report.bound.clone(),
            }
        })
        .collect();
    reports.sort_by(|a, b| a.path.cmp(&b.path));
    let mut standard_library_files: Vec<_> = report
        .standard_library_files
        .iter()
        .copied()
        .map(path)
        .collect();
    standard_library_files.sort();
    let mut dependencies: Vec<_> = report
        .project
        .dependencies
        .iter()
        .map(|dependency| {
            let outcome = match &dependency.outcome {
                DependencyOutcome::Resolved(id) => DependencySnapshot::Resolved(path(*id)),
                DependencyOutcome::Unresolved(error) => {
                    DependencySnapshot::Failed(format!("unresolved: {error}"))
                }
                DependencyOutcome::Unreadable(error) => {
                    DependencySnapshot::Failed(format!("unreadable: {error}"))
                }
                DependencyOutcome::InvalidSpan => DependencySnapshot::InvalidSpan,
            };
            (
                path(dependency.from),
                dependency.request.module.clone(),
                dependency.request.span.clone(),
                outcome,
            )
        })
        .collect();
    dependencies
        .sort_by(|a, b| (&a.0, &a.1, a.2.start, a.2.end).cmp(&(&b.0, &b.1, b.2.start, b.2.end)));
    let mut cycles: Vec<_> = report
        .project
        .cycles
        .iter()
        .map(|cycle| cycle.files.iter().copied().map(path).collect::<Vec<_>>())
        .collect();
    cycles.sort();
    let mut host_errors: Vec<_> = report
        .project
        .entry_errors
        .iter()
        .map(|error| format!("entry: {error}"))
        .chain(
            report
                .project
                .library_errors
                .iter()
                .map(|error| format!("library: {error}")),
        )
        .chain(
            report
                .project
                .role_conflicts
                .iter()
                .map(|error| format!("role: {error}")),
        )
        .chain(
            report
                .environment_errors
                .iter()
                .map(|error| format!("environment: {error}")),
        )
        .collect();
    host_errors.sort();
    ProjectSnapshot {
        sources,
        reports,
        entries: report.project.entries.iter().copied().map(path).collect(),
        libraries: report.project.libraries.iter().copied().map(path).collect(),
        standard_library_files,
        dependencies,
        cycles,
        host_errors,
        checked_file_count: report.checked_file_count(),
        has_errors: report.has_errors(),
    }
}

fn clean(report: &ProjectCheckReport) {
    assert!(!report.has_errors(), "{:#?}", report.files);
    assert!(
        report
            .files
            .iter()
            .all(|file| file.report.diagnostics.is_empty())
    );
}

fn binding<'a>(report: &'a ProjectCheckReport, file: &str, name: &str) -> &'a str {
    &report
        .files
        .iter()
        .find(|checked| report.source(checked.file).unwrap().path.ends_with(file))
        .expect(file)
        .report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .expect(name)
        .ty
}

fn error_at(report: &ProjectCheckReport, file: &str, code: &str, expression: &str) {
    let checked = report
        .files
        .iter()
        .find(|checked| report.source(checked.file).unwrap().path.ends_with(file))
        .expect(file);
    let source = &report.source(checked.file).unwrap().source;
    assert!(
        checked.report.diagnostics.iter().any(|error| {
            error.code == code && &source[error.span.start..error.span.end] == expression
        }),
        "expected {code} at original {file} bytes for {expression:?}: {:#?}",
        checked.report.diagnostics
    );
}

#[test]
fn unrelated_roots_share_global_aliases_and_generic_class_heads() {
    let fixture = Fixture::new();
    fixture.file(
        "early.def.lua",
        "---@alias Early Later\n---@alias Box<T> {value:T}\n",
    );
    fixture.file(
        "later.def.lua",
        "---@alias Later number\n---@class Named<T>\n---@field value T\n",
    );
    let main = "-- café\r\n---@type Early\r\nlocal original=42\r\n---@param value Box<number>\r\nlocal function read(value)\r\n---@type number\r\nlocal accepted=value.value\r\nreturn accepted\r\nend\r\n";
    fixture.file("main.lua", main);
    fixture.file("other.lua", "---@param value Named<string>\nlocal function read(value)\nlocal text=value.value\n---@type string\nlocal checked=text\nend\n");
    fixture.file(
        "third.lua",
        "---@type Box<string>\nlocal boxed={value='word'}\nlocal text=boxed.value\n",
    );
    fixture.file("fourth.lua", "---@type Early\nlocal another=17\n");
    let entries = ["main.lua", "other.lua", "third.lua", "fourth.lua"];
    let declarations = ["early.def.lua", "later.def.lua"];
    let first = fixture.minimal(&entries, &declarations);
    clean(&first);
    assert_eq!(first.checked_file_count(), 6);
    assert_eq!(binding(&first, "main.lua", "original"), "number");
    assert_eq!(binding(&first, "other.lua", "text"), "string");
    fixture.file(
        "main.lua",
        &main.replace("---@type number", "---@type string"),
    );
    let wrong = fixture.minimal(&entries, &declarations);
    error_at(&wrong, "main.lua", "TS2322", "value.value");
    fixture.file("main.lua", main);
    let corrected = fixture.minimal(&entries, &declarations);
    clean(&corrected);
    assert_eq!(
        snapshot(&corrected, &fixture.0),
        snapshot(&first, &fixture.0)
    );
}

#[test]
fn required_generic_closures_and_contextual_callbacks_keep_separate_frames() {
    let fixture = Fixture::new();
    fixture.file("factory.lua", "---@generic T\n---@param value T\nlocal function make(value)\n---@overload fun():T\n---@return T\nlocal function get()return value end\nreturn get\nend\nreturn make\n");
    fixture.file(
        "value.lua",
        "local value={id=42}\nreturn function() return value.id end\n",
    );
    let main = "local make=require('factory')\nlocal getter=make(42)\nlocal numeric=getter()\n---@generic T,R\n---@param value T\n---@param project fun(value:T):R\n---@return R\nlocal function transform(value,project) return project(value) end\nlocal contextual=transform({label='word'},function(value) local read=require('value') return read() end)\n---@type number\nlocal checked=numeric\n---@type number\nlocal checked_context=contextual\n";
    fixture.file("numeric.lua", main);
    fixture.file("textual.lua", "local make=require('factory')\nlocal getter=make('word')\nlocal textual=getter()\n---@type string\nlocal checked=textual\n");
    fixture.file("again.lua", "local make=require('factory')\nlocal getNumber=make(12)\nlocal getText=make('again')\nlocal first=getNumber()\nlocal second=getText()\nlocal third=getNumber()\n");
    fixture.file(
        "callback.lua",
        "local read=require('value')\nlocal numeric=read()\n",
    );
    let entries = ["numeric.lua", "textual.lua", "again.lua", "callback.lua"];
    let first = fixture.minimal(&entries, &[]);
    clean(&first);
    assert_eq!(first.checked_file_count(), 6);
    for name in ["numeric", "contextual"] {
        assert_eq!(binding(&first, "numeric.lua", name), "number");
    }
    assert_eq!(binding(&first, "textual.lua", "textual"), "string");
    assert_eq!(binding(&first, "again.lua", "third"), "number");
    fixture.file(
        "numeric.lua",
        &main.replace(
            "---@type number\nlocal checked=",
            "---@type string\nlocal checked=",
        ),
    );
    error_at(
        &fixture.minimal(&entries, &[]),
        "numeric.lua",
        "TS2322",
        "numeric",
    );
    fixture.file("numeric.lua", main);
    let corrected = fixture.minimal(&entries, &[]);
    clean(&corrected);
    assert_eq!(
        snapshot(&corrected, &fixture.0),
        snapshot(&first, &fixture.0)
    );
}

#[test]
fn overload_implementation_contracts_stay_private_and_are_still_checked() {
    let fixture = Fixture::new();
    let module = "---@overload fun(value:number):number\n---@overload fun(value:string):string\n---@param value number|string|boolean\n---@return number|string|boolean\nlocal function echo(value)return value end\nreturn echo\n";
    fixture.file("echo.lua", module);
    let main = "local echo=require('echo')\nlocal numeric=echo(42)\nlocal textual=echo('word')\n";
    fixture.file("main.lua", main);
    fixture.file(
        "second.lua",
        "local echo=require('echo')\nlocal textual=echo('second')\n",
    );
    fixture.file(
        "third.lua",
        "local echo=require('echo')\nlocal numeric=echo(17)\n",
    );
    fixture.file(
        "fourth.lua",
        "local echo=require('echo')\nlocal numeric=echo(29)\n",
    );
    let entries = ["main.lua", "second.lua", "third.lua", "fourth.lua"];
    let first = fixture.minimal(&entries, &[]);
    clean(&first);
    assert_eq!(binding(&first, "main.lua", "numeric"), "number");
    assert_eq!(binding(&first, "main.lua", "textual"), "string");
    assert!(!binding(&first, "main.lua", "echo").contains("boolean"));
    fixture.file("main.lua", &format!("{main}local rejected=echo(true)\n"));
    error_at(
        &fixture.minimal(&entries, &[]),
        "main.lua",
        "TS2345",
        "true",
    );
    fixture.file("main.lua", main);
    fixture.file(
        "echo.lua",
        &module.replace(
            "---@param value number|string|boolean",
            "---@param value number",
        ),
    );
    let incompatible = fixture.minimal(&entries, &[]);
    assert!(
        incompatible.files.iter().any(|file| file
            .report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2394")),
        "{:#?}",
        incompatible.files
    );
    fixture.file("echo.lua", module);
    let corrected = fixture.minimal(&entries, &[]);
    clean(&corrected);
    assert_eq!(
        snapshot(&corrected, &fixture.0),
        snapshot(&first, &fixture.0)
    );
}

#[test]
fn methods_keep_receiver_mappers_and_unrelated_classes_keep_nominal_origins() {
    let fixture = Fixture::new();
    fixture.file("base.def.lua", "---@class Base<T>\n---@field value T\n---@function :get():T\n---@function :identity():self\n");
    fixture.file("derived.def.lua", "---@class Derived:Base<number>\n---@field name string\n\n---@class Other\n---@field value number\n");
    let main = "---@param item Derived\n---@return Base<number>\nlocal function read(item)\nlocal numeric=item:get()\nlocal label=item:identity().name\n---@type number\nlocal checked=numeric\n---@type string\nlocal checked_label=label\nreturn item\nend\n";
    fixture.file("main.lua", main);
    fixture.file("dot.lua", "---@param item Derived\nlocal function read(item)\nlocal numeric=item.get(item)\n---@type number\nlocal checked=numeric\nend\n");
    fixture.file(
        "other.lua",
        "---@param item Other\n---@return Other\nlocal function same(item)return item end\n",
    );
    fixture.file("fourth.lua", "---@param item Base<string>\nlocal function read(item)\nlocal text=item:get()\n---@type string\nlocal checked=text\nend\n");
    let entries = ["main.lua", "dot.lua", "other.lua", "fourth.lua"];
    let declarations = ["base.def.lua", "derived.def.lua"];
    let first = fixture.minimal(&entries, &declarations);
    clean(&first);
    assert_eq!(binding(&first, "main.lua", "numeric"), "number");
    assert_eq!(binding(&first, "main.lua", "label"), "string");
    fixture.file(
        "main.lua",
        &main.replace("---@return Base<number>", "---@return Other"),
    );
    error_at(
        &fixture.minimal(&entries, &declarations),
        "main.lua",
        "TS2322",
        "item",
    );
    fixture.file("main.lua", main);
    clean(&fixture.minimal(&entries, &declarations));

    // Visibility enforcement is currently an explicit feature gate. Parallel
    // execution must retain that diagnostic rather than silently erasing it.
    fixture.file(
        "private.def.lua",
        "---@class Secret\n---@field private value number\n\n---@class Descendant:Secret\n",
    );
    let private_declarations = ["base.def.lua", "derived.def.lua", "private.def.lua"];
    let gated = fixture.minimal(&entries, &private_declarations);
    assert!(
        gated.files.iter().any(|file| file
            .report
            .diagnostics
            .iter()
            .any(|error| error.code == "LUA2031")),
        "{:#?}",
        gated.files
    );
    fixture.file(
        "private.def.lua",
        "---@class Secret\n---@field value number\n\n---@class Descendant:Secret\n",
    );
    clean(&fixture.minimal(&entries, &private_declarations));
}

#[test]
fn circular_require_keeps_guard_diagnostics_and_corrected_exports_are_rechecked() {
    let fixture = Fixture::new();
    fixture.file("first.lua", "local next=require('second')\nreturn next\n");
    fixture.file("second.lua", "local next=require('first')\nreturn next\n");
    fixture.file(
        "main.lua",
        "local value=require('first')\nlocal again=require('first')\n",
    );
    fixture.file("other.lua", "local value=require('first')\n");
    fixture.file("number.lua", "return 42\n");
    fixture.file("independent.lua", "local numeric=require('number')\n");
    fixture.file("fourth.lua", "local numeric=require('number')\n");
    let entries = ["main.lua", "other.lua", "independent.lua", "fourth.lua"];
    let circular = fixture.minimal(&entries, &[]);
    assert!(circular.has_errors());
    assert!(!circular.project.cycles.is_empty());
    assert!(
        circular.files.iter().any(|file| file
            .report
            .diagnostics
            .iter()
            .any(|error| error.code == "LUA2036" && error.message.contains("circular require"))),
        "{:#?}",
        circular.files
    );
    fixture.file("second.lua", "return 17\n");
    let corrected = fixture.minimal(&entries, &[]);
    clean(&corrected);
    assert!(corrected.project.cycles.is_empty());
    assert_eq!(binding(&corrected, "main.lua", "value"), "number");
    assert_eq!(binding(&corrected, "other.lua", "value"), "number");
    fixture.file(
        "main.lua",
        "local value=require('first')\n---@type string\nlocal rejected=value\n",
    );
    error_at(
        &fixture.minimal(&entries, &[]),
        "main.lua",
        "TS2322",
        "value",
    );
    fixture.file(
        "main.lua",
        "local value=require('first')\nlocal again=require('first')\n",
    );
    clean(&fixture.minimal(&entries, &[]));
}

#[test]
fn unknown_effects_keep_builtin_narrowing_but_explicit_replacement_does_not() {
    let fixture = Fixture::new();
    fixture.file(
        "host.def.lua",
        "---@return nil\nfunction hostEffect() end\n",
    );
    let main = "---@param value number|string\nlocal function probe(value)\nhostEffect()\nif type(value)=='number' then\n---@type number\nlocal numeric=value\nend\nend\nprobe(42)\n";
    fixture.file("main.lua", main);
    fixture.file("other.lua", "---@type number\nlocal numeric=42\n");
    fixture.file("third.lua", "---@type string\nlocal textual='word'\n");
    fixture.file("fourth.lua", "---@type boolean\nlocal flag=true\n");
    let entries = ["main.lua", "other.lua", "third.lua", "fourth.lua"];
    let declarations = ["host.def.lua"];
    let first = fixture.parity(&entries, &declarations, StandardEnvironmentProfile::Lua52);
    clean(&first);
    assert_eq!(binding(&first, "main.lua", "numeric"), "number");
    fixture.file(
        "main.lua",
        &main.replace("---@type number", "---@type string"),
    );
    error_at(
        &fixture.parity(&entries, &declarations, StandardEnvironmentProfile::Lua52),
        "main.lua",
        "TS2322",
        "value",
    );
    fixture.file("main.lua", &format!("---@param value unknown\n---@return \"number\"\nlocal function fake(value)return 'number' end\ntype=fake\n{main}"));
    error_at(
        &fixture.parity(&entries, &declarations, StandardEnvironmentProfile::Lua52),
        "main.lua",
        "TS2322",
        "value",
    );
    fixture.file("main.lua", main);
    let corrected = fixture.parity(&entries, &declarations, StandardEnvironmentProfile::Lua52);
    clean(&corrected);
    assert_eq!(
        snapshot(&corrected, &fixture.0),
        snapshot(&first, &fixture.0)
    );
}

#[test]
fn numeric_expandos_use_reached_values_and_frozen_store_receivers_on_every_worker() {
    let fixture = Fixture::new();
    let frozen = "local value={}\nlocal original=value\n---@return number\nlocal function key()\nvalue={}\nreturn 1\nend\nvalue[key()]='word'\nlocal reached=original[2]\nlocal missing=value[2]\n";
    fixture.file("frozen.lua", frozen);
    fixture.file("sparse.lua", "---@param index number\n---@param other number\nlocal function fill(index,other)\nlocal values={}\nvalues[index]='word'\nlocal reached=values[other]\n---@type nil|string\nlocal checked=reached\nend\n");
    fixture.file("bounded.lua", "---@param index number\nlocal function fill(index)\n---@type {[number]:string}\nlocal values={}\nvalues[index]='word'\nend\n");
    fixture.file(
        "fourth.lua",
        "local values={}\nvalues[1]='literal'\nlocal exact=values[1]\n",
    );
    let entries = ["frozen.lua", "sparse.lua", "bounded.lua", "fourth.lua"];
    let wrong = fixture.minimal(&entries, &[]);
    error_at(&wrong, "frozen.lua", "TS2339", "2");
    let reached = binding(&wrong, "frozen.lua", "reached");
    assert!(
        reached.contains("nil") && reached.contains("string"),
        "{reached}"
    );
    assert!(
        !reached.contains("any") && !reached.contains("unknown"),
        "{reached}"
    );
    fixture.file(
        "frozen.lua",
        &frozen.replace("local missing=value[2]", "local missing=original[2]"),
    );
    let corrected = fixture.minimal(&entries, &[]);
    clean(&corrected);
    assert_eq!(binding(&corrected, "fourth.lua", "exact"), "string");
    fixture.file("bounded.lua", "---@param index number\nlocal function fill(index)\n---@type {[number]:string}\nlocal values={}\nvalues[index]=true\nend\n");
    error_at(
        &fixture.minimal(&entries, &[]),
        "bounded.lua",
        "TS2322",
        "true",
    );
    fixture.file("bounded.lua", "---@param index number\nlocal function fill(index)\n---@type {[number]:string}\nlocal values={}\nvalues[index]='word'\nend\n");
    clean(&fixture.minimal(&entries, &[]));
}
