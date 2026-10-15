use luanalysis::{check, location};

#[test]
fn unannotated_scalar_returns_are_inferred_by_native_signatures() {
    let source = "local function answer() return 42 end\nlocal inferred = answer()\n---@type number\nlocal accepted = inferred\n---@type string\nlocal rejected = inferred";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(location(source, report.diagnostics[0].span), (6, 18));
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "inferred")
            .unwrap()
            .ty,
        "number"
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|error| error.code == "LUA2014")
    );
}

#[test]
fn annotated_parameters_supply_native_return_inference_without_return_tags() {
    let source = "---@param value number\nlocal function identity(value) return value end\nlocal result = identity(42)\nidentity(\"wrong\")";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2345");
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "result")
            .unwrap()
            .ty,
        "number"
    );
    assert_eq!(
        report.type_at(source.find("return value").unwrap() + 7),
        Some("number")
    );
}

#[test]
fn returned_closures_keep_native_identity_and_capture_parameter_declarations() {
    let source = "---@param value number\nlocal function make(value)\nreturn function() return value end\nend\nlocal getter = make(42)\nlocal result = getter()\n---@type number\nlocal accepted = result\n---@type string\nlocal rejected = result";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(location(source, report.diagnostics[0].span), (10, 18));
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "result")
            .unwrap()
            .ty,
        "number"
    );
    let bound = report.bound.as_ref().unwrap();
    let declaration = source.find("make(value)").unwrap() + 5;
    let capture = source.find("return value").unwrap() + 7;
    assert_eq!(bound.binding_at(declaration), bound.binding_at(capture));
}

#[test]
fn lazy_capture_demands_preserve_shadowing_and_initializer_activation() {
    let source = "local value = 42\nlocal function first() return value end\nlocal value = \"later\"\nlocal function second() return value end\n---@type number\nlocal a = first()\n---@type string\nlocal b = second()\nlocal previous = 1\nlocal previous = function() return previous end\n---@type number\nlocal c = previous()";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "a")
            .unwrap()
            .ty,
        "number"
    );
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "b")
            .unwrap()
            .ty,
        "string"
    );
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "c")
            .unwrap()
            .ty,
        "number"
    );
}

#[test]
fn mutable_captures_use_declared_constraints_during_lazy_body_checks() {
    let source = "---@type number|string\nlocal value = 1\nlocal function read() return value end\nvalue = \"later\"\n---@type number|string\nlocal accepted = read()\n---@type number\nlocal rejected = read()";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(location(source, report.diagnostics[0].span), (8, 18));
}

#[test]
fn unused_function_bodies_are_checked_and_keep_original_error_ranges() {
    let source = "local function unused()\nreturn absent\nend";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2304");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "absent"
    );
    assert_eq!(location(source, report.diagnostics[0].span), (2, 8));
}

#[test]
fn recursive_return_cycles_use_native_query_guards_and_source_diagnostics() {
    let source = "local function recursive() return recursive() end\nlocal result = recursive()";
    let report = check(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS7023"),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|error| error.code == "LUA2014")
    );
    let cycle = report
        .diagnostics
        .iter()
        .find(|error| error.code == "TS7023")
        .unwrap();
    assert_eq!(&source[cycle.span.start..cycle.span.end], "recursive");
    assert_eq!(location(source, cycle.span), (1, 16));
}

#[test]
fn mutually_dependent_nested_functions_report_native_return_cycles() {
    let source = "local function first()\nlocal function second() return first() end\nreturn second()\nend\nlocal result = first()";
    let report = check(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS7023"),
        "{:#?}",
        report.diagnostics
    );
    for diagnostic in &report.diagnostics {
        assert_eq!(diagnostic.code, "TS7023", "{diagnostic:#?}");
        assert!(matches!(
            &source[diagnostic.span.start..diagnostic.span.end],
            "first" | "second"
        ));
    }
}

#[test]
fn returning_a_function_identity_is_not_a_recursive_return_query() {
    let source = "local function identity() return identity end\nlocal copy = identity()\nlocal again = copy()";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let bound = report.bound.as_ref().unwrap();
    let declaration = source.find("function identity").unwrap() + 9;
    let returned = source.find("return identity").unwrap() + 7;
    assert_eq!(bound.binding_at(declaration), bound.binding_at(returned));
}

#[test]
fn unrelated_recursive_statements_do_not_create_return_inference_cycles() {
    for statement in [
        "recursive()",
        "local unused = recursive()",
        "local needed, unused = 1, recursive()",
    ] {
        let source = format!(
            "local function recursive() {statement}; return 1 end\n---@type number\nlocal value = recursive()"
        );
        let report = check(&source);
        assert!(
            report.diagnostics.is_empty(),
            "{statement}: {:#?}",
            report.diagnostics
        );
    }
    let source = "---@type {x: number|string, y: number|string}\nlocal item = {x = 1, y = 1}\nlocal function recursive() item.y = recursive(); return item.x end\n---@type number|string\nlocal value = recursive()";
    assert!(check(source).diagnostics.is_empty());
}

#[test]
fn copied_captures_retain_types_inferred_at_the_declaration_point() {
    let source = "---@type number|string\nlocal source = 1\nsource = \"text\"\nlocal copied = source\nlocal function read() return copied end\n---@type string\nlocal value = read()";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "copied")
            .unwrap()
            .ty,
        "string"
    );
}

#[test]
fn returned_closures_demand_copied_local_types_after_preceding_parameter_writes() {
    let source = "---@param value number|string\nlocal function make(value)\nvalue = \"text\"\nlocal copied = value\nreturn function() return copied end\nend\nlocal getter = make(42)\n---@type string\nlocal result = getter()";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "copied")
            .unwrap()
            .ty,
        "string"
    );
}

#[test]
fn declarations_keep_zero_results_while_returned_nonreturning_calls_infer_never() {
    let source = "---@return never\nlocal function spin() spin() end\nlocal function stopped() spin() end\nlocal value = stopped()\nlocal function forwarded() return spin() end\nlocal forwarded_value=forwarded()\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "value")
            .unwrap()
            .ty,
        "nil"
    );
    assert_eq!(report.bindings.iter().find(|binding| binding.name == "forwarded_value").unwrap().ty, "never");
}

#[test]
fn inferred_record_returns_use_native_widening_and_structural_relations() {
    let source = "local function make() return {x = 1, extra = true} end\n---@type {x: number}\nlocal accepted = make()\n---@type {x: string}\nlocal rejected = make()";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(location(source, report.diagnostics[0].span), (5, 18));
}

#[test]
fn native_inference_cli_rejects_then_accepts_an_annotation_correction() {
    use std::{fs, process::Command};
    let directory = std::env::temp_dir().join(format!(
        "luanalysis-native-inference-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("inference.lua");
    let source =
        "local function answer() return 42 end\n---@type string\nlocal result = answer()\n";
    fs::write(&file, source).unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .arg("check")
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    let error = String::from_utf8(rejected.stderr).unwrap();
    assert!(
        error.contains("inference.lua:3:16: error[TS2322]"),
        "{error}"
    );
    assert!(!error.contains("LUA2014"), "{error}");
    fs::write(&file, source.replace("---@type string", "---@type number")).unwrap();
    let accepted = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
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
