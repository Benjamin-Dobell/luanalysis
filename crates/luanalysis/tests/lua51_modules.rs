//! Lua 5.1's `module(name, ...)` and Luanalysis v1's `---@module Name`: after a top-level
//! `module("X")`, the chunk's global environment is the global table `X`. The names it assigns
//! are that table's fields, not globals, and the table's type is the class `X`, or the one
//! `---@module` names, which other files see the fields through. `module("a.b")` stores the table
//! in the field `b` of the global `a`, and `module(...)` is named by the `require` that loads it.
use std::{fs, path::PathBuf, process::{Command, Output}, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf, Vec<String>);
impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("luanalysis lua51 modules {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        for (path, source) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source).unwrap();
        }
        Self(root, files.iter().map(|(path, _)| (*path).to_owned()).collect())
    }
    fn check(&self, version: &str) -> String {
        let output: Output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&self.0)
            .args(["check", "--lua-version", version, "--module-root", ".", "--dump-types"]).args(&self.1).output().unwrap();
        format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn errors(report: &str) -> Vec<&str> {
    let mut errors: Vec<&str> = report.lines().filter(|line| line.contains(": error[")).collect();
    errors.sort_unstable();
    errors
}

const MODULE_A: &str = "---@module TypeA\nmodule(\"TypeA\")\n\nname = \"a\"\n\nfunction greet() return name end\n";

