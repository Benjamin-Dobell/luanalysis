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
fn ty<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .expect(name)
        .ty
}
const FACTORY: &str =
    "---@class Factory\n---@overload fun(value:number):number\nlocal factory={}\n";

#[test]
fn a_callable_definition_satisfies_table_ascription_and_length() {
    let report = clean(&format!(
        "{FACTORY}---@type table\nlocal asTable=factory\nlocal alias=factory\nlocal result=alias(42)\nlocal length=#asTable\nlocal originalLength=#factory"
    ));
    assert_eq!(ty(&report, "result"), "number");
    assert_eq!(ty(&report, "length"), "number");
    assert_eq!(ty(&report, "originalLength"), "number");
    let erased = check(&format!("{FACTORY}---@type table\nlocal asTable=factory\nasTable(42)"));
    assert!(erased.diagnostics.iter().any(|error| error.code == "TS2349"));
}

#[test]
fn callable_heads_do_not_allow_a_definition_to_satisfy_the_runtime_function_domain() {
    for annotation in ["function", "function|number"] {
        let source = format!("{FACTORY}---@type {annotation}\nlocal invalid=factory");
        let report = check(&source);
        assert!(
            report.diagnostics.iter().any(|error| error.code == "TS2322"
                && &source[error.span.start..error.span.end] == "factory"),
            "{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn a_table_union_accepts_a_definition_and_narrows_to_the_declared_table_member() {
    let report = clean(&format!(
        "{FACTORY}---@type table|number\nlocal value=factory\nlocal length=#value"
    ));
    assert_eq!(ty(&report, "length"), "number");
    let erased = check(&format!("{FACTORY}---@type table|number\nlocal value=factory\nvalue(42)"));
    assert!(erased.diagnostics.iter().any(|error| error.code == "TS2349"));
}

#[test]
fn a_table_generic_constraint_preserves_the_callable_definition() {
    let report = clean(&format!(
        "{FACTORY}---@generic T:table\n---@param value T\n---@return T\nlocal function identity(value) return value end\nlocal value=identity(factory)\nlocal result=value(42)"
    ));
    assert_eq!(ty(&report, "result"), "number");
}

#[test]
fn a_function_generic_constraint_rejects_an_overloaded_table_definition() {
    let source = format!(
        "{FACTORY}---@generic T:function\n---@param value T\n---@return T\nlocal function identity(value) return value end\nlocal invalid=identity(factory)"
    );
    let report = check(&source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2345"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_inferred_alias_keeps_partial_members_and_original_write_constraints() {
    let source = "---@class C\n---@field value number\nlocal definition={}\nlocal alias=definition\nlocal before=alias.value\nalias.value=42\nlocal after=alias.value";
    let report = clean(source);
    // A member not yet given reads as the class declares it.
    assert_eq!(ty(&report, "before"), "number");
    assert_eq!(ty(&report, "after"), "number");
    let invalid = source.replace("alias.value=42", "alias.value=\"wrong\"");
    assert!(
        check(&invalid)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2322")
    );
}

#[test]
fn table_constraints_do_not_weaken_the_complete_nominal_instance_check() {
    let prefix = "---@class C\n---@field value number\nlocal definition={}\nlocal alias=definition\n---@param value C\nlocal function consume(value) end\nconsume(alias)";
    // The class table is of its class before it has every member; a raw table is not.
    clean(prefix);
    clean(&prefix.replace("local definition={}", "local definition={value=42}"));
    let raw = prefix
        .replace("local definition={}", "local definition={value=42}")
        .replace("local alias=definition", "local alias={value=42}");
    assert!(
        check(&raw)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2345")
    );
    let erased = prefix
        .replace("local definition={}", "local definition={value=42}")
        .replace("local alias=definition", "---@type table\nlocal alias=definition");
    assert!(check(&erased).diagnostics.iter().any(|error| error.code == "TS2345"));
}

#[test]
fn actual_cli_preserves_the_definition_runtime_domain_and_reports_original_source() {
    use std::{fs, process::Command};
    let directory = std::env::temp_dir().join(format!(
        "luanalysis-definition-runtime-{}", std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("factory.ttslua");
    let source = format!("{FACTORY}---@type function\nlocal value=factory\n");
    let run = |source: &str| {
        fs::write(&path, source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .args(["check", "--dump-types"])
            .arg(&path)
            .output()
            .unwrap()
    };
    let rejected = run(&source);
    assert_eq!(rejected.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains(
        "factory.ttslua:5:13: error[TS2322]"
    ), "{}", String::from_utf8_lossy(&rejected.stderr));
    let corrected = source.replace("---@type function", "---@type table");
    let accepted = run(&format!("{corrected}local length=#value\nlocal result=factory(42)\n"));
    assert_eq!(accepted.status.code(), Some(0), "{}", String::from_utf8_lossy(&accepted.stderr));
    fs::remove_dir_all(&directory).unwrap();
}
