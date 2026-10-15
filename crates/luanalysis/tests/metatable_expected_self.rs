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
            "luanalysis expected metatable self {} {}",
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
            .args([
                "check",
                "--lua-version",
                "5.4",
                "--format",
                "json",
                "main.lua",
            ])
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
    stdout
}
fn clean(output: Output) {
    let report = report(output, 0);
    assert!(report.contains("\"diagnostics\":[]"), "{report}");
}
fn error_on(report: &str, source: &str, code: &str, statement: &str, expression: &str) {
    let start = source.find(statement).unwrap() + statement.rfind(expression).unwrap();
    let range = format!(
        "\"range\":{{\"start\":{start},\"end\":{}}}",
        start + expression.len()
    );
    let code = format!("\"code\":\"{code}\"");
    assert!(
        report
            .split("},{\"path\":")
            .any(|diagnostic| diagnostic.contains(&code) && diagnostic.contains(&range)),
        "{report}"
    );
}

const HEADER: &str = r#"---@class Instance
local Instance={}
---@class ChildInstance:Instance
local ChildInstance={}
---@class StaticBase
---@overload fun(value:number):Instance
local Base={}
---@class StaticChild:StaticBase
---@overload fun(value:string):ChildInstance
local Child={}
---@class Other
local Other={}
---@class Bare
local Bare={}
---@class Class
local Class={}
---@generic AncestorClass
---@generic Class:AncestorClass
---@param descendent Class
---@param parent nil|AncestorClass
---@return AncestorClass
function Class.parentConstructor(descendent,parent)
 local metatable=--[[---@type {__call:function}]] getmetatable(parent)
 local call=metatable.__call
 ---@vararg unknown
 local wrappedStaticParent=function(...) return call(descendent,...) end
 return --[[---@type AncestorClass]] wrappedStaticParent
end
"#;
const IDENTITY: &str = "---@generic T\n---@param meta T\n---@return T\nlocal function identity(meta) return meta end\n";
const FIRST: &str = "---@generic T,U\n---@param first T\n---@param ignored U\n---@return T\nlocal function first(first,ignored) return first end\n";
const CALLBACK: &str = r#"{
 ---@param class self
 ---@param value string
 __call=function(class,value)
  local parent=Class.parentConstructor(class,Base)
  ---@type string
  local checked=value
  return --[[---@type ChildInstance]] {}
 end,
 __index=Base,
}"#;

fn linked() -> String {
    format!("{HEADER}{IDENTITY}setmetatable(Child,identity({CALLBACK}))\n")
}

#[test]
fn a_linked_generic_parameter_transports_the_written_child_constructor_context() {
    let fixture = Fixture::new();
    let source = linked();
    clean(fixture.check(&source));
    let wrong = source.replace(
        "---@type string\n  local checked",
        "---@type number\n  local checked",
    );
    let rejected = report(fixture.check(&wrong), 1);
    error_on(&rejected, &wrong, "TS2322", "local checked=value", "value");
}

#[test]
fn expected_self_keeps_the_childs_actual_nominal_ancestor_constraint() {
    let fixture = Fixture::new();
    let source = linked().replace(
        "parentConstructor(class,Base)",
        "parentConstructor(class,Other)",
    );
    let rejected = report(fixture.check(&source), 1);
    error_on(
        &rejected,
        &source,
        "TS2345",
        "local parent=Class.parentConstructor(class,Other)",
        "class",
    );
}

#[test]
fn an_unlinked_ignored_parameter_does_not_receive_the_return_types_self_context() {
    let fixture = Fixture::new();
    let source = format!("{HEADER}{FIRST}setmetatable(Child,first({{}},{CALLBACK}))\n");
    let rejected = report(fixture.check(&source), 1);
    error_on(
        &rejected,
        &source,
        "TS2345",
        "local parent=Class.parentConstructor(class,Base)",
        "class",
    );
}

#[test]
fn a_target_without_a_written_constructor_head_does_not_invent_a_child_self_context() {
    let fixture = Fixture::new();
    let source = linked().replace("setmetatable(Child,", "setmetatable(Bare,");
    let rejected = report(fixture.check(&source), 1);
    error_on(
        &rejected,
        &source,
        "TS2345",
        "local parent=Class.parentConstructor(class,Base)",
        "class",
    );
}
