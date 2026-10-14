use luanalysis::{check, location};

#[test]
fn source_generics_resolve_formals_and_returns_without_erasing_parameter_identity() {
    let source = "---@generic T\n---@param value T\n---@return T\nlocal function identity(value) return value end";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let declaration = source.find("identity(value)").unwrap() + 9;
    let returned = source.find("return value").unwrap() + 7;
    assert_eq!(report.type_at(declaration), Some("T"));
    assert_eq!(report.type_at(returned), Some("T"));
    assert!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "identity")
            .unwrap()
            .ty
            .contains("T")
    );
}

#[test]
fn sibling_constraints_are_available_before_native_bound_queries() {
    let source = "---@generic A : B, B : number\n---@param value A\n---@return A\nlocal function identity(value) return value end";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report.type_at(source.find("return value").unwrap() + 7),
        Some("A")
    );
}

#[test]
fn constrained_parameter_member_lookup_uses_native_apparent_types() {
    let source = "---@generic T : {x: number}\n---@param value T\nlocal function read(value) return value.x end";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let member = source.find("value.x").unwrap() + 6;
    assert_eq!(report.type_at(member), Some("number"));
}

#[test]
fn nested_generic_names_shadow_parent_scopes() {
    let source = "---@generic T : number\n---@param value T\nlocal function outer(value)\n---@generic T : string\n---@param inner T\n---@return T\nlocal function nested(inner) return inner end\nreturn value\nend";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report.type_at(source.find("return inner").unwrap() + 7),
        Some("T")
    );
    assert_eq!(
        report.type_at(source.find("return value").unwrap() + 7),
        Some("T")
    );
}

#[test]
fn captured_parent_parameter_does_not_become_a_shadowing_generic() {
    let source = "---@generic T : number\n---@param value T\nlocal function outer(value)\n---@generic T : string\n---@return T\nlocal function nested() return value end\nreturn value\nend";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "value"
    );
    assert_eq!(location(source, report.diagnostics[0].span), (6, 32));
}

#[test]
fn returned_closure_captures_retain_outer_generic_type_identity() {
    let source = "---@generic T\n---@param value T\nlocal function make(value)\nreturn function() return value end\nend";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report.type_at(source.find("return value").unwrap() + 7),
        Some("T")
    );
    assert!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "make")
            .unwrap()
            .ty
            .contains("fun(): T")
    );
}

#[test]
fn unknown_generic_constraints_diagnose_original_annotation_ranges() {
    let source = "---@generic T : Missing\n---@param value T\nlocal function identity(value) return value end";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "LUA2001");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "Missing"
    );
    assert_eq!(location(source, report.diagnostics[0].span), (1, 17));
}

#[test]
fn circular_generic_constraints_are_rejected_by_native_query_guards() {
    let source =
        "---@generic T : T\n---@param value T\nlocal function identity(value) return value end";
    let report = check(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2313"),
        "{:#?}",
        report.diagnostics
    );
    let cycle = report
        .diagnostics
        .iter()
        .find(|error| error.code == "TS2313")
        .unwrap();
    assert_eq!(&source[cycle.span.start..cycle.span.end], "T");
    assert_eq!(location(source, cycle.span), (1, 13));
}

#[test]
fn native_generic_metadata_cli_rejects_then_accepts_a_constraint_correction() {
    use std::{fs, process::Command};
    let directory = std::env::temp_dir().join(format!(
        "luanalysis-generic-metadata-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("generic.lua");
    let source = "---@generic T : Missing\n---@param value T\nlocal function identity(value) return value end";
    fs::write(&file, source).unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .arg("check")
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    let error = String::from_utf8(rejected.stderr).unwrap();
    assert!(
        error.contains("generic.lua:1:17: error[LUA2001]"),
        "{error}"
    );
    fs::write(&file, source.replace("Missing", "number")).unwrap();
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

#[test]
fn source_generic_identity_calls_use_native_argument_inference() {
    let source = "---@generic T\n---@param value T\n---@return T\nlocal function identity(value) return value end\nlocal inferred = identity(42)\n---@type number\nlocal accepted = inferred\n---@type string\nlocal rejected = inferred";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "inferred")
            .unwrap()
            .ty,
        "number"
    );
    assert_eq!(location(source, report.diagnostics[0].span), (9, 18));
}

#[test]
fn source_generic_calls_check_constraints_at_original_argument_spans() {
    let source = "---@generic T : number\n---@param value T\n---@return T\nlocal function identity(value) return value end\nlocal accepted = identity(42)\nlocal rejected = identity(\"wrong\")";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2345");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "\"wrong\""
    );
    assert_eq!(location(source, report.diagnostics[0].span), (6, 27));
}

#[test]
fn generic_return_context_flows_through_annotated_locals_and_parenthesized_returns() {
    let source = "---@type any\nlocal payload = 42\n---@generic T\n---@return T\nlocal function make() return payload end\n---@type number\nlocal localResult = (make())\n---@return number\nlocal function result() return (make()) end\nlocal functionResult = result()";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "localResult")
            .unwrap()
            .ty,
        "number"
    );
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "functionResult")
            .unwrap()
            .ty,
        "number"
    );
}

#[test]
fn generic_returned_closures_preserve_native_mapper_context_and_lua_presentation() {
    let source = "---@generic T\n---@param value T\nlocal function make(value) return function() return value end end\nlocal getter = make(42)\nlocal inferred = getter()\n---@type number\nlocal accepted = inferred\n---@type string\nlocal rejected = inferred";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    let getter = report
        .bindings
        .iter()
        .find(|binding| binding.name == "getter")
        .unwrap();
    assert!(getter.ty.starts_with("fun("), "{}", getter.ty);
    assert!(!getter.ty.contains("=>"), "{}", getter.ty);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "inferred")
            .unwrap()
            .ty,
        "number"
    );
}

#[test]
fn annotated_initializer_context_is_preserved_during_lazy_flow_demands() {
    let source = "---@type any\nlocal payload=42\n---@generic T\n---@return T\nlocal function make() return payload end\nlocal function read()\n---@type number\nlocal value=make()\nreturn value\nend\nlocal inferred=read()\n---@type number\nlocal accepted=inferred\n---@type string\nlocal rejected=inferred";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(&source[report.diagnostics[0].span.start..report.diagnostics[0].span.end], "inferred");
    assert_eq!(report.bindings.iter().find(|binding| binding.name == "inferred").unwrap().ty, "number");
}
