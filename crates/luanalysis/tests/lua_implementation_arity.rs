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

const DEFAULT: &str = "---@overload fun():string\n---@overload fun(value:string):string\n---@param value string|nil\n---@return string\nlocal function example(value) return value or 'default' end\nlocal empty=example()\nlocal provided=example('text')\n";

#[test]
fn physical_nil_padding_preserves_public_arity_and_written_body_types() {
    let report = clean(DEFAULT);
    let parameter = DEFAULT.find("example(value)").unwrap() + "example(".len();
    let parameter_type = report.type_at(parameter).unwrap();
    assert!(
        parameter_type.contains("string") && parameter_type.contains("nil"),
        "{parameter_type}"
    );
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
fn omitted_and_optional_public_arguments_are_nil_in_the_private_contract() {
    let nonnullable = DEFAULT.replace("---@param value string|nil", "---@param value string");
    let report = check(&nonnullable);
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|error| error.code == "TS2394")
            .count(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(
        report.type_at(nonnullable.find("example(value)").unwrap() + "example(".len()),
        Some("string")
    );
    clean(&nonnullable.replace("---@param value string", "---@param value string|nil"));
    let optional = "---@overload fun(value?:string):string\n---@param value string\n---@return string\nlocal function optional(value) return value or 'default' end\n";
    assert!(
        check(optional)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2394")
    );
    clean(&optional.replace("---@param value string", "---@param value string|nil"));
    let second = "---@overload fun(value:string):string\n---@param value string\n---@param count number\n---@return string\nlocal function second(value,count) return value end\n";
    assert!(
        check(second)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2394")
    );
    clean(&second.replace("---@param count number", "---@param count number|nil"));
}

#[test]
fn overload_parameter_types_never_retype_the_long_form_body() {
    let precise = "---@overload fun(value:\"literal\"):string\n---@param value string\n---@return string\nlocal function precise(value) return value end\n";
    let report = clean(precise);
    assert_eq!(
        report.type_at(precise.find("precise(value)").unwrap() + "precise(".len()),
        Some("string")
    );
    let broad = precise.replace("value:\"literal\"", "value:string|number");
    let report = check(&broad);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2394")
    );
    assert_eq!(
        report.type_at(broad.find("precise(value)").unwrap() + "precise(".len()),
        Some("string")
    );
}

#[test]
fn a_public_overload_does_not_annotate_an_untyped_implementation_parameter() {
    let source = "---@overload fun(value:number):number\nlocal function untyped(value) return value end\nlocal result=untyped(1)\n";
    let report = luanalysis::check_with_options(source, luanalysis::CheckOptions { no_implicit_any: true, ..Default::default() });
    let errors: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|error| error.code == "TS7006")
        .collect();
    assert_eq!(errors.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(&source[errors[0].span.start..errors[0].span.end], "value");
    assert_eq!(
        report.type_at(source.find("untyped(value)").unwrap() + "untyped(".len()),
        Some("any")
    );
    let corrected = source.replacen(
        "local function",
        "---@param value number\nlocal function",
        1,
    );
    clean(&corrected);
    let rejected = format!("{corrected}untyped('wrong')\n");
    assert!(
        check(&rejected)
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2345" | "TS2769"))
    );
}

#[test]
fn an_erased_generic_implementation_parameter_does_not_imply_nil_admission() {
    let source = "---@overload fun():string\n---@generic T:string\n---@param value T\n---@return string\nlocal function generic(value) return 'result' end\n";
    assert!(
        check(source)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2394")
    );
    clean(&source.replace("---@param value T", "---@param value T|nil"));
}

#[test]
fn public_rest_arguments_include_nil_when_the_fixed_implementation_slot_is_absent() {
    let source = "---@overload fun(...:number):string\n---@param value number|nil\n---@return string\nlocal function rest(value) return 'result' end\nrest()\nrest(1,2)\n";
    clean(source);
    assert!(
        check(&source.replace("---@param value number|nil", "---@param value number"))
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2394")
    );
}

