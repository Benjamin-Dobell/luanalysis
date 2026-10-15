use luanalysis::{check, location};

const CALLBACKS: &str = r#"---@param produce (fun(): number, string)
---@return number
local function consume(produce)
    local count, label = produce()
    ---@type string
    local text = label
    return count
end
---@type fun(producer: (fun(): number, string)): number
local use = consume
local result = use(function() return 7, "ok" end)
"#;

const RETURNED: &str = r#"local function factory()
    ---@type fun(value: number): number
    return function(value) return value end
end
local callback = factory()
local result = callback(7)
"#;

fn accepted(source: &str) -> luanalysis::CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}

#[test]
fn encoder_callback_parameter_reduction_preserves_both_original_return_slots() {
    // ge_tts/lunajson/encoder.lua:18 uses this same nested callback grammar.
    let report = accepted(CALLBACKS);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "result")
            .unwrap()
            .ty,
        "number"
    );
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "text")
            .unwrap()
            .ty,
        "string"
    );
    let bad = CALLBACKS.replace("return 7, \"ok\"", "return 7, false");
    let report = check(&bad);
    let error = report
        .diagnostics
        .iter()
        .find(|error| error.code == "TS2345")
        .unwrap_or_else(|| panic!("{:#?}", report.diagnostics));
    assert_eq!(
        &bad[error.span.start..error.span.end],
        "function() return 7, false end"
    );
    accepted(&bad.replace("false", "\"corrected\""));
}

#[test]
fn tableutils_returned_closure_contract_types_and_checks_the_original_body() {
    let report = accepted(RETURNED);
    let parameter = RETURNED.find("function(value)").unwrap() + "function(".len();
    assert_eq!(report.type_at(parameter), Some("number"));
    let bad = RETURNED.replace("return value end", "return \"wrong\" end");
    let report = check(&bad);
    let error = report
        .diagnostics
        .iter()
        .find(|error| error.code == "TS2322")
        .unwrap_or_else(|| panic!("{:#?}", report.diagnostics));
    assert_eq!(&bad[error.span.start..error.span.end], "\"wrong\"");
    assert!(report
        .diagnostics
        .iter()
        .all(|error| error.code != "TS7006"));
    accepted(&bad.replace("\"wrong\"", "value"));
}

#[test]
fn zone_bare_returns_and_tableutils_parenthesized_write_check_as_original_lua() {
    let source = r#"---@param value number
local function stop(value)
    if value == 1 then return
    elseif value == 2 then return
    else return
    end
end
local function once() repeat return until true end
---@type number[]
local selected = {}
local i = 0
i = i + 1
(--[[---@type number[] ]] selected)[i] = 7
stop(i)
once()
local function zero() end
zero(stop(i))
zero(once())
"#;
    accepted(source);
    let scalarized = source.replace("zero(stop(i))", "zero((stop(i)))");
    assert!(check(&scalarized)
        .diagnostics
        .iter()
        .any(|error| error.code == "TS2554"));
}

#[test]
fn cli_reports_real_incomplete_annotations_then_checks_the_original_source_corrections() {
    let directory =
        std::env::temp_dir().join(format!("luanalysis corpus grammar {}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("original source.lua");
    let run = |source: &str| {
        std::fs::write(&path, source).unwrap();
        std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .arg("check")
            .arg(&path)
            .output()
            .unwrap()
    };
    // Exact malformed forms from UI.def.lua:109 and Json.ttslua:308 stay errors.
    let invalid = "---@param v\nlocal function echo(v) return v end\n---@shape UI\n---@field tooltipPosition nil |\n";
    let output = run(invalid);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr)
            .matches("error[LUA1001]")
            .count(),
        2,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let corrected = invalid
        .replace("nil |\n", "nil | string\n")
        .replace("@param v\n", "@param v number\n");
    let output = run(&corrected);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bad = RETURNED.replace("return value end", "return \"wrong\" end");
    let output = run(&bad);
    assert_eq!(output.status.code(), Some(1));
    let byte = bad.find("\"wrong\"").unwrap();
    let (line, column) = location(&bad, luanalysis_syntax::Span::new(byte, byte));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains(&format!(":{line}:{column}: error[TS2322]")),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = run(RETURNED);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(directory).unwrap();
}
