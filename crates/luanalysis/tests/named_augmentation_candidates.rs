//! Every project file's `---@field Owner.member` augmentations are candidate
//! members of every declaration. One with an unusable owner belongs to no other
//! declaration and must not invalidate the objects of other files.
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
            "luanalysis named augmentation candidates {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, source: &str) {
        fs::write(self.0.join(name), source).unwrap();
    }
    fn check(&self, files: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args(["check", "--jobs", "1", "--lua-version", "5.2", "--module-root", "."])
            .args(files)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn an_augmentation_of_a_union_alias_does_not_invalidate_other_files_objects() {
    let fixture = Fixture::new();
    fixture.file(
        "aliases.lua",
        "---@class First\n---@field id number\n\n---@class Second\n---@field id number\n\n---@alias Either First | Second\n\n---@field Either.extra number\n\nreturn {}\n",
    );
    fixture.file(
        "main.lua",
        "local aliases = require(\"aliases\")\n---@class Point\n---@field x number\nlocal Point = {}\n---@param point Point\n---@return number\nlocal function read(point) return point.x end\n---@type number\nlocal value = math.max(1, read(--[[---@type Point]] {x = 2}))\nreturn value\n",
    );
    let output = fixture.check(&["aliases.lua", "main.lua"]);
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    // The augmentation's own file reports its unusable owner.
    assert!(text.contains("aliases.lua:9:11: error[TS2304]"), "{text}");
    // Nothing else is affected: not the other file's class, not the standard
    // library (`math.max`, `table` overloads).
    assert!(!text.contains("main.lua:"), "{text}");
    assert!(!text.contains(".def.lua:"), "{text}");
}
