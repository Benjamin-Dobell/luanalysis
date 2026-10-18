use luanalysis::check;

#[test]
fn completion_hints_do_not_widen_parameter_types_or_resolve_suggested_expressions() {
    let source = "---@param level number|`MissingLogger.ERROR`|`MissingLogger.WARNING`\nlocal function set(level)\n---@type number\nlocal valid=level\nreturn valid\nend\nlocal result=set(42)";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert!(
        report
            .bindings
            .iter()
            .any(|binding| binding.name == "result" && binding.ty == "number"),
        "{:#?}",
        report.bindings
    );
    let wrong = source.replace("set(42)", "set(\"wrong\")");
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2345");
    assert_eq!(
        &wrong[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "\"wrong\""
    );
}

#[test]
fn aliases_generic_capture_and_record_contexts_retain_their_semantic_types() {
    let source = "---@alias Box<T> {value:T|`Suggested.VALUE`}\n---@generic T\n---@param value T\nlocal function wrap(value)\n---@type Box<T>\nlocal box={value=value}\nreturn function() return box.value end\nend\nlocal numeric=wrap(42)\nlocal text=wrap(\"word\")\nlocal first=numeric()\nlocal second=text()\nlocal again=numeric()";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    for (name, ty) in [
        ("first", "number"),
        ("second", "string"),
        ("again", "number"),
    ] {
        assert!(
            report
                .bindings
                .iter()
                .any(|binding| binding.name == name && binding.ty == ty),
            "{:#?}",
            report.bindings
        );
    }
}

#[test]
fn completion_metadata_in_overload_heads_does_not_change_candidate_applicability() {
    let source = "---@overload fun(value:number|`Suggested.NUMBER`):number\n---@overload fun(value:string|`Suggested.TEXT`):string\n---@param value any\n---@return any\nlocal function identity(value) return value end\nlocal first=identity(42)\nlocal second=identity(\"word\")";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    for (name, ty) in [("first", "number"), ("second", "string")] {
        assert!(
            report
                .bindings
                .iter()
                .any(|binding| binding.name == name && binding.ty == ty),
            "{:#?}",
            report.bindings
        );
    }
    let wrong = source.replace("identity(42)", "identity(true)");
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let plain = wrong
        .replace("|`Suggested.NUMBER`", "")
        .replace("|`Suggested.TEXT`", "");
    let plain_report = check(&plain);
    assert_eq!(
        plain_report.diagnostics.len(),
        1,
        "{:#?}",
        plain_report.diagnostics
    );
    assert_eq!(report.diagnostics[0].code, plain_report.diagnostics[0].code);
}

#[test]
fn completion_hints_preserve_direct_literal_overload_priority() {
    let source = "---@overload fun(value:string):number\n---@overload fun(value:\"special\"|`Suggested.SPECIAL`):string\n---@param value any\n---@return any\nlocal function choose(value) return value end\nlocal result=choose(\"special\")";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert!(
        report
            .bindings
            .iter()
            .any(|binding| binding.name == "result" && binding.ty == "string"),
        "{:#?}",
        report.bindings
    );
}

#[test]
fn completion_hints_do_not_remove_context_from_a_generic_function_annotation() {
    let source = "---@type (fun<T>(value:T):T)|`Suggested.identity`\nlocal identity=function(value)\n---@type T\nlocal copy=value\nreturn copy\nend\nlocal numeric=identity(42)\nlocal text=identity(\"word\")";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    for (name, ty) in [("numeric", "number"), ("text", "string")] {
        assert!(
            report
                .bindings
                .iter()
                .any(|binding| binding.name == name && binding.ty == ty),
            "{:#?}",
            report.bindings
        );
    }
}

#[test]
fn cli_metadata_keeps_original_unicode_crlf_error_and_corrected_input() {
    use std::{fs, path::PathBuf, process::Command};
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "luanalysis completion metadata {}",
        std::process::id()
    )));
    fs::create_dir_all(&fixture.0).unwrap();
    let path = fixture.0.join("hint workflow.lua");
    let source = "-- é😀\r\n---@param value number|`Suggested.OK`\r\nlocal function take(value) return value end\r\nlocal result=take(\"wrong\")\r\n";
    fs::write(&path, source).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .args(["check", "--dump-types"])
        .arg(&path)
        .output()
        .unwrap();
    let errors = String::from_utf8_lossy(&result.stderr);
    assert_eq!(result.status.code(), Some(1), "{errors}");
    assert!(
        errors.contains("hint workflow.lua:4:19: error[TS2345]"),
        "{errors}"
    );
    fs::write(&path, source.replace("\"wrong\"", "42")).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .args(["check", "--dump-types"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("result: number"));
}
