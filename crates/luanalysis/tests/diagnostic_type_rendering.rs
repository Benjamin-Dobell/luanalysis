use luanalysis::{CheckReport, check};
use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

const SOURCE: &str = r#"---@class Receipt
---@field id number

---@param receipt Receipt
local function take_receipt(receipt) end

---@param value number
local function take_number(value) end

---@generic T
---@param value T
local function generic(value)
    take_number(value)
end

---@param callback fun(value:string):number
local function take_callback(callback) end

---@param value number
---@return string
local function wrong_callback(value) return 'word' end

local receipt=--[[---@type Receipt]] {id=1}
local missing=receipt.missing

take_receipt(42)
take_callback(wrong_callback)

---@param value Receipt|number
local function not_callable(value) value() end

---@param record {label:string,callback:fun(value:number):string}
local function take_record(record) end
take_record(42)

---@type string|nil
local optional=1

---@param callback fun(value:number|nil):string|nil
local function take_optional_callback(callback) end
take_optional_callback(42)

---@type number
local literal='undefined'
"#;

fn no_placeholders(text: &str) {
    for placeholder in [
        "contextual type",
        "named type",
        "type parameter",
        "SessionType(",
        "NativeFunctionKey",
    ] {
        assert!(!text.contains(placeholder), "{placeholder}: {text}");
    }
}
fn error<'a>(report: &'a CheckReport, code: &str, fragment: &str) -> &'a luanalysis::Diagnostic {
    report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == code && diagnostic.message.contains(fragment))
        .unwrap_or_else(|| panic!("missing {code} with {fragment}: {:#?}", report.diagnostics))
}

#[test]
fn api_errors_render_original_named_generic_union_function_and_table_types() {
    let report = check(SOURCE);
    assert!(!report.diagnostics.is_empty());
    for diagnostic in &report.diagnostics {
        no_placeholders(&diagnostic.message);
    }
    assert!(
        error(&report, "TS2339", "missing")
            .message
            .contains("Receipt")
    );
    assert!(error(&report, "TS2345", "Receipt").message.contains("'42'"));
    let generic = error(&report, "TS2345", "'T'");
    assert!(generic.message.contains("number"));
    assert_eq!(&SOURCE[generic.span.start..generic.span.end], "value");
    let callback = report
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.code == "TS2345"
                && &SOURCE[diagnostic.span.start..diagnostic.span.end] == "wrong_callback"
        })
        .unwrap();
    for expected in ["value", "number", "string"] {
        assert!(callback.message.contains(expected), "{callback:?}");
    }
    let union = error(&report, "TS2349", "Receipt");
    assert!(
        union.message.contains("number") && union.message.contains('|'),
        "{union:?}"
    );
    let table = error(&report, "TS2345", "label");
    assert!(
        table.message.contains("callback")
            && table.message.contains("number")
            && table.message.contains("string"),
        "{table:?}"
    );
    let optional = error(&report, "TS2322", "nil");
    assert!(!optional.message.contains("undefined"), "{optional:?}");
    let optional_callback = error(&report, "TS2345", "nil");
    assert!(
        !optional_callback.message.contains("undefined"),
        "{optional_callback:?}"
    );
    assert!(
        error(&report, "TS2322", "undefined")
            .message
            .contains("\"undefined\"")
    );
}

#[test]
fn a_callable_union_member_groups_its_own_nullable_return_type() {
    let source =
        "---@param value string | (fun(): nil | string)\nlocal function read(value) value() end\n";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.code, "TS2349");
    assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], "value");
    assert!(
        diagnostic.message.contains("(fun(): nil | string)"),
        "{diagnostic:?}"
    );
    assert!(
        diagnostic.message.ends_with("has no call signatures."),
        "{diagnostic:?}"
    );
    no_placeholders(&diagnostic.message);
}

#[test]
fn a_function_type_written_without_results_is_shown_without_them() {
    let source = "---@param value string | fun(id: number)\nlocal function read(value) value(1) end\n---@return fun(name: string)\nlocal function make() return 1 end\n";
    let report = check(source);
    // Unspecified results are not shown, nor do they need parentheses in a union.
    assert!(error(&report, "TS2349", "has no call signatures").message.contains("'string | fun(id: number)'"), "{:#?}", report.diagnostics);
    assert!(error(&report, "TS2322", "Expected").message.ends_with("Expected: fun(name: string)."), "{:#?}", report.diagnostics);
}

static NEXT: AtomicU64 = AtomicU64::new(0);

const OVERLOAD: &str = "-- Unicode: λ\r\n---@shape Receipt<T>\r\n---@field value T\r\n\r\n---@overload fun(value:Receipt<number[]>):Receipt<number[]>\r\n---@param value Receipt<string[]>\r\n---@return Receipt<string[]>\r\nlocal function exchange(value) return value end\r\n";

