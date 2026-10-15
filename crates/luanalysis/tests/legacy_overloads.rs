use luanalysis::{CheckReport, OverloadMode, check, check_with_overload_mode};
use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

fn legacy(source: &str) -> CheckReport {
    check_with_overload_mode(source, OverloadMode::LegacyExposeImplementation)
}
fn clean(report: &CheckReport) {
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}
fn has(report: &CheckReport, code: &str) {
    assert!(
        report.diagnostics.iter().any(|error| error.code == code),
        "{:#?}",
        report.diagnostics
    );
}

const IDENTITY: &str = "---@overload fun(value:number):number\n---@generic T\n---@param value T\n---@return T\nlocal function identity(value) return value end\n";

#[test]
fn default_v2_hides_the_main_while_legacy_exposes_its_generic_inference() {
    let source = format!("{IDENTITY}---@type string\nlocal result=identity('word')");
    has(&check(&source), "TS2345");
    let report = legacy(&source);
    clean(&report);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "result")
            .unwrap()
            .ty,
        "string"
    );
    has(
        &legacy(&source.replace("---@type string", "---@type number")),
        "TS2322",
    );
}

#[test]
fn legacy_keeps_private_parameter_and_return_body_errors() {
    let source = "---@overload fun(value:number):number\n---@param value number|string\n---@return number\nlocal function bad(value)\n---@type number\nlocal wrong=value\nreturn 'wrong'\nend";
    has(&check(source), "TS2322");
    has(&legacy(source), "TS2322");
    let corrected = source
        .replace("local wrong=value", "local wrong=42")
        .replace("return 'wrong'", "return 42");
    clean(&check(&corrected));
    clean(&legacy(&corrected));
}

#[test]
fn legacy_still_checks_every_public_overload_against_its_implementation() {
    let source = "---@overload fun(value:string):number\n---@param value number\n---@return number\nlocal function narrow(value) return value end";
    has(&legacy(source), "TS2394");
    clean(&legacy(
        &source
            .replace("---@param value number", "---@param value number|string")
            .replace("return value", "return 42"),
    ));
}

#[test]
fn legacy_uses_original_nested_capture_mappers_and_return_packs() {
    let source = "---@generic T\n---@param captured T\nlocal function outer(captured)\n---@overload fun(value:number):number,T\n---@generic V\n---@param value V\n---@return V,T\nlocal function choose(value) return value,captured end\nreturn choose\nend\nlocal choose=outer(true)\n---@type string\nlocal text,flag=choose('word')\n---@type boolean\nlocal accepted=flag";
    has(&check(source), "TS2345");
    clean(&legacy(source));
    has(
        &legacy(&source.replace("local accepted=flag", "local accepted=text")),
        "TS2322",
    );
}

static NEXT: AtomicU64 = AtomicU64::new(0);
#[test]
fn cli_flag_applies_to_both_implementation_and_ambient_declarations() {
    let root = std::env::temp_dir().join(format!(
        "luanalysis legacy overload {} {}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    let declarations = IDENTITY
        .replace("local function", "function")
        .replace("return value", "return placeholder()");
    fs::write(root.join("api.def.lua"), declarations).unwrap();
    fs::write(
        root.join("main.lua"),
        "---@type string\nlocal accepted=identity('word')",
    )
    .unwrap();
    for ambient in [false, true] {
        if !ambient {
            fs::write(
                root.join("main.lua"),
                format!("{IDENTITY}---@type string\nlocal accepted=identity('word')"),
            )
            .unwrap();
        } else {
            fs::write(
                root.join("main.lua"),
                "---@type string\nlocal accepted=identity('word')",
            )
            .unwrap();
        }
        for enabled in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis"));
            command
                .current_dir(&root)
                .args(["check", "--no-stdlib", "--format", "json"]);
            if enabled {
                command.arg("--legacy-overloads");
            }
            if ambient {
                command.args(["--library", "api.def.lua"]);
            }
            let output = command.arg("main.lua").output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(if enabled { 0 } else { 1 }),
                "ambient={ambient}, enabled={enabled}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    fs::remove_dir_all(root).unwrap();
}
