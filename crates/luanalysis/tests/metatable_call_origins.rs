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
            "luanalysis metatable call origins {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, source: &str) {
        fs::write(self.0.join(name), source).unwrap();
    }
    fn check(&self, source: &str) -> Output {
        self.file("main.lua", source);
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args([
                "check",
                "--lua-version",
                "5.4",
                "--module-root",
                ".",
                "--dump-types",
                "main.lua",
            ])
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn clean(output: &Output) {
    assert!(output.status.success(), "{}", text(output));
}
fn no_owner(output: &Output) {
    assert!(!output.status.success(), "{}", text(output));
    assert!(text(output).contains("Property 'read'"), "{}", text(output));
}

const FACTORY: &str = r#"---@class Instance

---@class StaticInstance
---@overload fun(): Instance
local Static = {}
setmetatable(Static, {
    ---@return Instance
    __call = function()
        local self = --[[---@type self]] {}
        ---@return number
        function self.read() return 1 end
        return self
    end,
})
---@type number
local result = Static().read()
"#;
const ORDINARY: &str = r#"---@generic T
---@param value T
---@param meta nil | table
---@return T
local function ordinary(value, meta) return value end
"#;

#[test]
fn exact_transitive_local_calls_preserve_standard_origin_but_real_writes_do_not() {
    let fixture = Fixture::new();
    let helpers =
        "local function leaf() return 1 end\nlocal function helper() leaf() end\nhelper()\n";
    clean(&fixture.check(&format!("{helpers}{FACTORY}")));
    let writes = format!(
        "{ORDINARY}local function leaf() setmetatable=ordinary end\nlocal function helper() leaf() end\nhelper()\n"
    );
    no_owner(&fixture.check(&format!("{writes}{FACTORY}")));
    let corrected = format!(
        "local attach=setmetatable\n{writes}{}",
        FACTORY.replace("setmetatable(Static", "attach(Static")
    );
    clean(&fixture.check(&corrected));
}

#[test]
fn original_required_module_execution_preserves_only_unchanged_standard_values() {
    let fixture = Fixture::new();
    fixture.file(
        "leaf.lua",
        "local function leaf() return 1 end\nleaf()\nreturn {}\n",
    );
    fixture.file("helper.lua", "require('leaf')\nreturn {}\n");
    let source = format!("require('helper')\n{FACTORY}");
    clean(&fixture.check(&source));
    fixture.file(
        "leaf.lua",
        &format!("{ORDINARY}setmetatable=ordinary\nreturn {{}}\n"),
    );
    no_owner(&fixture.check(&source));
    let corrected = format!(
        "local attach=setmetatable\nrequire('helper')\n{}",
        FACTORY.replace("setmetatable(Static", "attach(Static")
    );
    clean(&fixture.check(&corrected));
}

#[test]
fn loader_mutations_inside_known_calls_prevent_source_imports_from_certifying_execution() {
    let fixture = Fixture::new();
    fixture.file("helper.lua", "return {}\n");
    let source = format!(
        "local function replace_loader() package.path='custom/?.lua' end\nreplace_loader()\nrequire('helper')\n{FACTORY}"
    );
    no_owner(&fixture.check(&source));
    clean(&fixture.check(&format!("require('helper')\n{FACTORY}")));
}

#[test]
fn written_member_contracts_do_not_require_a_constructor_origin_certificate() {
    let fixture = Fixture::new();
    let source = r#"---@function opaque()

---@class Instance
---@field value number
---@function read(): number

---@class StaticInstance
---@overload fun(): Instance
local Static = {}
opaque()
setmetatable(Static, {
    ---@return Instance
    __call = function()
        local instance = --[[---@type Instance]] {value=1, read=function() return 1 end}
        return instance
    end,
})
local instance = Static()
---@type number
local field = instance.value
---@type number
local result = instance.read()
"#;
    clean(&fixture.check(source));
    let wrong = source.replace("local result = instance.read()", "local result = 'wrong'");
    let output = fixture.check(&wrong);
    assert!(text(&output).contains("TS2322"), "{}", text(&output));
    clean(&fixture.check(source));
}
