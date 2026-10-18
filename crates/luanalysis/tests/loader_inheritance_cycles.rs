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
        let root = std::env::temp_dir().join(format!(
            "luanalysis loader inheritance cycles {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn file(&self, name: &str, source: &str) {
        fs::write(self.0.join(name), source).unwrap();
    }

    fn check(&self, lua54: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"));
        command
            .current_dir(&self.0)
            .args(["check", "--format", "json", "--strict-standard-effects"]);
        if lua54 {
            command.args(["--lua-version", "5.4"]);
        }
        command
            .args(["--module-root", ".", "main.lua"])
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
    assert_eq!(
        output.status.code(),
        Some(exit),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stderr.is_empty(), "{stderr}");
    assert!(stdout.starts_with("{\"formatVersion\":1,"), "{stdout}");
    stdout.into_owned()
}

#[test]
fn a_second_require_of_a_cyclic_module_finishes_with_unresolved_loader_diagnostics() {
    let fixture = Fixture::new();
    fixture.file("first.lua", "local value=require('second')\nreturn value\n");
    fixture.file("second.lua", "local value=require('first')\nreturn value\n");
    fixture.file(
        "main.lua",
        "local first=require('first')\nlocal again=require('first')\nreturn again\n",
    );
    for lua54 in [false, true] {
        let output = fixture.check(lua54);
        let result = report(&output, 1);
        assert!(
            result.contains("\"code\":\"LUA2036\""),
            "lua54={lua54}: {result}"
        );
    }
}

#[test]
fn repeated_acyclic_imports_keep_independent_completed_loader_inputs() {
    let fixture = Fixture::new();
    fixture.file("leaf.lua", "return {value=42}\n");
    fixture.file("first.lua", "local value=require('leaf')\nreturn value\n");
    fixture.file("main.lua", "local first=require('first')\nlocal again=require('first')\n---@type number\nlocal checked=again.value\nreturn checked\n");
    for lua54 in [false, true] {
        let output = fixture.check(lua54);
        let result = report(&output, 0);
        assert!(
            result.contains("\"diagnostics\":[]"),
            "lua54={lua54}: {result}"
        );
    }
}

#[test]
fn a_required_module_loader_write_still_invalidates_the_following_import() {
    let fixture = Fixture::new();
    fixture.file("mutate.lua", "package.searchers=package.searchers\nreturn {}\n");
    fixture.file("leaf.lua", "return {value=42}\n");
    fixture.file(
        "main.lua",
        "require('mutate')\nlocal after=require('leaf')\nreturn after\n",
    );
    let output = fixture.check(true);
    let result = report(&output, 1);
    assert!(result.contains("\"code\":\"LUA2036\""), "{result}");
    assert!(
        result.contains("\"line\":2,"),
        "the real mutation must invalidate its continuation: {result}"
    );
}
