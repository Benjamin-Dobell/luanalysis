use luanalysis::check;

#[test]
fn documentation_links_do_not_create_type_dependencies_or_change_call_contracts() {
    let source = "---@see MissingDocumentationTarget#method\n---@param value number\n---@return number\nlocal function echo(value) return value end\nlocal result=echo(1)";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let wrong = source.replace("echo(1)", "echo('wrong')");
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2345");
    let span = report.diagnostics[0].span;
    assert_eq!(&wrong[span.start..span.end], "'wrong'");
}

#[test]
fn cli_documentation_links_preserve_original_utf8_crlf_errors_and_corrections() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis see links {}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("links.ttslua");
    let source = "-- résumé\r\n---@see Unknown#method\r\n---@param value number\r\nlocal function checked(value) end\r\nchecked('wrong')\r\n";
    fs::write(&path, source).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .arg("check")
            .arg(&path)
            .output()
            .unwrap()
    };
    let invalid = run();
    let stderr = String::from_utf8_lossy(&invalid.stderr);
    assert_eq!(invalid.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("links.ttslua:5:9: error[TS2345]"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("LUA1001") && !stderr.contains("TS2304"),
        "{stderr}"
    );
    fs::write(&path, source.replace("'wrong'", "42")).unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    assert!(String::from_utf8_lossy(&corrected.stdout).contains("No type errors in 1 file(s)."));
    fs::remove_dir_all(directory).unwrap();
}
