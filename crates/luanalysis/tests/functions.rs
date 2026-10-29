use luanalysis::{check, location};

#[test]
fn annotated_functions_and_nested_scopes_use_original_binding_types() {
    let source = include_str!("../../../examples/functions.lua");
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let inner_read = source.find("title = value").unwrap() + "title = ".len();
    let outer_read = source.find("return value").unwrap() + "return ".len();
    assert_eq!(report.type_at(inner_read), Some("string"));
    assert_eq!(report.type_at(outer_read), Some("number"));
    let bound = report.bound.as_ref().unwrap();
    assert_ne!(bound.binding_at(inner_read), bound.binding_at(outer_read));
    for (name, expected) in [
        ("checked", "{ id: number, title: string }"),
        ("checkedOuter", "number"),
        ("checkedExpression", "string"),
    ] {
        assert_eq!(
            report
                .bindings
                .iter()
                .find(|binding| binding.name == name)
                .unwrap()
                .ty,
            expected
        );
    }
}

#[test]
fn function_argument_and_return_failures_keep_original_lua_ranges() {
    let source = include_str!("../../../examples/functions-error.lua");
    let report = check(source);
    assert_eq!(
        report
            .diagnostics
            .iter()
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        ["TS2345", "TS2322"]
    );
    assert_eq!(location(source, report.diagnostics[0].span), (12, 36));
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "\"card\""
    );
    assert_eq!(location(source, report.diagnostics[1].span), (18, 12));
    let corrected = source
        .replace("identityId(\"card\")", "identityId(7)")
        .replace("id = \"seven\"", "id = 7");
    assert!(check(&corrected).diagnostics.is_empty());
}

#[test]
fn local_function_recursion_is_distinct_from_initializer_self_reference() {
    let named = "---@param value number\n---@return number\nlocal function recursive(value) return recursive(value) end";
    assert!(check(named).diagnostics.is_empty());
    let ordinary = "---@param value number\n---@return number\nlocal recursive = function(value) return recursive(value) end";
    assert_eq!(
        check(ordinary)
            .diagnostics
            .iter()
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        ["TS2304"]
    );
}

#[test]
fn function_type_annotations_contextualize_native_parameters_and_returns() {
    for initializer in [
        "function(value) return value end",
        "(function(value) return value end)",
    ] {
        let source = format!(
            "---@type fun(value: number): number\nlocal identity = {initializer}\nlocal result = identity(42)"
        );
        let report = check(&source);
        assert!(
            report.diagnostics.is_empty(),
            "{initializer}: {:#?}",
            report.diagnostics
        );
        let read = source.find("return value").unwrap() + "return ".len();
        assert_eq!(report.type_at(read), Some("number"));
    }
}

#[test]
fn optional_parameters_and_native_call_arity_are_checked() {
    let source = "---@param value? number\n---@return number|nil\nlocal function optional(value) return value end\noptional()\noptional(nil)\noptional(1)\noptional(1, 2)";
    let report = check(source);
    // An optional parameter may be absent, but takes no nil it does not declare, as in v1
    // (function_optional_parameters.lua).
    assert_eq!(
        report
            .diagnostics
            .iter()
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        ["TS2345", "TS2554"]
    );
    let body_read = source.find("return value").unwrap() + "return ".len();
    assert_eq!(report.type_at(body_read), Some("nil | number"));
    let missing = "---@param value number\n---@return number\nlocal function required(value) return value end\nrequired()";
    assert_eq!(check(missing).diagnostics[0].code, "TS2554");
}

#[test]
fn higher_order_calls_use_native_function_variance() {
    let source = "---@param value number\n---@return number\nlocal function identity(value) return value end\n---@param callback fun(value: number): number\n---@return number\nlocal function apply(callback) return callback(42) end\nlocal result = apply(identity)";
    assert!(check(source).diagnostics.is_empty());
    let rejected = source
        .replacen("---@param value number", "---@param value string", 1)
        .replacen("return value", "return 42", 1);
    assert_eq!(
        check(&rejected)
            .diagnostics
            .iter()
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        ["TS2345"]
    );
}

