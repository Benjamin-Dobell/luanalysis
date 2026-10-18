use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis deferred local members {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn check(&self, source: &str) -> Output {
        fs::write(self.0.join("main.lua"), source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args(["check", "--no-stdlib", "--format", "json", "main.lua"])
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn report(output: Output, exit: i32) -> String {
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(exit), "{stdout}\n{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    assert!(stdout.starts_with("{\"formatVersion\":1,"), "{stdout}");
    assert!(!stdout.contains("\"code\":\"LUA2041\""), "{stdout}");
    stdout
}
fn clean(fixture: &Fixture, source: &str) {
    let output = report(fixture.check(source), 0);
    assert!(output.contains("\"diagnostics\":[]"), "{output}");
}
fn rejects_on_line(fixture: &Fixture, source: &str, code: &str, marker: &str) {
    let line = source[..source.find(marker).expect("original diagnostic marker")]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let output = report(fixture.check(source), 1);
    assert!(
        output.split("},{\"path\":").any(|diagnostic| {
            diagnostic.contains(&format!("\"code\":\"{code}\""))
                && diagnostic.contains(&format!("\"position\":{{\"line\":{line},"))
        }),
        "{output}"
    );
}

// Zone's original constructor declares the instance before selecting its
// parent-constructor branch, then installs methods on the assigned value.
const SOURCE: &str = r#"---@class Zone
local Zone={}
---@param choose boolean
---@return Zone
local function make(choose)
    ---@type Zone
    local self
    if choose then
        self=--[[---@type Zone]] {}
    else
        self=--[[---@type Zone]] {}
    end
    ---@param value number
    ---@return number
    function self.read(value) return value end
    return self
end
---@param zone Zone
local function use(zone)
    ---@type number
    local checked=zone.read(42)
end
use(make(true))
"#;

#[test]
fn branch_assigned_typed_local_installs_its_written_class_members() {
    let fixture = Fixture::new();
    clean(&fixture, SOURCE);
    // Existing initialized locals retain exactly the same member contract.
    clean(
        &fixture,
        &SOURCE.replace("local self\n", "local self=--[[---@type Zone]] {}\n"),
    );
}

#[test]
fn deferred_members_check_their_written_bodies_and_calls() {
    let fixture = Fixture::new();
    let wrong_body = SOURCE.replace("return value end", "return 'wrong' end");
    rejects_on_line(&fixture, &wrong_body, "TS2322", "function self.read");
    let wrong_call = SOURCE.replace("zone.read(42)", "zone.read('wrong')");
    rejects_on_line(&fixture, &wrong_call, "TS2345", "local checked=");
    clean(&fixture, SOURCE);
}

#[test]
fn an_explicit_nil_assignment_still_checks_the_written_local_contract() {
    let fixture = Fixture::new();
    // Frozen 3a7192f already accepted `---@type Zone; local value` followed
    // by `---@type Zone; local checked=value`, without any member definitions.
    // Definite assignment for that implicit nil is a pre-existing gap. This
    // control checks an explicit incompatible write, not an uninitialized read.
    let nil_write = SOURCE.replace("    local self\n", "    local self\n    self=nil\n");
    rejects_on_line(&fixture, &nil_write, "TS2322", "self=nil");
    clean(&fixture, SOURCE);
}

#[test]
fn deferred_members_do_not_leak_from_their_written_owner() {
    let fixture = Fixture::new();
    let source = SOURCE
        .replace("Zone", "PrivateOwner")
        .replace(
            "---@param zone PrivateOwner",
            "---@class OtherOwner\nlocal OtherOwner={}\n---@param zone OtherOwner",
        )
        .replace("use(make(true))", "");
    rejects_on_line(&fixture, &source, "TS2339", "local checked=");
    clean(
        &fixture,
        &source.replace("---@param zone OtherOwner", "---@param zone PrivateOwner"),
    );
}

#[test]
fn inferred_values_and_parameters_do_not_create_written_local_owners() {
    let fixture = Fixture::new();
    let untyped = SOURCE.replace("    ---@type Zone\n    local self", "    local self");
    rejects_on_line(&fixture, &untyped, "TS2339", "local checked=");
    let parameter = r#"---@class Zone
local Zone={}
---@param self Zone
local function install(self)
    ---@param value number
    ---@return number
    function self.read(value) return value end
end
---@param zone Zone
local function use(zone)
    local checked=zone.read(42)
end
"#;
    rejects_on_line(&fixture, parameter, "TS2339", "local checked=");
    clean(&fixture, SOURCE);
}

#[test]
fn a_local_written_as_an_alias_of_a_union_has_no_owner_for_its_stores() {
    let fixture = Fixture::new();
    let source = r#"---@shape A
---@field n number

---@shape B
---@field n number

---@alias AB A | B

---@param state any
local function f(state)
    local s = --[[---@type AB]] state
    s.n = 1
end
"#;
    clean(&fixture, source);
    rejects_on_line(&fixture, &source.replace("s.n = 1", "s.n = 'x'"), "TS2322", "s.n = 'x'");
}
