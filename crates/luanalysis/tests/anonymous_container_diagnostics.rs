use luanalysis::{CheckReport, check, location};
use std::{fs, process::Command};

const CONTRACTS: &str = "---@class Holder\nlocal Holder={}\n---@type number[]\nHolder.values={'wrong'}\n---@type table<string,number>\nHolder.dictionary={value='wrong'}\n";

fn corrected_contracts() -> String {
    CONTRACTS.replace("'wrong'", "1")
}

fn contract_report() -> CheckReport {
    let report = check(CONTRACTS);
    assert_eq!(report.diagnostics.len(), 2, "{:#?}", report.diagnostics);
    for (diagnostic, original, expected) in [
        (&report.diagnostics[0], "{'wrong'}", "number[]"),
        (
            &report.diagnostics[1],
            "{value='wrong'}",
            "table<string, number>",
        ),
    ] {
        assert_eq!(diagnostic.code, "TS2322");
        assert_eq!(
            &CONTRACTS[diagnostic.span.start..diagnostic.span.end],
            original
        );
        assert!(
            diagnostic.message.ends_with(&format!("type '{expected}'.")),
            "{diagnostic:#?}"
        );
        for placeholder in [
            "type 'array'",
            "type 'table'",
            "contextual type",
            "named type",
        ] {
            assert!(!diagnostic.message.contains(placeholder), "{diagnostic:#?}");
        }
    }
    report
}

fn accepted(source: &str) {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn written_namespace_containers_render_their_original_element_and_indexer_contracts() {
    contract_report();
    accepted(&corrected_contracts());
}

#[test]
fn rendering_preserves_strict_dictionary_key_and_value_checks() {
    let source = format!(
        "{}Holder.dictionary[true]=1\nHolder.dictionary.value='wrong'\n",
        corrected_contracts()
    );
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 2, "{:#?}", report.diagnostics);
    for (diagnostic, code, original) in [
        (&report.diagnostics[0], "TS2538", "true"),
        (&report.diagnostics[1], "TS2322", "'wrong'"),
    ] {
        assert_eq!(diagnostic.code, code, "{diagnostic:#?}");
        assert_eq!(
            &source[diagnostic.span.start..diagnostic.span.end],
            original
        );
    }
    assert!(report.diagnostics[0].message.contains("'string'"));
    assert!(report.diagnostics[1].message.contains("'number'"));
    accepted(
        &source
            .replace("[true]=1", "['key']=1")
            .replace(".value='wrong'", ".value=2"),
    );
}

#[test]
fn nullable_and_function_array_elements_keep_postfix_parentheses() {
    let source = "---@class Holder\nlocal Holder={}\n---@type (number|nil)[]\nHolder.nullable={'wrong'}\n---@type (fun(value:number):string)[]\nHolder.callbacks={42}\n";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 2, "{:#?}", report.diagnostics);
    let targets: Vec<_> = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            assert_eq!(diagnostic.code, "TS2322");
            let target = diagnostic
                .message
                .split_once("is not assignable to type '")
                .unwrap_or_else(|| panic!("{diagnostic:#?}"))
                .1
                .strip_suffix("'.")
                .unwrap();
            let element = target
                .strip_suffix("[]")
                .unwrap_or_else(|| panic!("{target}"));
            assert!(
                element.starts_with('(') && element.ends_with(')'),
                "{target}"
            );
            target
        })
        .collect();
    for expected in ["number", "nil", " | "] {
        assert!(targets[0].contains(expected), "{}", targets[0]);
    }
    for expected in ["value: number", "string"] {
        assert!(targets[1].contains(expected), "{}", targets[1]);
    }
    accepted(
        &source
            .replace("Holder.nullable={'wrong'}", "Holder.nullable={nil,1}")
            .replace(
                "Holder.callbacks={42}",
                "Holder.callbacks={function(value) return 'word' end}",
            ),
    );
}

#[test]
fn text_and_json_publish_anonymous_container_contracts_and_accept_numeric_corrections() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis anonymous container diagnostics {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("main.lua");
    let report = contract_report();
    for format in ["text", "json"] {
        fs::write(&file, CONTRACTS).unwrap();
        let run = || {
            Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
                .args(["check", "--no-stdlib", "--format", format])
                .arg(&file)
                .output()
                .unwrap()
        };
        let output = run();
        assert_eq!(output.status.code(), Some(1));
        let text = String::from_utf8(if format == "json" {
            output.stdout
        } else {
            output.stderr
        })
        .unwrap();
        for diagnostic in &report.diagnostics {
            if format == "json" {
                // Literal field types contain quotes in the message; JSON
                // escapes those characters instead of publishing raw text.
                let message = diagnostic.message.replace('\\', "\\\\").replace('"', "\\\"");
                assert!(text.contains(&format!("\"message\":\"{message}\"")), "{text}");
                assert!(
                    text.contains(&format!(
                        "\"range\":{{\"start\":{},\"end\":{}}}",
                        diagnostic.span.start, diagnostic.span.end
                    )),
                    "{text}"
                );
            } else {
                assert!(text.contains(&diagnostic.message), "{text}");
                let (line, column) = location(CONTRACTS, diagnostic.span);
                assert!(
                    text.contains(&format!("main.lua:{line}:{column}: error[TS2322]")),
                    "{text}"
                );
            }
        }
        fs::write(&file, corrected_contracts()).unwrap();
        let corrected = run();
        assert!(
            corrected.status.success(),
            "{}",
            String::from_utf8_lossy(&corrected.stderr)
        );
    }
    fs::remove_dir_all(directory).unwrap();
}
