use luanalysis::{check, location};
use std::{fs, process::Command};

const WRONG: &str = "---@class Zone\nlocal Zone={}\n---@type {IGNORE:0,ACCEPT:1,REJECT:2,FORCE_IGNORE:3,FORCE_REJECT:4}\nZone.FilterResult={IGNORE=6,ACCEPT=1,REJECT=2,FORCE_IGNORE=3,FORCE_REJECT=4}\n";

fn check_message(message: &str) {
    let expected = message
        .split("is not assignable to type '")
        .nth(1)
        .expect(message);
    for (field, value) in [
        ("IGNORE", 0),
        ("ACCEPT", 1),
        ("REJECT", 2),
        ("FORCE_IGNORE", 3),
        ("FORCE_REJECT", 4),
    ] {
        assert!(expected.contains(field), "{expected}");
        assert!(expected.contains(&format!(": {value}")), "{expected}");
    }
    assert!(!expected.starts_with("record'"), "{message}");
    assert!(!message.contains("contextual type"), "{message}");
}

#[test]
fn an_anonymous_written_member_contract_renders_its_actual_singleton_fields() {
    let report = check(WRONG);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.code, "TS2322");
    assert_eq!(
        &WRONG[diagnostic.span.start..diagnostic.span.end],
        "{IGNORE=6,ACCEPT=1,REJECT=2,FORCE_IGNORE=3,FORCE_REJECT=4}"
    );
    check_message(&diagnostic.message);
    let corrected = check(&WRONG.replace("IGNORE=6", "IGNORE=0"));
    assert!(
        corrected.diagnostics.is_empty(),
        "{:#?}",
        corrected.diagnostics
    );
}

#[test]
fn genuine_named_classes_shapes_and_aliases_called_record_keep_their_identity() {
    for declaration in [
        "---@class record\n---@field value number\nlocal record={}\n",
        "---@shape record\n---@field value number\nlocal record={value=1}\n",
        "---@alias record {value:number}\n",
    ] {
        let source = format!("{declaration}\n---@type record\nlocal rejected=42\n");
        let report = check(&source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{source}\n{:#?}",
            report.diagnostics
        );
        assert_eq!(report.diagnostics[0].code, "TS2322");
        assert!(
            report.diagnostics[0].message.ends_with("type 'record'."),
            "{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn anonymous_contracts_render_nested_records_and_written_function_types() {
    let source = "---@class Zone\nlocal Zone={}\n---@type {callback:fun(value:number):string,next:{flag:true}|nil}\nZone.callbacks=42\n";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let message = &report.diagnostics[0].message;
    for expected in [
        "callback", "value", "number", "string", "next", "flag", "true", "nil",
    ] {
        assert!(message.contains(expected), "{message}");
    }
    assert!(!message.contains("'record'"), "{message}");
}

#[test]
fn text_and_json_publish_the_same_anonymous_fields_original_span_and_correction() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis anonymous record diagnostics {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("main.lua");
    fs::write(&path, WRONG).unwrap();
    let report = check(WRONG);
    let diagnostic = &report.diagnostics[0];
    let (line, column) = location(WRONG, diagnostic.span);
    for format in ["text", "json"] {
        let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--no-stdlib", "--format", format])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let text = String::from_utf8(if format == "json" {
            output.stdout
        } else {
            output.stderr
        })
        .unwrap();
        for field in ["IGNORE", "ACCEPT", "REJECT", "FORCE_IGNORE", "FORCE_REJECT"] {
            assert!(text.contains(field), "{text}");
        }
        assert!(!text.contains("type 'record'"), "{text}");
        if format == "json" {
            assert!(
                text.contains(&format!(
                    "\"range\":{{\"start\":{},\"end\":{}}}",
                    diagnostic.span.start, diagnostic.span.end
                )),
                "{text}"
            );
        } else {
            assert!(
                text.contains(&format!("main.lua:{line}:{column}: error[TS2322]")),
                "{text}"
            );
            assert!(text.contains(&diagnostic.message), "{text}");
        }
    }
    fs::write(&path, WRONG.replace("IGNORE=6", "IGNORE=0")).unwrap();
    let corrected = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--no-stdlib"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        corrected.status.success(),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    fs::remove_dir_all(&directory).unwrap();
}
