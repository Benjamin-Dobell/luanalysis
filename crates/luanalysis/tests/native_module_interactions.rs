//! Interactions between original module identities and later source protocols.
use luanalysis::project_check::{ProjectCheckReport, check_project, static_require_requests};
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis module interactions {} {}",
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
    fn check(&self, source: &str) -> ProjectCheckReport {
        let entry = self.file("main.lua", source);
        let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
        check_project(Project::load_plan(
            &resolver,
            ProjectLoadPlan::new([entry]),
            static_require_requests,
        ))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn clean(report: &ProjectCheckReport) {
    for file in &report.files {
        assert!(
            file.report.diagnostics.is_empty(),
            "{}: {:#?}",
            report.source(file.file).unwrap().path.display(),
            file.report.diagnostics
        );
    }
    assert!(!report.has_errors());
}
fn ty<'a>(report: &'a ProjectCheckReport, name: &str) -> &'a str {
    &report
        .file(report.project.entries[0])
        .unwrap()
        .report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .unwrap()
        .ty
}

const ECHO: &str = "---@overload fun(value:number):number\n---@overload fun(value:string):string\n---@param value number|string|boolean\n---@return number|string|boolean\nlocal function echo(value)return value end\nreturn echo\n";

#[test]
fn exported_overload_heads_keep_their_public_view_and_cli_source_correction() {
    let fixture = Fixture::new();
    fixture.file("echo.lua", ECHO);
    let source = "local echo=require('echo')\nlocal numeric=echo(1)\nlocal textual=echo('word')\n";
    let report = fixture.check(source);
    clean(&report);
    assert_eq!(ty(&report, "numeric"), "number");
    assert_eq!(ty(&report, "textual"), "string");
    let public = ty(&report, "echo");
    assert!(
        public.contains("number") && public.contains("string"),
        "{public}"
    );
    assert!(
        !public.contains("boolean"),
        "private implementation leaked: {public}"
    );
    let bad = format!("{source}local rejected=echo(true)\n");
    let main = fixture.file("main.lua", &bad);
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .current_dir(&fixture.0)
            .args(["check", "--dump-types", "main.lua"])
            .output()
            .unwrap()
    };
    let failed = run();
    assert_eq!(
        failed.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("main.lua:4:21: error[TS2769]: No overload matches this call."),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    fs::write(main, bad.replace("echo(true)", "echo(42)")).unwrap();
    let fixed = run();
    assert_eq!(
        fixed.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&fixed.stderr)
    );
}

#[test]
fn exported_generic_closure_heads_retain_each_capture_mapper() {
    let fixture = Fixture::new();
    fixture.file("factory.lua","---@generic T\n---@param value T\nlocal function make(value)\n---@overload fun():T\n---@return T\nlocal function get()return value end\nreturn get\nend\nreturn make\n");
    let report=fixture.check("local make=require('factory')\nlocal numeric=make(42)\nlocal textual=make('word')\nlocal one=numeric()\nlocal two=textual()\nlocal again=numeric()\n---@type number\nlocal number_check=one\n---@type string\nlocal string_check=two\n");
    clean(&report);
    assert_eq!(ty(&report, "one"), "number");
    assert_eq!(ty(&report, "two"), "string");
    assert_eq!(ty(&report, "again"), "number");
}

#[test]
fn module_table_anonymous_documented_functions_keep_overload_identity() {
    let fixture = Fixture::new();
    fixture.file("callbacks.lua","return {\n---@overload fun(value:number):number\n---@overload fun(value:string):string\n---@param value number|string|boolean\n---@return number|string|boolean\necho=function(value)return value end\n}\n");
    let source = "local callbacks=require('callbacks')\nlocal numeric=callbacks.echo(1)\nlocal textual=callbacks.echo('word')\n";
    let report = fixture.check(source);
    clean(&report);
    assert_eq!(ty(&report, "numeric"), "number");
    assert_eq!(ty(&report, "textual"), "string");
    let bad = format!("{source}local rejected=callbacks.echo(true)\n");
    let invalid = fixture.check(&bad);
    let main = &invalid.file(invalid.project.entries[0]).unwrap().report;
    assert_eq!(main.diagnostics.len(), 1, "{:#?}", main.diagnostics);
    assert_eq!(main.diagnostics[0].code, "TS2769");
    assert_eq!(
        &bad[main.diagnostics[0].span.start..main.diagnostics[0].span.end],
        "true"
    );
}

#[test]
fn nested_type_assertion_and_exclusion_on_require_preserve_original_pack_boundaries() {
    let fixture = Fixture::new();
    fixture.file("count.lua", "return 42");
    let source = "local value=--[[---@type number]] (--[[---@not nil]] require('count'))\nlocal scalar,absent=(--[[---@type number,string|nil]] require('count'))\nlocal first,loader=--[[---@type number,string|nil]] require('count')\n---@type number\nlocal checked=value\n---@type string|nil\nlocal checked_loader=loader\n";
    let report = fixture.check(source);
    clean(&report);
    assert_eq!(ty(&report, "value"), "number");
    assert_eq!(ty(&report, "scalar"), "number");
    assert_eq!(ty(&report, "absent"), "nil");
    assert_eq!(ty(&report, "first"), "number");
    let converted =
        "local text=--[[---@type string]] require('count')\n---@type string\nlocal checked=text\n";
    let report = fixture.check(converted);
    clean(&report);
    assert_eq!(ty(&report, "text"), "string");
    let wrong = converted.replace(
        "---@type string\nlocal checked",
        "---@type number\nlocal checked",
    );
    let invalid = fixture.check(&wrong);
    let diagnostics = &invalid
        .file(invalid.project.entries[0])
        .unwrap()
        .report
        .diagnostics;
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "TS2322");
    assert_eq!(
        &wrong[diagnostics[0].span.start..diagnostics[0].span.end],
        "text"
    );
}

