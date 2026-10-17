//! Original-source result-contract checks beyond Lua list adjustment.

use luanalysis::{CheckReport};

// Keep contextual inference regressions meaningful with missing contracts denied.
fn check(source: &str) -> luanalysis::CheckReport {
    luanalysis::check_with_options(source, luanalysis::CheckOptions {
        no_implicit_any: true,
        ..Default::default()
    })
}


fn accepted(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}

fn binding(report: &CheckReport, name: &str, expected: &str) {
    let actual = report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .unwrap();
    assert_eq!(actual.ty, expected, "{name}: {:#?}", report.bindings);
}

#[test]
fn explicit_void_contract_has_no_physical_result_slot() {
    let report = accepted(
        "---@return void\nlocal function empty() return end\n\
---@return void\nlocal function forwarded() return empty() end\n\
local first,second=forwarded()",
    );
    binding(&report, "first", "nil");
    binding(&report, "second", "nil");
    let source = "---@return void\nlocal function invalid() return nil end";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    let span = report.diagnostics[0].span;
    assert_eq!(&source[span.start..span.end], "nil");
}

#[test]
fn declared_fixed_pack_requires_every_physical_slot() {
    let source = "---@return number, nil\nlocal function missing() return 1 end";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    let source = "---@return number, nil\nlocal function missing() end";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2355");
    accepted("---@return number, nil\nlocal function complete() return 1,nil end");
}

#[test]
fn captured_generic_pack_mapping_preserves_distinct_call_environments() {
    let declarations = "---@generic T\n---@param value T\n\
local function make(value)\n\
---@return T, string\n\
local function read() return value,\"tag\" end\n\
return read\nend\n\
local numeric=make(42)\nlocal textual=make(\"text\")\n\
local number,number_tag=numeric()\nlocal text,text_tag=textual()\n\
local again,again_tag=numeric()\n";
    let report = accepted(declarations);
    binding(&report, "number", "number");
    binding(&report, "text", "string");
    binding(&report, "again", "number");
    for name in ["number_tag", "text_tag", "again_tag"] {
        binding(&report, name, "string");
    }
    let source = format!("{declarations}---@type string\nlocal wrong=numeric()");
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    let span = report.diagnostics[0].span;
    assert_eq!(&source[span.start..span.end], "numeric()");
}

#[test]
fn contextual_callback_returns_a_whole_fixed_pack() {
    let declarations = "---@param callback fun(value: number): number, string\n\
---@return number, string\n\
local function invoke(callback) return callback(42) end\n";
    let report = accepted(&format!(
        "{declarations}local number,text=invoke(function(value) return value,\"tag\" end)"
    ));
    binding(&report, "number", "number");
    binding(&report, "text", "string");
    let report = check(&format!(
        "{declarations}invoke(function(value) return \"wrong\",value end)"
    ));
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2345");
}

#[test]
fn method_calls_share_full_pack_and_void_scalar_adjustment() {
    let report = accepted(
        "---@type {pair: fun(self: {}): (number, string), empty: fun(self: {}): void}\n\
local object={pair=function(self) return 1,\"tail\" end, empty=function(self) end}\n\
local number,text=object:pair()\n\
local scalar=(object:pair())\n\
local absent=(object:empty())",
    );
    binding(&report, "number", "number");
    binding(&report, "text", "string");
    binding(&report, "scalar", "number");
    binding(&report, "absent", "nil");
}

#[test]
fn a_partial_local_annotation_does_not_contextualize_an_unannotated_extra_slot_as_nil() {
    let source = "---@generic T\n---@param input any\n---@return T\n\
local function make(input) return input end\n\
---@type number\nlocal typed,untyped,last=make(1),make(\"text\"),99";
    let report = accepted(source);
    binding(&report, "typed", "number");
    // Nothing infers its T, which is `any`, as in a JavaScript call, rather than nil.
    binding(&report, "untyped", "any");
}

#[test]
fn an_unknown_final_pack_cannot_erase_a_guaranteed_argument_prefix() {
    let source = "---@param callback any\nlocal function run(callback)\n\
local function zero() end\nzero(1,callback())\nend";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2554");
    let span = report.diagnostics[0].span;
    assert_eq!(&source[span.start..span.end], "zero(1,callback())");
}

#[test]
fn method_pack_cli_rejects_then_accepts_the_original_lua_annotation_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis method packs {}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("methods.lua");
    let source = "---@type {pair: fun(self: {}): (number, string), empty: fun(self: {}): void}\n\
local object={pair=function(self) return 1,\"tail\" end, empty=function(self) end}\n\
local number,text=object:pair()\nlocal absent=(object:empty())\n\
---@type string\nlocal scalar=(object:pair())";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    let span = report.diagnostics[0].span;
    assert_eq!(&source[span.start..span.end], "(object:pair())");
    fs::write(&path, source).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .arg("check")
            .arg("--dump-types")
            .arg(&path)
            .output()
            .unwrap()
    };
    let failed = run();
    assert_eq!(failed.status.code(), Some(1));
    let errors = String::from_utf8_lossy(&failed.stderr);
    assert!(
        errors.contains("TS2322") && !errors.contains("TS7006"),
        "{errors}"
    );
    fs::write(
        &path,
        source.replace(
            "---@type string\nlocal scalar",
            "---@type number\nlocal scalar",
        ),
    )
    .unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    let types = String::from_utf8_lossy(&corrected.stdout);
    assert!(
        types.contains("number: number")
            && types.contains("text: string")
            && types.contains("absent: nil")
            && types.contains("scalar: number"),
        "{types}"
    );
    fs::remove_dir_all(directory).unwrap();
}
