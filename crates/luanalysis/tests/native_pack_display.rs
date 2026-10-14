use luanalysis::check;

#[test]
fn signatures_display_whole_return_alternatives_and_varargs() {
    let source = "---@param value number\n---@return number,string\nlocal function pair(value) return value,\"done\" end\n---@param ready boolean\n---@return false|(true,number)\nlocal function conditional(ready) if ready then return true,1 end return false end\n---@vararg number\n---@return number...\nlocal function forward(...) return ... end\nlocal function empty() end";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let display = |name: &str| {
        &report
            .bindings
            .iter()
            .find(|binding| binding.name == name)
            .unwrap()
            .ty
    };
    assert_eq!(display("pair"), "fun(value: number): (number, string)");
    let conditional = display("conditional");
    assert!(
        conditional.contains("false") && conditional.contains("(true, number)"),
        "{conditional}"
    );
    assert_eq!(display("forward"), "fun(...: number): (number...)");
    assert_eq!(display("empty"), "fun(): void");
}

#[test]
fn mapped_returned_callable_displays_both_instantiated_result_slots() {
    let source = "---@generic T\n---@param value T\nlocal function make(value) return function() return value,\"tag\" end end\nlocal number=make(42)\nlocal string=make(\"text\")";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    for (name, expected) in [("number", "number"), ("string", "string")] {
        let display = &report
            .bindings
            .iter()
            .find(|binding| binding.name == name)
            .unwrap()
            .ty;
        assert!(
            display.contains(&format!("({expected}, string)")),
            "{display}"
        );
    }
}

#[test]
fn cli_prints_full_pack_signature_and_original_slot_error_then_accepts_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis pack display {}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("pack.ttslua");
    let source = "-- α😀\r\n---@return number,string\r\nlocal function pair() return 42,\"text\" end\r\n---@type number,number\r\nlocal first,second=pair()";
    fs::write(&path, source).unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--dump-types"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    let stderr = String::from_utf8(rejected.stderr).unwrap();
    assert!(
        stderr.contains("pack.ttslua:5:20: error[TS2322]"),
        "{stderr}"
    );
    let output = String::from_utf8(rejected.stdout).unwrap();
    assert!(output.contains("fun(): (number, string)"), "{output}");
    fs::write(
        &path,
        source.replace("---@type number,number", "---@type number,string"),
    )
    .unwrap();
    let accepted = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .arg("check")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        accepted.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    fs::remove_dir_all(&directory).unwrap();
}
