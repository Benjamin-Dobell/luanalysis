use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis json {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, source: &str) {
        fs::write(self.0.join(name), source).unwrap();
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .arg("check")
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn json(output: &Output, exit: i32) -> String {
    assert_eq!(output.status.code(), Some(exit), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(
        text.starts_with("{\"formatVersion\":1,") && text.ends_with("}\n"),
        "{text}"
    );
    assert!(text.contains(&format!("\"exitCode\":{exit}")), "{text}");
    text
}
#[test]
fn original_utf8_byte_ranges_positions_paths_and_correction_are_machine_readable() {
    let fixture = Fixture::new();
    // Windows file names cannot contain the quote that tests path escaping.
    let name = if cfg!(windows) { "résumé.ttslua" } else { "résumé\".ttslua" };
    let source = "-- 前置\r\n---@type number\r\nlocal résultat='wrong'\r\n";
    fixture.file(name, source);
    let invalid = json(&fixture.run(&["--format", "json", "--dump-types", name]), 1);
    let start = source.find("'wrong'").unwrap();
    assert!(invalid.contains("\"code\":\"TS2322\""), "{invalid}");
    assert!(
        invalid.contains(&format!(
            "\"range\":{{\"start\":{start},\"end\":{}}}",
            start + "'wrong'".len()
        )),
        "{invalid}"
    );
    assert!(
        invalid.contains("\"position\":{\"line\":3,\"column\":16}"),
        "{invalid}"
    );
    assert!(
        invalid.contains(if cfg!(windows) { "résumé.ttslua" } else { "résumé\\\".ttslua" })
            && invalid.contains("\"rangeEncoding\":\"utf-8-bytes\""),
        "{invalid}"
    );
    fixture.file(name, &source.replace("'wrong'", "42"));
    let corrected = json(&fixture.run(&["--format", "json", "--dump-types", name]), 0);
    assert!(
        corrected.contains("\"success\":true") && corrected.contains("\"diagnostics\":[]"),
        "{corrected}"
    );
    assert!(
        corrected.contains("\"name\":\"résultat\",\"type\":\"number\""),
        "{corrected}"
    );
}
#[test]
fn json_host_errors_and_source_errors_are_retained_in_one_failing_report() {
    let fixture = Fixture::new();
    fixture.file("wrong.lua", "---@type number\nlocal value='wrong'");
    let output = json(
        &fixture.run(&["--format", "json", "missing.lua", "wrong.lua"]),
        2,
    );
    assert!(
        output.contains("\"code\":\"LUA0001\"") && output.contains("\"code\":\"TS2322\""),
        "{output}"
    );
    assert!(
        output.contains("\"range\":null,\"position\":null")
            && output.contains("\"filesChecked\":1"),
        "{output}"
    );
}
#[test]
fn json_module_and_role_failures_keep_their_existing_exit_policies() {
    let fixture = Fixture::new();
    fixture.file(
        "missing-module.lua",
        "local value=require('does.not.exist')",
    );
    let missing = json(&fixture.run(&["--format", "json", "missing-module.lua"]), 1);
    assert!(
        missing.contains("\"code\":\"TS2307\"")
            && missing.contains("\"range\":{\"start\":20,\"end\":36}"),
        "{missing}"
    );
    fixture.file("library.lua", "---@type number\ncount=0");
    let conflict = json(
        &fixture.run(&[
            "--format",
            "json",
            "--library",
            "library.lua",
            "library.lua",
        ]),
        2,
    );
    assert!(conflict.contains("\"code\":\"LUA0002\""), "{conflict}");
}
#[test]
fn json_usage_errors_are_formatted_even_before_the_format_flag() {
    let fixture = Fixture::new();
    for args in [
        vec!["--unknown", "--format", "json"],
        vec!["--format", "json", "--library"],
        vec!["--format", "json"],
    ] {
        let output = json(&fixture.run(&args), 2);
        assert!(
            output.contains("\"code\":\"LUA0000\"") && output.contains("\"filesChecked\":0"),
            "{output}"
        );
    }
}
#[test]
fn format_like_option_values_and_positional_paths_do_not_change_output_policy() {
    let fixture = Fixture::new();
    fixture.file("--format", "---@alias Count number");
    fixture.file("main.lua", "---@type Count\nlocal value=42");
    let output = fixture.run(&["--library", "--format", "main.lua"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "No type errors in 2 file(s).\n"
    );
    fixture.file("--format", "local value=42");
    let output = json(&fixture.run(&["--format", "json", "--", "--format"]), 0);
    assert!(output.contains("\"filesChecked\":1"), "{output}");
}
#[test]
fn malformed_lua_keeps_original_syntax_ranges_in_json() {
    let fixture = Fixture::new();
    fixture.file("syntax.lua", "local value=\n");
    let output = json(&fixture.run(&["--format", "json", "syntax.lua"]), 1);
    assert!(
        output.contains("\"code\":\"LUA1001\"")
            && output.contains("\"range\":{\"start\":13,\"end\":13}"),
        "{output}"
    );
}
