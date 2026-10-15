use luanalysis::{CheckReport, check};
use std::{fs, process::Command};

fn source(visibility: &str) -> String {
    format!("---@class Example\n---@field {visibility}value number\nlocal Example={{}}\n")
}

fn readable(report: &CheckReport, source: &str) {
    assert!(!report.diagnostics.is_empty());
    for diagnostic in &report.diagnostics {
        assert_eq!(diagnostic.code, "LUA2031", "{diagnostic:?}");
        assert!(
            diagnostic
                .message
                .contains("Private/protected member semantics"),
            "{diagnostic:?}"
        );
        for internal in [
            "NativeTypeMemberKey",
            "NativeTypeDeclarationKey",
            "snapshot:",
            "index:",
            "UnsupportedVisibility(",
        ] {
            assert!(!diagnostic.message.contains(internal), "{diagnostic:?}");
        }
    }
    let class_start = source.find("Example").unwrap();
    let initializer_start = source.rfind("{}").unwrap();
    for (start, end) in [
        (class_start, class_start + "Example".len()),
        (initializer_start, initializer_start + 2),
    ] {
        assert!(
            report
                .diagnostics
                .iter()
                .any(|error| error.span.start == start && error.span.end == end),
            "{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn unsupported_private_and_protected_class_errors_render_text_at_original_spans() {
    for visibility in ["private ", "protected "] {
        let original = source(visibility);
        readable(&check(&original), &original);
        let public = source("");
        assert!(check(&public).diagnostics.is_empty());
    }
}

#[test]
fn cli_class_definition_errors_hide_declaration_ids_and_public_correction_passes() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis class definition diagnostics {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("main.lua");
    let run = |source: &str, version: &str, exit: i32| {
        fs::write(&path, source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--format", "json", "--lua-version", version])
            .arg(&path)
            .output()
            .unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(exit), "{text}\n{stderr}");
        assert!(stderr.is_empty(), "{stderr}");
        text
    };
    for version in ["5.2", "5.4"] {
        for visibility in ["private ", "protected "] {
            let original = source(visibility);
            let rejected = run(&original, version, 1);
            assert!(rejected.contains("\"code\":\"LUA2031\""), "{rejected}");
            assert!(
                rejected.contains("Private/protected member semantics"),
                "{rejected}"
            );
            for internal in [
                "NativeTypeMemberKey",
                "snapshot:",
                "index:",
                "UnsupportedVisibility(",
            ] {
                assert!(!rejected.contains(internal), "{rejected}");
            }
            for (start, count) in [
                (original.find("Example").unwrap(), "Example".len()),
                (original.rfind("{}").unwrap(), 2),
            ] {
                assert!(
                    rejected.contains(&format!(
                        "\"range\":{{\"start\":{start},\"end\":{}}}",
                        start + count
                    )),
                    "{rejected}"
                );
            }
            let corrected = run(&source(""), version, 0);
            assert!(corrected.contains("\"diagnostics\":[]"), "{corrected}");
        }
    }
    fs::remove_dir_all(directory).unwrap();
}
