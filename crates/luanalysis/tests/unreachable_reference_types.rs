use luanalysis::{check, location};
use std::{fs, process::Command};

const DEAD_CONTINUATION: &str = r#"
---@param callback fun(value:number):void
---@param record {value:number}
local function single(callback,record)
    if true then return end
    callback(record.value)
end
---@param flag boolean
---@param callback fun(value:number):void
---@param record {value:number}
local function joined(flag,callback,record)
    if flag then return else return end
    callback(record.value)
end
"#;

#[test]
fn entirely_dead_continuations_check_calls_and_members_against_their_declarations() {
    let report = check(DEAD_CONTINUATION);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    for offset in DEAD_CONTINUATION
        .match_indices("callback(record.value)")
        .map(|(offset, _)| offset)
    {
        assert_eq!(report.type_at(offset), Some("fun(value: number): void"));
        assert_eq!(
            report.type_at(offset + "callback(record.".len()),
            Some("number")
        );
    }
}

#[test]
fn dead_continuations_still_check_arguments_instead_of_replacing_the_callee_with_never() {
    let source = DEAD_CONTINUATION.replace("callback(record.value)", "callback('wrong')");
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 2, "{:#?}", report.diagnostics);
    for diagnostic in &report.diagnostics {
        assert_eq!(diagnostic.code, "TS2345", "{diagnostic:#?}");
        assert_eq!(
            &source[diagnostic.span.start..diagnostic.span.end],
            "'wrong'"
        );
        assert!(diagnostic.message.contains("number"), "{diagnostic:#?}");
    }
    let corrected = check(DEAD_CONTINUATION);
    assert!(
        corrected.diagnostics.is_empty(),
        "{:#?}",
        corrected.diagnostics
    );
}

#[test]
fn an_impossible_predicate_on_the_reference_itself_still_narrows_it_to_never() {
    let source = "---@param value number|string\nlocal function impossible(value)\n if type(value)=='boolean' then value() end\nend\n";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.code, "TS2349");
    assert!(diagnostic.message.contains("'never'"), "{diagnostic:#?}");
    assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], "value");
    assert_eq!(report.type_at(diagnostic.span.start), Some("never"));
    let corrected = check(&source.replace("value()", "local ignored=value"));
    assert!(
        corrected.diagnostics.is_empty(),
        "{:#?}",
        corrected.diagnostics
    );
}

#[test]
fn cli_preserves_dead_code_argument_errors_and_accepts_the_original_correction() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis unreachable references {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("main.lua");
    fs::write(&file, DEAD_CONTINUATION).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--lua-version", "5.2"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let wrong = DEAD_CONTINUATION.replacen("callback(record.value)", "callback('wrong')", 1);
    fs::write(&file, &wrong).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--lua-version", "5.2"])
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8(output.stderr).unwrap();
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let (line, column) = location(&wrong, report.diagnostics[0].span);
    assert!(
        text.contains(&format!("main.lua:{line}:{column}: error[TS2345]")),
        "{text}"
    );
    assert!(!text.contains("'never'"), "{text}");
    fs::write(&file, DEAD_CONTINUATION).unwrap();
    let corrected = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--lua-version", "5.2"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        corrected.status.success(),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    fs::remove_dir_all(&directory).unwrap();
}
