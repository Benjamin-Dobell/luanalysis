//! Annotation syntax of EmmyLua Analyzer Rust that Luanalysis reads as EmmyLua Analyzer does with
//! `--emmylua-syntax`, its `---@readonly` locals always, and LuaLS forms its tests exposed.
use std::{fs, process::Command, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn report(source: &str) -> String {
    checked(source, &[])
}

fn emmylua(source: &str) -> String {
    checked(source, &["--emmylua-syntax"])
}

fn checked(source: &str, options: &[&str]) -> String {
    let directory = std::env::temp_dir().join(format!("luanalysis emmylua syntax {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("main.lua"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&directory)
        .args(["check", "--lua-version", "5.4", "--dump-types"]).args(options).arg("main.lua").output().unwrap();
    fs::remove_dir_all(directory).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

fn errors(report: &str) -> Vec<&str> {
    report.lines().filter(|line| line.contains(": error[")).collect()
}

#[test]
fn a_type_minus_another_excludes_its_members() {
    let report = emmylua("---@alias NotNull<T> T - ?\n---@type NotNull<string | nil>\nlocal a\n---@type (1 | 2 | 3) - 2\nlocal b\n---@type sync fun(): number\nlocal c\n");
    for expected in ["main.lua:3:7: a: string", "main.lua:5:7: b: 1 | 3", "main.lua:7:7: c: fun(): number"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    assert!(errors(&report).is_empty(), "{report}");
}

#[test]
fn return_overload_rows_are_the_results_a_call_returns() {
    let source = "---@generic T, E\n---@param ok boolean\n---@param success T\n---@param failure E\n---@return boolean\n---@return T|E\n\
---@return_overload true, T\n---@return_overload false, E\nlocal function pick(ok, success, failure)\n    if ok then return true, success end\n    return false, failure\nend\n\
local ok, value = pick(math.random() > 0.5, 1, \"error\")\nif ok then\n    ---@type number\n    local n = value\nend\n\
---@return_overload true, number\nlocal function wrong() return false, 1 end\n";
    let report = emmylua(source);
    assert!(report.contains("main.lua:9:16: pick: fun(ok: boolean, success: T, failure: E): (false, E) | (true, T)"), "{report}");
    let errors = errors(&report);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:19:31: error[TS2322]")), "{report}");
    assert_eq!(errors.len(), 1, "{report}");
}

#[test]
fn a_readonly_local_tables_fields_are_not_assigned() {
    let source = "---@readonly\nlocal codes = { OK = 0 }\ncodes.NOT_FOUND = 10\ncodes[\"x\"] = 1\n---@type number\nlocal key\ncodes[key] = 2\nlocal ok = codes.OK\n---@readonly\nGlobal = {}\nGlobal.x = 1\n";
    let report = report(source);
    let errors = errors(&report);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:3:1: error[TS2540]: Cannot assign to 'NOT_FOUND' because it is a read-only property.")), "{report}");
    assert!(errors.iter().any(|error| error.starts_with("main.lua:4:1: error[TS2540]: Cannot assign to 'x'")), "{report}");
    assert!(errors.iter().any(|error| error.starts_with("main.lua:7:1: error[TS2542]")), "{report}");
    // Above anything but a local, `---@readonly` says nothing, as EmmyLua Analyzer reads it.
    assert_eq!(errors.len(), 3, "{report}");
}

#[test]
fn signature_tags_on_a_local_whose_value_is_no_function_declare_its_type() {
    let source = "---@param x number\n---@return string\nlocal format = tostring\nlocal text = format(1)\nformat(\"x\")\n\
---@generic T\n---@param value T\n---@return T\n---@overload fun(value: string, count: number): T\nlocal identity = nil\n";
    let checked = emmylua(source);
    for expected in [
        "main.lua:3:7: format: fun(x: number): string",
        "main.lua:4:7: text: string",
        "main.lua:10:7: identity: (fun(value: T): T) & (fun(value: string, count: number): T)",
    ] {
        assert!(checked.contains(expected), "{expected}\n{checked}");
    }
    let errors = errors(&checked);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:5:8: error[TS2345]")), "{checked}");
    assert!(!errors.iter().any(|error| error.contains("LUA1001") || error.contains("LUA2034")), "{checked}");
    // The signature is the local's type, which its value must satisfy.
    let declared = emmylua("---@param x number\nlocal x = 1\n");
    assert!(declared.contains("main.lua:2:11: error[TS2322]"), "{declared}");
    // A class's own `---@overload` above its table calls the class, and a lone `---@generic`
    // describes no signature.
    let class = emmylua("---@class Callable\n---@overload fun(x: number): string\nlocal Callable = {}\n---@generic T\nlocal value = 42\n");
    assert!(class.contains("main.lua:3:7: Callable: fun(x: number): string"), "{class}");
    assert!(class.contains("LUA1001"), "{class}");
}

#[test]
fn param_dots_types_the_varargs() {
    let report = report("---@param a string\n---@param ... number\nlocal function f(a, ...) end\nf(\"x\", \"z\")\nf(\"x\", 1, 2)\n");
    assert!(report.contains("main.lua:3:16: f: fun(a: string, ...: number): void"), "{report}");
    let errors = errors(&report);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:4:8: error[TS2345]")), "{report}");
    assert_eq!(errors.len(), 1, "{report}");
}

#[test]
fn an_optional_field_may_be_absent_whatever_its_type() {
    let report = report("---@shape O\n---@field field? any\n---@field other? unknown\n---@field required any\n\n---@param o O\nlocal function g(o) end\ng({ required = 1 })\ng({})\n");
    let errors = errors(&report);
    assert!(errors.iter().any(|error| error.starts_with("main.lua:9:3: error[TS2345]")), "{report}");
    assert_eq!(errors.len(), 1, "{report}");
}

#[test]
fn a_functions_generic_parameters_are_each_of_its_overloads_own() {
    let report = report("---@generic T\n---@overload fun(x: T): T\n---@param x any\n---@return any\nlocal function h(x) return x end\nlocal a = h(1)\nlocal b = h(\"s\")\n");
    assert!(report.contains("main.lua:6:7: a: number"), "{report}");
    assert!(report.contains("main.lua:7:7: b: string"), "{report}");
    assert!(errors(&report).is_empty(), "{report}");
}

#[test]
fn select_counts_with_hash_and_returns_values_with_an_index() {
    let report = report("---@param ... string\nlocal function f(...)\n    for i = 1, select('#', ...) do\n        local value = select(i, ...)\n    end\nend\n");
    assert!(report.contains("main.lua:4:15: value: string"), "{report}");
    assert!(errors(&report).is_empty(), "{report}");
}

#[test]
fn emmylua_syntax_is_read_only_when_asked() {
    let source = "---@type (1 | 2 | 3) - 2\nlocal b\n---@type sync fun(): number\nlocal c\n\
---@return_overload true, number\nlocal function r() return true, 1 end\n\
---@param x number\n---@return string\nlocal format = tostring\n\
---@param t number?\nlocal function f(t) end\n---@type fun(x: string?)\nlocal g\n";
    let plain = report(source);
    let errors = errors(&plain);
    for expected in [
        "main.lua:1:22: error[LUA1001]",
        "main.lua:5:4: error[LUA1001]: unsupported Luanalysis annotation @return_overload",
        "main.lua:10:19: error[LUA1001]: `T?` here is EmmyLua Analyzer syntax",
        "main.lua:12:23: error[LUA1001]: `T?` here is EmmyLua Analyzer syntax",
    ] {
        assert!(errors.iter().any(|error| error.starts_with(expected)), "{expected}\n{plain}");
    }
    assert!(plain.contains("main.lua:4:7: c: any"), "{plain}");
    assert!(!plain.contains("format: fun(x: number): string"), "{plain}");
    // With its syntax, each is read as EmmyLua Analyzer reads it, and a parameter's `T?` is its name's.
    let read = emmylua(source);
    for expected in ["main.lua:2:7: b: 1 | 3", "main.lua:4:7: c: fun(): number", "main.lua:6:16: r: fun(): (true, number)",
        "main.lua:9:7: format: fun(x: number): string", "main.lua:11:16: f: fun(t?: number): void"] {
        assert!(read.contains(expected), "{expected}\n{read}");
    }
    assert!(self::errors(&read).is_empty(), "{read}");
}
