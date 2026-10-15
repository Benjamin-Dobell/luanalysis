use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis effect loops {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("main.lua"), source).unwrap();
        Self(path)
    }
    fn check(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args(["check", "--lua-version", "5.4", "--dump-types", "main.lua"])
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
fn rejects(output: &Output, code: &str) {
    assert_eq!(output.status.code(), Some(1), "{}", text(output));
    assert!(text(output).contains(code), "{}", text(output));
}
const ORDINARY: &str =
    "---@param value any\n---@return any\nlocal function ordinary(value) return value end\n";

#[test]
fn a_later_assert_store_through_a_written_function_contract_keeps_runtime_identity() {
    let fixture = Fixture::new(&format!(
        "{ORDINARY}---@param value number|nil\nlocal function read(value)\n ---@type fun(value:any):any\n local guard=ordinary\n guard=assert\n guard(value)\n ---@type number\n local narrowed=value\n return narrowed\nend\nlocal result=read(42)\n"
    ));
    pass(&fixture.check());
}

#[test]
fn a_loop_callee_replaced_by_an_ordinary_function_cannot_keep_assertion_effects() {
    let source = include_str!("fixtures/native_effects/loop_replaced_guard.lua");
    let fixture = Fixture::new(source);
    let output = fixture.check();
    rejects(&output, "TS2322");
    assert!(
        text(&output).contains("main.lua:12:18: error[TS2322]"),
        "{}",
        text(&output)
    );
    fs::write(
        fixture.0.join("main.lua"),
        source.replace("  guard(value)", "  guard=assert\n  guard(value)"),
    )
    .unwrap();
    pass(&fixture.check());
}

#[test]
fn restoring_the_real_assert_before_each_loop_call_keeps_its_refinement() {
    let fixture = Fixture::new(&format!(
        "{ORDINARY}---@param value number|nil\n---@param again boolean\nlocal function read(value,again)\n ---@type fun(value:any):any\n local guard=ordinary\n while again do\n  guard=assert\n  guard(value)\n  ---@type number\n  local narrowed=value\n  guard=ordinary\n  value=nil\n end\nend\nlocal result=read(42,true)\n"
    ));
    pass(&fixture.check());
}

#[test]
fn restoring_a_callee_copy_cannot_hide_loop_mutation_of_the_original_standard_slot() {
    for (setup, mutation) in [
        ("", "  assert=ordinary\n"),
        ("local environment=_G\n", "  environment.assert=ordinary\n"),
        ("local function replace() assert=ordinary end\n", "  replace()\n"),
    ] {
        let source = format!("{ORDINARY}{setup}---@param value number|nil\n---@param again boolean\nlocal function read(value,again)\n ---@type fun(value:any):any\n local guard=assert\n while again do\n  guard=assert\n  guard(value)\n  ---@type number\n  local narrowed=value\n{mutation}  value=nil\n end\nend\nread(42,true)\n");
        let fixture = Fixture::new(&source);
        let output = fixture.check();
        rejects(&output, "TS2322");
        assert!(!text(&output).contains("LUA2041"), "{}", text(&output));
        let narrowed_line = source.lines().position(|line| line.contains("local narrowed=value")).unwrap() + 1;
        assert!(text(&output).contains(&format!("main.lua:{narrowed_line}:18: error[TS2322]")), "{}", text(&output));
        let corrected = if setup.contains("local function replace()") {
            source.replace("local function replace() assert=ordinary end", "local function replace() end")
        } else { source.replace(mutation, "") };
        fs::write(fixture.0.join("main.lua"), corrected).unwrap();
        pass(&fixture.check());
    }
}

#[test]
fn a_same_shape_ordinary_function_has_no_assertion_effect() {
    let fixture = Fixture::new(&format!(
        "{ORDINARY}---@param value number|nil\nlocal function read(value)\n ordinary(value)\n ---@type number\n local narrowed=value\nend\nlocal result=read(42)\n"
    ));
    rejects(&fixture.check(), "TS2322");
}

#[test]
fn unused_call_reachability_uses_written_never_without_inferring_an_unwritten_effect() {
    let fixture = Fixture::new(
        "---@return never\nlocal function stop() error('stop') end\n---@param value number|nil\nlocal function explicit(value)\n stop()\n ---@type number\n local unreachable=value\nend\nlocal function unwritten() error('stop') end\n---@param value number|nil\nlocal function ordinary(value)\n unwritten()\n ---@type number\n local reachable=value\nend\n",
    );
    let output = fixture.check();
    rejects(&output, "TS2322");
    assert!(
        text(&output).contains("main.lua:14:18: error[TS2322]"),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("main.lua:7:"), "{}", text(&output));
    let original = fs::read_to_string(fixture.0.join("main.lua")).unwrap();
    fs::write(
        fixture.0.join("main.lua"),
        original.replace("---@param value number|nil", "---@param value number"),
    )
    .unwrap();
    pass(&fixture.check());
}
