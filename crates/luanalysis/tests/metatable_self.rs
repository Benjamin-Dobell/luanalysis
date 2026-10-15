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
