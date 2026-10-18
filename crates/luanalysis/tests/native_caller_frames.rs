use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(module: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis caller frames {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("module.lua"), module).unwrap();
        Self(path)
    }
    fn check(&self, source: &str) -> Output {
        fs::write(self.0.join("main.lua"), source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .current_dir(&self.0)
            .args([
                "check",
                "--lua-version",
                "5.4",
                "--dump-types",
                "--module-root",
                ".",
                "main.lua",
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
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn clean(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{}", text(output));
}
fn incompatible(output: &Output) {
    assert_eq!(output.status.code(), Some(1), "{}", text(output));
    assert!(text(output).contains("TS2322"), "{}", text(output));
}

const COPY: &str = r#"
---@generic T
---@param tab T[]
---@return T[]
local function copy(tab)
    ---@type T[]
    local result={}
    for index,value in ipairs(tab) do result[index]=value end
    return result
end
return copy
"#;

#[test]
fn foreign_generic_iterator_callee_keeps_original_scalar_and_pack_argument_callers() {
    let fixture = Fixture::new(COPY);
    let source = r#"
local copy=require('module')
---@generic U
---@param event string
---@param initial U[]
local function outer(event,initial)
    ---@type table<string,U[]>
    local handlers={one=initial}
    return function()
        local function current() return handlers[event] end
        local copied=copy(ARGUMENT)
        return copied
    end
end
local numbers=outer('one',{1,2})()
local strings=outer('one',{'one','two'})()
---@type number
local number=numbers[1]
---@type string
local string=strings[1]
"#;
    for argument in ["handlers[event]", "current()"] {
        let source = source.replace("ARGUMENT", argument);
        clean(&fixture.check(&source));
        incompatible(
            &fixture.check(&source.replace("local number=numbers[1]", "local number=strings[1]")),
        );
        clean(&fixture.check(&source));
    }
}

#[test]
fn plain_foreign_factory_constructs_closures_in_its_own_declaration_frame() {
    let fixture = Fixture::new(
        r#"
---@generic T
---@param value T
local function make(value)
    return function() return value end
end
return make
"#,
    );
    let source = r#"
local make=require('module')
---@generic U
---@param value U
local function outer(value)
    return function()
        local made=make(value)
        return made()
    end
end
local number=outer(42)()
local string=outer('text')()
---@type number
local n=number
---@type string
local s=string
---@type number
local literal=make(7)()
"#;
    clean(&fixture.check(source));
    incompatible(&fixture.check(&source.replace("local n=number", "local n=string")));
    clean(&fixture.check(source));
}

#[test]
fn canonical_member_body_recovers_sibling_helpers_through_original_constructor_ownership() {
    let fixture = Fixture::new("");
    let source = r#"
---@class Example
local Constructor={}
setmetatable(Constructor,{__call=function()
    local self=--[[---@type Example]] {}
    ---@param value number
    ---@return number
    local function first(value) return value end
    ---@param value number
    ---@return number
    function self.second(value) return first(value) end
    return self
end})
---@type Example
local instance
---@type number
local result=instance.second(42)
"#;
    clean(&fixture.check(source));
    let rejected =
        fixture.check(&source.replace("instance.second(42)", "instance.second('wrong')"));
    assert_eq!(rejected.status.code(), Some(1), "{}", text(&rejected));
    assert!(text(&rejected).contains("TS2345"), "{}", text(&rejected));
    clean(&fixture.check(source));
}
