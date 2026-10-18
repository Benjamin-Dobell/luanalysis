use luanalysis::{check, location};

#[test]
fn primitive_type_tests_narrow_original_references_in_both_branches() {
    let source = "---@param value number|string\nlocal function convert(value)\nif type(value) == \"number\" then\nreturn value + 1\nelse\nreturn \"value: \" .. value\nend\nend\nlocal number = convert(42)\nlocal string = convert(\"text\")";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report.type_at(source.find("value +").unwrap()),
        Some("number")
    );
    assert_eq!(
        report.type_at(source.find(".. value").unwrap() + 3),
        Some("string")
    );
}

#[test]
fn field_runtime_type_tests_share_canonical_reference_paths() {
    let source = "---@param record {value: number|string}\nlocal function convert(record)\nif type(record[\"value\"]) == \"number\" then\nreturn record.value + 1\nend\nreturn #record[(\"value\")]\nend";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let number = source.find("record.value +").unwrap();
    let string = source.find("record[(").unwrap();
    assert_eq!(report.type_at(number + 7), Some("number"));
    assert_eq!(
        report.type_at(string + "record[(\"value\")]".len() - 1),
        Some("string")
    );
}

// Reachability is structural, as in TypeScript: a contradictory `type` test
// narrows its subject to never but leaves the branch's return reachable.
#[test]
fn contradictory_runtime_type_branches_still_add_inferred_returns() {
    let source = "---@param value number\nlocal function convert(value)\nif type(value) == \"string\" then return \"unreachable\" end\nreturn value\nend\nlocal result = convert(42)";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "result")
            .unwrap()
            .ty,
        "string | number"
    );
}

#[test]
fn nil_boolean_and_swapped_parenthesized_type_tests_use_lua_domains() {
    let source = "---@param value nil|boolean|string\nlocal function convert(value)\nif \"nil\" == (type(value)) then\nreturn \"nil\"\nelseif (type)(value) ~= (\"boolean\") then\nreturn #value\nelse\nreturn not value\nend\nend";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report.type_at(source.find("#value").unwrap() + 1),
        Some("string")
    );
    assert_eq!(
        report.type_at(source.find("not value").unwrap() + 4),
        Some("boolean")
    );
}

#[test]
fn shadowed_type_function_cannot_supply_a_runtime_type_predicate() {
    let source = "---@param value number|string\nlocal function convert(value)\n---@param ignored any\nlocal function type(ignored) return \"number\" end\nif type(value) == \"number\" then return value + 1 end\nreturn 0\nend";
    let report = check(source);
    let error = report
        .diagnostics
        .iter()
        .find(|error| error.code == "TS2362")
        .unwrap_or_else(|| panic!("{:#?}", report.diagnostics));
    assert_eq!(&source[error.span.start..error.span.end], "value");
}

#[test]
fn runtime_type_cli_reports_original_error_then_accepts_a_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis runtime types {}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("runtime.lua");
    let source = "---@param value number|string\nlocal function convert(value)\nif type(value) == \"number\" then return value + 1 end\nreturn value - 1\nend";
    let report = check(source);
    let error = report
        .diagnostics
        .iter()
        .find(|error| error.code == "TS2362")
        .unwrap_or_else(|| panic!("{:#?}", report.diagnostics));
    assert_eq!(location(source, error.span), (4, 8));
    fs::write(&file, source).unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .arg("check")
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    assert!(
        String::from_utf8(rejected.stderr)
            .unwrap()
            .contains("runtime.lua:4:8: error[TS2362]")
    );
    fs::write(&file, source.replace("value - 1", "\"value: \" .. value")).unwrap();
    let accepted = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .arg("check")
        .arg(&file)
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
