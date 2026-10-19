use luanalysis::{Diagnostic, check};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

const PREFIX: &str = "Lua result pack is not assignable to the declared result pack.";
const SECOND_SLOT: &str = "-- 前置\r\n---@param first number\r\n---@param second boolean\r\n---@return number, string\r\nlocal function wrong(first,second)\r\n    return first,second\r\nend\r\n";
const SECOND_SLOT_MESSAGE: &str = "Lua result pack is not assignable to the declared result pack. Actual: (number, boolean). Expected: (number, string).";

fn no_placeholders(text: &str) {
    for placeholder in [
        "contextual type",
        "named type",
        "type parameter",
        "SessionType",
        "SessionPack",
        "NativeFunctionKey",
        "NativeSignatureKey",
        "undefined",
    ] {
        assert!(!text.contains(placeholder), "{placeholder}: {text}");
    }
}

fn failure(source: &str, original: &str) -> Diagnostic {
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let diagnostic = report.diagnostics.into_iter().next().unwrap();
    assert_eq!(diagnostic.code, "TS2322", "{diagnostic:?}");
    assert_eq!(
        &source[diagnostic.span.start..diagnostic.span.end],
        original
    );
    assert!(diagnostic.message.starts_with(PREFIX), "{diagnostic:?}");
    no_placeholders(&diagnostic.message);
    diagnostic
}

fn operands(message: &str) -> (&str, &str) {
    let operands = message
        .strip_prefix(&format!("{PREFIX} Actual: "))
        .unwrap_or_else(|| panic!("missing typed actual pack: {message}"));
    let (actual, expected) = operands
        .split_once(". Expected: ")
        .unwrap_or_else(|| panic!("missing typed expected pack: {message}"));
    let expected = expected
        .strip_suffix('.')
        .unwrap_or_else(|| panic!("missing diagnostic terminator: {message}"));
    assert!(!actual.is_empty() && !expected.is_empty(), "{message}");
    (actual, expected)
}

