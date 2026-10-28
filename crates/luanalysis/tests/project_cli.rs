use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis project cli {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        path
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"));
        command.current_dir(&self.0).arg("check");
        command
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn passes(output: &Output, count: usize) {
    assert_eq!(output.status.code(), Some(0), "{}", text(output));
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains(&format!("No type errors in {count} file(s).")),
        "{}",
        text(output)
    );
    assert!(output.stderr.is_empty(), "{}", text(output));
}

#[test]
fn separate_positional_files_share_original_named_declarations_and_native_inference() {
    let fixture = Fixture::new();
    let types = fixture.file("types.lua", "---@alias Box<T> {value:T}\n");
    let main=fixture.file("main.ttslua","---@generic T\n---@param value Box<T>\n---@return T\nlocal function read(value) return value.value end\nlocal value=read({value=42})\n---@type number\nlocal valid=value\n");
    let output = fixture
        .command()
        .arg("--dump-types")
        .arg(main)
        .arg(types)
        .output()
        .unwrap();
    passes(&output, 2);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("value: number"),
        "{}",
        text(&output)
    );
}

#[test]
fn explicit_library_alias_contract_reports_original_crlf_error_then_accepts_correction() {
    let fixture = Fixture::new();
    fixture.file("types/count.def.lua", "---@alias Count number\n");
    let wrong = "-- café\r\n---@type Count\r\nlocal value='wrong'\r\n";
    let main = fixture.file("src/main.lua", wrong);
    let invalid = fixture
        .command()
        .args([
            "--library",
            "types/count.def.lua",
            "--dump-types",
            "src/main.lua",
        ])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(1), "{}", text(&invalid));
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains("src/main.lua:3:13: error[TS2322]"),
        "{}",
        text(&invalid)
    );
    assert!(
        !String::from_utf8_lossy(&invalid.stderr)
            .contains(&fixture.0.to_string_lossy().to_string()),
        "relative source spelling was lost: {}",
        text(&invalid)
    );
    fs::write(main, wrong.replace("'wrong'", "42")).unwrap();
    let corrected = fixture
        .command()
        .args([
            "--library",
            "types/count.def.lua",
            "--dump-types",
            "src/main.lua",
        ])
        .output()
        .unwrap();
    passes(&corrected, 2);
    assert!(
        String::from_utf8_lossy(&corrected.stdout).contains("src/main.lua:3:7: value: number"),
        "{}",
        text(&corrected)
    );
}

#[test]
fn nested_declaration_directory_membership_is_separate_from_runtime_discovery() {
    let fixture = Fixture::new();
    fixture.file("src/types/count.def.lua", "---@alias Count number\n");
    fixture.file("src/main.ttslua", "---@type Count\nlocal count=42\n");
    let output = fixture
        .command()
        .args(["--library", "src/types", "src"])
        .output()
        .unwrap();
    passes(&output, 2);
}

#[test]
fn repeated_library_paths_and_canonical_directory_discovery_do_not_duplicate_types() {
    let fixture = Fixture::new();
    fixture.file("types/count.def.lua", "---@alias Count number\n");
    fixture.file("main.lua", "---@type Count\nlocal count=42\n");
    let output = fixture
        .command()
        .args([
            "--library",
            "types/count.def.lua",
            "--library",
            "types",
            "main.lua",
        ])
        .output()
        .unwrap();
    passes(&output, 2);
}

#[test]
fn an_explicit_implementation_file_conflicting_with_library_role_reports_configuration_error() {
    let fixture = Fixture::new();
    // A declaration file is one however it is named, so only an ordinary file conflicts.
    fixture.file("count.def.lua", "---@alias Count number\n");
    passes(&fixture.command().args(["--library", "count.def.lua", "count.def.lua"]).output().unwrap(), 1);
    fixture.file("count.lua", "---@alias Count number\n");
    let output = fixture
        .command()
        .args(["--library", "count.lua", "count.lua"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("error[LUA0002]"), "{error}");
    assert!(
        error.contains("loaded as declaration, requested implementation"),
        "{error}"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("No type errors"));
}

#[test]
fn declaration_stub_values_report_capability_gap_without_executing_empty_bodies() {
    let fixture = Fixture::new();
    fixture.file(
        "types/external.def.lua",
        "---@return number\nlocal function external() end\n",
    );
    fixture.file("main.lua", "local value=42");
    let output = fixture
        .command()
        .args(["--library", "types", "main.lua"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("error[LUA2034]"), "{error}");
    assert!(
        !error.contains("TS2355"),
        "declaration stub was treated as runtime code: {error}"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("No type errors"));
}

#[test]
fn missing_library_input_and_missing_library_option_value_are_usage_or_io_errors() {
    let fixture = Fixture::new();
    fixture.file("main.lua", "local value=42");
    let missing = fixture
        .command()
        .args(["--library", "absent.def.lua", "main.lua"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2), "{}", text(&missing));
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("absent.def.lua: error[LUA0001]"),
        "{}",
        text(&missing)
    );
    let option = fixture.command().arg("--library").output().unwrap();
    assert_eq!(option.status.code(), Some(2), "{}", text(&option));
    assert!(
        String::from_utf8_lossy(&option.stderr).contains("--library requires"),
        "{}",
        text(&option)
    );
}

#[test]
fn declaration_only_invocation_validates_unused_named_contracts() {
    let fixture = Fixture::new();
    let contract = fixture.file("count.def.lua", "---@alias Count number\n");
    let clean = fixture
        .command()
        .args(["--library", "count.def.lua"])
        .output()
        .unwrap();
    passes(&clean, 1);
    fs::write(contract, "---@alias Count Missing\n").unwrap();
    let invalid = fixture
        .command()
        .args(["--library", "count.def.lua"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(1), "{}", text(&invalid));
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains("count.def.lua:1:17: error[TS2304]"),
        "{}",
        text(&invalid)
    );
}

#[test]
fn positional_separator_preserves_a_file_named_like_the_library_option() {
    let fixture = Fixture::new();
    fixture.file("--library", "local value=42");
    let output = fixture
        .command()
        .args(["--dump-types", "--", "--library"])
        .output()
        .unwrap();
    passes(&output, 1);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("--library:1:7: value: number"),
        "{}",
        text(&output)
    );
}

#[test]
fn unresolved_runtime_require_remains_an_error_while_project_names_are_available() {
    let fixture = Fixture::new();
    fixture.file("types/count.def.lua", "---@alias Count number\n");
    fixture.file(
        "main.lua",
        "local imported=require('absent')\n---@type Count\nlocal count=42\n",
    );
    let output = fixture
        .command()
        .args(["--library", "types", "main.lua"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("error[TS2307]"),
        "{}",
        text(&output)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("No type errors"));
}
