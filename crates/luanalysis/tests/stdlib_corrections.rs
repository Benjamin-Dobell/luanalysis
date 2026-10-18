use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(declarations: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis stdlib corrections {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("api.def.lua"), declarations).unwrap();
        let fixture = Self(path);
        fixture.source(source);
        fixture
    }
    fn source(&self, source: &str) {
        fs::write(self.0.join("main.lua"), source).unwrap();
    }
    fn check(&self) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis"));
        command.current_dir(&self.0).args(["check", "--no-stdlib", "--dump-types", "--library", "api.def.lua"]);
        for name in ["math.def.lua", "table.def.lua"] {
            if self.0.join(name).exists() { command.args(["--library", name]); }
        }
        command.arg("main.lua").output().unwrap()
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
fn fail(output: &Output, code: &str) {
    assert_eq!(output.status.code(), Some(1), "{}", text(output));
    assert!(text(output).contains(code), "{}", text(output));
}
const GLOBAL: &str = include_str!("../stdlib/Lua54/global.def.lua");
const MATH: &str = include_str!("../stdlib/Lua54/math.def.lua");
const TABLE: &str = include_str!("../stdlib/Lua54/table.def.lua");
fn excerpt<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start = source.find(start).unwrap();
    let end = start + source[start..].find(end).unwrap() + end.len();
    &source[start..end]
}
#[test]
fn corrected_original_xpcall_names_its_real_handler_and_checks_the_original_argument() {
    let original = excerpt(
        GLOBAL,
        "---@overload fun(f: (fun: void), msgh:",
        "function xpcall(f, msgh, ...) end",
    );
    let fixture = Fixture::new(
        original,
        "local ok,value=xpcall(function() return 42 end,function(message) return message end)\n",
    );
    pass(&fixture.check());
    fixture.source("local rejected=xpcall(function() return 42 end,123)\n");
    fail(&fixture.check(), "TS2345");
    fixture.source(
        "local accepted=xpcall(function() return 42 end,function(message) return message end)\n",
    );
    pass(&fixture.check());
}
#[test]
fn corrected_original_error_contract_terminates_both_public_arities_without_body_inference() {
    let original = excerpt(
        GLOBAL,
        "---@param message any\n---@param level? number",
        "function error(message, level) end",
    );
    for call in ["error({message='missing'})", "error('missing',0)"] {
        let source = format!(
            "---@param value number|nil\nlocal function consume(value)\nif value==nil then {call} end\n---@type number\nlocal checked=value\nend\n"
        );
        let fixture = Fixture::new(original, &source);
        pass(&fixture.check());
        fixture.source("error('missing','wrong')\n");
        fail(&fixture.check(), "TS2345");
        fixture.source(&source);
        pass(&fixture.check());
    }
}
#[test]
fn the_fundamental_assert_forwards_its_real_extra_arguments() {
    // `assert` is always the standard one, even without the standard library: a host's
    // library cannot replace it.
    let fixture = Fixture::new("", "---@type number\nlocal first=assert(42,'label',false)\n");
    pass(&fixture.check());
    fixture.source("---@type number,string\nlocal first,second=assert(42,'label',false)\n");
    pass(&fixture.check());
    fixture.source("---@type number,number\nlocal first,second=assert(42,'label',false)\n");
    fail(&fixture.check(), "TS2322");
    let original = excerpt(
        GLOBAL,
        "---@overload fun<T>(v: T, ...: unknown): T",
        "function assert(v, message) end",
    );
    let fixture = Fixture::new(original, "local first=assert(42)\n");
    fail(&fixture.check(), "LUA2039");
}
#[test]
fn minimally_corrected_original_math_and_table_resources_keep_actual_result_contracts() {
    let declarations = excerpt(GLOBAL, "---@param message any\n---@param level? number", "function error(message, level) end");
    let fixture = Fixture::new(
        declarations,
        "local a,b=math.modf(1.5)\n---@type number\nlocal fraction=b\nlocal x,y=math.randomseed()\n---@type number\nlocal seed=y\n---@type number|nil\nlocal integer=math.tointeger({})\n---@type 'integer'|'float'|nil\nlocal category=math.type({})\nlocal values={1,2}\ntable.insert(values,1,3)\nlocal removed=table.remove(values)\nif removed==nil then error('empty') end\n---@type number\nlocal checked=removed\ntable.sort(values)\n",
    );
    fs::write(fixture.0.join("math.def.lua"), MATH).unwrap();
    fs::write(fixture.0.join("table.def.lua"), TABLE).unwrap();
    pass(&fixture.check());
    for (source, code) in [
        ("---@type number\nlocal wrong=table.sort({1,2})\n", "TS2322"),
        (
            "---@type number\nlocal wrong=table.remove({1,2})\n",
            "TS2322",
        ),
        ("local wrong=math.max(1,'wrong')\n", "TS2345"),
        ("local wrong=math.min(1,'wrong')\n", "TS2345"),
    ] {
        fixture.source(source);
        fail(&fixture.check(), code);
    }
    fixture.source("local value=math.max(1,2)\ntable.sort({1,2})\n");
    pass(&fixture.check());
}
#[test]
fn original_iterator_controls_and_version_keep_their_exact_literal_contracts() {
    let declarations = format!(
        "{}\n{}\n{}\n",
        excerpt(
            GLOBAL,
            "---@generic V\n---@param t V[]",
            "function ipairs(t) end"
        ),
        excerpt(
            GLOBAL,
            "---@generic K, V\n---@param t table<K, V>",
            "function pairs(t) end"
        ),
        excerpt(GLOBAL, "---@type \"Lua 5.4\"", "_VERSION = \"Lua 5.4\"")
    );
    let fixture = Fixture::new(
        &declarations,
        "---@type 'Lua 5.4'\nlocal version=_VERSION\n---@type unknown,unknown,0\nlocal iterator,state,initial=ipairs({1,2})\n---@type 0\nlocal zero=initial\n---@param values table<boolean,string>\nlocal function visit(values)\nlocal iterator,state,initial=pairs(values)\n---@type nil\nlocal control=initial\nend\n",
    );
    pass(&fixture.check());
    fixture.source("---@type unknown,unknown,0\nlocal iterator,state,initial=ipairs({1})\n---@type 1\nlocal rejected=initial\n");
    fail(&fixture.check(), "TS2322");
    fixture.source("---@type unknown,unknown,0\nlocal iterator,state,initial=ipairs({1})\n---@type 0\nlocal corrected=initial\n");
    pass(&fixture.check());
}
