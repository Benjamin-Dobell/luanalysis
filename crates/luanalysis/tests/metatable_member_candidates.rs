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
            "luanalysis metatable member candidates {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn check(&self, source: &str) -> Output {
        fs::write(self.0.join("main.lua"), source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .current_dir(&self.0)
            .args(["check", "--lua-version", "5.4", "--dump-types", "main.lua"])
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

fn result_type(output: &Output, expected: &str) {
    let output = text(output);
    assert!(
        output
            .lines()
            .any(|line| line.ends_with(&format!(": result: {expected}"))),
        "{output}"
    );
}

const HEADER: &str = r#"---@class Instance

---@class StaticInstance
---@overload fun(): Instance
local Static = {}

---@generic T
---@param value T
---@param meta nil | table
---@return T
local function ordinary(value, meta) return value end
"#;

fn constructor(attach: &str, returned: &str, value: &str, extra: &str) -> String {
    format!(
        r#"{attach}(Static, {{
    ---@return Instance
    __call = function()
        local self = --[[---@type self]] {{}}
        ---@return {returned}
        function self.read() return {value} end
        {extra}
        return self
    end,
}})
"#
    )
}

const USE: &str = "local result = Static().read()\n---@type number\nlocal checked = result\n";

#[test]
fn an_inactive_constructor_cannot_erase_a_later_genuine_member_declaration() {
    let fixture = Fixture::new();
    let inactive = constructor("ordinary", "string", "'inactive'", "");
    let active = constructor("setmetatable", "number", "1", "");
    let source = format!("{HEADER}{inactive}{active}{USE}");
    let output = fixture.check(&source);
    // The ordinary call's own false self assertion remains an error. Its
    // function must nevertheless contribute no Instance member contract.
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    result_type(&output, "number");
    let use_line = source[..source.find("local result").unwrap()]
        .lines()
        .count()
        + 1;
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains(&format!("main.lua:{use_line}:")),
        "{}",
        text(&output)
    );

    let corrected = format!("{HEADER}{active}{USE}");
    clean(&fixture.check(&corrected));
    let wrong_result = corrected.replace("local checked = result", "local checked = 'wrong'");
    assert!(text(&fixture.check(&wrong_result)).contains("TS2322"));
    clean(&fixture.check(&corrected));
}

#[test]
fn a_second_active_constructor_checks_writes_against_the_first_member_contract() {
    let fixture = Fixture::new();
    let first = constructor("setmetatable", "number", "1", "");
    let second = constructor("setmetatable", "string", "'wrong'", "");
    let output = fixture.check(&format!("{HEADER}{first}{second}{USE}"));
    assert!(text(&output).contains("TS2322"), "{}", text(&output));
    assert!(!text(&output).contains("Duplicate"), "{}", text(&output));
    result_type(&output, "number");

    let corrected = constructor("setmetatable", "number", "2", "");
    clean(&fixture.check(&format!("{HEADER}{first}{corrected}{USE}")));
}

#[test]
fn later_method_writes_in_one_constructor_do_not_form_a_union_contract() {
    let fixture = Fixture::new();
    let later = "---@return string\nfunction self.read() return 'wrong' end";
    let first = constructor("setmetatable", "number", "1", later);
    let output = fixture.check(&format!("{HEADER}{first}{USE}"));
    assert!(text(&output).contains("TS2717"), "{}", text(&output));
    result_type(&output, "number");

    let corrected = "---@return number\nfunction self.read() return 2 end";
    let first = constructor("setmetatable", "number", "1", corrected);
    clean(&fixture.check(&format!("{HEADER}{first}{USE}")));
}

#[test]
fn an_unguarded_original_member_remains_the_contract_before_guarded_writes() {
    let fixture = Fixture::new();
    let header = HEADER.replace(
        "---@class Instance\n",
        "---@class Instance\nlocal Prototype = {\n---@return number\nread = function() return 1 end,\n}\n",
    );
    let later = constructor("setmetatable", "string", "'wrong'", "");
    let output = fixture.check(&format!("{header}{later}{USE}"));
    assert!(text(&output).contains("TS2322"), "{}", text(&output));
    result_type(&output, "number");

    let corrected = constructor("setmetatable", "number", "2", "");
    clean(&fixture.check(&format!("{header}{corrected}{USE}")));
}

#[test]
fn an_inactive_written_field_does_not_supply_context_for_a_later_inferred_field() {
    let fixture = Fixture::new();
    let inactive = r#"ordinary(Static, {
    ---@return Instance
    __call = function()
        local self = --[[---@type self]] {}
        ---@type string
        self.value = 'inactive'
        return self
    end,
})
"#;
    let active = r#"setmetatable(Static, {
    ---@return Instance
    __call = function()
        local self = --[[---@type self]] {}
        self.value = 1
        return self
    end,
})
"#;
    let use_value = "local result = Static().value\n---@type number\nlocal checked = result\n";
    let output = fixture.check(&format!("{HEADER}{inactive}{active}{use_value}"));
    result_type(&output, "number");
    assert!(!text(&output).contains("LUA204"), "{}", text(&output));
    clean(&fixture.check(&format!("{HEADER}{active}{use_value}")));
    let invalid = active.replace("self.value = 1", "self.value = 'wrong'");
    assert!(text(&fixture.check(&format!("{HEADER}{invalid}{use_value}"))).contains("TS2322"));
}

#[test]
fn an_inactive_assignment_cannot_hide_a_later_method_override_of_an_inherited_field() {
    let fixture = Fixture::new();
    let header = HEADER.replace(
        "---@class Instance\n",
        "---@class Base\n---@field read fun(value: number): number\n---@class Instance : Base\n",
    );
    let initial =
        "{\n---@param value number\n---@return number\nread = function(value) return value end,\n}";
    let inactive = format!(
        r#"ordinary(Static, {{
    ---@return Instance
    __call = function()
        local self = --[[---@type self]] {initial}
        ---@type fun(value: number): number
        self.read = function(value) return value end
        return self
    end,
}})
"#
    );
    let active = format!(
        r#"setmetatable(Static, {{
    ---@return Instance
    __call = function()
        local self = --[[---@type self]] {initial}
        ---@param value number | string
        ---@return number
        function self.read(value) return 1 end
        return self
    end,
}})
"#
    );
    let use_value =
        "local result = Static().read('word')\n---@type number\nlocal checked = result\n";
    let output = fixture.check(&format!("{header}{inactive}{active}{use_value}"));
    result_type(&output, "number");
    assert!(!text(&output).contains("TS2345"), "{}", text(&output));
    let corrected = format!("{header}{active}{use_value}");
    clean(&fixture.check(&corrected));
    let invalid = corrected.replace("read('word')", "read(true)");
    assert!(text(&fixture.check(&invalid)).contains("TS2345"));
}
