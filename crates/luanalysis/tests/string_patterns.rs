//! A literal pattern types what the standard pattern functions return: a capture is a string, and
//! an empty capture, `()`, a position.
use std::{fs, process::Command, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn types(source: &str) -> String {
    let directory = std::env::temp_dir().join(format!("luanalysis string patterns {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("main.lua"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&directory)
        .args(["check", "--lua-version", "5.4", "--dump-types", "main.lua"]).output().unwrap();
    fs::remove_dir_all(directory).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn literal_patterns_type_their_captures() {
    let report = types("---@param text string\nlocal function f(text)\n    local key, value = text:match('(%w+)=(%w+)')\n    local whole = string.match(text, '^local%s')\n    local s, e, at = text:find('()x')\n    for a, b in text:gmatch('(%a)(%d)') do local joined = a .. b end\n    local plain = text:find('(', 1, true)\n    local dynamic = text:match(text)\nend\n");
    for expected in [
        "main.lua:3:11: key: nil | string", "main.lua:3:16: value: nil | string",
        "main.lua:4:11: whole: nil | string",
        "main.lua:5:17: at: nil | number",
        "main.lua:6:9: a: string", "main.lua:6:12: b: string",
        "main.lua:7:11: plain: nil | number",
        "main.lua:8:11: dynamic: nil | string | number",
    ] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    assert!(!report.contains("error["), "{report}");
}
