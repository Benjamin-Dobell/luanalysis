//! A field's visibility (`private`, `protected`, LuaLS's `package`) is for editors, as in
//! Luanalysis for IntelliJ, where it chooses completions: whatever it is, the field is a member
//! of its class. Class definition errors describe types, never internal identities, at the
//! original spans.
use luanalysis::check;
use std::{fs, process::Command};

fn source(visibility: &str) -> String {
    format!("---@class Example\n---@field {visibility}value number\nlocal Example={{value=1}}\n---@return number\nfunction Example:read() return self.value end\n")
}

#[test]
fn fields_of_every_visibility_are_members() {
    for visibility in ["", "public ", "protected ", "private ", "package "] {
        let report = check(&source(visibility));
        assert!(report.diagnostics.is_empty(), "{visibility}: {:#?}", report.diagnostics);
    }
}

#[test]
fn cli_class_definition_errors_hide_declaration_ids_at_original_spans() {
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
    let original = "---@class Example\n---@field value number\nlocal Example=1\n";
    for version in ["5.2", "5.4"] {
        let rejected = run(original, version, 1);
        assert!(rejected.contains("Class definition requires a table, found '1'."), "{rejected}");
        for internal in ["NativeTypeMemberKey", "NativeTypeDeclarationKey", "snapshot:", "index:"] {
            assert!(!rejected.contains(internal), "{rejected}");
        }
        let start = original.rfind('1').unwrap();
        assert!(rejected.contains(&format!("\"range\":{{\"start\":{start},\"end\":{}}}", start + 1)), "{rejected}");
        let corrected = run(&source("private "), version, 0);
        assert!(corrected.contains("\"diagnostics\":[]"), "{corrected}");
    }
    fs::remove_dir_all(directory).unwrap();
}
