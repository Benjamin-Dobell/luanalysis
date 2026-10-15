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
            "luanalysis frozen member receivers {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("api.def.lua"), "---@type number\nanswer=nil\n").unwrap();
        Self(path)
    }

    fn check(&self, source: &str) -> Output {
        fs::write(self.0.join("main.lua"), source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args([
                "check",
                "--lua-version",
                "5.4",
                "--library",
                "api.def.lua",
                "--format",
                "json",
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

fn json(output: &Output, expected_exit: i32) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(expected_exit),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stderr.is_empty(), "{stderr}");
    assert!(stdout.starts_with("{\"formatVersion\":1,"), "{stdout}");
    assert!(
        stdout.contains(&format!("\"exitCode\":{expected_exit}")),
        "{stdout}"
    );
    stdout.into_owned()
}

fn clean(output: &Output) {
    let report = json(output, 0);
    assert!(report.contains("\"diagnostics\":[]"), "{report}");
}

fn rejects_checked_value(source: &str, output: &Output) {
    let report = json(output, 1);
    let marker = "local checked=value";
    let start = source.find(marker).expect("original checked assignment") + "local checked=".len();
    let end = start + "value".len();
    assert!(report.contains("\"code\":\"TS2322\""), "{report}");
    assert!(
        report.contains(&format!("\"range\":{{\"start\":{start},\"end\":{end}}}")),
        "the rejected assignment must be the refinement control, not an unrelated setup error:\n{report}"
    );
    for unexpected in ["TS2304", "TS2339", "LUA2036"] {
        assert!(
            !report.contains(&format!("\"code\":\"{unexpected}\"")),
            "{report}"
        );
    }
}

const ORDINARY: &str = r#"
---@param value any
---@return 'number'
local function ordinary_type(value)
    return 'number'
end
"#;

#[test]
fn asserted_environment_aliases_keep_runtime_origin_through_field_index_and_parentheses() {
    let fixture = Fixture::new();
    for access in [
        "alias.type",
        "((alias)).type",
        "((--[[---@type TypeView]] alias)).type",
        "((--[[---@type TypeView]] alias))['type']",
        "_G.type",
    ] {
        let source = format!(
            r#"---@shape TypeView
---@field type fun(value: any): string
{ORDINARY}
local alias=_G
local kind={access}
---@param value number|string
local function use(value)
    if kind(value)=='number' then
        ---@type number
        local checked=value
    end
end
use('text')
"#
        );
        clean(&fixture.check(&source));

        // The direct _G form also needs a real ordinary receiver control:
        // a local with that spelling cannot certify the global capability.
        let ordinary = source.replace(
            "local alias=_G",
            "local _G={type=ordinary_type}\nlocal alias=_G",
        );
        rejects_checked_value(&ordinary, &fixture.check(&ordinary));
        let corrected = ordinary.replace(
            "---@type number\n        local checked=value",
            "---@type number|string\n        local checked=value",
        );
        clean(&fixture.check(&corrected));
    }
}

#[test]
fn replacing_environment_type_before_member_lookup_does_not_narrow() {
    let fixture = Fixture::new();
    let source = format!(
        r#"{ORDINARY}
local alias=_G
alias.type=ordinary_type
local kind=alias.type
---@param value number|string
local function use(value)
    if kind(value)=='number' then
        ---@type number
        local checked=value
    end
end
use('text')
"#
    );
    rejects_checked_value(&source, &fixture.check(&source));
    let corrected = source.replace(
        "---@type number\n        local checked=value",
        "---@type number|string\n        local checked=value",
    );
    clean(&fixture.check(&corrected));
}

#[test]
fn saving_environment_type_before_member_replacement_keeps_the_original_predicate() {
    let fixture = Fixture::new();
    let source = format!(
        r#"{ORDINARY}
local alias=_G
local kind=alias.type
alias.type=ordinary_type
---@param value number|string
local function use(value)
    if kind(value)=='number' then
        ---@type number
        local checked=value
    end
end
use('text')
"#
    );
    clean(&fixture.check(&source));
    let rejected = source.replace(
        "---@type number\n        local checked=value",
        "---@type string\n        local checked=value",
    );
    rejects_checked_value(&rejected, &fixture.check(&rejected));
}

#[test]
fn overloaded_body_type_predicates_keep_the_original_frozen_flow_frame() {
    let fixture = Fixture::new();
    let source = r#"---@overload fun(value:number):number
---@overload fun(value:string):number
---@param value number|string|boolean
---@return number
local function measure(value)
    if type(value)=='number' then
        return value
    elseif type(value)=='string' then
        ---@type string
        local checked=value
        return #value
    else
        return 0
    end
end
---@type number
local numeric=measure(42)
---@type number
local textual=measure('Lua')
"#;
    clean(&fixture.check(source));
    let rejected = source.replace(
        "---@type string\n        local checked=value",
        "---@type number\n        local checked=value",
    );
    rejects_checked_value(&rejected, &fixture.check(&rejected));
}

#[test]
fn computed_member_read_freezes_global_receiver_before_key_rebinds_its_base_to_an_ordinary_table() {
    let fixture = Fixture::new();
    // Check the receiver's concrete static view across key evaluation. The
    // key still rebinds base; later closure-write type propagation is separate.
    let source = r#"local ordinary={answer='ordinary'}
---@type {answer:number}|{answer:string}
local base=_G
---@return 'answer'
local function key()
    base=ordinary
    return 'answer'
end
local value=(--[[---@type {answer:number}]] base)[key()]
---@type number
local checked=value
"#;
    clean(&fixture.check(source));

    let rejected = source.replace(
        "---@type number\nlocal checked=value",
        "---@type string\nlocal checked=value",
    );
    rejects_checked_value(&rejected, &fixture.check(&rejected));
}

#[test]
fn computed_member_read_keeps_ordinary_receiver_when_key_rebinds_its_base_to_global_environment() {
    let fixture = Fixture::new();
    // A later global receiver must not replace the original concrete string
    // view. No broad table/any contract can make the wrong-type control pass.
    let source = r#"local ordinary={answer='ordinary'}
---@type {answer:number}|{answer:string}
local base=ordinary
---@return 'answer'
local function key()
    base=_G
    return 'answer'
end
local value=(--[[---@type {answer:string}]] base)[key()]
---@type string
local checked=value
"#;
    clean(&fixture.check(source));

    let rejected = source.replace(
        "---@type string\nlocal checked=value",
        "---@type number\nlocal checked=value",
    );
    rejects_checked_value(&rejected, &fixture.check(&rejected));
}