#[test]
fn declaration_checking_does_not_execute_captured_writes() {
    let source = "---@type number|string\nlocal value = 1\n---@return nil\nlocal function write() value = \"later\"; return nil end\n---@type number\nlocal current = value";
    assert!(check(source).diagnostics.is_empty());
    let unsafe_capture = "---@type number|string\nlocal value = 1\n---@return number\nlocal function read() return value end\nvalue = \"later\"";
    assert_eq!(
        check(unsafe_capture)
            .diagnostics
            .iter()
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        ["TS2322"]
    );
}

#[test]
fn fixed_result_packs_and_empty_call_scalarization_use_native_types() {
    let source = "---@return number, string\nlocal function pair() return 1, \"second\" end\nlocal number,text,missing=pair()\n---@return void\nlocal function nothing() end\nlocal nil_value=(nothing())\n---@type string\nlocal rejected=number";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    for (name, expected) in [
        ("number", "number"),
        ("text", "string"),
        ("missing", "nil"),
        ("nil_value", "nil"),
    ] {
        assert_eq!(
            report
                .bindings
                .iter()
                .find(|binding| binding.name == name)
                .unwrap()
                .ty,
            expected
        );
    }
}

#[test]
fn function_contracts_and_optional_order_cannot_silently_disagree() {
    let conflicting = "---@type fun(value: number): number\n---@param value string\n---@return string\nlocal function changed(value) return value end";
    assert_eq!(
        check(conflicting)
            .diagnostics
            .iter()
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        ["TS2322"]
    );
    let order = "---@return number\nlocal function value() return 1 end\n---@type fun(first?: number, second: number): number\nlocal invalid = value";
    assert!(
        check(order)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS1016")
    );
    let exposed = "---@type fun(): unknown\n---@return number\nlocal function known() return 42 end\n---@type number\nlocal rejected = known()";
    assert_eq!(
        check(exposed)
            .diagnostics
            .iter()
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        ["TS2322"]
    );
}

#[test]
fn native_implicit_return_policy_distinguishes_nil_from_nullable_unions() {
    for returned in ["number", "unknown", "number|nil"] {
        let source = format!("---@return {returned}\nlocal function empty() end");
        assert_eq!(
            check(&source)
                .diagnostics
                .iter()
                .map(|error| error.code)
                .collect::<Vec<_>>(),
            ["TS2355"],
            "{returned}"
        );
    }
    for returned in ["nil", "void", "any", "number|void"] {
        let source = format!("---@return {returned}\nlocal function empty() end");
        assert!(check(&source).diagnostics.is_empty(), "{returned}");
    }
    assert_eq!(
        check("---@return never\nlocal function empty() end").diagnostics[0].code,
        "TS2534"
    );
    let terminal = "---@return never\nlocal function spin() spin() end\n---@return number\nlocal function stopped() spin() end";
    assert!(check(terminal).diagnostics.is_empty());
}

#[test]
fn anonymous_function_declaration_queries_return_binding_types() {
    let source = "---@param value number\n---@return number\nlocal identity = function(value) local bodyLocal = value return bodyLocal end";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let parameter = source.find("function(value)").unwrap() + "function(".len();
    let local = source.find("local bodyLocal").unwrap() + "local ".len();
    assert_eq!(report.type_at(parameter), Some("number"));
    assert_eq!(report.type_at(local), Some("number"));
}

#[test]
fn annotated_call_cli_passes_fails_then_accepts_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis-functions-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("functions.lua");
    let prefix = "---@param value number\n---@return number\nlocal function identity(value) return value end\n";
    fs::write(&file, format!("{prefix}local result = identity(\"wrong\")")).unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .arg("check")
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("functions.lua:4:25: error[TS2345]")
    );
    fs::write(&file, format!("{prefix}local result = identity(42)")).unwrap();
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
