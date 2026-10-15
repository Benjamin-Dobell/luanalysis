use std::{fs, process::Command};

// Original ge_tts helper and yieldAll body/annotations, with unrelated methods
// and the license require omitted. The Wait declarations are original too.
const SOURCE: &str = include_str!("fixtures/unreachable/coroutine_yield_all.lua.fixture");
const WAIT: &str = include_str!("fixtures/unreachable/Wait.def.lua.fixture");

#[test]
fn original_coroutine_callback_checks_dead_stores_and_calls_against_written_types() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis coroutine unreachable {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("main.lua");
    let library = directory.join("Wait.def.lua");
    fs::write(&library, WAIT).unwrap();
    let check = |source: &str| {
        fs::write(&file, source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--lua-version", "5.2", "--library"])
            .arg(&library)
            .arg(&file)
            .output()
            .unwrap()
    };
    let original = check(SOURCE);
    assert!(
        original.status.success(),
        "{}",
        String::from_utf8_lossy(&original.stderr)
    );
    for (source, code) in [
        (
            SOURCE.replace("results[i] = userResult", "results[i] = false"),
            "TS2322",
        ),
        (
            SOURCE.replace(
                "resumeWithErrorHandling(co, onError)",
                "resumeWithErrorHandling(42, onError)",
            ),
            "TS2345",
        ),
    ] {
        assert_ne!(source, SOURCE);
        let output = check(&source);
        assert_eq!(output.status.code(), Some(1));
        let text = String::from_utf8(output.stderr).unwrap();
        assert!(text.contains(&format!("error[{code}]")), "{text}");
        assert!(!text.contains("'never'"), "{text}");
        assert!(!text.contains("LUA2041"), "{text}");
        let corrected = check(SOURCE);
        assert!(
            corrected.status.success(),
            "{}",
            String::from_utf8_lossy(&corrected.stderr)
        );
    }
    fs::remove_dir_all(&directory).unwrap();
}