fn accepted(source: &str) {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn a_wrong_second_result_renders_both_full_packs_at_the_first_incompatible_value() {
    let diagnostic = failure(SECOND_SLOT, "second");
    let start = SECOND_SLOT.rfind("return first,second").unwrap() + "return first,".len();
    assert_eq!(diagnostic.span.start, start);
    assert_eq!(diagnostic.span.end, start + "second".len());
    assert_eq!(diagnostic.message, SECOND_SLOT_MESSAGE);
    accepted(&SECOND_SLOT.replace("---@return number, string", "---@return number, boolean"));
}

#[test]
fn omitted_results_are_not_diagnosed_as_nil_padded_physical_rows() {
    let missing = "---@return number, nil\nlocal function wrong() return 1 end\n";
    let diagnostic = failure(missing, "1");
    let (actual, expected) = operands(&diagnostic.message);
    assert_eq!(actual, "1");
    assert_eq!(expected, "(number, nil)");
    accepted(&missing.replace("return 1 end", "return 1,nil end"));

    let empty = "---@return number, nil\nlocal function wrong() return end\n";
    let diagnostic = failure(empty, "return");
    let (actual, expected) = operands(&diagnostic.message);
    assert_eq!(actual, "void");
    assert_eq!(expected, "(number, nil)");
    accepted(&empty.replace("return end", "return 1,nil end"));
}

#[test]
fn nullable_result_slots_keep_lua_nil_in_the_expected_pack() {
    let source = "---@param first number\n---@param second boolean\n---@return number, string|nil\nlocal function wrong(first,second) return first,second end\n";
    let diagnostic = failure(source, "second");
    let (actual, expected) = operands(&diagnostic.message);
    assert_eq!(actual, "(number, boolean)");
    assert!(
        expected.starts_with("(number, ") && expected.ends_with(')'),
        "{expected}"
    );
    for ty in ["string", "nil", "|"] {
        assert!(expected.contains(ty), "{expected}");
    }
    accepted(&source.replace("string|nil", "boolean|nil"));
}

#[test]
fn correlated_alternative_rows_are_not_flattened_into_independent_slot_unions() {
    let source = "---@param ok boolean\n---@return (true, number) | (false, string)\nlocal function choose(ok)\n if ok then return true,1 end\n return false,'word'\nend\n---@param ok boolean\n---@return (true, string) | (false, number)\nlocal function wrong(ok) return choose(ok) end\n";
    let diagnostic = failure(source, "choose(ok)");
    let (actual, expected) = operands(&diagnostic.message);
    for row in ["(true, number)", "(false, string)"] {
        assert!(actual.contains(row), "{actual}");
    }
    for row in ["(true, string)", "(false, number)"] {
        assert!(expected.contains(row), "{expected}");
    }
    assert!(actual.contains(" | ") && expected.contains(" | "));
    assert!(!actual.contains("number | string"), "{actual}");
    accepted(&source.replace(
        "---@return (true, string) | (false, number)",
        "---@return (true, number) | (false, string)",
    ));
}

#[test]
fn a_variadic_return_diagnostic_identifies_the_wrong_tail_and_keeps_the_guaranteed_prefix() {
    let source = "---@vararg boolean\n---@return number, string...\nlocal function wrong(...) return 1,... end\n";
    let diagnostic = failure(source, "...");
    let (actual, expected) = operands(&diagnostic.message);
    assert_eq!(actual, "(1, boolean...)");
    assert_eq!(expected, "(number, string...)");
    accepted(&source.replace("---@vararg boolean", "---@vararg string"));
}

#[test]
fn named_records_and_original_generic_names_survive_late_pack_rendering() {
    let named = "---@class Receipt\n---@field id number\nlocal Receipt={id=1}\n---@param value Receipt\n---@return boolean, {label:string}\nlocal function wrong(value) return true,value end\n";
    let diagnostic = failure(named, "value");
    let (actual, expected) = operands(&diagnostic.message);
    assert!(actual.contains("Receipt"), "{actual}");
    assert!(
        expected.contains("boolean") && expected.contains("label") && expected.contains("string"),
        "{expected}"
    );
    accepted(&named.replace("return true,value", "return true,{label='word'}"));

    let generic = "---@generic T\n---@param value T\n---@return number, string\nlocal function wrong(value) return 1,value end\n";
    let diagnostic = failure(generic, "value");
    let (actual, expected) = operands(&diagnostic.message);
    assert_eq!(actual, "(1, T)");
    assert_eq!(expected, "(number, string)");
    accepted(&generic.replace("---@return number, string", "---@return number, T"));
}

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "luanalysis return pack diagnostics {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
    fn write(&self, source: &str) {
        fs::write(self.0.join("input.lua"), source).unwrap();
    }
    fn run(&self, format: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .current_dir(&self.0)
            .args(["check", "--no-stdlib", "--format", format, "input.lua"])
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn text_cli_reports_typed_return_packs_at_the_wrong_slots_original_unicode_crlf_position() {
    let fixture = Fixture::new();
    fixture.write(SECOND_SLOT);
    let invalid = fixture.run("text");
    let stderr = String::from_utf8(invalid.stderr).unwrap();
    assert_eq!(invalid.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "input.lua:6:18: error[TS2322]: {SECOND_SLOT_MESSAGE}"
        )),
        "{stderr}"
    );
    no_placeholders(&stderr);
    fixture.write(&SECOND_SLOT.replace("---@return number, string", "---@return number, boolean"));
    let corrected = fixture.run("text");
    assert_eq!(corrected.status.code(), Some(0), "{corrected:?}");
    assert!(corrected.stderr.is_empty(), "{corrected:?}");
    assert_eq!(
        String::from_utf8(corrected.stdout).unwrap(),
        "No type errors in 1 file(s).\n"
    );
}

#[test]
fn json_cli_keeps_typed_pack_messages_codes_and_the_wrong_slots_original_utf8_byte_ranges() {
    let fixture = Fixture::new();
    fixture.write(SECOND_SLOT);
    let invalid = fixture.run("json");
    assert_eq!(invalid.status.code(), Some(1), "{invalid:?}");
    assert!(invalid.stderr.is_empty(), "{invalid:?}");
    let stdout = String::from_utf8(invalid.stdout).unwrap();
    assert!(stdout.contains("\"code\":\"TS2322\""), "{stdout}");
    assert!(
        stdout.contains(&format!("\"message\":\"{SECOND_SLOT_MESSAGE}\"")),
        "{stdout}"
    );
    let start = SECOND_SLOT.rfind("return first,second").unwrap() + "return first,".len();
    assert!(
        stdout.contains(&format!(
            "\"range\":{{\"start\":{start},\"end\":{}}}",
            start + "second".len()
        )),
        "{stdout}"
    );
    assert!(
        stdout.contains("\"position\":{\"line\":6,\"column\":18}"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\"rangeEncoding\":\"utf-8-bytes\""),
        "{stdout}"
    );
    no_placeholders(&stdout);
    fixture.write(&SECOND_SLOT.replace("---@return number, string", "---@return number, boolean"));
    let corrected = fixture.run("json");
    assert_eq!(corrected.status.code(), Some(0), "{corrected:?}");
    assert!(corrected.stderr.is_empty(), "{corrected:?}");
    let stdout = String::from_utf8(corrected.stdout).unwrap();
    assert!(
        stdout.contains("\"success\":true") && stdout.contains("\"diagnostics\":[]"),
        "{stdout}"
    );
}
