//! EmmyLua Analyzer Rust's remaining syntax, read with `--emmylua-syntax` and reported as that
//! syntax without it, and the forms beside it that need no option: `---@namespace` and
//! lua-language-server's.
use std::{fs, process::Command, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn checked(files: &[(&str, &str)], options: &[&str]) -> String {
    let directory = std::env::temp_dir().join(format!("luanalysis emmylua forms {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&directory).unwrap();
    for (name, source) in files { fs::write(directory.join(name), source).unwrap(); }
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&directory)
        .args(["check", "--lua-version", "5.4", "--dump-types"]).args(options)
        .args(files.iter().map(|(name, _)| *name)).output().unwrap();
    fs::remove_dir_all(directory).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

fn emmylua(source: &str) -> String {
    checked(&[("main.lua", source)], &["--emmylua-syntax"])
}

fn plain(source: &str) -> String {
    checked(&[("main.lua", source)], &[])
}

fn errors(report: &str) -> Vec<&str> {
    report.lines().filter(|line| line.contains(": error[")).collect()
}

/// Every error is one of `expected`, a `file:line:column: error[CODE]` prefix, and each is there.
fn assert_errors(report: &str, expected: &[&str]) {
    let errors = errors(report);
    for prefix in expected {
        assert!(errors.iter().any(|error| error.starts_with(prefix)), "{prefix}\n{report}");
    }
    assert_eq!(errors.len(), expected.len(), "{report}");
}

#[test]
fn a_namespace_names_its_files_types_and_resolves_short_names_in_it() {
    let files = [
        ("game.lua", "---@namespace Game\n---@class Unit\n---@field hp number\n\n---@class Player: Unit\n---@field name string\n\n---@type Player\nlocal p\nlocal hp = p.hp\n"),
        ("user.lua", "---@using Game\n\n---@type Unit\nlocal u\nlocal hp = u.hp\n\n---@type Game.Player\nlocal q\nlocal name = q.name\n"),
        ("plain.lua", "---@type Unit\nlocal missing\n"),
    ];
    let report = checked(&files, &["--emmylua-syntax"]);
    for expected in ["game.lua:9:7: p: Game.Player", "game.lua:10:7: hp: number", "user.lua:4:7: u: Game.Unit", "user.lua:9:7: name: string"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    // A file outside the namespace names its types in full.
    assert_errors(&report, &["plain.lua:1:10: error[TS2304]: Cannot find name 'Unit'."]);
    // It needs no option: the user wants namespaces whichever syntax a project otherwise writes.
    assert_eq!(checked(&files, &[]), report);
}

#[test]
fn emmylua_builtin_types_are_the_types_they_name() {
    let source = "---@param v int\n---@param o std.NotNull<string | nil>\n---@param r std.RawGet<{ a: number }, \"a\">\n---@param k type\n---@param l Language<\"sql\">\nlocal function f(v, o, r, k, l) end\n\
---@class MyClass\n---@class MyInherited: MyClass\n---@field test fun()\n\n---@param instance MyClass\n---@return TypeGuard<MyInherited>\nlocal function guard(instance) return true end\n\
---@type MyClass\nlocal instance\nif guard(instance) then local narrowed = instance end\n---@type TypeGuard<string>\nlocal flag\n";
    let report = emmylua(source);
    assert!(report.contains("main.lua:6:16: f: fun(v: number, o: string, r: number, k: \"boolean\" | \"function\" | \"nil\" | \"number\" | \"string\" | \"table\" | \"thread\" | \"userdata\", l: string): void"), "{report}");
    assert!(report.contains("main.lua:16:31: narrowed: MyInherited"), "{report}");
    assert!(report.contains("main.lua:18:7: flag: boolean"), "{report}");
    assert_errors(&report, &[]);
    let report = plain(source);
    assert!(report.contains("main.lua:1:13: error[TS2304]: Cannot find name 'int'. It is EmmyLua Analyzer's builtin type (--emmylua-syntax)."), "{report}");
    assert!(report.contains("main.lua:12:12: error[TS2304]: Cannot find name 'TypeGuard'. It is EmmyLua Analyzer's builtin type (--emmylua-syntax)."), "{report}");
}

#[test]
fn classes_take_several_supers_primitive_supers_and_share_a_comment() {
    let source = "---@class A\n---@field a number\n---@class B\n---@field b string\n---@class C: A, B\n\n---@type C\nlocal c\nlocal a, b = c.a, c.b\n\n\
---@class Key: integer\n\n---@return Key\nlocal function key() return 0 end\n---@param n integer\nlocal function take(n) end\ntake(key())\n\n\
---@class Observable<T>\n---@class CountObservable<T>: Observable<integer>\nCountObservable = {}\n---@return CountObservable<T>\nfunction CountObservable:new() end\n";
    let report = emmylua(source);
    for expected in ["main.lua:9:7: a: number", "main.lua:9:10: b: string", "main.lua:14:16: key: fun(): number"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    // The class named as the variable is the table's, so its method's `T` is its own.
    assert_errors(&report, &["main.lua:22:4: error[TS2355]"]);
    let report = plain(source);
    assert!(report.contains("main.lua:11:16: error[TS2312]: Type 'number' must resolve to a class or shape record schema before it can be a base type. A class without fields that extends a primitive is EmmyLua Analyzer syntax (--emmylua-syntax)."), "{report}");
    assert!(report.contains("(EmmyLua Analyzer attaches one and reads the others as standalone declarations: --emmylua-syntax)"), "{report}");
}

#[test]
fn a_tuple_rest_element_is_any_number_of_its_type() {
    let source = "---@type [string...]\nlocal a = { \"x\", \"y\" }\n---@type [number, string...]\nlocal b = { 1, \"x\", \"y\" }\nlocal s, n = b[2], b[1]\n---@type [number, string...]\nlocal c = { 1, 2 }\n";
    let report = plain(source);
    for expected in ["main.lua:2:7: a: string[]", "main.lua:5:7: s: string", "main.lua:5:10: n: number"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    assert_errors(&report, &["main.lua:7:11: error[TS2322]"]);
}

#[test]
fn a_backtick_parameter_captures_the_type_its_argument_names() {
    let source = "---@class Component\n---@field id number\n---@class G.A\n---@class G.B: Component\n---@class G.C: G.B\n\n\
---@generic T: Component\n---@param name `T`\n---@return T\nlocal function new(name) return name end\n\
local c = new(\"G.C\")\nlocal id = c.id\nnew(\"G.A\")\n\n---@generic T\n---@param a `T`\nlocal function bar(a) end\n---@generic T\n---@param a `T`\nlocal function foo(a) bar(a) end\n";
    let report = plain(source);
    for expected in ["main.lua:11:7: c: G.C", "main.lua:12:7: id: number"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    assert_errors(&report, &[
        "main.lua:10:33: error[TS2322]",
        "main.lua:13:5: error[TS2345]: Argument of type '\"G.A\"' is not assignable to parameter of type '\"Component\" | \"G.B\" | \"G.C\"'.",
    ]);
}

#[test]
fn casts_widen_cast_field_paths_and_cast_the_expression_before_them() {
    let source = "---@type string\nlocal val = \"a\"\n---@cast val +boolean\nval = true\nval = 1\n\n\
---@class CastField\n---@field value string\n\n---@type CastField\nlocal obj\n---@cast obj.value +boolean\nobj.value = true\nobj.value = 1\n\n\
---@class Cast1\n---@field get fun(self: self, a: number): Cast1?\n---@field get2 fun(self: self, a: number): Cast1?\n\n---@type Cast1\nlocal A\n\
local a = A:get(1) --[[@cast -?]]\n    :get2(2)\nlocal b = A:get(1) --[[@as Cast1]]:get2(2)\n\n---@return integer\nlocal function count() return \"x\" ---@as integer\nend\n\
---@param m string | nil\nlocal function g(m)\n---@cast -?\nlocal n = m\nend\n";
    let report = emmylua(source);
    assert!(report.contains("main.lua:32:7: n: string"), "{report}");
    assert_errors(&report, &[
        "main.lua:5:7: error[TS2322]: Type '1' is not assignable to type 'string | false | true'.",
        "main.lua:14:13: error[TS2322]",
    ]);
    let report = plain(source);
    // lua-language-server writes `+T` on a variable too: it widens without the option.
    assert!(report.contains("main.lua:5:7: error[TS2322]: Type '1' is not assignable to type 'string | false | true'."), "{report}");
    assert!(!report.contains("main.lua:4:7: error"), "{report}");
    for expected in [
        "main.lua:12:4: error[LUA1001]: @cast of a field path is EmmyLua Analyzer syntax (--emmylua-syntax)",
        "main.lua:22:20: error[LUA1001]: a cast between an expression and its suffix is EmmyLua Analyzer syntax (--emmylua-syntax)",
        "main.lua:24:20: error[LUA1001]: a cast between an expression and its suffix is EmmyLua Analyzer syntax (--emmylua-syntax)",
        "main.lua:27:35: error[LUA1001]: a trailing `---@as T` is EmmyLua Analyzer syntax (--emmylua-syntax)",
        "main.lua:31:4: error[LUA1001]: @cast without a name is EmmyLua Analyzer syntax (--emmylua-syntax)",
    ] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn emmylua_tags_declare_classes_predicates_and_nothing_else() {
    let source = "---@interface I\n---@field x number\n\n---@type I\nlocal i\nlocal x = i.x\n\n---@language sql\nlocal q = \"select 1\"\n---@mapping other\n---@schema schema.json\n---@[deprecated(\"use g\")]\nlocal z = 1\n\n\
---@param v any\n---@return boolean\n---@return_cast v string\nlocal function isString(v) return type(v) == \"string\" end\n---@type string | number\nlocal sn\nif isString(sn) then local s = sn end\n";
    let report = emmylua(source);
    for expected in ["main.lua:6:7: x: number", "main.lua:21:28: s: string"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    assert_errors(&report, &[]);
    let report = plain(source);
    for tag in ["1:4: error[LUA1001]: @interface", "8:4: error[LUA1001]: @language", "10:4: error[LUA1001]: @mapping", "11:4: error[LUA1001]: @schema",
        "12:4: error[LUA1001]: `---@[attribute]`", "17:4: error[LUA1001]: @return_cast"]
    {
        assert!(report.contains(&format!("main.lua:{tag}")), "{tag}\n{report}");
    }
}

#[test]
fn a_module_annotation_makes_a_local_the_module() {
    let files = [
        ("main.lua", "---@module \"helper\"\nlocal helper\nlocal greeting = helper.hello()\n"),
        ("helper.lua", "local M = {}\nfunction M.hello() return \"hi\" end\nreturn M\n"),
    ];
    let report = checked(&files, &[]);
    assert!(report.contains("main.lua:3:7: greeting: string"), "{report}");
    assert_errors(&report, &[]);
}

#[test]
fn const_generics_new_functions_and_field_overloads() {
    let source = "---@generic const T\n---@param x T\n---@return { value: T }\nlocal function box(x) return { value = x } end\nlocal boxed = box(\"literal\")\nlocal value = boxed.value\n\n\
---@type new fun(n: number): string\nlocal construct\n---@type ConstructorParameters<fun(n: number): string>\nlocal parameters\n\n\
---@class F\n---@field f fun(a: string): string\n---@field f fun(a: number): number\n\n---@type F\nlocal ff\nlocal s, n = ff.f(\"x\"), ff.f(1)\n";
    let report = emmylua(source);
    for expected in ["main.lua:5:7: boxed: { value: \"literal\" }", "main.lua:9:7: construct: fun(n: number): string", "main.lua:11:7: parameters: [number]", "main.lua:19:7: s: string", "main.lua:19:10: n: number"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    assert_errors(&report, &[]);
    let report = plain(source);
    assert!(report.contains("main.lua:1:13: error[LUA1001]: `---@generic const T` is EmmyLua Analyzer syntax (--emmylua-syntax)"), "{report}");
    assert!(report.contains("main.lua:5:7: boxed: { value: string }"), "{report}");
    // Repeated function fields are overloads without the option too, as lua-language-server reads them.
    assert!(report.contains("main.lua:19:7: s: string") && report.contains("main.lua:19:10: n: number"), "{report}");
}

#[test]
fn diagnostic_directives_disable_diagnostics_with_the_option() {
    let source = "---@diagnostic disable-next-line: undefined-global\nprint(one)\nprint(two) ---@diagnostic disable-line: TS2304\n\
---@diagnostic disable-next-line: assign-type-mismatch\nprint(three)\n---@diagnostic disable\nprint(four)\n---@diagnostic enable\nprint(five)\n";
    assert_errors(&emmylua(source), &["main.lua:5:7: error[TS2304]", "main.lua:9:7: error[TS2304]"]);
    // lua-language-server writes them too: without the option they are left to the editor.
    assert_eq!(errors(&plain(source)).len(), 5);
}

#[test]
fn global_declarations_and_luajit_numerals_are_read_with_the_option() {
    let source = "local a, b, c, d = 1LL, 0x10ULL, 3i, 0b101\n---@type string\nlocal s = d\nglobal function shout() return 1 end\nglobal level = 2\nglobal <const> *\nglobal later\nlocal total = shout() + level\n";
    let report = emmylua(source);
    assert!(report.contains("main.lua:8:7: total: number"), "{report}");
    assert_errors(&report, &["main.lua:3:11: error[TS2322]: Type 'number' is not assignable to type 'string'."]);
    let report = plain(source);
    for expected in [
        "main.lua:1:20: error[LUA1001]: LuaJIT's numeric literal `1LL` is EmmyLua Analyzer syntax (--emmylua-syntax)",
        "main.lua:1:38: error[LUA1001]: LuaJIT's numeric literal `0b101` is EmmyLua Analyzer syntax (--emmylua-syntax)",
        "main.lua:4:1: error[LUA1001]: Lua 5.5's `global` declaration is EmmyLua Analyzer syntax (--emmylua-syntax)",
    ] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn a_cast_without_a_name_needs_a_value_after_it() {
    assert_errors(&emmylua("local function f()\n    ---@cast -?\nend\n"), &["main.lua:2:8: error[LUA1001]: @cast without a name requires a value after it to cast"]);
}

#[test]
fn varargs_outside_a_variadic_function_are_reported_once() {
    assert_errors(&plain("local function f() return ... end\n"), &["main.lua:1:27: error[LUA1001]: '...' is outside a variadic Lua function"]);
}
