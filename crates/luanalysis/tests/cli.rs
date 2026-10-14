use std::fs;
use std::process::Command;

#[test]
fn cli_checks_original_lua_then_accepts_a_corrected_file() {
    let directory = std::env::temp_dir().join(format!("luanalysis-cli-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("input.lua");
    fs::write(&file, "---@type number\nlocal answer = \"wrong\"\n").unwrap();
    let invalid = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--dump-types"])
        .arg(&file)
        .output()
        .unwrap();
    let error = String::from_utf8(invalid.stderr).unwrap();
    assert!(error.contains("input.lua:2:16: error[TS2322]"), "{error}");
    assert_eq!(invalid.status.code(), Some(1));
    fs::write(&file, "---@type number\nlocal answer = 42\n").unwrap();
    let corrected = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
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
