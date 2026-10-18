use std::{fs, process::Command};

#[test]
fn cli_rejects_a_constrained_generic_call_then_infers_the_corrected_call() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis-native-generic-call-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("generic-call.lua");
    let source = "---@generic T : number\n---@param value T\n---@return T\nlocal function identity(value) return value end\nlocal inferred = identity(\"wrong\")\n---@type number\nlocal accepted = inferred\n";
    fs::write(&file, source).unwrap();

    let rejected = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .args(["check", "--dump-types"])
        .arg(&file)
        .output()
        .unwrap();
    let errors = String::from_utf8(rejected.stderr).unwrap();
    assert_eq!(rejected.status.code(), Some(1), "{errors}");
    assert!(
        errors.starts_with(&format!("{}:5:27: error[TS2345]:", file.display())),
        "{errors}"
    );
    assert_eq!(errors.lines().count(), 1, "{errors}");

    fs::write(&file, source.replace("\"wrong\"", "42")).unwrap();
    let accepted = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .args(["check", "--dump-types"])
        .arg(&file)
        .output()
        .unwrap();
    let errors = String::from_utf8(accepted.stderr).unwrap();
    let types = String::from_utf8(accepted.stdout).unwrap();
    assert_eq!(accepted.status.code(), Some(0), "{errors}");
    assert!(errors.is_empty(), "{errors}");
    assert!(
        types
            .lines()
            .any(|line| line == format!("{}:5:7: inferred: number", file.display())),
        "{types}"
    );
    assert!(
        types
            .lines()
            .any(|line| line == format!("{}:7:7: accepted: number", file.display())),
        "{types}"
    );
    assert!(types.ends_with("No type errors in 1 file(s).\n"), "{types}");
    fs::remove_dir_all(&directory).unwrap();
}
