use luanalysis::{CheckReport, check};

fn accepted(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}
fn ty<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .unwrap()
        .ty
}

#[test]
fn raw_table_accepts_only_tables_and_permits_original_member_operations() {
    let source = "---@param value table\nlocal function use(value)\nvalue.field=42\nvalue[false]=\"word\"\nlocal read=value.field\nlocal size=#value\nreturn read,size\nend\nlocal read,size=use({field=42})";
    let report = accepted(source);
    assert_eq!(ty(&report, "read"), "any");
    assert_eq!(ty(&report, "size"), "number");
    for invalid in ["42", "\"word\"", "false", "nil", "function() end"] {
        let report = check(&source.replace("use({field=42})", &format!("use({invalid})")));
        assert!(
            report.diagnostics.iter().any(|d| d.code == "TS2345"),
            "{invalid}: {:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn raw_function_accepts_concrete_signatures_and_calls_keep_any_result_packs() {
    let source = "---@param callback function\nlocal function use(callback)\nlocal first,second,third=callback(42,\"word\")\nreturn first,second,third\nend\nlocal first,second,third=use(function() return 42 end)";
    let report = accepted(source);
    for name in ["first", "second", "third"] {
        assert_eq!(ty(&report, name), "any");
    }
    for invalid in ["42", "\"word\"", "false", "nil", "{}"] {
        let report =
            check(&source.replace("use(function() return 42 end)", &format!("use({invalid})")));
        assert!(
            report.diagnostics.iter().any(|d| d.code == "TS2345"),
            "{invalid}: {:#?}",
            report.diagnostics
        );
    }
    let with_parameters = source.replace(
        "use(function() return 42 end)",
        "use(function(value) return value end)",
    );
    let report = accepted(&with_parameters);
    assert_eq!(ty(&report, "value"), "any");
}

#[test]
fn primitive_aliases_and_generic_constraints_retain_runtime_domains() {
    let source = "---@alias RawTable table\n---@alias RawFunction function\n---@generic T:RawTable\n---@param value T\n---@return T\nlocal function identity(value) return value end\nlocal object=identity({field=42})\n---@type number\nlocal field=object.field\n---@param callback RawFunction\nlocal function apply(callback) return callback() end\nlocal result=apply(function() return \"word\" end)";
    let report = accepted(source);
    assert_eq!(ty(&report, "field"), "number");
    assert_eq!(ty(&report, "result"), "any");
    let report = check(&source.replace("identity({field=42})", "identity(42)"));
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| matches!(d.code, "TS2345" | "TS2344")),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn raw_table_generic_dictionary_calls_infer_their_existing_open_domains() {
    let source = "---@generic K,V\n---@param tab table<K,V>\n---@return table<K,V>\nlocal function identity(tab) return tab end\n---@param tab table\nlocal function apply(tab)\nlocal dictionary=identity(tab)\nreturn dictionary[false]\nend\nlocal result=apply({[false]=42})";
    let report = accepted(source);
    assert_eq!(ty(&report, "result"), "any");
    for argument in ["42", "function() end"] {
        let wrong = source.replace("apply({[false]=42})", &format!("apply({argument})"));
        let report = check(&wrong);
        assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
        assert_eq!(report.diagnostics[0].code, "TS2345");
    }
}

#[test]
fn raw_function_generic_bound_retains_the_inferred_specific_callable() {
    let source = "---@alias Callable function\n---@generic F:Callable\n---@param callback F\n---@return F\nlocal function keep(callback) return callback end\n---@param value number\n---@return number\nlocal function numeric(value) return value end\nlocal specific=keep(numeric)\nlocal result=specific(42)";
    let report = accepted(source);
    assert_eq!(ty(&report, "result"), "number");
    let report = check(&source.replace("specific(42)", "specific(\"wrong\")"));
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
fn runtime_type_predicates_keep_table_and_function_domains_separate() {
    let source = "---@param value table|function|number\nlocal function use(value)\nif type(value)==\"table\" then\n---@type table\nlocal object=value\nobject.field=42\nelseif type(value)==\"function\" then\n---@type function\nlocal callback=value\ncallback()\nelse\n---@type number\nlocal numeric=value\nend\nend";
    let report = accepted(source);
    assert_eq!(ty(&report, "object"), "table");
    assert_eq!(ty(&report, "callback"), "function");
    assert_eq!(ty(&report, "numeric"), "number");
}

#[test]
fn unknown_and_generic_runtime_predicates_refine_to_native_operation_types() {
    accepted(
        "---@param value unknown\nlocal function unknown(value)\nif type(value)==\"table\" then value.field=42 end\nif type(value)==\"function\" then value() end\nend\n---@generic T\n---@param value T\nlocal function generic(value)\nif type(value)==\"table\" then value.field=42 end\nif type(value)==\"function\" then value() end\nend",
    );
}

#[test]
fn bare_fun_is_a_raw_callable_while_explicit_empty_parentheses_keep_zero_arity() {
    let source = "---@param callback fun\nlocal function use(callback) return callback(42,\"word\") end\nlocal result=use(function() return 42 end)";
    let report = accepted(source);
    assert_eq!(ty(&report, "callback"), "function");
    assert_eq!(ty(&report, "result"), "any");
    let report = check(&source.replace("@param callback fun", "@param callback fun()"));
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2554"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn cli_raw_runtime_annotation_error_uses_original_source_and_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis raw primitives {}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("primitives.lua");
    let source = "---@param value table\r\nlocal function use(value) return value.field end\r\nlocal é=use(function() end)";
    fs::write(&path, source).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .arg("check")
            .arg("--dump-types")
            .arg(&path)
            .output()
            .unwrap()
    };
    let rejected = run();
    assert_eq!(rejected.status.code(), Some(1));
    let errors = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        errors.contains("primitives.lua:3:13: error[TS2345]"),
        "{errors}"
    );
    assert!(errors.contains("'table'"), "{errors}");
    fs::write(
        &path,
        source.replace("use(function() end)", "use({field=42})"),
    )
    .unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    assert!(String::from_utf8_lossy(&corrected.stdout).contains("é: any"));
    fs::remove_dir_all(directory).unwrap();
}