#[test]
fn callable_table_exports_preserve_runtime_category_across_original_module_queries() {
    let fixture = Fixture::new();
    fixture.file(
        "callable.lua",
        "---@shape Callable\n---@overload fun(value:number):number\nlocal callable=--[[---@type Callable]] {}\nreturn callable\n",
    );
    fixture.file("raw.lua", "return function() return 42 end\n");
    let source = "local callable=require('callable')\nlocal raw=require('raw')\n---@param value table\nlocal function take_table(value) end\n---@param value function\nlocal function take_function(value) end\ntake_table(callable)\ntake_function(raw)\nlocal result=callable(42)\n";
    let report = fixture.check(source);
    clean(&report);
    assert_eq!(ty(&report, "result"), "number");
    for call in ["take_function(callable)", "take_table(raw)"] {
        let rejected = fixture.check(&format!("{source}{call}\n"));
        let diagnostics = &rejected
            .file(rejected.project.entries[0])
            .unwrap()
            .report
            .diagnostics;
        assert!(
            diagnostics.iter().any(|error| error.code == "TS2345"),
            "{call}: {diagnostics:#?}"
        );
    }
    let main = fixture.file("main.lua", &format!("{source}take_function(callable)\n"));
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .current_dir(&fixture.0)
            .args(["check", "--dump-types", "main.lua"])
            .output()
            .unwrap()
    };
    let rejected = run();
    assert_eq!(rejected.status.code(), Some(1));
    let errors = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        errors.contains("main.lua:10:") && errors.contains("error[TS2345]"),
        "{errors}"
    );
    fs::write(main, format!("{source}take_table(callable)\n")).unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
}

#[test]
fn imported_raw_and_headerless_typed_signatures_preserve_rest_arguments_and_packs() {
    let fixture = Fixture::new();
    fixture.file(
        "raw.lua",
        "---@param callback function\nlocal function apply(callback) return callback(42,'word') end\nreturn apply\n",
    );
    fixture.file(
        "typed.lua",
        "---@param callback fun:number\nlocal function apply(callback) return callback(42,'word') end\nreturn apply\n",
    );
    fixture.file(
        "generic.lua",
        "---@param callback fun<T:number>\nlocal function apply(callback) return callback(42,'word') end\nreturn apply\n",
    );
    let source = "local raw=require('raw')\nlocal typed=require('typed')\nlocal generic=require('generic')\nlocal first,second=raw(function() return 42 end)\nlocal numeric=typed(function() return 42 end)\nlocal generic_first,generic_second=generic(function() return 42 end)\n";
    let report = fixture.check(source);
    clean(&report);
    assert_eq!(ty(&report, "first"), "any");
    assert_eq!(ty(&report, "second"), "any");
    assert_eq!(ty(&report, "numeric"), "number");
    assert_eq!(ty(&report, "generic_first"), "any");
    assert_eq!(ty(&report, "generic_second"), "any");
    let invalid = fixture.check(&format!("{source}local rejected=typed(42)\n"));
    assert!(
        invalid
            .files
            .iter()
            .flat_map(|file| &file.report.diagnostics)
            .any(|error| error.code == "TS2345"),
        "{:#?}",
        invalid.files
    );
}

#[test]
fn importer_callback_runtime_predicates_do_not_reparent_foreign_chunk_or_capture_contexts() {
    let fixture = Fixture::new();
    fixture.file(
        "getter.lua",
        "local value={count=42}\nreturn function() return value.count end\n",
    );
    let source = "---@param value table|function|number\n---@param callback fun(value:table|function|number):number\n---@return number\nlocal function visit(value,callback) return callback(value) end\nlocal result=visit({field=42},function(value)\nlocal getter=require('getter')\nif type(value)=='table' then\n---@type table\nlocal object=value\nreturn getter()\nelseif type(value)=='function' then\n---@type function\nlocal raw_callback=value\nraw_callback()\nreturn getter()\nelse\n---@type number\nlocal scalar=value\nreturn getter()\nend\nend)\n";
    let report = fixture.check(source);
    clean(&report);
    assert_eq!(ty(&report, "result"), "number");
    assert_eq!(ty(&report, "object"), "table");
    assert_eq!(ty(&report, "raw_callback"), "function");
    assert_eq!(ty(&report, "scalar"), "number");
    let invalid = fixture.check(&source.replace("local scalar=value", "local scalar='wrong'"));
    assert!(
        invalid
            .files
            .iter()
            .flat_map(|file| &file.report.diagnostics)
            .any(|error| error.code == "TS2322"),
        "{:#?}",
        invalid.files
    );
}
