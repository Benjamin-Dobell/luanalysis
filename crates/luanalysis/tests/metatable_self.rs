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
            "luanalysis metatable self {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn check(&self, source: &str) -> Output {
        fs::write(self.0.join("main.lua"), source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
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

const FACTORY: &str = r#"---@class Vector
---@field x number

---@class StaticVector
---@overload fun(): Vector
local Vector = {}

ATTACH(Vector, {
    ---@param target StaticVector
    ---@return Vector
    __call = function(target)
        local self = --[[---@type self]] {x = 1}
        ---@return number
        function self.read() return self.x end
        ---@return self
        function self.identity() return self end
        return self
    end,
})

local vector = Vector()
---@type number
local result = vector.identity().read()
"#;

const ORDINARY: &str = r#"---@generic T
---@param value T
---@param meta nil | table
---@return T
local function ordinary(value, meta) return value end
"#;

const INFERRED_OWNER: &str = r#"---@class Instance
---@field id number

---@class Static
---@overload fun(): Instance
local Static = {}
ATTACH(Static, {
    __call = function()
        return --[[---@type Instance]] {id = 1}
    end,
    ---@param target self
    ---@return number
    __len = function(target)
        return target.id
    end,
})
"#;

#[test]
fn unannotated_call_result_supplies_its_named_owner_to_sibling_metatable_fields() {
    // v1 LuaPsiImplUtil.guessParentType(LuaTableField) uses the inferred
    // __call result before falling back to the setmetatable target.
    let fixture = Fixture::new();
    let source = INFERRED_OWNER.replace("ATTACH", "setmetatable");
    clean(&fixture.check(&source));
    let invalid = source.replace("return target.id", "return 'wrong'");
    let output = fixture.check(&invalid);
    assert!(text(&output).contains("TS2322"), "{}", text(&output));
    assert!(!text(&output).contains("TS2339"), "{}", text(&output));
    let wrong_result = source.replace("---@return number", "---@return string");
    let output = fixture.check(&wrong_result);
    assert!(
        text(&output).contains("Actual: number. Expected: string."),
        "{}",
        text(&output)
    );
    let missing_member = source.replace("target.id", "target.missing");
    let output = fixture.check(&missing_member);
    assert!(
        text(&output).contains("Property 'missing' does not exist"),
        "{}",
        text(&output)
    );
    clean(&fixture.check(&source));
}

#[test]
fn inferred_call_result_does_not_grant_an_ordinary_function_metatable_context() {
    let fixture = Fixture::new();
    let source = format!(
        "{ORDINARY}{}",
        INFERRED_OWNER.replace("ATTACH", "ordinary")
    );
    let output = fixture.check(&source);
    assert!(text(&output).contains("TS2339"), "{}", text(&output));
    clean(&fixture.check(&INFERRED_OWNER.replace("ATTACH", "setmetatable")));
}

#[test]
fn a_self_dependent_inferred_call_result_keeps_the_original_target_fallback() {
    let fixture = Fixture::new();
    let source = r#"---@class SelfInstance
---@field id number
---@overload fun(): self
local Static = {id = 1}
setmetatable(Static, {
    __call = function()
        local self = --[[---@type self]] {id = 2}
        return self
    end,
    ---@param target self
    ---@return number
    __len = function(target)
        return target.id
    end,
})
"#;
    clean(&fixture.check(source));
    let output = fixture.check(&source.replace("return target.id", "return 'wrong'"));
    assert!(text(&output).contains("TS2322"), "{}", text(&output));
    clean(&fixture.check(source));
}

#[test]
fn genuine_metatable_context_uses_the_written_instance_class_and_checks_its_real_members() {
    let fixture = Fixture::new();
    let source = FACTORY.replace("ATTACH", "setmetatable");
    clean(&fixture.check(&source));
    let bad = source.replace("return self.x end", "return 'wrong' end");
    let output = fixture.check(&bad);
    assert!(text(&output).contains("TS2322"), "{}", text(&output));
    clean(&fixture.check(&source));
}

#[test]
fn a_structurally_matching_shadow_does_not_acquire_standard_constructor_context() {
    let fixture = Fixture::new();
    let source = format!(
        "{ORDINARY}local setmetatable=ordinary\n{}",
        FACTORY.replace("ATTACH", "setmetatable")
    );
    let output = fixture.check(&source);
    assert!(
        text(&output).contains("Property 'identity'"),
        "{}",
        text(&output)
    );
    clean(&fixture.check(&FACTORY.replace("ATTACH", "setmetatable")));
}

#[test]
fn a_saved_standard_alias_keeps_its_creation_context_after_global_replacement() {
    let fixture = Fixture::new();
    let source = format!(
        "{ORDINARY}local attach=setmetatable\nsetmetatable=ordinary\n{}",
        FACTORY.replace("ATTACH", "attach")
    );
    clean(&fixture.check(&source));
    let replaced = source.replace("setmetatable=ordinary", "attach=ordinary");
    let output = fixture.check(&replaced);
    assert!(
        text(&output).contains("Property 'identity'"),
        "{}",
        text(&output)
    );
    clean(&fixture.check(&source));
}

#[test]
fn a_later_write_does_not_reinterpret_an_earlier_original_metatable_call() {
    let fixture = Fixture::new();
    let source = format!(
        "{ORDINARY}{}\nsetmetatable=ordinary\n",
        FACTORY.replace("ATTACH", "setmetatable")
    );
    clean(&fixture.check(&source));
}

#[test]
fn a_possible_ordinary_callee_cannot_grant_context_from_its_standard_union_arm() {
    let fixture = Fixture::new();
    let source = format!(
        "{ORDINARY}---@type boolean\nlocal choose\nlocal attach=choose and setmetatable or ordinary\n{}",
        FACTORY.replace("ATTACH", "attach")
    );
    let output = fixture.check(&source);
    assert!(
        text(&output).contains("Property 'identity'"),
        "{}",
        text(&output)
    );
    clean(&fixture.check(&FACTORY.replace("ATTACH", "setmetatable")));
}

#[test]
fn a_known_standard_sibling_field_can_prove_owner_without_activating_constructor_members() {
    let fixture = Fixture::new();
    let source = FACTORY
        .replace(
            "local Vector = {}",
            "local Vector = {attach = setmetatable}",
        )
        .replace("ATTACH", "Vector.attach");
    clean(&fixture.check(&source));
    let shadowed = format!(
        "{ORDINARY}{}",
        source.replace("attach = setmetatable", "attach = ordinary")
    );
    let output = fixture.check(&shadowed);
    assert!(
        text(&output).contains("Property 'identity'"),
        "{}",
        text(&output)
    );
    clean(&fixture.check(&source));
}

#[test]
fn a_generic_return_type_promise_cannot_attest_the_runtime_standard_value() {
    let fixture = Fixture::new();
    let identity = "---@generic T\n---@param value T\n---@return T\nlocal function identity(value) return value end\n";
    let source = format!(
        "{ORDINARY}{identity}---@type boolean\nlocal choose\nlocal attach=identity(choose and setmetatable or ordinary)\n{}",
        FACTORY.replace("ATTACH", "attach")
    );
    let output = fixture.check(&source);
    assert!(
        text(&output).contains("Property 'identity'"),
        "{}",
        text(&output)
    );
    let member = FACTORY
        .replace(
            "local Vector = {}",
            "local Vector = {attach = identity(choose and setmetatable or ordinary)}",
        )
        .replace("ATTACH", "Vector.attach");
    let output = fixture.check(&format!(
        "{ORDINARY}{identity}---@type boolean\nlocal choose\n{member}"
    ));
    assert!(
        text(&output).contains("Property 'identity'"),
        "{}",
        text(&output)
    );
    clean(&fixture.check(&FACTORY.replace("ATTACH", "setmetatable")));
}

#[test]
fn a_saved_sibling_tracks_actual_member_writes_through_table_aliases() {
    let fixture = Fixture::new();
    let source = format!(
        "{ORDINARY}{}",
        FACTORY
            .replace(
                "local Vector = {}",
                "local Vector = {attach = setmetatable}\nlocal alias=Vector\nalias.attach=ordinary"
            )
            .replace("ATTACH", "Vector.attach")
    );
    let output = fixture.check(&source);
    assert!(
        text(&output).contains("Property 'identity'"),
        "{}",
        text(&output)
    );
    let restored = source.replace(
        "alias.attach=ordinary",
        "alias.attach=ordinary\nalias.attach=setmetatable",
    );
    clean(&fixture.check(&restored));
}

#[test]
fn an_original_saved_value_survives_an_ordinary_call_that_replaces_the_global_slot() {
    let fixture = Fixture::new();
    let prefix = format!(
        "{ORDINARY}local saved=setmetatable\nlocal function replace() setmetatable=ordinary end\nreplace()\n"
    );
    let output = fixture.check(&format!(
        "{prefix}{}",
        FACTORY.replace("ATTACH", "setmetatable")
    ));
    assert!(
        text(&output).contains("Property 'identity'"),
        "{}",
        text(&output)
    );
    clean(&fixture.check(&format!("{prefix}{}", FACTORY.replace("ATTACH", "saved"))));
}
