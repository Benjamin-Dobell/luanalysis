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
            "luanalysis callable table loader {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("factory.lua"), module).unwrap();
        fs::write(path.join("leaf.lua"), "return {value=42}\n").unwrap();
        fs::write(path.join("main.lua"), "local factory=require('factory')\nlocal after=require('leaf')\n---@type number\nlocal checked=after.value\nreturn checked\n").unwrap();
        Self(path)
    }

    fn check(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args([
                "check",
                "--format",
                "json",
                "--lua-version",
                "5.4",
                "--strict-standard-effects",
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
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn report(output: &Output, exit: i32) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(exit), "{stdout}\n{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    assert!(stdout.starts_with("{\"formatVersion\":1,"), "{stdout}");
    stdout.into_owned()
}

const HEADER: &str = "---@class Callable\n---@overload fun(value?:number):table\nlocal Static={}\n";
const SAFE: &str = "setmetatable(Static,{__call=function() return {} end})\n";
const MUTATING: &str =
    "setmetatable(Static,{__call=function() package.path='custom/?.lua'; return {} end})\n";

fn invalidates(module: &str) {
    let output = Fixture::new(module).check();
    let result = report(&output, 1);
    assert!(result.contains("\"code\":\"LUA2036\""), "{result}");
    assert!(result.contains("\"line\":2,"), "{result}");
}


#[test]
fn reached_call_metamethod_executes_its_actual_loader_write() {
    invalidates(&format!(
        "{HEADER}{MUTATING}local instance=Static()\nreturn Static\n"
    ));
}


#[test]
fn argument_evaluation_cannot_reuse_the_earlier_safe_call_metamethod() {
    invalidates(&format!("{HEADER}{SAFE}local function replace()\n{MUTATING}return 1\nend\nlocal instance=Static(replace())\nreturn Static\n"));
}

#[test]
fn an_ordinary_attach_function_does_not_attest_a_call_metamethod() {
    invalidates(&format!("{HEADER}---@generic T\n---@param value T\n---@param meta table\n---@return T\nlocal function ordinary(value,meta) return value end\nordinary(Static,{{__call=function() return {{}} end}})\nlocal instance=Static()\nreturn Static\n"));
}


#[test]
fn a_replacement_newindex_handler_still_invalidates_the_loader() {
    invalidates(&format!("{HEADER}{SAFE}local instance=Static()\nsetmetatable(Static,{{__newindex=function() package.path='custom/?.lua' end}})\nfunction Static.method() return 1 end\nreturn Static\n"));
}

#[test]
fn a_constructor_cannot_hide_its_reached_metatable_replacement() {
    invalidates(&format!("{HEADER}setmetatable(Static,{{__call=function()\nsetmetatable(Static,{{__newindex=function() package.path='custom/?.lua' end}})\nreturn {{}}\nend}})\nlocal instance=Static()\nfunction Static.method() return 1 end\nreturn Static\n"));
}

#[test]
fn retrieving_and_mutating_the_metatable_is_an_effect_barrier() {
    invalidates(&format!("{HEADER}{SAFE}local instance=Static()\nlocal meta=getmetatable(Static)\nmeta.__newindex=function() package.path='custom/?.lua' end\nfunction Static.method() return 1 end\nreturn Static\n"));
}
