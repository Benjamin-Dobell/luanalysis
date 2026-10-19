use std::{fs, path::PathBuf, process::{Command, Output}, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("luanalysis recursive metatable self {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn check(&self, source: &str) -> Output {
        fs::write(self.0.join("main.lua"), source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&self.0)
            .args(["check", "--lua-version", "5.4", "--dump-types", "main.lua"]).output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}
fn text(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

const SOURCE: &str = r#"---@class Recursive
---@overload fun():self
local Recursive={}
ATTACH(Recursive,{__call=function(_)
    ---@type self
    local value
    function value.identity() return value end
    return value
end})
---@type Recursive
local value=Recursive()
---@type Recursive
local again=value.identity()
"#;

#[test]
fn contextual_self_members_do_not_demand_their_own_target_initialization() {
    let fixture = Fixture::new();
    let source = SOURCE.replace("ATTACH", "setmetatable");
    let output = fixture.check(&source);
    assert!(output.status.success(), "{}", text(&output));
    for value in ["Recursive()", "value.identity()"] {
        let wrong = format!("{source}---@type number\nlocal wrong={value}\n");
        let output = fixture.check(&wrong);
        let report = text(&output);
        assert_eq!(report.matches("error[").count(), 1, "{report}");
        assert!(report.contains("error[TS2322]"), "{report}");
        assert!(!report.contains("LUA204"), "{report}");
    }
    assert!(fixture.check(&source).status.success());
}

#[test]
fn a_matching_ordinary_function_cannot_activate_recursive_self_members() {
    let fixture = Fixture::new();
    let source = format!("---@generic T\n---@param value T\n---@param meta table\n---@return T\nlocal function ordinary(value,meta) return value end\n{}", SOURCE.replace("ATTACH", "ordinary"));
    let report = text(&fixture.check(&source));
    assert!(report.contains("Property 'identity'"), "{report}");
    assert!(fixture.check(&SOURCE.replace("ATTACH", "setmetatable")).status.success());
}

#[test]
fn using_the_attached_owner_does_not_accept_replacing_it_with_another_class() {
    let fixture = Fixture::new();
    let source = SOURCE.replace("ATTACH", "setmetatable")
        .replace("setmetatable(Recursive", "---@class Other\nlocal Other={}\nRecursive=Other\nsetmetatable(Recursive");
    let report = text(&fixture.check(&source));
    assert!(report.contains("error[TS2322]"), "{report}");
}
