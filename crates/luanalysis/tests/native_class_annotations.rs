use luanalysis::{CheckReport, check};
fn binding<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .unwrap()
        .ty
}
fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

#[test]
fn standalone_class_annotations_keep_same_and_unrelated_empty_origins() {
    clean(
        "---@class A\n\n---@param value A\n---@return A\nlocal function same(value) return value end\n",
    );
    let source = "---@class A\n\n---@class B\n\n---@param value A\n---@return B\nlocal function other(value) return value end\n";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "value"
    );
}

#[test]
fn class_fields_and_generic_instance_members_still_use_primitive_type_checks() {
    let source = "---@class Box<T>\n---@field value T\n\n---@param box Box<number>\nlocal function read(box)\nlocal value=box.value\n---@type string\nlocal wrong=value\nend\n";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "value"
    );
    assert_eq!(binding(&report, "value"), "number");
}

#[test]
fn raw_record_returns_cannot_satisfy_a_nominal_class_annotation() {
    let source = "---@class A\n\n---@return A\nlocal function manufacture() return {} end\n";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "{}"
    );
}

#[test]
fn unsupported_class_definition_forms_remain_explicitly_gated() {
    clean("---@class C\nlocal C={}\n");
    for source in [
        "---@class C\n---@type C\nlocal C={}\n",
        "---@class C\nnamespace.C={}\n",
    ] {
        let report = check(source);
        let diagnostic = report
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "LUA2031")
            .unwrap_or_else(|| panic!("{source}\n{:#?}", report.diagnostics));
        assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], "C");
        assert!(diagnostic.message.contains("unsupported"));
    }
}

#[test]
fn cli_rejects_unrelated_class_return_and_accepts_same_origin_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis-class-origins-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("class-origin.ttslua");
    let wrong = "---@class A\n\n---@class B\n\n---@param value A\n---@return B\nlocal function convert(value)\n    return value\nend\n";
    fs::write(&path, wrong).unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--dump-types"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        rejected.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("class-origin.ttslua:8:12: error[TS2322]"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    fs::write(&path, wrong.replace("---@return B", "---@return A")).unwrap();
    let accepted = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--dump-types"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        accepted.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    fs::remove_dir_all(directory).unwrap();
}
