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
            "luanalysis namespace {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, source: &str) {
        fs::write(self.0.join(name), source).unwrap();
    }
    fn check(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args([
                "check",
                "--dump-types",
                "--library",
                "math.def.lua",
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
fn pass(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{}", text(output));
}
const MATH: &str = "---@class std__Math\n---@field pi number\n---@field config {name:string}\nmath = {}\n---@param value number\n---@return number\nfunction math.abs(value) return unresolved_placeholder(value) end\n";

#[test]
fn namespace_members_link_original_class_fields_and_ignore_placeholder_bodies() {
    let fixture = Fixture::new();
    fixture.file("math.def.lua", MATH);
    fixture.file("main.lua", "---@type number\nlocal result = math.abs(-42)\n---@type number\nlocal pi = math.pi\n---@type string\nlocal name = math.config.name\n");
    let output = fixture.check();
    pass(&output);
    assert!(
        text(&output).contains("result: number"),
        "{}",
        text(&output)
    );
    assert!(
        !text(&output).contains("unresolved_placeholder"),
        "{}",
        text(&output)
    );
}

#[test]
fn actual_namespace_argument_error_uses_original_unicode_crlf_source_then_accepts_correction() {
    let fixture = Fixture::new();
    fixture.file("math.def.lua", MATH);
    fixture.file("main.lua", "-- é😀\r\nlocal result = math.abs('wrong')\r\n");
    let output = fixture.check();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("main.lua:2:25: error[TS2345]"),
        "{}",
        text(&output)
    );
    fixture.file("main.lua", "-- é😀\r\nlocal result = math.abs(-42)\r\n");
    pass(&fixture.check());
}

#[test]
fn namespace_overload_and_generic_contracts_keep_original_signature_owners() {
    let fixture = Fixture::new();
    fixture.file("math.def.lua", "---@class std__Math\nmath = {}\n---@overload fun(value:number):number\n---@overload fun(value:string):string\nfunction math.identity(value) return unresolved_placeholder(value) end\n---@generic T\n---@param value T\n---@return T\nfunction math.keep(value) return unresolved_placeholder(value) end\n");
    fixture.file("main.lua", "---@type number\nlocal numeric = math.identity(42)\n---@type string\nlocal textual = math.identity('word')\n---@type number\nlocal generic_number = math.keep(42)\n---@type string\nlocal generic_string = math.keep('word')\n");
    pass(&fixture.check());
    fixture.file("main.lua", "local rejected = math.identity(false)\n");
    let output = fixture.check();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("TS2345") || text(&output).contains("TS2769"),
        "{}",
        text(&output)
    );
}

#[test]
fn namespace_member_requires_an_original_unambiguous_attached_owner() {
    let fixture = Fixture::new();
    for declaration in [
        "---@param value number\n---@return number\nfunction missing.abs(value) return 0 end\n",
        "---@class std__First\nmath = {}\n---@class std__Second\nmath = {}\n---@param value number\n---@return number\nfunction math.abs(value) return 0 end\n",
        "---@class std__Math\nmath = {}\n---@return number\nfunction math.nested.abs() return 0 end\n",
    ] {
        fixture.file("math.def.lua", declaration);
        fixture.file("main.lua", "local value = 42\n");
        let output = fixture.check();
        assert_eq!(output.status.code(), Some(1), "{}", text(&output));
        assert!(text(&output).contains("LUA2038"), "{}", text(&output));
    }
}

#[test]
fn reviewed_namespace_contracts_spell_every_public_overload_head_explicitly() {
    let fixture = Fixture::new();
    let original = "---@class std__Math\nmath = {}\n---@overload fun(y:number):number\n---@param y number\n---@param x number\n---@return number\nfunction math.atan(y, x) return 0 end\n";
    fixture.file("math.def.lua", original);
    fixture.file("main.lua", "local result = math.atan(1, 2)\n");
    let output = fixture.check();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("TS2554"), "{}", text(&output));
    let reviewed = original.replace(
        "---@overload fun(y:number):number",
        "---@overload fun(y:number):number\n---@overload fun(y:number,x:number):number",
    );
    fixture.file("math.def.lua", &reviewed);
    pass(&fixture.check());
}
