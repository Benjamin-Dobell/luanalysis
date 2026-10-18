use luanalysis::{OverloadMode, check, check_with_overload_mode};

// The real ge_tts implementation with its declaration dependencies reduced to
// their original field contracts. Keep all four public overloads and the body.
const SOURCE: &str = include_str!("fixtures/object_utils_transform.lua");

#[test]
fn transform_state_checks_all_public_forms_and_the_private_body() {
    let report = check(SOURCE);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);

    let wrong = SOURCE.replace(
        "state.posX = (--[[---@type tts__CharVectorShape]] position).x or (--[[---@type tts__NumVectorShape]] position)[1]",
        "state.posX = 'wrong'",
    );
    let report = check(&wrong);
    assert!(
        report.diagnostics.iter().any(|error| error.code == "TS2322"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn optional_private_parameters_do_not_expose_an_extra_zero_argument_overload() {
    let source = format!("{SOURCE}\nlocal hidden=ObjectUtils.transformState()\n");
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2554");
    let legacy = check_with_overload_mode(&source, OverloadMode::LegacyExposeImplementation);
    assert!(legacy.diagnostics.is_empty(), "{:#?}", legacy.diagnostics);
}

#[test]
fn actual_cli_rejects_a_bad_transform_and_accepts_the_correction() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis-transform-contracts-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("transform.lua");
    let invoke = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .args(["check", "--no-stdlib", "--format", "text"])
            .arg(&path)
            .output()
            .unwrap()
    };
    std::fs::write(
        &path,
        format!("{SOURCE}\nObjectUtils.transformState({{position='wrong'}})\n"),
    )
    .unwrap();
    let rejected = invoke();
    assert_eq!(rejected.status.code(), Some(1));
    let errors = String::from_utf8(rejected.stderr).unwrap();
    assert!(errors.contains("TS2345"), "{errors}");
    assert!(!errors.contains("TS2394"), "{errors}");
    std::fs::write(&path, SOURCE).unwrap();
    let corrected = invoke();
    assert!(
        corrected.status.success(),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    std::fs::remove_dir_all(directory).unwrap();
}
