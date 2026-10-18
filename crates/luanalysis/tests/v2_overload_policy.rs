use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(library: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis v2 overload policy {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        let fixture = Self(path);
        fixture.file("api.def.lua", library);
        fixture.file("main.lua", source);
        fixture
    }
    fn file(&self, name: &str, text: &str) {
        fs::write(self.0.join(name), text).unwrap();
    }
    fn check(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .current_dir(&self.0)
            .args([
                "check",
                "--no-stdlib",
                "--dump-types",
                "--library",
                "api.def.lua",
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
fn fail(output: &Output, code: &str) {
    assert_eq!(output.status.code(), Some(1), "{}", text(output));
    assert!(text(output).contains(code), "{}", text(output));
}
#[test]
fn ambient_overload_hides_a_complete_longform_main_from_public_calls_and_display() {
    let fixture = Fixture::new(
        "---@overload fun(value:number):number\n---@param value number|string\n---@return number|string\nfunction choose(value) return unresolved_placeholder(value) end\n",
        "local rejected=choose('private')\n",
    );
    fail(&fixture.check(), "TS2345");
    fixture.file(
        "main.lua",
        "local callable=choose\n---@type number\nlocal accepted=choose(42)\n",
    );
    let output = fixture.check();
    pass(&output);
    let displayed = text(&output);
    assert!(
        displayed.contains("callable: fun(value: number): number"),
        "{displayed}"
    );
    assert!(!displayed.contains("number | string"), "{displayed}");
}
#[test]
fn a_plain_ambient_function_without_an_overload_keeps_its_original_contract() {
    let fixture = Fixture::new(
        "---@generic T\n---@param first T\n---@param second T\n---@return T\nfunction choose(first,second) return unresolved_placeholder(first,second) end\n",
        "---@type string\nlocal result=choose('left','right')\n",
    );
    pass(&fixture.check());
    fixture.file("main.lua", "local rejected=choose('left')\n");
    fail(&fixture.check(), "TS2554");
    fixture.file(
        "main.lua",
        "---@type string\nlocal corrected=choose('left','right')\n",
    );
    pass(&fixture.check());
}
#[test]
fn signature_local_generics_remain_public_but_function_longform_generics_are_private() {
    let fixture = Fixture::new(
        "---@overload fun<T:number>(value:T):T\n---@generic T\n---@param value T\n---@return T\nfunction choose(value) return unresolved_placeholder(value) end\n",
        "local rejected=choose('private')\n",
    );
    fail(&fixture.check(), "TS2345");
    fixture.file("main.lua", "---@type number\nlocal corrected=choose(42)\n");
    pass(&fixture.check());
}
#[test]
fn colon_public_heads_keep_one_real_receiver_and_derived_self_without_exposing_main_arity() {
    let library = "---@class Color\n---@field r number\nColor={}\n---@overload fun():self\n---@param alpha? boolean\n---@return self\nfunction Color:copy(alpha) return unresolved_placeholder(self,alpha) end\n---@class NamedColor:Color\n---@field name string\n";
    let fixture = Fixture::new(
        library,
        "---@param color NamedColor\nlocal function check(color) return color:copy(true) end\n",
    );
    fail(&fixture.check(), "TS2554");
    fixture.file("main.lua","---@param color NamedColor\n---@return string\nlocal function check(color) return color:copy().name end\n---@type Color\nlocal color\nlocal copy=color.copy(color)\n");
    pass(&fixture.check());
}
#[test]
fn runtime_body_uses_only_longform_parameters_instead_of_a_public_overload_branch() {
    let fixture = Fixture::new(
        "",
        "---@overload fun(value:number):number\n---@param value number|string\n---@return number\nlocal function choose(value) return value+1 end\nlocal selected=choose(42)\n",
    );
    let output = fixture.check();
    fail(&output, "TS2362");
    fixture.file("main.lua","---@overload fun(value:number):number\n---@param value number|string\n---@return number\nlocal function choose(value) return 42 end\nlocal selected=choose(42)\n");
    pass(&fixture.check());
}
