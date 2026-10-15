use luanalysis::{CheckReport, check};

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

const DEFAULT: &str = "---@overload fun():string\n---@overload fun(value:string):string\n---@param value string\n---@return string\nlocal function example(value) return value or 'default' end\nlocal empty=example()\nlocal provided=example('text')\n";

#[test]
fn missing_runtime_arguments_do_not_make_public_parameters_or_body_types_optional() {
    let report = clean(DEFAULT);
    let parameter = DEFAULT.find("example(value)").unwrap() + "example(".len();
    assert_eq!(report.type_at(parameter), Some("string"));
    for call in ["example(true)", "example('text','extra')"] {
        let report = check(&format!("{DEFAULT}{call}\n"));
        assert!(
            report
                .diagnostics
                .iter()
                .any(|error| matches!(error.code, "TS2345" | "TS2554" | "TS2769")),
            "{:#?}",
            report.diagnostics
        );
        assert!(
            report
                .diagnostics
                .iter()
                .all(|error| error.code != "TS2394"),
            "{:#?}",
            report.diagnostics
        );
    }
    let ordinary = "---@param value string\n---@return string\nlocal function ordinary(value) return value end\nordinary()\n";
    assert!(
        check(ordinary)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2554")
    );
    let wrong_body = DEFAULT.replace("return value or 'default'", "return 42");
    let report = check(&wrong_body);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2322"
                && &wrong_body[error.span.start..error.span.end] == "42"),
        "{:#?}",
        report.diagnostics
    );
    clean(&wrong_body.replace("return 42", "return value or 'default'"));
}

#[test]
fn lua_implementation_arity_preserves_native_parameter_and_result_compatibility() {
    for source in [
        "---@overload fun(value:number):string\n---@param value string\n---@return string\nlocal function wrong(value) return value end\n",
        "---@overload fun(value:string):number\n---@param value string\n---@return string\nlocal function wrong(value) return value end\n",
        "---@overload fun(): (number,string)\n---@param value number\n---@return number\nlocal function wrong(value) return value end\n",
    ] {
        let report = check(source);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|error| error.code == "TS2394"),
            "{source}\n{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn omitted_fixed_arguments_keep_the_original_variadic_element_contract() {
    let source = "---@overload fun():string\n---@overload fun(value:string,...:number):string\n---@param value string\n---@vararg number\n---@return string\nlocal function variadic(value,...) return value or 'default' end\nvariadic()\nvariadic('value',1,2)\n";
    clean(source);
    let report = check(&format!("{source}variadic('value',true)\n"));
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2345" | "TS2769")),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|error| error.code != "TS2394"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn captured_generic_implementations_keep_original_context_and_required_body_types() {
    let source = r#"---@generic T
---@param captured T
local function make(captured)
---@overload fun():T
---@overload fun(value:T):T
---@param value T
---@return T
local function pick(value)
---@type T
local checked=value
return value or captured
end
return pick
end
local numeric=make(42)
local textual=make("word")
---@type number
local number=numeric()
---@type string
local string=textual()
"#;
    clean(source);
    let invalid = source.replace("local checked=value", "local checked=42");
    assert!(
        check(&invalid)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2322")
    );
    clean(&invalid.replace("local checked=42", "local checked=value"));
}

#[test]
fn original_base64_decoding_map_short_heads_accept_the_runtime_implementation() {
    let source = r#"---@class Base64
local Base64={}
---@overload fun(char62:string,char63:string):table<number,number>
---@overload fun(char62:string):table<number,number>
---@overload fun():table<number,number>
---@param char62 string
---@param char63 string
---@param charPad? string
---@return table<number,number>
function Base64.decodingMap(char62,char63,charPad)
---@type string
local first=char62
---@type string
local second=char63
return {}
end
local empty=Base64.decodingMap()
local one=Base64.decodingMap("+")
local two=Base64.decodingMap("+","/")
"#;
    clean(source);
    assert!(
        check(&format!("{source}Base64.decodingMap(42)\n"))
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2345" | "TS2769"))
    );
}

#[test]
fn cli_missing_argument_overload_keeps_original_body_error_and_correction() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis implementation arity {}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("original.lua");
    let invalid = DEFAULT.replace("return value or 'default'", "return 42");
    std::fs::write(&path, invalid).unwrap();
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--no-stdlib"])
            .arg(&path)
            .output()
            .unwrap()
    };
    let output = run();
    assert_eq!(output.status.code(), Some(1));
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert!(
        diagnostics.contains(":5:38: error[TS2322]"),
        "{diagnostics}"
    );
    assert!(!diagnostics.contains("TS2394"), "{diagnostics}");
    std::fs::write(&path, DEFAULT).unwrap();
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(directory).unwrap();
}
