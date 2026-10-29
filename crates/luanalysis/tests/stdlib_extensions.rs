//! Functions added to a table whose class another file declares, as a project adds helpers to
//! the standard library's `math`, `string` and `table`.
use std::{fs, process::Command, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn checked(files: &[(&str, &str)]) -> String {
    checked_with(files, &[])
}

/// Checks `files`, each a path below a new directory, with the CLI `options` before them.
fn checked_with(files: &[(&str, &str)], options: &[&str]) -> String {
    let directory = std::env::temp_dir().join(format!("luanalysis stdlib extensions {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    for (name, source) in files {
        let path = directory.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&directory)
        .args(["check", "--lua-version", "5.4", "--dump-types"]).args(options)
        .args(files.iter().map(|(name, _)| *name).filter(|name| !name.starts_with("types/"))).output().unwrap();
    fs::remove_dir_all(directory).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

fn errors(report: &str) -> Vec<&str> {
    report.lines().filter(|line| line.contains(": error[")).collect()
}

#[test]
fn a_helper_that_reads_its_library_table_finishes_before_a_later_helper() {
    // The body of `math.f` reads `math`; calling it before `math.g` is stored once left the
    // body unfinished (LUA2041) and the call `any`.
    let source = "---@param x string\n---@return number\nfunction math.f(x) return math.abs(1) end\nlocal d = math.f(\"a\")\n\
---@return number\nfunction math.g() return 1 end\n\
---@param start string\n---@return boolean\nfunction string.startswith(s, start) return s:sub(1, #start) == start end\n\
local b = string.startswith(\"abc\", \"a\")\n---@return string\nfunction string:trimmed() return self end\n\
---@generic T\n---@param t T[]\n---@param value T\n---@return boolean\nfunction table.contains(t, value)\n    for _, v in ipairs(t) do if v == value then return true end end\n    return false\nend\n\
local yes = table.contains({ 1, 2 }, 2)\n---@return number\nfunction table.size() return 0 end\n";
    let report = checked(&[("main.lua", source)]);
    for expected in ["main.lua:4:7: d: number", "main.lua:10:7: b: boolean", "main.lua:21:7: yes: boolean"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    assert!(errors(&report).is_empty(), "{report}");
}

#[test]
fn a_librarys_class_with_helpers_added_by_the_project_reads_the_class() {
    let class = "---@meta\n---@class Lib\nLib = {}\n\n---@return number\nfunction Lib.base() end\n";
    let helpers = "---@param x string\n---@return number\nfunction Lib.f(x) return Lib.base() + #x end\nlocal d = Lib.f(\"a\")\n\
---@return number\nfunction Lib.g() return Lib.f(\"b\") end\nlocal e = Lib.g()\n";
    let report = checked_with(&[("types/lib.lua", class), ("src/helpers.lua", helpers)], &["--library", "types"]);
    for expected in ["src/helpers.lua:4:7: d: number", "src/helpers.lua:7:7: e: number"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    assert!(errors(&report).is_empty(), "{report}");
}
