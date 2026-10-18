use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis declared drop flow {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("library")).unwrap();
        fs::write(path.join("library/player.def.lua"), DECLARATIONS).unwrap();
        fs::write(path.join("main.lua"), source).unwrap();
        Self(path)
    }
    fn check(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .current_dir(&self.0)
            .args(["check", "--lua-version", "5.2", "--library", "library", "main.lua"])
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn output_text(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}
fn settled(output: &Output) {
    assert!(!output_text(output).contains("LUA2041"), "{}", output_text(output));
}
const DECLARATIONS: &str = r#"
---@shape tts__Object
---@field guid string

---@shape tts__Player
---@field getHandCount fun():number
---@field getHandObjects fun(handIndex:number):tts__Object[]

---@class tts__GlobalPlayer
---@field White tts__Player
Player={}
---@return 'White'[]
function Player.getAvailableColors() end
"#;

// The original DropZone.isInHand control flow: an inferred local player,
// numeric and generic nested loops, and an early return from object comparison.
// All queried host fields have complete written declarations.
const HAND_LOOKUP: &str = r#"
---@param object tts__Object
local function isInHand(object)
    for _, colorName in ipairs(Player.getAvailableColors()) do
        local player = Player[colorName]
        for i = 1, player.getHandCount() do
            for _, handObject in ipairs(player.getHandObjects(i)) do
                if object == handObject then
                    return true
                end
            end
        end
    end
    return false
end
---@type boolean
local result = isInHand({guid='target'})
"#;

#[test]
fn original_nested_hand_lookup_infers_a_settled_boolean() {
    let output = Fixture::new(HAND_LOOKUP).check();
    settled(&output);
    assert_eq!(output.status.code(), Some(0), "{}", output_text(&output));
}

#[test]
fn written_host_call_arguments_are_still_checked_inside_nested_loops() {
    let source = HAND_LOOKUP.replace("getHandObjects(i)", "getHandObjects('wrong')");
    let output = Fixture::new(&source).check();
    settled(&output);
    assert_eq!(output.status.code(), Some(1), "{}", output_text(&output));
    assert!(output_text(&output).contains("TS2345"), "{}", output_text(&output));
}

#[test]
fn written_lookup_return_contracts_still_reject_the_original_boolean_return() {
    let source = HAND_LOOKUP.replace("local function isInHand", "---@return number\nlocal function isInHand");
    let output = Fixture::new(&source).check();
    settled(&output);
    assert_eq!(output.status.code(), Some(1), "{}", output_text(&output));
    assert!(output_text(&output).contains("TS2322"), "{}", output_text(&output));
}
