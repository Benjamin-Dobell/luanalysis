use std::fs;
use std::process::Command;

#[test]
fn cli_checks_original_lua_then_accepts_a_corrected_file() {
    let directory = std::env::temp_dir().join(format!("luanalysis-cli-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("input.lua");
    fs::write(&file, "---@type number\nlocal answer = \"wrong\"\n").unwrap();
    let invalid = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .args(["check", "--dump-types"])
        .arg(&file)
        .output()
        .unwrap();
    let error = String::from_utf8(invalid.stderr).unwrap();
    assert!(error.contains("input.lua:2:16: error[TS2322]"), "{error}");
    assert_eq!(invalid.status.code(), Some(1));
    fs::write(&file, "---@type number\nlocal answer = 42\n").unwrap();
    let corrected = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .arg("check")
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(corrected.stdout).unwrap(),
        "No type errors in 1 file(s).\n"
    );
    assert_eq!(corrected.status.code(), Some(0));
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn cli_discovers_nested_lua_and_ttslua_sources_and_deduplicates_explicit_paths() {
    let directory =
        std::env::temp_dir().join(format!("luanalysis directory {}", std::process::id()));
    let nested = directory.join("nested folder");
    fs::create_dir_all(&nested).unwrap();
    let lua = directory.join("input.lua");
    let ttslua = nested.join("input.ttslua");
    fs::write(&lua, "---@type number\nlocal answer = 42\n").unwrap();
    fs::write(&ttslua, "---@type string\nlocal label = 42\n").unwrap();
    fs::write(directory.join("ignored.txt"), "invalid lua").unwrap();
    let invalid = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .arg("check")
        .arg(&directory)
        .arg(&lua)
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(1));
    let error = String::from_utf8(invalid.stderr).unwrap();
    assert!(
        error.contains("input.ttslua:2:15: error[TS2322]"),
        "{error}"
    );
    fs::write(&ttslua, "---@type string\nlocal label = \"corrected\"\n").unwrap();
    let corrected = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .arg("check")
        .arg(&lua)
        .arg(&directory)
        .output()
        .unwrap();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    assert_eq!(
        String::from_utf8(corrected.stdout).unwrap(),
        "No type errors in 2 file(s).\n"
    );
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn cli_keeps_io_failures_distinct_from_type_errors() {
    let file = std::env::temp_dir().join(format!("luanalysis missing {}.lua", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .arg("check")
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("error[LUA0001]")
    );
}
