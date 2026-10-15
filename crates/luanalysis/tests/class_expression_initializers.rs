use luanalysis::check;
use std::{fs, process::Command};

#[test]
fn class_expression_initializers_validate_actual_tables_and_preserve_nominal_identity() {
    let source = "---@generic T\n---@param value T\n---@return T\nlocal function identity(value) return value end\n---@class C\n---@field value number\nlocal definition=identity({value=42})\n---@param instance C\nlocal function consume(instance) end\nconsume(definition)\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let expanded = source
        .replace("---@return T\n", "---@return T,string\n")
        .replace("return value end", "return value,'discarded' end");
    let report = check(&expanded);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let parenthesized = source.replace("identity({value=42})", "(identity({value=42}))");
    let report = check(&parenthesized);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    for invalid in [
        source.replace("identity({value=42})", "identity(42)"),
        source.replace("identity({value=42})", "identity({value='wrong'})"),
        source.replace("consume(definition)", "consume({value=42})"),
        source.replace(
            "consume(definition)",
            "---@class Rebranded\nlocal other=identity(definition)",
        ),
    ] {
        let report = check(&invalid);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|error| matches!(error.code, "TS2322" | "TS2345")),
            "{invalid}\n{:#?}",
            report.diagnostics
        );
        assert!(
            !report
                .diagnostics
                .iter()
                .any(|error| error.code == "LUA2031"),
            "{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn real_standard_setmetatable_can_initialize_a_checked_class_without_name_based_trust() {
    let root = std::env::temp_dir().join(format!(
        "luanalysis class expression {}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("null.lua");
    let source = "---@class JsonNull\nlocal NULL=setmetatable({}, {__index={}, __metatable=false})\n---@return JsonNull\nlocal function null() return NULL end\nlocal accepted=null()\n";
    let run = |source: &str| {
        fs::write(&path, source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--format", "json", "--lua-version", "5.4"])
            .arg(&path)
            .output()
            .unwrap()
    };
    let accepted = run(source);
    assert_eq!(
        accepted.status.code(),
        Some(0),
        "{}{}",
        String::from_utf8_lossy(&accepted.stdout),
        String::from_utf8_lossy(&accepted.stderr)
    );
    let shadowed = format!(
        "---@param table table\n---@param meta table\n---@return number\nlocal function setmetatable(table,meta) return 42 end\n{source}"
    );
    let rejected = run(&shadowed);
    assert_eq!(
        rejected.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&rejected.stdout)
    );
    assert!(
        String::from_utf8_lossy(&rejected.stdout).contains("TS2322"),
        "{}",
        String::from_utf8_lossy(&rejected.stdout)
    );
    assert_eq!(run(source).status.code(), Some(0));
    fs::remove_dir_all(root).unwrap();
}