fn assert_overload_message(message: &str) {
    assert_eq!(
        message,
        "Overload signature 'fun(value: Receipt<number[]>): Receipt<number[]>' is not compatible with implementation signature 'fun(value: Receipt<string[]>): Receipt<string[]>'."
    );
    no_placeholders(message);
}

#[test]
fn incompatible_overloads_render_public_and_private_signatures_at_the_original_head() {
    let report = check(OVERLOAD);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.code, "TS2394");
    assert_eq!(
        &OVERLOAD[diagnostic.span.start..diagnostic.span.end],
        "@overload fun(value:Receipt<number[]>):Receipt<number[]>"
    );
    assert_overload_message(&diagnostic.message);
    assert!(
        check(&OVERLOAD.replace("Receipt<string[]>", "Receipt<number[]>"))
            .diagnostics
            .is_empty()
    );
}

#[test]
fn text_and_json_render_ambient_implementation_contracts_without_checking_placeholder_bodies() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis overload diagnostics {} {}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();
    let library = directory.join("signatures.def.lua");
    let file = directory.join("main.lua");
    fs::write(&file, "").unwrap();
    let source = OVERLOAD
        .replace("local function exchange", "function exchange")
        .replace("return value end", "return missing_body(value) end");
    let diagnostic = &check(OVERLOAD).diagnostics[0];
    for format in ["text", "json"] {
        let run = || {
            Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
                .args(["check", "--no-stdlib", "--format", format, "--library"])
                .arg(&library)
                .arg(&file)
                .output()
                .unwrap()
        };
        fs::write(&library, &source).unwrap();
        let output = run();
        assert_eq!(output.status.code(), Some(1));
        let text = String::from_utf8(if format == "json" {
            assert!(output.stderr.is_empty());
            output.stdout
        } else {
            output.stderr
        })
        .unwrap();
        assert!(text.contains(&diagnostic.message), "{text}");
        assert!(!text.contains("missing_body"), "{text}");
        if format == "json" {
            assert_eq!(text.matches("\"code\":\"TS2394\"").count(), 1, "{text}");
            assert!(
                text.contains(&format!(
                    "\"range\":{{\"start\":{},\"end\":{}}}",
                    diagnostic.span.start, diagnostic.span.end
                )),
                "{text}"
            );
        } else {
            assert_eq!(text.matches("error[").count(), 1, "{text}");
            assert!(
                text.contains("signatures.def.lua:5:4: error[TS2394]"),
                "{text}"
            );
        }
        fs::write(
            &library,
            source.replace("Receipt<string[]>", "Receipt<number[]>"),
        )
        .unwrap();
        let corrected = run();
        assert_eq!(
            corrected.status.code(),
            Some(0),
            "{}{}",
            String::from_utf8_lossy(&corrected.stdout),
            String::from_utf8_lossy(&corrected.stderr)
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn named_generic_arguments_render_nil_and_preserve_the_undefined_string_literal() {
    let source = "---@class Envelope<T>\n---@field value T\n\n---@type Envelope<nil>\nlocal nil_argument=42\n\n---@type Envelope<\"undefined\">\nlocal literal_argument=42\n";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 2, "{:#?}", report.diagnostics);
    assert!(
        error(&report, "TS2322", "Envelope<nil>")
            .message
            .contains("42")
    );
    assert!(
        error(&report, "TS2322", "Envelope<\"undefined\">")
            .message
            .contains("42")
    );
    for diagnostic in &report.diagnostics {
        no_placeholders(&diagnostic.message);
    }

    let directory = std::env::temp_dir().join(format!(
        "luanalysis named nil {} {}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("main.lua");
    fs::write(&file, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .args(["check", "--no-stdlib"])
        .arg(&file)
        .output()
        .unwrap();
    fs::remove_dir_all(&directory).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("Envelope<nil>"), "{stderr}");
    assert!(stderr.contains("Envelope<\"undefined\">"), "{stderr}");
    assert!(!stderr.contains("Envelope<undefined>"), "{stderr}");
}

#[test]
fn cli_json_errors_render_actual_types_and_keep_original_codes_and_ranges() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis diagnostic types {} {}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("main.lua");
    fs::write(&file, SOURCE).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .args(["check", "--format", "json", "--no-stdlib"])
        .arg(&file)
        .output()
        .unwrap();
    fs::remove_dir_all(&directory).unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stdout}\n{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    no_placeholders(&stdout);
    for expected in ["Receipt", "callback", "label", "TS2345", "TS2339", "TS2349"] {
        assert!(stdout.contains(expected), "{stdout}");
    }
    let start = SOURCE.find("    take_number(value)").unwrap() + "    take_number(".len();
    assert!(
        stdout.contains(&format!(
            "\"range\":{{\"start\":{start},\"end\":{}}}",
            start + "value".len()
        )),
        "{stdout}"
    );
}
