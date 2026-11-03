//! LuaJIT 2.1's standard environment (`--lua-version LuaJIT`, or `.luarc.json`'s "LuaJIT"): Lua
//! 5.1's library with LuaJIT's extensions, its `bit`, `jit`, `ffi` and `string.buffer` libraries,
//! C data as a runtime type, its numerals, and the standard libraries `require` returns in every
//! Lua version.
use std::{fs, path::PathBuf, process::{Command, Output}, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);
impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("luanalysis luajit {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        for (path, source) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source).unwrap();
        }
        Self(root)
    }
    fn check(&self, args: &[&str]) -> String {
        let output: Output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&self.0).arg("check").args(args).output().unwrap();
        format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn luajit(source: &str) -> String {
    Fixture::new(&[("main.lua", source)]).check(&["--lua-version", "LuaJIT", "--dump-types", "main.lua"])
}

fn errors(report: &str) -> Vec<&str> {
    report.lines().filter(|line| line.contains(": error[")).collect()
}

#[test]
fn require_returns_luajits_libraries() {
    let report = luajit("local ffi = require('ffi')\nlocal bit = require('bit')\nlocal jit = require('jit')\nlocal opt = require('jit.opt')\nlocal profile = require('jit.profile')\nlocal util = require('jit.util')\nlocal buffer = require('string.buffer')\nlocal new = require('table.new')\nlocal clear = require('table.clear')\n");
    assert!(errors(&report).is_empty(), "{report}");
    for expected in ["ffi: ffilib", "bit: bitlib", "jit: jitlib", "opt: jit.optlib", "profile: jit.profilelib", "util: jit.utillib",
        "buffer: string.bufferlib", "new: fun(narray: number, nhash: number): table", "clear: fun(tab: table): void"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn the_ffi_library_types_its_calls_and_c_data_is_any_inside() {
    let report = luajit("local ffi = require('ffi')\nffi.cdef[[\ntypedef struct { int x; double y[4]; } point;\nint printf(const char *fmt, ...);\n]]\nlocal point_t = ffi.typeof('point')\nlocal p = point_t(1)\nlocal q = ffi.new('point', { x = 2 })\nlocal sum = p.x + q.y[0] * 2\nq.x = 3\nffi.C.printf('%d\\n', p.x)\nlocal size = ffi.sizeof(p)\nlocal text = ffi.string(ffi.cast('const char *', 'abc'), 2)\nlocal libc = ffi.load('c')\nlocal callback = ffi.cast('void (*)(int)', function(value) end)\ncallback:free()\nlocal wide = p + 1\nlocal null = ffi.cast('void *', 0) == nil\nffi.new(1)\n");
    for expected in ["p: cdata", "q: cdata", "size: nil | number", "text: string", "libc: ffi.namespace*", "callback: cdata"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    // A C type is a number, a C declaration or C data.
    assert_eq!(errors(&report), ["main.lua:19:9: error[TS2345]: Argument of type '1' is not assignable to parameter of type 'string | cdata | ffi.ctype*'."], "{report}");
}

#[test]
fn type_narrows_to_and_from_c_data() {
    let report = luajit("---@param v number | ffi.cdata*\nlocal function plus(v)\n    if type(v) == 'cdata' then return tonumber(v) end\n    return v + 1\nend\n---@param w string | cdata | userdata\nlocal function name(w)\n    if type(w) == 'userdata' then return 'userdata' end\n    if type(w) ~= 'cdata' then return w:upper() end\n    local c = w\nend\n---@param u unknown\nlocal function found(u)\n    if type(u) == 'cdata' then local d = u end\nend\n");
    assert!(errors(&report).is_empty(), "{report}");
    assert!(report.contains("main.lua:10:11: c: cdata"), "{report}");
    assert!(report.contains("main.lua:14:38: d: cdata"), "{report}");
}

#[test]
fn bit_jit_and_string_buffers_are_declared() {
    let report = luajit("local x = bit.band(0xff, 0x0f, 3)\nlocal hex = bit.tohex(bit.bswap(x), -4)\nlocal on, feature = jit.status()\njit.off(true, true)\njit.on()\njit.flush(3)\njit.opt.start(2, '-dce', 'hotloop=10')\nlocal os_name = jit.os\nlocal buffer = require('string.buffer')\nlocal buf = buffer.new(64, { dict = { 'a' } })\nbuf:put('a', 1):putf('%d', 2):encode({ 1 })\nlocal all = buf:get()\nlocal length = #buf\nlocal joined = buf .. 'x'\nlocal encoded = buffer.encode({ a = 1 })\nlocal decoded = buffer.decode(encoded)\nbit.band('x')\n");
    for expected in ["x: number", "hex: string", "on: boolean", "feature: nil | string", "os_name: \"BSD\" | \"Linux\" | \"OSX\" | \"Other\" | \"POSIX\" | \"Windows\"",
        "buf: string.buffer", "all: string", "length: number", "joined: string", "decoded: any"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
    assert_eq!(errors(&report), ["main.lua:17:10: error[TS2345]: Argument of type '\"x\"' is not assignable to parameter of type 'number'."], "{report}");
}

#[test]
fn lua51_functions_have_luajits_extensions() {
    let report = luajit("local ok, sum = xpcall(function(a, b) return a + b end, debug.traceback, 1, 2)\nlocal chunk = load('return 1', 'chunk', 't', {})\nlocal same = loadstring(function() return nil end)\nlocal file = loadfile('x.lua', 'bt')\nlocal defaults = loadstring('return 1', nil, nil, {}) or loadfile(nil, nil, {})\nlocal rep = string.rep('a', 3, ',')\nlocal dumped = string.dump(function() end, true)\nlocal base2 = math.log(8, 2)\nlocal moved = table.move({ 1 }, 1, 1, 2)\nlocal yieldable = coroutine.isyieldable()\nlocal running = coroutine.running()\nlocal running_gc = collectgarbage('isrunning')\nlocal found = package.searchpath('x', './?.lua')\nlocal what = debug.getinfo(1).what == 'Lua'\nlocal id = debug.upvalueid(print, 1)\nlocal line = io.read('l')\nfor first, second in io.lines('x.txt', 'n', '*l') do end\nlocal kind = type(1) == 'cdata'\nlocal number = tonumber(1LL)\nos.exit(true, true)\n");
    assert!(errors(&report).is_empty(), "{report}");
    for expected in ["sum: any", "running: nil | thread", "running_gc: boolean", "found: nil | string", "line: nil | string", "number: nil | number"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn what_luajits_default_build_leaves_out_is_not_declared() {
    // `table.pack`, `table.unpack` and `rawlen` need LUAJIT_ENABLE_LUA52COMPAT; `utf8`, `bit32` and
    // Lua 5.3's operators are no LuaJIT's.
    let report = luajit("local a = table.unpack({ 1 })\nlocal b = table.pack(1)\nlocal c = rawlen({})\nlocal d = utf8.char(65)\nlocal e = bit32.band(1, 2)\n");
    assert_eq!(errors(&report), [
        "main.lua:1:17: error[TS2339]: Property 'unpack' does not exist on type 'std__Table'.",
        "main.lua:2:17: error[TS2339]: Property 'pack' does not exist on type 'std__Table'.",
        "main.lua:3:11: error[TS2304]: Cannot find name 'rawlen'.",
        "main.lua:4:11: error[TS2304]: Cannot find name 'utf8'.",
        "main.lua:5:11: error[TS2304]: Cannot find name 'bit32'.",
    ], "{report}");
    let operator = luajit("local value = 1 & 2\n");
    assert!(operator.contains("error[LUA1001]: The binary operator '&' only available in Lua 5.3 and above"), "{operator}");
}

#[test]
fn luajits_numerals_are_read_only_for_luajit() {
    let source = "local a, b, c, d = 1LL, 0x10ULL, 0b101, 3i\n";
    let report = luajit(source);
    assert!(errors(&report).is_empty(), "{report}");
    assert!(report.contains("main.lua:1:7: a: number"), "{report}");
    let other = Fixture::new(&[("main.lua", source)]).check(&["--lua-version", "5.1", "main.lua"]);
    assert!(other.contains("main.lua:1:20: error[LUA1001]: LuaJIT's numeric literal `1LL` is read with --lua-version LuaJIT or --emmylua-syntax"), "{other}");
}

#[test]
fn luarc_and_command_line_select_luajit() {
    let fixture = Fixture::new(&[(".luarc.json", "{ \"runtime\": { \"version\": \"LuaJIT\" } }"), ("main.lua", "local ffi = require('ffi')\nlocal x = bit.bor(1, 2)\n")]);
    let report = fixture.check(&["."]);
    assert!(report.contains("No type errors"), "{report}");
    let lower = fixture.check(&["--lua-version", "luajit", "main.lua"]);
    assert!(lower.contains("No type errors"), "{lower}");
    let wrong = fixture.check(&["--lua-version", "jit", "main.lua"]);
    assert!(wrong.contains("supported versions: 5.0, 5.1, 5.2, 5.3, 5.4, LuaJIT"), "{wrong}");
}

#[test]
fn an_allow_list_names_luajits_libraries_and_their_members() {
    let fixture = Fixture::new(&[("main.lua", "local ffi = require('ffi')\nlocal p = ffi.new('int')\nffi.cast('int', 1)\nlocal x = bit.band(1, 2)\n")]);
    let only_new = fixture.check(&["--lua-version", "LuaJIT", "--stdlib", "basic,package,bit,ffi.new", "main.lua"]);
    assert_eq!(errors(&only_new), ["main.lua:3:5: error[TS2339]: Property 'cast' does not exist on type 'ffilib'."], "{only_new}");
    let without = fixture.check(&["--lua-version", "LuaJIT", "--stdlib", "basic,package", "main.lua"]);
    assert!(without.contains("main.lua:1:21: error[TS2307]"), "{without}");
    assert!(without.contains("main.lua:4:11: error[TS2304]: Cannot find name 'bit'."), "{without}");
    let unknown = fixture.check(&["--lua-version", "LuaJIT", "--stdlib", "basic,ffi.nope", "main.lua"]);
    assert!(unknown.contains("--stdlib: the LuaJIT standard library has no library or member ffi.nope"), "{unknown}");
}

#[test]
fn require_returns_each_versions_standard_libraries() {
    let fixture = Fixture::new(&[("main.lua", "local s = require('string')\nlocal t = require('table')\nlocal b = require('bit32')\nlocal u = require('utf8')\nlocal g = require('_G')\n")]);
    let lua52 = fixture.check(&["--lua-version", "5.2", "--dump-types", "main.lua"]);
    assert!(lua52.contains("main.lua:1:7: s: string"), "{lua52}");
    assert!(lua52.contains("main.lua:2:7: t: std__Table"), "{lua52}");
    assert_eq!(errors(&lua52), ["main.lua:4:19: error[TS2307]: module \"utf8\" was not found in 4 candidate paths"], "{lua52}");
    let lua54 = fixture.check(&["--lua-version", "5.4", "main.lua"]);
    assert_eq!(errors(&lua54), ["main.lua:3:19: error[TS2307]: module \"bit32\" was not found in 4 candidate paths"], "{lua54}");
    // A file on the module path does not replace a standard library, as `package.loaded` holds it.
    let shadowed = Fixture::new(&[("string.lua", "return 1\n"), ("main.lua", "local s = require('string')\nlocal upper = s.upper('a')\n")]);
    let report = shadowed.check(&["--lua-version", "5.4", "--dump-types", "main.lua"]);
    assert!(report.contains("main.lua:2:7: upper: string"), "{report}");
}

#[test]
fn a_type_name_may_end_in_a_star() {
    let report = Fixture::new(&[("main.lua", "---@class ffi.thing*\n---@field x number\n\n---@param thing ffi.thing*\nlocal function f(thing) return thing.x end\n---@type file*\nlocal handle\n")])
        .check(&["--lua-version", "5.4", "--dump-types", "main.lua"]);
    assert!(errors(&report).is_empty(), "{report}");
    assert!(report.contains("main.lua:5:16: f: fun(thing: ffi.thing*): number"), "{report}");
    assert!(report.contains("main.lua:7:7: handle: file"), "{report}");
}
