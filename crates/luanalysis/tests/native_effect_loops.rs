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
        self.check_lua_version("5.4")
    }
    fn check_lua_version(&self, version: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args(["check", "--lua-version", version, "--dump-types", "main.lua"])
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
fn settled_body(output: &Output) {
    assert!(matches!(output.status.code(), Some(0) | Some(1)), "{}", text(output));
    assert!(!text(output).contains("LUA2041"), "{}", text(output));
}
fn rejects_checked_value(output: &Output, source: &str) {
    rejects(output, "TS2322");
    let (line, checked) = source.lines().enumerate().find(|(_, line)| line.contains("local checked=value")).unwrap();
    let column = checked.rfind("value").unwrap() + 1;
    assert!(text(output).contains(&format!("main.lua:{}:{column}: error[TS2322]", line + 1)), "{}", text(output));
}
const ORDINARY: &str =
    "---@param value any\n---@return any\nlocal function ordinary(value) return value end\n";

#[test]
fn an_instance_registry_store_does_not_make_a_direct_assert_provisional() {
    let source = "---@type table<string,table>\nlocal allInstances={}\n---@param value number|nil\n---@param instanceGuid string\nlocal function make(value,instanceGuid)\n local self={check=assert}\n allInstances[instanceGuid]=self\n assert(value)\n ---@type number\n local checked=value\n return self\nend\nmake(42,'instance')\n";
    let fixture = Fixture::new(source);
    pass(&fixture.check());
    fs::write(fixture.0.join("main.lua"), source.replace("---@type number\n local checked", "---@type string\n local checked")).unwrap();
    rejects_checked_value(&fixture.check(), source);
}

// Intrinsics are recognised by name (Luanalysis v1 semantics): a table member
// holding `assert` is an ordinary call, whatever stores reached it.
#[test]
fn member_aliases_of_assert_are_ordinary_calls() {
    let source = "---@param value number|nil\nlocal function probe(value)\n local holder={check=assert}\n local copy=holder\n copy.check=assert\n holder.check(value)\n ---@type number\n local checked=value\nend\nprobe(42)\n";
    let fixture = Fixture::new(source);
    rejects_checked_value(&fixture.check(), source);
}

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
    ] {
        let source = format!("{ORDINARY}{setup}---@param value number|nil\n---@param again boolean\nlocal function read(value,again)\n ---@type fun(value:any):any\n local guard=assert\n while again do\n  guard=assert\n  guard(value)\n  ---@type number\n  local narrowed=value\n{mutation}  value=nil\n end\nend\nread(42,true)\n");
        let fixture = Fixture::new(&source);
        let output = fixture.check();
        rejects(&output, "TS2322");
        assert!(!text(&output).contains("LUA2041"), "{}", text(&output));
        let narrowed_line = source.lines().position(|line| line.contains("local narrowed=value")).unwrap() + 1;
        assert!(text(&output).contains(&format!("main.lua:{narrowed_line}:18: error[TS2322]")), "{}", text(&output));
        fs::write(fixture.0.join("main.lua"), source.replace(mutation, "")).unwrap();
        pass(&fixture.check());
    }
    // A call preserves the standard slot, as an unknown effect does, even when
    // its body replaces it.
    let source = format!("{ORDINARY}local function replace() assert=ordinary end\n---@param value number|nil\n---@param again boolean\nlocal function read(value,again)\n ---@type fun(value:any):any\n local guard=assert\n while again do\n  guard=assert\n  guard(value)\n  ---@type number\n  local narrowed=value\n  replace()\n  value=nil\n end\nend\nread(42,true)\n");
    pass(&Fixture::new(&source).check());
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

#[test]
fn a_generic_sparse_array_store_preserves_type_policy_and_checks_the_body_result() {
    let source = include_str!("fixtures/native_effects/json_sparse_fill.lua");
    let fixture = Fixture::new(source);
    let original_return = "return --[[---@type (T | ge_tts__JsonNull)[] ]] sparseArray";
    let wrong = source.replace(original_return, "return 'wrong'");
    assert_ne!(wrong, source);
    let line = wrong.lines().position(|line| line.contains("return 'wrong'")).unwrap() + 1;
    for version in ["5.2", "5.4"] {
        pass(&fixture.check_lua_version(version));
        fs::write(fixture.0.join("main.lua"), &wrong).unwrap();
        let output = fixture.check_lua_version(version);
        rejects(&output, "TS2322");
        assert!(!text(&output).contains("LUA2041"), "{}", text(&output));
        assert!(text(&output).contains(&format!("main.lua:{line}:12: error[TS2322]")), "{}", text(&output));
        fs::write(fixture.0.join("main.lua"), source).unwrap();
        pass(&fixture.check_lua_version(version));
    }
}

#[test]
fn a_captured_iterator_callee_in_nested_loops_keeps_its_original_creation_value() {
    let source = include_str!("fixtures/native_effects/captured_iterator_callback.lua");
    let fixture = Fixture::new(source);
    pass(&fixture.check_lua_version("5.2"));
    let wrong = source.replace("---@type number\n                local checked", "---@type string\n                local checked");
    assert_ne!(wrong, source);
    fs::write(fixture.0.join("main.lua"), &wrong).unwrap();
    let output = fixture.check_lua_version("5.2");
    rejects(&output, "TS2322");
    settled_body(&output);
    let line = wrong.lines().position(|line| line.contains("local checked = value")).unwrap() + 1;
    assert!(text(&output).contains(&format!("main.lua:{line}:33: error[TS2322]")), "{}", text(&output));
    fs::write(fixture.0.join("main.lua"), source).unwrap();
    pass(&fixture.check_lua_version("5.2"));
}

#[test]
fn the_actual_select_inject_body_settles_without_hiding_a_written_body_error() {
    let source = include_str!("fixtures/native_effects/table_utils_select_inject.lua");
    let fixture = Fixture::new(source);
    // This original body has independent generic compatibility diagnostics.
    // Preserve settlement without freezing those errors as expected behavior.
    settled_body(&fixture.check_lua_version("5.2"));
    let wrong = source.replace("local arrayLength = 0", "local arrayLength = 0\n    ---@type string\n    local wrong = arrayLength");
    assert_ne!(wrong, source);
    fs::write(fixture.0.join("main.lua"), &wrong).unwrap();
    let output = fixture.check_lua_version("5.2");
    rejects(&output, "TS2322");
    settled_body(&output);
    let line = wrong.lines().position(|line| line.contains("local wrong = arrayLength")).unwrap() + 1;
    assert!(text(&output).contains(&format!("main.lua:{line}:19: error[TS2322]")), "{}", text(&output));
    fs::write(fixture.0.join("main.lua"), source).unwrap();
    settled_body(&fixture.check_lua_version("5.2"));
}