#[test]
fn a_modules_names_are_its_tables_fields_and_not_globals() {
    let fixture = Fixture::new(&[
        ("moduleA.lua", MODULE_A),
        ("B.lua", "---@type TypeA\nlocal a\n---@type number\nlocal n = a.name\nlocal greeting = a.greet()\nlocal global = TypeA.name\nprint(name, greet)\n"),
    ]);
    let report = fixture.check("5.1");
    assert_eq!(errors(&report), [
        "B.lua:4:11: error[TS2322]: Type 'string' is not assignable to type 'number'.",
        "B.lua:7:13: error[TS2304]: Cannot find name 'greet'.",
        "B.lua:7:7: error[TS2304]: Cannot find name 'name'.",
    ], "{report}");
    for expected in ["B.lua:5:7: greeting: string", "B.lua:6:7: global: string"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn the_chunk_reads_its_tables_fields_and_the_globals() {
    let module = "local tostring = tostring\n\
        before = 1\n\
        ---@module Counter\n\
        ---@field label string\n\
        module(\"Counter\", package.seeall)\n\
        \n\
        count = 0\n\
        \n\
        ---@param by number\n\
        function increment(by)\n\
        \x20   count = count + by\n\
        \x20   local text = tostring(count) .. label\n\
        \x20   local itself, named, package = _M, _NAME, _PACKAGE\n\
        \x20   local name = os.time()\n\
        \x20   return count\n\
        end\n\
        \n\
        function reset()\n\
        \x20   count = \"zero\"\n\
        \x20   resets = 1\n\
        end\n";
    let fixture = Fixture::new(&[
        ("counter.lua", module),
        ("user.lua", "---@type boolean\nlocal resets = Counter.resets\nlocal b = before\nprint(Counter.before)\n"),
    ]);
    let report = fixture.check("5.1");
    assert_eq!(errors(&report), [
        "counter.lua:19:13: error[TS2322]: Type '\"zero\"' is not assignable to type 'number'.",
        "user.lua:2:16: error[TS2322]: Type 'number' is not assignable to type 'boolean'.",
        "user.lua:4:15: error[TS2339]: Property 'before' does not exist on type 'Counter'.",
    ], "{report}");
    for expected in [
        "counter.lua:12:11: text: string", "counter.lua:13:11: itself: Counter", "counter.lua:13:19: named: string",
        "counter.lua:13:26: package: string", "counter.lua:14:11: name: number", "user.lua:3:7: b: number",
    ] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn require_returns_the_table_by_the_name_module_gives_it() {
    let fixture = Fixture::new(&[
        ("Counter.lua", "module(\"Counter\")\ncount = 0\n"),
        // Loaded by another name, `package.loaded` holds no table for it, and `require` returns true.
        ("other.lua", "module(\"Elsewhere\")\nvalue = 1\n"),
        // A chunk's own result is what `require` returns.
        ("Returned.lua", "module(\"Returned\")\nvalue = 1\nreturn \"text\"\n"),
        ("user.lua", "local counter = require(\"Counter\")\nlocal other = require(\"other\")\nlocal returned = require(\"Returned\")\n"),
    ]);
    let report = fixture.check("5.1");
    assert!(errors(&report).is_empty(), "{report}");
    for expected in ["user.lua:1:7: counter: Counter", "user.lua:2:7: other: boolean", "user.lua:3:7: returned: string"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn only_lua_51s_module_function_with_a_name_makes_a_table() {
    let fixture = Fixture::new(&[
        // Another function named `module`.
        ("local_function.lua", "local function module(name) end\nmodule(\"Local\")\nfromLocal = 1\n"),
        // A local named as the table would take the names meant for the table.
        ("local_table.lua", "local Shadowed = {}\nmodule(\"Shadowed\")\nfromShadowed = 1\n"),
        // A name `module` cannot store as a path of tables.
        ("unnamed.lua", "module(\"not a name\")\nfromUnnamed = 1\n"),
        // A local named as a dotted name's root would take its table's declaration.
        ("local_root.lua", "local root = {}\nmodule(\"root.leaf\")\nfromRoot = 1\n"),
        // Inside a block, it is no statement of the chunk's own.
        ("nested.lua", "do module(\"Nested\") end\nfromNested = 1\n"),
        ("user.lua", "local values = { fromLocal, fromShadowed, fromUnnamed, fromRoot, fromNested }\n"),
    ]);
    let report = fixture.check("5.1");
    assert!(errors(&report).is_empty(), "{report}");
}

#[test]
fn each_module_call_starts_its_tables_names() {
    let fixture = Fixture::new(&[
        ("two.lua", "module(\"First\")\nfirst = 1\nmodule(\"Second\")\nsecond = \"s\"\nfunction both() return first, second end\n"),
        ("user.lua", "local a = First.first\nlocal b = Second.second\nlocal c = First.second\n"),
    ]);
    let report = fixture.check("5.1");
    assert_eq!(errors(&report), [
        "two.lua:5:24: error[TS2304]: Cannot find name 'first'.",
        "user.lua:3:17: error[TS2339]: Property 'second' does not exist on type 'First'.",
    ], "{report}");
    assert!(report.contains("user.lua:1:7: a: number") && report.contains("user.lua:2:7: b: string"), "{report}");
}

#[test]
fn v1_reads_a_module_chunk_as_one_at_any_lua_version() {
    // Lua 5.3 has no `module`, which the call reports; the rest of the chunk is still read as
    // the module it declares, as v1 read it.
    let fixture = Fixture::new(&[
        ("moduleA.lua", MODULE_A),
        ("B.lua", "---@type TypeA\nlocal a\nlocal n = a.name\nprint(name)\n"),
    ]);
    let report = fixture.check("5.3");
    assert_eq!(errors(&report), [
        "B.lua:4:7: error[TS2304]: Cannot find name 'name'.",
        "moduleA.lua:2:1: error[TS2304]: Cannot find name 'module'.",
    ], "{report}");
    assert!(report.contains("B.lua:3:7: n: string"), "{report}");
}

#[test]
fn a_module_annotation_without_a_module_call_declares_a_class() {
    let fixture = Fixture::new(&[
        ("settings.lua", "---@module Settings\n---@field path string\nlocal Settings = {}\nSettings.path = \"/\"\nreturn Settings\n"),
    ]);
    let report = fixture.check("5.1");
    assert!(errors(&report).is_empty(), "{report}");
    assert!(report.contains("settings.lua:3:7: Settings: def Settings"), "{report}");
}

#[test]
fn a_dotted_names_table_is_a_field_of_the_tables_it_is_in() {
    // LuaSocket's: `socket.http` is the field `http` of the table `socket`, whose own chunk is
    // another file's, and whose other fields are other modules'.
    let fixture = Fixture::new(&[
        ("socket.lua", "module(\"socket\")\nversion = \"2.0\"\nfunction connect(host) return 1 end\n"),
        ("socket/http.lua", "module(\"socket.http\")\nTIMEOUT = 60\nfunction request(url) return \"body\", TIMEOUT end\n\
            local package, name, itself = _PACKAGE, _NAME, _M\n"),
        ("socket/url.lua", "module(\"socket.url\")\nfunction escape(s) return s end\n"),
        ("user.lua", "local http = require(\"socket.http\")\nlocal version = socket.version\n\
            local body, timeout = socket.http.request(\"x\")\n---@type socket.url\nlocal url\nlocal escape = url.escape\n\
            local same = http.TIMEOUT\nlocal missing = socket.ftp\nprint(request, TIMEOUT)\n"),
    ]);
    let report = fixture.check("5.1");
    assert_eq!(errors(&report), [
        "user.lua:8:24: error[TS2339]: Property 'ftp' does not exist on type 'socket'.",
        "user.lua:9:16: error[TS2304]: Cannot find name 'TIMEOUT'.",
        "user.lua:9:7: error[TS2304]: Cannot find name 'request'.",
    ], "{report}");
    for expected in [
        "socket/http.lua:4:7: package: string", "socket/http.lua:4:16: name: string", "socket/http.lua:4:22: itself: socket.http",
        "user.lua:1:7: http: socket.http", "user.lua:2:7: version: string", "user.lua:3:7: body: string",
        "user.lua:3:13: timeout: number", "user.lua:6:7: escape: fun(s?: any): any", "user.lua:7:7: same: number",
    ] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn the_tables_a_dotted_name_is_in_need_no_module_of_their_own() {
    // `module` makes the tables `a` and `a.b` when there are none; `_PACKAGE` is the prefix.
    let fixture = Fixture::new(&[
        ("a/b/c.lua", "module(\"a.b.c\", package.seeall)\ndeep = true\nlocal package = _PACKAGE\n"),
        ("a/b/d.lua", "module(\"a.b.d\")\nother = 1\n"),
        ("user.lua", "local deep = a.b.c.deep\nlocal other = a.b.d.other\n---@type a.b\nlocal b\nlocal c = b.c\n"),
    ]);
    let report = fixture.check("5.1");
    assert!(errors(&report).is_empty(), "{report}");
    for expected in ["user.lua:1:7: deep: boolean", "user.lua:2:7: other: number", "user.lua:5:7: c: a.b.c"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn a_vararg_module_is_named_by_the_require_that_loads_it() {
    // `module(...)` gets its name from `require`, which passes it to the chunk as `...`.
    let fixture = Fixture::new(&[
        ("socket/url.lua", "module(..., package.seeall)\nfunction escape(s) return s end\nlocal name = ...\nlocal named = _NAME\n"),
        ("counter.lua", "module(...)\ncount = 0\nfunction bump() count = count + 1 return count end\n"),
        ("user.lua", "local url = require(\"socket.url\")\nrequire(\"counter\")\nlocal escaped = socket.url.escape\n\
            local count = counter.count\nlocal bumped = counter.bump()\nprint(bump, escape)\n"),
    ]);
    let report = fixture.check("5.1");
    assert_eq!(errors(&report), [
        "user.lua:6:13: error[TS2304]: Cannot find name 'escape'.",
        "user.lua:6:7: error[TS2304]: Cannot find name 'bump'.",
    ], "{report}");
    for expected in [
        "socket/url.lua:3:7: name: string", "socket/url.lua:4:7: named: string", "user.lua:1:7: url: socket.url",
        "user.lua:3:7: escaped: fun(s?: any): any", "user.lua:4:7: count: number", "user.lua:5:7: bumped: number",
    ] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn an_unrequired_vararg_modules_table_is_anonymous() {
    // No `require` names it, so no global holds its table; its chunk still reads its own fields.
    let fixture = Fixture::new(&[
        ("lone.lua", "module(..., package.seeall)\ncount = 0\nfunction reset() count = 0 end\nlocal n = count + 1\nlocal itself = _M.count\n"),
        ("user.lua", "local value = count\nlocal module = lone\n"),
    ]);
    let report = fixture.check("5.1");
    assert_eq!(errors(&report), [
        "user.lua:1:15: error[TS2304]: Cannot find name 'count'.",
        "user.lua:2:16: error[TS2304]: Cannot find name 'lone'.",
    ], "{report}");
    assert!(report.contains("lone.lua:4:7: n: number") && report.contains("lone.lua:5:7: itself: number"), "{report}");
}

#[test]
fn require_returns_a_vararg_modules_table_by_each_of_its_names() {
    // `pkg/init.lua` is both `pkg` and `pkg.init`: the fewer parts name its table.
    let fixture = Fixture::new(&[
        ("pkg/init.lua", "module(...)\nvalue = 1\n"),
        ("user.lua", "local short = require(\"pkg\")\nlocal long = require(\"pkg.init\")\nlocal value = pkg.value\n"),
    ]);
    let report = fixture.check("5.1");
    assert!(errors(&report).is_empty(), "{report}");
    for expected in ["user.lua:1:7: short: pkg", "user.lua:2:7: long: pkg", "user.lua:3:7: value: number"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn a_vararg_modules_annotation_names_its_tables_type() {
    // As v1 read `---@module Name` above a chunk it did not name.
    let fixture = Fixture::new(&[
        ("a/x.lua", "---@module Ex\nmodule(...)\nvalue = 1\n"),
        ("user.lua", "local x = require(\"a.x\")\nlocal value = a.x.value\n---@type Ex\nlocal ex\nlocal same = ex.value\n"),
    ]);
    let report = fixture.check("5.1");
    assert!(errors(&report).is_empty(), "{report}");
    for expected in ["user.lua:1:7: x: Ex", "user.lua:2:7: value: number", "user.lua:5:7: same: number"] {
        assert!(report.contains(expected), "{expected}\n{report}");
    }
}

#[test]
fn a_chunks_varargs_are_what_require_passes_it() {
    // Lua 5.1's `require` passes the module's name; from 5.2 it also passes the file it found.
    let fixture = Fixture::new(&[("loaded.lua", "local name, path = ...\n")]);
    let report = fixture.check("5.1");
    assert!(errors(&report).is_empty(), "{report}");
    assert!(report.contains("loaded.lua:1:7: name: string") && report.contains("loaded.lua:1:13: path: nil"), "{report}");
    let report = fixture.check("5.4");
    assert!(errors(&report).is_empty(), "{report}");
    assert!(report.contains("loaded.lua:1:7: name: string") && report.contains("loaded.lua:1:13: path: string"), "{report}");
}
