use luanalysis::{CheckReport, check, location};

fn accepted(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}

fn binding_type<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .unwrap()
        .ty
}

#[test]
fn documented_anonymous_argument_uses_its_original_parameter_and_return_types() {
    let source = "---@param callback fun(value:number):number\n---@return number\nlocal function invoke(callback) return callback(42) end\nlocal result=invoke(\n---@param résumé number\n---@return number\nfunction(résumé) return résumé end)";
    let report = accepted(source);
    assert_eq!(binding_type(&report, "result"), "number");
    let parameter = source.find("function(résumé)").unwrap() + "function(".len();
    assert_eq!(report.type_at(parameter), Some("number"));
    let returned = source.find("return résumé").unwrap() + "return ".len();
    assert_eq!(report.type_at(returned), Some("number"));
}

#[test]
fn field_and_initializer_documentation_share_the_functions_lexical_generic_scope() {
    let source = "local callbacks={\n---@generic T\n---@param value T\nidentity=\n---@return T\n(function(value) return value end)}\nlocal numeric=callbacks.identity(42)\nlocal text=callbacks.identity(\"word\")";
    let report = accepted(source);
    assert_eq!(binding_type(&report, "numeric"), "number");
    assert_eq!(binding_type(&report, "text"), "string");
    assert_eq!(
        report.type_at(source.find("function(value)").unwrap() + 9),
        Some("T")
    );
    assert_eq!(
        report.type_at(source.find("return value").unwrap() + 7),
        Some("T")
    );
}

#[test]
fn documented_table_callback_checks_bad_body_and_accepts_original_source_correction() {
    let source = "local callbacks={\n---@param value number\n---@return number\nread=function(value) return \"wrong\" end}";
    let report = check(source);
    let error = report
        .diagnostics
        .iter()
        .find(|error| error.code == "TS2322")
        .unwrap_or_else(|| panic!("{:#?}", report.diagnostics));
    assert_eq!(&source[error.span.start..error.span.end], "\"wrong\"");
    assert!(
        report
            .diagnostics
            .iter()
            .all(|error| error.code != "TS7006")
    );
    accepted(&source.replace("return \"wrong\"", "return value"));
}

#[test]
fn documented_field_does_not_supply_annotations_to_its_sibling() {
    let source = "local callbacks={\n---@param value number\nread=function(value) return value end,\nplain=function(untyped) return untyped end}";
    let report = luanalysis::check_with_options(source, luanalysis::CheckOptions { no_implicit_any: true, ..Default::default() });
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert_eq!(error.code, "TS7006");
    assert_eq!(&source[error.span.start..error.span.end], "untyped");
    assert_eq!(
        report.type_at(source.find("function(value)").unwrap() + 9),
        Some("number")
    );
}

#[test]
fn cli_documented_field_rejects_wrong_call_at_original_unicode_crlf_position() {
    use std::{fs, process::Command};
    let directory = std::env::temp_dir().join(format!(
        "luanalysis callback documentation {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("callbacks.lua");
    let source = "local callbacks={\r\n---@param value number\r\nread=function(value) return value end\r\n}\r\nlocal é=callbacks.read(\"wrong\")";
    fs::write(&path, source).unwrap();
    let report = check(source);
    let error = report
        .diagnostics
        .iter()
        .find(|error| error.code == "TS2345")
        .unwrap_or_else(|| panic!("{:#?}", report.diagnostics));
    assert_eq!(&source[error.span.start..error.span.end], "\"wrong\"");
    let (line, column) = location(source, error.span);
    assert_eq!(line, 5);
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .arg("check")
            .arg("--dump-types")
            .arg(&path)
            .output()
            .unwrap()
    };
    let rejected = run();
    assert_eq!(rejected.status.code(), Some(1));
    let errors = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        errors.contains(&format!("callbacks.lua:{line}:{column}: error[TS2345]")),
        "{errors}"
    );
    assert!(!errors.contains("TS7006"), "{errors}");
    fs::write(&path, source.replace("\"wrong\"", "42")).unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    assert!(String::from_utf8_lossy(&corrected.stdout).contains("é: number"));
    fs::remove_dir_all(directory).unwrap();
}
