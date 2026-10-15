use std::{fs, path::PathBuf, process::{Command, Output}, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("luanalysis explicit effects {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn check(&self, source: &str, declarations: Option<&str>) -> Output {
        fs::write(self.0.join("main.lua"), source).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis"));
        command.current_dir(&self.0).args(["check", "--lua-version", "5.4", "--format", "json"]);
        if let Some(declarations) = declarations {
            fs::write(self.0.join("api.def.lua"), declarations).unwrap();
            command.args(["--library", "api.def.lua"]);
        }
        command.arg("main.lua").output().unwrap()
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
fn report(output: Output, exit: i32) -> String {
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(exit), "{stdout}\n{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    assert!(stdout.starts_with("{\"formatVersion\":1,"), "{stdout}");
    for pending in ["LUA2040", "LUA2041"] { assert!(!stdout.contains(pending), "{stdout}"); }
    stdout
}
fn clean(output: Output) { let report = report(output, 0); assert!(report.contains("\"diagnostics\":[]"), "{report}"); }
fn rejected(output: Output, code: &str) -> String {
    let report = report(output, 1);
    assert!(report.contains(&format!("\"code\":\"{code}\"")), "{report}");
    report
}

#[test]
fn recursive_copy_settles_written_effects_and_publishes_real_body_errors() {
    let fixture = Fixture::new();
    // The ordinary explicit map has the same any key/value domain as Lua's
    // broad table top; it also matches the standard pairs written contract.
    let source = r#"---@generic T
---@param tab T
---@return T
local function copy(tab)
    ---@type table<any,any>
    local copied={}
    for k,v in pairs(--[[---@type table<any,any>]] tab) do
        copied[type(k)=='table' and copy(--[[---@type table<any,any>]] k) or k]=type(v)=='table' and copy(--[[---@type table<any,any>]] v) or v
    end
    return --[[---@type T]] copied
end
return copy
"#;
    clean(fixture.check(source, None));
    let wrong = source.replace("    return --[[---@type T]] copied", "    ---@type number\n    local checked='wrong'\n    return --[[---@type T]] copied");
    let report = rejected(fixture.check(&wrong, None), "TS2322");
    let start = wrong.find("'wrong'").unwrap();
    assert!(report.contains(&format!("\"range\":{{\"start\":{start},\"end\":{}}}", start + "'wrong'".len())), "{report}");
    clean(fixture.check(source, None));
}

#[test]
fn written_never_ordinary_and_imported_contracts_preserve_reachability() {
    let fixture = Fixture::new();
    let source = r#"---@return never
local function stop() error('stop') end
---@return nil
local function resume() return nil end
---@type fun():nil
local callee=stop
local saved=stop
callee=resume
---@return number
local function through_saved() saved() end
---@return number
local function through_mutated() callee() end
"#;
    rejected(fixture.check(source, None), "TS2355");
    let corrected = source.replace("through_mutated() callee() end", "through_mutated() callee() return 42 end");
    clean(fixture.check(&corrected, None));

    let consumer = "---@return number\nlocal function use() halt() end\n";
    clean(fixture.check(consumer, Some("---@return never\nfunction halt() end\n")));
    rejected(fixture.check(consumer, Some("---@return void\nfunction halt() end\n")), "TS2355");
}

#[test]
fn explicit_effect_views_cannot_attest_a_replaced_intrinsic_but_saved_values_keep_identity() {
    let fixture = Fixture::new();
    let source = r#"---@generic T
---@param value T
---@vararg unknown
---@return T
local function custom(value,...) return value end
local saved=assert
assert=custom
---@param value number|nil
local function original(value)
    saved(value)
    ---@type number
    local retained=value
end
---@param value number|nil
local function replacement(value)
    assert(value)
    ---@type number
    local checked=value
end
original(42)
replacement(42)
"#;
    let output = rejected(fixture.check(source, None), "TS2322");
    let start = source.find("local checked=value").unwrap() + "local checked=".len();
    assert!(output.contains(&format!("\"range\":{{\"start\":{start},\"end\":{}}}", start + "value".len())), "{output}");
    let corrected = source.replace("---@type number\n    local checked=value", "---@type number|nil\n    local checked=value");
    clean(fixture.check(&corrected, None));
}
