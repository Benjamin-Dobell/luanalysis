use luanalysis::check;
use std::{fs, process::Command};

const SOURCE: &str = "---@class Record\n---@field value number\n\n---@param record Record\n---@param key number\nlocal function read(record,key) return record[key] end\n---@param record Record\nlocal function readName(record) return record['missing'] end\n---@param record Record\nlocal function write(record) record.missing=42 end\n";

#[test]
fn missing_table_keys_retain_the_original_read_or_write_receiver_type() {
    let report = check(SOURCE);
    assert_eq!(report.diagnostics.len(), 3, "{:#?}", report.diagnostics);
    for diagnostic in &report.diagnostics {
        assert_eq!(diagnostic.code, "TS2339");
        assert!(
            diagnostic.message.contains("type 'Record'"),
            "{diagnostic:?}"
        );
        assert!(
            !diagnostic.message.contains("this table type"),
            "{diagnostic:?}"
        );
        assert!(matches!(
            &SOURCE[diagnostic.span.start..diagnostic.span.end],
            "key" | "'missing'" | "missing"
        ));
    }
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Property number"))
    );
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("Property 'missing'"))
            .count(),
        2
    );
    let corrected = SOURCE
        .replace("record[key]", "record.value")
        .replace("record['missing']", "record.value")
        .replace("record.missing=42", "record.value=42");
    let report = check(&corrected);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn anonymous_missing_key_errors_render_the_receivers_written_fields() {
    let source = "---@param record {value:number}\nlocal function read(record) return record['missing'] end\n";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let message = &report.diagnostics[0].message;
    for expected in ["missing", "value", "number"] {
        assert!(message.contains(expected), "{message}");
    }
    assert!(!message.contains("this table type"), "{message}");
    let report = check(&source.replace("record['missing']", "record.value"));
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn text_and_json_keep_the_table_owner_and_accept_the_original_correction() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis missing owner diagnostics {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("main.lua");
    fs::write(&path, SOURCE).unwrap();
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
        assert!(text.contains("type 'Record'"), "{text}");
        assert!(!text.contains("this table type"), "{text}");
        assert!(text.contains("TS2339"), "{text}");
    }
    fs::write(
        &path,
        SOURCE
            .replace("record[key]", "record.value")
            .replace("record['missing']", "record.value")
            .replace("record.missing=42", "record.value=42"),
    )
    .unwrap();
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
