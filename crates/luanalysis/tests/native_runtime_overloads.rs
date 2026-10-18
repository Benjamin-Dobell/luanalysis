use luanalysis::{CheckReport, check, location};

fn accepted(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}
fn binding_type<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .unwrap()
        .ty
}

const CLASSIFY: &str = "---@overload fun(value:table):\"table\"\n---@overload fun(value:function):\"function\"\n---@param value table|function\n---@return \"table\"|\"function\"\nlocal function classify(value)\nif type(value)==\"table\" then return \"table\" end\nreturn \"function\"\nend\n";

#[test]
fn overload_heads_select_original_raw_runtime_parameter_domains() {
    let source = format!(
        "{CLASSIFY}---@type \"table\"\nlocal object=classify({{}})\n---@type \"function\"\nlocal callback=classify(function() end)"
    );
    let report = accepted(&source);
    assert_eq!(binding_type(&report, "object"), "\"table\"");
    assert_eq!(binding_type(&report, "callback"), "\"function\"");
    let public = report
        .type_at(source.find("classify(value)").unwrap())
        .unwrap();
    assert!(
        public.contains("fun(value: table)") && public.contains("fun(value: function)"),
        "{public}"
    );
    let bad = format!("{source}\nlocal rejected=classify(42)");
    let report = check(&bad);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2345" | "TS2769")),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn callable_shape_and_class_remain_runtime_tables_in_overloads_and_predicates() {
    for declaration in ["shape", "class"] {
        let source = format!(
            "{CLASSIFY}---@param value Callable\nlocal function use(value)\n---@type table\nlocal original=value\n---@type \"table\"\nlocal category=classify(value)\nif type(value)==\"table\" then\n---@type number\nlocal result=value(42)\nend\nreturn category\nend\n---@{declaration} Callable\n---@overload fun(value:number):number"
        );
        let report = accepted(&source);
        assert_eq!(
            binding_type(&report, "category"),
            "\"table\"",
            "{declaration}"
        );
        assert_eq!(binding_type(&report, "result"), "number", "{declaration}");
        let bad = format!(
            "---@param callback function\nlocal function take(callback) end\n{}",
            source.replace("local original=value", "local original=value\ntake(value)")
        );
        let rejected = check(&bad);
        assert!(
            rejected
                .diagnostics
                .iter()
                .any(|error| error.code == "TS2345"),
            "{declaration}: {:#?}",
            rejected.diagnostics
        );
    }
}

#[test]
fn overloaded_generic_runtime_bounds_keep_specific_record_and_function_types() {
    let source = "---@overload fun<T:table>(value:T):T\n---@param value any\n---@return any\nlocal function tableIdentity(value) return value end\nlocal object=tableIdentity({field=42})\n---@type number\nlocal field=object.field\n---@overload fun<F:function>(value:F):F\n---@param value any\n---@return any\nlocal function functionIdentity(value) return value end\n---@param value number\n---@return number\nlocal function numeric(value) return value end\nlocal callback=functionIdentity(numeric)\nlocal result=callback(42)";
    let report = accepted(source);
    assert_eq!(binding_type(&report, "field"), "number");
    assert_eq!(binding_type(&report, "result"), "number");
    let rejected = check(&source.replace("callback(42)", "callback(\"wrong\")"));
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2345"),
        "{:#?}",
        rejected.diagnostics
    );
}

#[test]
fn headerless_typed_and_generic_signatures_keep_any_rest_in_public_queries() {
    let source = "---@param callback fun:number\nlocal function typed(callback) return callback(42,\"word\") end\n---@param callback fun<T:number>\nlocal function generic(callback) return callback(42,\"word\") end";
    let report = accepted(source);
    let callback = report
        .type_at(source.find("typed(callback)").unwrap() + "typed(".len())
        .unwrap();
    assert!(
        callback.contains("...") && callback.contains("number"),
        "{callback}"
    );
    let generic = report
        .type_at(source.find("generic(callback)").unwrap() + "generic(".len())
        .unwrap();
    assert!(
        generic.contains("any") && generic.contains("..."),
        "{generic}"
    );
    let rejected = check(&source.replace("fun<T:number>", "fun<T:Missing>"));
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2304"),
        "{:#?}",
        rejected.diagnostics
    );
}

#[test]
fn bare_fun_overload_heads_keep_unknown_physical_results_and_public_any_rest() {
    let source = "---@overload fun\n---@vararg any\n---@return any...\nlocal function forward(...) return ... end\nlocal first,second=forward(42,\"word\")";
    let report = accepted(source);
    assert_eq!(binding_type(&report, "first"), "any");
    assert_eq!(binding_type(&report, "second"), "any");
    let public = report
        .type_at(source.find("forward(...)").unwrap())
        .unwrap();
    assert!(public.contains("...") && public.contains("any"), "{public}");
}

#[test]
fn generic_and_union_callable_tables_cannot_cross_the_raw_function_boundary() {
    for parameter in ["Callable|function", "T"] {
        let generic = if parameter == "T" {
            "---@generic T:Callable\n"
        } else {
            ""
        };
        let source = format!(
            "---@param callback function\nlocal function take(callback) end\n{generic}---@param value {parameter}\nlocal function use(value) take(value) end\n---@shape Callable\n---@overload fun():number"
        );
        let report = check(&source);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|error| error.code == "TS2345"),
            "{parameter}: {:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn repeated_generic_runtime_tests_keep_the_existing_runtime_intersection() {
    accepted(
        "---@generic T\n---@param value T\nlocal function use(value)\nif type(value)==\"table\" then\nif type(value)==\"table\" then value.field=42 end\nend\nif type(value)==\"function\" then\nif type(value)==\"function\" then value() end\nend\nend",
    );
}

#[test]
fn cli_overloaded_runtime_domain_error_is_corrected_in_original_unicode_source() {
    use std::{fs, process::Command};
    let directory = std::env::temp_dir().join(format!(
        "luanalysis primitive overload {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("runtime-overload.lua");
    let source = format!(
        "{}---@type \"table\"\r\nlocal é=classify(42)",
        CLASSIFY.replace('\n', "\r\n")
    );
    fs::write(&path, &source).unwrap();
    let report = check(&source);
    let error = report
        .diagnostics
        .iter()
        .find(|error| matches!(error.code, "TS2345" | "TS2769"))
        .unwrap_or_else(|| panic!("{:#?}", report.diagnostics));
    assert_eq!(&source[error.span.start..error.span.end], "42");
    let (line, column) = location(&source, error.span);
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
        errors.contains(&format!("runtime-overload.lua:{line}:{column}: error[")),
        "{errors}"
    );
    fs::write(&path, source.replace("classify(42)", "classify({})")).unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    assert!(String::from_utf8_lossy(&corrected.stdout).contains("é: \"table\""));
    fs::remove_dir_all(directory).unwrap();
}
