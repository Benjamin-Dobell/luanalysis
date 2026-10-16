use std::{fs, process::Command};

const CORRELATED: &str = include_str!("fixtures/json-dispatch/correlated.lua.fixture");
const INDEPENDENT: &str = include_str!("fixtures/json-dispatch/independent.lua.fixture");
const BRANCHES: &str = include_str!("fixtures/json-dispatch/branches.lua.fixture");
const WRONG_NUMBER: &str = include_str!("fixtures/json-dispatch/wrong-number.lua.fixture");

#[test]
fn indexed_function_unions_accept_gradual_any_and_reject_unknown_and_wrong_typed_arguments() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis json dispatch union {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("main.lua");
    let run = |source: &str, version: &str, expected: i32| {
        fs::write(&path, source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--format", "json", "--lua-version", version])
            .arg(&path)
            .output()
            .unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(expected), "{text}\n{stderr}");
        assert!(stderr.is_empty(), "{stderr}");
        assert!(
            !text.contains("LUA2041") && !text.contains("TS2589"),
            "{text}"
        );
        if expected == 0 {
            assert!(text.contains("\"diagnostics\":[]"), "{text}");
        }
        text
    };
    for version in ["5.2", "5.4"] {
        run(BRANCHES, version, 0);
        run(CORRELATED, version, 0);
        run(INDEPENDENT, version, 0);
        let unknown_correlated = CORRELATED.replace("---@param v any", "---@param v unknown");
        let unknown_independent = INDEPENDENT.replace("---@param v any", "---@param v unknown");
        for (source, argument, expected_type) in [
            (unknown_correlated.as_str(), "v", "never"),
            (unknown_independent.as_str(), "v", "never"),
            (WRONG_NUMBER, "'wrong'", "number"),
        ] {
            let rejected = run(source, version, 1);
            assert_eq!(
                rejected.matches("\"severity\":\"error\"").count(),
                1,
                "{rejected}"
            );
            assert!(rejected.contains("\"code\":\"TS2345\""), "{rejected}");
            assert!(
                rejected.contains(&format!("parameter of type '{expected_type}'")),
                "{rejected}"
            );
            let call_argument = format!("({argument})");
            let start = source.rfind(&call_argument).unwrap() + 1;
            assert!(
                rejected.contains(&format!(
                    "\"range\":{{\"start\":{start},\"end\":{}}}",
                    start + argument.len()
                )),
                "{rejected}"
            );
            run(BRANCHES, version, 0);
        }
    }
    fs::remove_dir_all(directory).unwrap();
}
