use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

// Original encoder structure: saved standard aliases, a true-valued visited
// indexer, captured output state, and a pairs body that rejects non-string keys.
const ENCODER: &str = r#"
local error=error
local pairs,type=pairs,type
---@type number, string[], table<table,true>
local i,builder,visited=1,{},{}
---@param text string
local function emit(text)
    builder[i]=text
end
---@param o table
local function encode(o)
    if visited[o] then error('loop detected') end
    visited[o]=true
    for k,v in pairs(o) do
        if type(k)~='string' then error('non-string key') end
        emit(k)
    end
end
"#;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "luanalysis encoder iterator frame {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }

    fn check(&self, source: &str) -> Output {
        let file = self.0.join("encoder.lua");
        fs::write(&file, source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .args(["check", "--lua-version", "5.2"])
            .arg(file)
            .output()
            .unwrap()
    }

    fn accepted(&self, source: &str) {
        let output = self.check(source);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn rejected(&self, source: &str, code: &str, actual: &str, expected: &str) {
        let output = self.check(source);
        let text = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(1), "{text}");
        assert_eq!(text.matches("error[").count(), 1, "{text}");
        assert!(text.contains(&format!("error[{code}]")), "{text}");
        assert!(text.contains(actual), "{text}");
        assert!(text.contains(expected), "{text}");
        for spurious in ["LUA2020", "LUA2041", "'never'", "'unknown'"] {
            assert!(!text.contains(spurious), "{text}");
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn encoder_pairs_body_keeps_saved_standard_aliases_and_strict_callback_arguments() {
    let fixture = Fixture::new();
    fixture.accepted(ENCODER);
    fixture.rejected(
        &ENCODER.replace("emit(k)", "emit(123)"),
        "TS2345",
        "'123'",
        "'string'",
    );
    fixture.accepted(ENCODER);
}

#[test]
fn encoder_pairs_body_retains_the_written_map_element_contract_after_its_key_guard() {
    let fixture = Fixture::new();
    let source = ENCODER
        .replace("---@param o table\n", "---@param o table<string,number>\n")
        .replace("emit(k)", "---@type number\n        local checked=v");
    fixture.accepted(&source);
    let wrong = source.replace("---@type number\n", "---@type string\n");
    fixture.rejected(&wrong, "TS2322", "'number'", "'string'");
    fixture.accepted(&source);
}
