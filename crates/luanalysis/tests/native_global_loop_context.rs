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
            "luanalysis global loop context {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("api.def.lua"), DECLARATIONS).unwrap();
        Self(path)
    }
    fn check(&self, source: &str, version: &str) -> Output {
        fs::write(self.0.join("main.lua"), source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args([
                "check", "--format", "json", "--lua-version", version,
                "--library", "api.def.lua", "main.lua",
            ])
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn report(output: Output, exit: i32) -> String {
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(exit), "{stdout}\n{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    for code in ["LUA2035", "LUA2041"] {
        assert!(!stdout.contains(code), "{stdout}");
    }
    stdout
}
fn clean(output: Output) {
    assert!(report(output, 0).contains("\"diagnostics\":[]"));
}
fn anchored(report: &str, source: &str, statement: &str, expression: &str) {
    let start = source.find(statement).unwrap() + statement.rfind(expression).unwrap();
    let range = format!(
        "\"range\":{{\"start\":{start},\"end\":{}}}",
        start + expression.len()
    );
    assert!(
        report.split("},{\"path\":").any(|diagnostic| {
            diagnostic.contains("\"code\":\"TS2322\"") && diagnostic.contains(&range)
        }),
        "{report}"
    );
}

const DECLARATIONS: &str = r#"---@type table<string, function>
_LOADED = {}
---@class DeferredWork
Wait = {}
---@overload fun(func: fun(): void, frames: number): number
---@overload fun(func: fun(): void): number
---@param func fun(): void
---@param frames number | nil
---@return number
function Wait.frames(func, frames) end
"#;

// Reduced from ge_tts.Debug.createGlobals: an original callback, generic
// iteration, environment-table write and a later call. The string declaration
// is first demanded while that consumer's loop feedback is still incomplete.
const ORIGINAL: &str = r#"local globalTable = (--[[---@type table<string, any>]] _G)
Wait.frames(function()
    for packageName, package in pairs(_LOADED) do
        local identifier = packageName:gsub('a', 'b')
        globalTable[identifier] = package
    end
    print('done')
end, 1)
"#;

#[test]
fn written_standard_declarations_remain_usable_during_callback_loop_feedback() {
    let fixture = Fixture::new();
    for version in ["5.2", "5.4"] {
        clean(fixture.check(ORIGINAL, version));
    }
}

#[test]
fn original_callback_body_errors_are_anchored_and_accept_correction() {
    let fixture = Fixture::new();
    let invalid = ORIGINAL.replace(
        "        globalTable[identifier] = package",
        "        ---@type number\n        local wrong = identifier\n        globalTable[identifier] = package",
    );
    for version in ["5.2", "5.4"] {
        let rejected = report(fixture.check(&invalid, version), 1);
        anchored(&rejected, &invalid, "local wrong = identifier", "identifier");
        clean(fixture.check(&invalid.replace("---@type number", "---@type string"), version));
        clean(fixture.check(ORIGINAL, version));
    }
}

#[test]
fn a_written_global_contract_does_not_restore_an_explicitly_replaced_builtin() {
    let fixture = Fixture::new();
    let source = format!(
        "---@param value unknown\n---@return 'number'\nlocal function ordinaryKind(value) return 'number' end\ntype = ordinaryKind\n---@param value number | string\nlocal function probe(value)\n{}\nend\nprobe('text')\n",
        ORIGINAL.replace(
            "    print('done')",
            "    print('done')\n    if type(value) == 'number' then\n        ---@type number\n        local wrong = value\n    end",
        )
    );
    for version in ["5.2", "5.4"] {
        let rejected = report(fixture.check(&source, version), 1);
        anchored(&rejected, &source, "local wrong = value", "value");
        let corrected = source.replace("---@type number\n        local wrong", "---@type number | string\n        local wrong");
        clean(fixture.check(&corrected, version));
    }
}
