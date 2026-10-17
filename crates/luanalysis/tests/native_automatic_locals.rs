//! Original Lua acceptance for automatic local declarations and closure crossings.
use luanalysis::{CheckOptions, CheckReport, check, check_with_options, location};

fn check_strict(source: &str) -> CheckReport {
    let report = check_with_options(source, CheckOptions { no_implicit_any: true, ..CheckOptions::default() });
    // Without --no-implicit-any the undetermined local is `any`, as in TypeScript.
    assert!(check(source).diagnostics.is_empty(), "{source}");
    report
}
use std::{fs, process::Command};

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

fn read_type<'a>(report: &'a CheckReport, source: &str, anchor: &str) -> &'a str {
    report
        .type_at(source.find(anchor).unwrap() + anchor.rfind("value").unwrap())
        .unwrap()
}

#[test]
fn nil_initial_reads_and_successive_writes_keep_their_original_flow_types() {
    for declaration in ["local value", "local value=nil", "local value=((nil))"] {
        let source = format!(
            "{declaration}\n---@type nil\nlocal before=value\nvalue=42\n---@type number\nlocal numeric=value\nvalue=\"later\"\n---@type string\nlocal text=value\nvalue=nil\n---@type nil\nlocal empty=value"
        );
        let report = clean(&source);
        assert_eq!(
            report
                .bindings
                .iter()
                .find(|binding| binding.name == "value")
                .unwrap()
                .ty,
            "nil"
        );
        assert_eq!(read_type(&report, &source, "before=value"), "nil");
        assert_eq!(read_type(&report, &source, "numeric=value"), "number");
        assert_eq!(read_type(&report, &source, "text=value"), "string");
        assert_eq!(read_type(&report, &source, "empty=value"), "nil");
    }
}

#[test]
fn an_automatic_local_is_not_any_before_its_first_write() {
    let source = "local value\n---@type number\nlocal rejected=value\nvalue=42";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "value"
    );
    assert_eq!(location(source, report.diagnostics[0].span), (3, 16));
    clean(&source.replace("---@type number", "---@type nil"));
}

#[test]
fn successive_writes_replace_the_previous_value_instead_of_accumulating_it() {
    let source = "local value\nvalue=42\nvalue=\"later\"\n---@type number\nlocal rejected=value";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(read_type(&report, source, "rejected=value"), "string");
    clean(&source.replace("---@type number", "---@type string"));
}

#[test]
fn branch_joins_use_backend_inference_and_preserve_unwritten_nil_paths() {
    for (branch, expected, required) in [
        (
            "if flag then value=42 else value=\"text\" end",
            "number|string",
            vec!["number", "string"],
        ),
        (
            "if flag then value=42 end",
            "number|nil",
            vec!["number", "nil"],
        ),
    ] {
        let source = format!(
            "---@param flag boolean\nlocal function choose(flag)\nlocal value\n{branch}\nreturn value\nend\n---@type {expected}\nlocal result=choose(true)"
        );
        let report = clean(&source);
        let ty = read_type(&report, &source, "return value");
        for member in required {
            assert!(ty.contains(member), "{ty}");
        }
    }
}

#[test]
fn self_updates_use_the_rhs_input_and_shared_loop_worker() {
    let source = "---@param flag boolean\nlocal function update(flag)\nlocal value\nwhile flag do\nvalue=(value or 0)+1\nflag=false\nend\nreturn value\nend\n---@type number|nil\nlocal result=update(true)";
    let report = clean(source);
    let ty = read_type(&report, source, "return value");
    assert!(ty.contains("number") && ty.contains("nil"), "{ty}");
}

#[test]
fn simultaneous_writes_keep_their_frozen_rhs_values() {
    clean(
        "local first,second\nfirst,second=42,\"text\"\nfirst,second=second,first\n---@type string\nlocal text=first\n---@type number\nlocal numeric=second",
    );
}

