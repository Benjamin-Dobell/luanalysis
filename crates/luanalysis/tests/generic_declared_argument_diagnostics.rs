use luanalysis::{CheckReport, check, location};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

// Original reduction of Http's Response<B> through its Callback<B> alias.
const RESPONSE: &str = r#"---@shape Response<B>
---@field body B

---@alias Callback<B> fun(response:Response<B>):void

---@generic B
---@param callback Callback<B>
---@param value B
local function submit(callback,value)
    callback(42)
end

---@generic V
---@param response Response<V>
local function direct(response)
    ---@type number
    local wrong=response
end
"#;

const MESSAGES: [&str; 2] = [
    "Argument of type '42' is not assignable to parameter of type 'Response<B>'.",
    "Type 'Response<V>' is not assignable to type 'number'.",
];

const PAIRS: &str = r#"---@shape Pair<K,V>
---@field first K
---@field second V

---@alias DuplicateCallback<B> fun(pair:Pair<B,B>):void

---@generic B
---@param callback DuplicateCallback<B>
---@param value B
local function repeated(callback,value)
    callback(42)
end

---@alias Reordered<V,K> fun(pair:Pair<K,V>):void

---@param callback Reordered<number,string>
local function ordered(callback)
    callback(42)
end
"#;

fn response_correction() -> String {
    RESPONSE
        .replace("callback(42)", "callback({body=value})")
        .replace("---@type number", "---@type Response<V>")
}

fn response_report() -> CheckReport {
    let report = check(RESPONSE);
    assert_eq!(report.diagnostics.len(), 2, "{:#?}", report.diagnostics);
    for (diagnostic, code, original, message) in [
        (&report.diagnostics[0], "TS2345", "42", MESSAGES[0]),
        (&report.diagnostics[1], "TS2322", "response", MESSAGES[1]),
    ] {
        assert_eq!(diagnostic.code, code, "{diagnostic:#?}");
        assert_eq!(diagnostic.message, message);
        assert_eq!(
            &RESPONSE[diagnostic.span.start..diagnostic.span.end],
            original
        );
    }
    report
}

fn accepted(source: &str) {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn callback_alias_mappers_do_not_add_public_response_arguments_to_api_diagnostics() {
    response_report();
    accepted(&response_correction());
}

#[test]
fn genuine_repeated_arguments_and_the_declarations_own_parameter_order_are_preserved() {
    let report = check(PAIRS);
    assert_eq!(report.diagnostics.len(), 2, "{:#?}", report.diagnostics);
    for (diagnostic, expected) in report
        .diagnostics
        .iter()
        .zip(["Pair<B, B>", "Pair<string, number>"])
    {
        assert_eq!(diagnostic.code, "TS2345", "{diagnostic:#?}");
        assert_eq!(
            diagnostic.message,
            format!("Argument of type '42' is not assignable to parameter of type '{expected}'.")
        );
        assert_eq!(&PAIRS[diagnostic.span.start..diagnostic.span.end], "42");
    }
    // Reordered's own arguments are number,string, but Pair's are string,number.
    // Both original parameters must remain semantically active after printing.
    let corrected = PAIRS
        .replacen("callback(42)", "callback({first=value,second=value})", 1)
        .replace("callback(42)", "callback({first='key',second=1})");
    accepted(&corrected);
    let wrong_order = corrected.replace("{first='key',second=1}", "{first=1,second='key'}");
    let rejected = check(&wrong_order);
    assert_eq!(rejected.diagnostics.len(), 1, "{:#?}", rejected.diagnostics);
    assert_eq!(rejected.diagnostics[0].code, "TS2345");
    assert!(
        rejected.diagnostics[0]
            .message
            .ends_with("parameter of type 'Pair<string, number>'."),
        "{:#?}",
        rejected.diagnostics
    );
    accepted(&corrected);
}

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "luanalysis generic declared diagnostic arguments {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
    fn run(&self, source: &str, format: &str) -> Output {
        let file = self.0.join("main.lua");
        fs::write(&file, source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--no-stdlib", "--format", format])
            .arg(file)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn text_and_json_publish_the_declared_response_arity_at_the_original_spans() {
    let fixture = Fixture::new();
    let report = response_report();
    for format in ["text", "json"] {
        let output = fixture.run(RESPONSE, format);
        assert_eq!(output.status.code(), Some(1));
        let text = String::from_utf8(if format == "json" {
            output.stdout
        } else {
            output.stderr
        })
        .unwrap();
        let published_errors = if format == "json" {
            text.matches("\"severity\":\"error\"").count()
        } else {
            text.matches("error[").count()
        };
        assert_eq!(published_errors, report.diagnostics.len(), "{text}");
        for diagnostic in &report.diagnostics {
            assert!(text.contains(&diagnostic.message), "{text}");
            if format == "json" {
                assert!(
                    text.contains(&format!("\"code\":\"{}\"", diagnostic.code)),
                    "{text}"
                );
                assert!(
                    text.contains(&format!(
                        "\"range\":{{\"start\":{},\"end\":{}}}",
                        diagnostic.span.start, diagnostic.span.end
                    )),
                    "{text}"
                );
            } else {
                let (line, column) = location(RESPONSE, diagnostic.span);
                assert!(
                    text.contains(&format!(
                        "main.lua:{line}:{column}: error[{}]",
                        diagnostic.code
                    )),
                    "{text}"
                );
            }
        }
        assert!(!text.contains("Response<B, B>"), "{text}");
        assert!(!text.contains("contextual type"), "{text}");
        let corrected = fixture.run(&response_correction(), format);
        assert!(
            corrected.status.success(),
            "{}",
            String::from_utf8_lossy(&corrected.stderr)
        );
    }
}
