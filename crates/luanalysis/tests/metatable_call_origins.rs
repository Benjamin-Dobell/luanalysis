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

#[test]
fn a_copied_member_cannot_recover_standard_identity_across_an_implicit_getter() {
    let fixture = Fixture::new();
    let prefix = r#"---@shape Holder
---@field attach fun<T>(value:T, meta:nil|table):T

---@type Holder
local holder

---@shape Probe
---@field missing number | nil

local trigger = --[[---@type Probe]] setmetatable({}, {
    ---@param ignored table
    ---@param key string
    ---@return number
    __index=function(ignored, key) holder.attach=ordinary; return 1 end,
})
holder={attach=setmetatable}
local observed=trigger.missing
local alias=holder

"#;
    let source = format!(
        "{ORDINARY}{prefix}{}",
        FACTORY.replace("setmetatable(Static", "alias.attach(Static")
    );
    no_owner(&fixture.check(&source));
    let corrected = source.replace("local observed=trigger.missing", "local observed=1");
    clean(&fixture.check(&corrected));
}

#[test]
fn local_namespace_initialization_preserves_require_but_real_newindex_effects_do_not() {
    let fixture = Fixture::new();
    fixture.file("Helper.lua", "return {value=1}\n");
    let source = "local Class=require('Class')\nlocal Helper=require('Helper')\n---@type number\nlocal result=Class.identity(Helper.value)\n";
    let namespace = "---@class UserClass\nlocal Class={}\n\n---@param value number\n---@return number\nfunction Class.identity(value) return value end\n\nreturn Class\n";
    let changing_loader = namespace.replace(
        "local Class={}\n",
        "local Class={}\n\nsetmetatable(Class, {\n---@param target table\n---@param key string\n---@param value function\n__newindex=function(target,key,value) package.path='custom/?.lua' end,\n})\n",
    );
    fixture.file("Class.lua", &changing_loader);
    let denied = fixture.check(source);
    assert!(text(&denied).contains("LUA2036"), "{}", text(&denied));
    fixture.file("Class.lua", namespace);
    clean(&fixture.check(source));
}