#[test]
fn omitted_fixed_arguments_keep_the_original_variadic_element_contract() {
    let source = "---@overload fun():string\n---@overload fun(value:string,...:number):string\n---@param value string|nil\n---@vararg number\n---@return string\nlocal function variadic(value,...) return value or 'default' end\nvariadic()\nvariadic('value',1,2)\n";
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
---@param value T|nil
---@return T
local function pick(value)
value = value or captured
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
fn generic_logical_fallback_assignments_reduce_only_the_ordinary_value_type() {
    for absent in ["nil", "false", "nil|false"] {
        let source = format!(
            "---@generic T\n---@param value T|{absent}\n---@param fallback T\n---@return T\nlocal function pick(value,fallback)\n value=value or fallback\n ---@type T\n local checked=value\n return checked\nend\n"
        );
        clean(&source);
        let invalid = source.replace("local checked=value", "local checked=42");
        assert!(
            check(&invalid).diagnostics.iter().any(|error| error.code == "TS2322"),
            "{invalid}"
        );
        let unnarrowed = source.replace("value=value or fallback", "value=value");
        assert!(
            check(&unnarrowed).diagnostics.iter().any(|error| error.code == "TS2322"),
            "{unnarrowed}"
        );
        clean(&invalid.replace("local checked=42", "local checked=value"));
    }
}

#[test]
fn original_base64_decoding_map_short_heads_accept_the_runtime_implementation() {
    let source = r#"---@class Base64
local Base64={}
---@overload fun(char62:string,char63:string):table<number,number>
---@overload fun(char62:string):table<number,number>
---@overload fun():table<number,number>
---@param char62 string|nil
---@param char63 string|nil
---@param charPad? string
---@return table<number,number>
function Base64.decodingMap(char62,char63,charPad)
char62=char62 or "+"
char63=char63 or "/"
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

#[test]
fn ambient_private_main_checks_nil_padding_without_executing_placeholder_bodies() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis ambient implementation arity {}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let library = directory.join("original.def.lua");
    let main = directory.join("main.lua");
    std::fs::write(&main, "local result=example()\n").unwrap();
    let source = "---@overload fun():string\n---@overload fun(value:string):string\n---@param value string\n---@return string\nfunction example(value) unknown_placeholder_body() end\n";
    std::fs::write(&library, source).unwrap();
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--no-stdlib", "--format", "json", "--library"])
            .arg(&library)
            .arg(&main)
            .output()
            .unwrap()
    };
    let output = run();
    let report = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{report}");
    assert_eq!(report.matches("\"code\":\"TS2394\"").count(), 1, "{report}");
    assert!(
        !report.contains("TS2304"),
        "a placeholder body was executed: {report}"
    );
    std::fs::write(
        &library,
        source.replace("---@param value string", "---@param value string|nil"),
    )
    .unwrap();
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    std::fs::write(&main, "example(nil)\n").unwrap();
    let output = run();
    let report = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{report}");
    assert!(!report.contains("TS2394"), "{report}");
    assert!(
        report.contains("TS2345") || report.contains("TS2769"),
        "the private nullable main leaked publicly: {report}"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ambient_protected_call_main_keeps_the_zero_result_success_row() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis protected result rows {}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let library = directory.join("original.def.lua");
    let main = directory.join("main.lua");
    std::fs::write(&main, "local success=protected(function() end)\n").unwrap();
    let source = "---@overload fun(f:(fun:void)):true|(false,any)\n---@overload fun<T>(f:(fun:T)):(true,T)|(false,any)\n---@generic T\n---@param f (fun:T)|(fun:void)\n---@return (true,T)|(false,any)\nfunction protected(f) end\n";
    std::fs::write(&library, source).unwrap();
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--no-stdlib", "--format", "json", "--library"])
            .arg(&library)
            .arg(&main)
            .output()
            .unwrap()
    };
    let output = run();
    let report = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{report}");
    assert_eq!(report.matches("\"code\":\"TS2394\"").count(), 1, "{report}");
    std::fs::write(
        &library,
        source.replace(
            "---@return (true,T)|(false,any)",
            "---@return true|(true,T)|(false,any)",
        ),
    )
    .unwrap();
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    std::fs::remove_dir_all(directory).unwrap();
}
