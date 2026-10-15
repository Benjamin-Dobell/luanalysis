use std::{fs, path::PathBuf, process::Command, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("luanalysis declaration returns {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn check(&self, source: &str) -> serde_json::Value {
        fs::write(self.0.join("main.lua"), source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args(["check", "--lua-version", "5.4", "--format", "json", "main.lua"])
            .output().unwrap();
        assert!(matches!(output.status.code(), Some(0 | 1)), "{output:?}");
        assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        for diagnostic in report["diagnostics"].as_array().unwrap() {
            assert!(!matches!(diagnostic["code"].as_str(), Some("LUA2040" | "LUA2041")), "{report}");
        }
        report
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
fn clean(report: &serde_json::Value) { assert!(report["diagnostics"].as_array().unwrap().is_empty(), "{report}"); }
fn rejects_at(report: &serde_json::Value, source: &str, code: &str, token: &str) {
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1, "{report}");
    let start = source.rfind(token).unwrap();
    assert_eq!(diagnostics[0]["code"], code, "{report}");
    assert_eq!(diagnostics[0]["range"]["start"], start, "{report}");
    assert_eq!(diagnostics[0]["range"]["end"], start + token.len(), "{report}");
}

#[test]
fn unannotated_no_value_declarations_have_zero_results_even_when_they_throw() {
    let fixture = Fixture::new();
    let source = r#"local function declarationStop() error('stop') end
local function bareStop() error('stop') return end
local function infiniteStop() while true do end end
local methods={}
function methods.stop() error('stop') end
---@type fun():void
local declaration=declarationStop
---@type nil
local result=declarationStop()
---@type nil
local bare=bareStop()
---@type nil
local infinite=infiniteStop()
---@type nil
local method=methods.stop()
"#;
    clean(&fixture.check(source));
    let wrong = source.replace("---@type nil\nlocal result=", "---@type string\nlocal result=");
    rejects_at(&fixture.check(&wrong), &wrong, "TS2322", "declarationStop()");
    clean(&fixture.check(source));
}

#[test]
fn expressions_explicit_never_and_valued_never_returns_keep_their_never_result() {
    let fixture = Fixture::new();
    let source = r#"local expressionStop=function() error('stop') end
local methods={stop=function() error('stop') end}
---@return never
local function explicitStop() error('stop') end
local function valuedNever() return explicitStop() end
---@type fun():never
local expression=expressionStop
---@type fun():never
local method=methods.stop
---@type fun():never
local explicit=explicitStop
---@type fun():never
local valued=valuedNever
"#;
    clean(&fixture.check(source));
}

#[test]
fn zero_result_inference_does_not_skip_a_real_unused_declaration_body_error() {
    let fixture = Fixture::new();
    let wrong = r#"local function unused()
    ---@type number
    local checked='wrong unused body'
    error('stop')
end
"#;
    rejects_at(&fixture.check(wrong), wrong, "TS2322", "'wrong unused body'");
    clean(&fixture.check(&wrong.replace("'wrong unused body'", "42")));
}

const SAVED: &str = r#"---@class Parent

---@class Child : Parent

---@return Parent
local function parent()
    local self=--[[---@type Parent]] {}
    ---@param value number
    function self.method(value) end
    return self
end

---@return Child
local function child()
    local self=--[[---@type Child]] parent()
    local saved=self.method
    ---@param value number
    function self.method(value)
        saved(value)
    end
    return self
end
local instance=child()
---@type nil
local result=instance.method(1)
"#;

#[test]
fn no_value_saved_override_keeps_the_asserted_public_contract_and_checks_its_body() {
    let fixture = Fixture::new();
    clean(&fixture.check(SAVED));
    let wrong_call = SAVED.replace("instance.method(1)", "instance.method('wrong child argument')");
    rejects_at(&fixture.check(&wrong_call), &wrong_call, "TS2345", "'wrong child argument'");
    let wrong_body = SAVED.replace("        saved(value)", "        saved(value)\n        ---@type number\n        local checked='wrong override body'");
    rejects_at(&fixture.check(&wrong_body), &wrong_body, "TS2322", "'wrong override body'");
    let wrong_result = SAVED.replace("---@type nil\nlocal result=", "---@type string\nlocal result=");
    rejects_at(&fixture.check(&wrong_result), &wrong_result, "TS2322", "instance.method(1)");
    clean(&fixture.check(SAVED));
}