#[test]
fn written_nil_annotations_and_nil_returning_calls_keep_fixed_constraints() {
    for declaration in [
        "---@type nil\nlocal value=nil",
        "---@return nil\nlocal function nothing() return nil end\nlocal value=nothing()",
    ] {
        let source = format!("{declaration}\nvalue=42");
        let report = check(&source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{source}\n{:#?}",
            report.diagnostics
        );
        assert_eq!(report.diagnostics[0].code, "TS2322");
        assert_eq!(
            &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
            "42"
        );
        clean(&source.replace("value=42", "value=nil"));
    }
}

#[test]
fn nonhoisted_closures_after_the_last_write_preserve_the_value_type() {
    for creation in [
        "local function read() return value end",
        "local read=function() return value end",
    ] {
        let source =
            format!("local value\nvalue=42\n{creation}\n---@type number\nlocal result=read()");
        clean(&source);
    }
}

#[test]
fn later_and_nested_writes_report_unresolved_auto_at_original_declaration_and_read() {
    for tail in ["value=42", "local function mutate() value=42 end"] {
        let source = format!(
            "local value\nlocal function read() return value end\n{tail}\nlocal result=read()"
        );
        let report = check_strict(&source);
        assert_eq!(
            report.diagnostics.len(),
            2,
            "{source}\n{:#?}",
            report.diagnostics
        );
        assert_eq!(report.diagnostics[0].code, "TS7034");
        assert_eq!(location(&source, report.diagnostics[0].span), (1, 7));
        assert_eq!(report.diagnostics[1].code, "TS7005");
        assert_eq!(
            &source[report.diagnostics[1].span.start..report.diagnostics[1].span.end],
            "value"
        );
        assert_eq!(location(&source, report.diagnostics[1].span), (2, 30));
    }
}

#[test]
fn a_closure_inside_the_writing_loop_does_not_capture_one_iteration_type() {
    let source = "---@param flag boolean\nlocal function run(flag)\nlocal value\nwhile flag do\nvalue=42\nlocal function read() return value end\nlocal result=read()\nflag=false\nend\nend\nrun(true)";
    let report = check_strict(source);
    assert_eq!(report.diagnostics.len(), 2, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS7034");
    assert_eq!(location(source, report.diagnostics[0].span), (3, 7));
    assert_eq!(report.diagnostics[1].code, "TS7005");
    assert_eq!(location(source, report.diagnostics[1].span), (6, 30));
}

#[test]
fn a_local_declared_inside_the_loop_has_its_own_capture_boundary() {
    clean(
        "---@param flag boolean\nlocal function run(flag)\nwhile flag do\nlocal value\nvalue=42\nlocal function read() return value end\n---@type number\nlocal result=read()\nflag=false\nend\nend\nrun(true)",
    );
}

#[test]
fn forwarded_captures_cross_each_original_creation_edge() {
    clean(
        "local value\nvalue=42\nlocal function outer()\nreturn function() return value end\nend\nlocal read=outer()\n---@type number\nlocal result=read()",
    );
}

#[test]
fn a_capture_introduced_by_an_automatic_write_keeps_its_generic_mapper() {
    let source = "---@generic T\n---@param value T\nlocal function make(value)\nlocal stored\nstored=value\nreturn function() return stored end\nend\nlocal number_read=make(42)\nlocal string_read=make(\"text\")\nlocal again_read=make(7)\n---@type number\nlocal first=number_read()\n---@type string\nlocal text=string_read()\n---@type number\nlocal again=again_read()";
    clean(source);
}

#[test]
fn an_automatic_contextual_callback_capture_keeps_distinct_instantiations() {
    let source = "---@generic T, R\n---@param value T\n---@param project fun(value: T): R\n---@return R\nlocal function transform(value,project) return project(value) end\n---@generic T\n---@param input T\nlocal function make(input)\nreturn transform(input,function(value)\nlocal stored\nstored=value\nreturn function() return stored end\nend)\nend\nlocal number_read=make(42)\nlocal string_read=make(\"text\")\nlocal again_read=make(7)\n---@type number\nlocal first=number_read()\n---@type string\nlocal text=string_read()\n---@type number\nlocal again=again_read()";
    let report = clean(source);
    for (name, expected) in [("first", "number"), ("text", "string"), ("again", "number")] {
        assert_eq!(
            report
                .bindings
                .iter()
                .find(|binding| binding.name == name)
                .unwrap()
                .ty,
            expected
        );
    }
    let rejected = format!("{source}\n---@type string\nlocal rejected=again_read()");
    let report = check(&rejected);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &rejected[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "again_read()"
    );
}

#[test]
fn a_closure_can_write_captured_auto_before_reading_its_concrete_value() {
    let source = "local value\nlocal function read()\nvalue=42\nreturn value\nend\n---@type number\nlocal result=read()";
    let report = clean(source);
    assert_eq!(read_type(&report, source, "return value"), "number");
    let rejected = format!("{source}\n---@type string\nlocal rejected=read()");
    let report = check(&rejected);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &rejected[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "read()"
    );
}

#[test]
fn cli_reports_original_automatic_flow_error_and_accepts_the_correction() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis native automatic {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("automatic.ttslua");
    let source = "local value\nvalue=42\nvalue=\"later\"\n---@type number\nlocal result=value\n";
    fs::write(&path, source).unwrap();
    let invalid = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--dump-types"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(1));
    let errors = String::from_utf8(invalid.stderr).unwrap();
    assert!(errors.contains(":5:14: error[TS2322]"), "{errors}");
    fs::write(&path, source.replace("---@type number", "---@type string")).unwrap();
    let corrected = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--dump-types"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        corrected.status.success(),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    assert!(
        String::from_utf8(corrected.stdout)
            .unwrap()
            .contains("No type errors in 1 file(s).")
    );
    fs::remove_dir_all(directory).unwrap();
}
