use luanalysis::{location};

// Keep contextual inference regressions meaningful with missing contracts denied.
fn check(source: &str) -> luanalysis::CheckReport {
    luanalysis::check_with_options(source, luanalysis::CheckOptions {
        no_implicit_any: true,
        ..Default::default()
    })
}


const TRANSFORM: &str = "---@generic T, R\n---@param value T\n---@param project fun(value: T): R\n---@return R\nlocal function transform(value, project) return project(value) end\n";

#[test]
fn unannotated_callback_member_access_is_checked_in_native_context() {
    let source = format!(
        "{TRANSFORM}local result=transform({{id=42}},function(value) return value.id end)\n---@type number\nlocal accepted=result\n---@type string\nlocal rejected=result"
    );
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(location(&source, report.diagnostics[0].span), (10, 16));
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS7006")
    );
    let parameter = source.find("function(value)").unwrap() + 9;
    let ty = report.type_at(parameter).unwrap();
    assert!(ty.contains("id") && ty.contains("number"), "{ty}");
}

#[test]
fn independent_callbacks_keep_numeric_and_string_parameter_facets() {
    let source = format!(
        "{TRANSFORM}local number=transform(42,function(value) return value end)\nlocal string=transform(\"text\",function(value) return value end)\n---@type number\nlocal a=number\n---@type string\nlocal b=string"
    );
    let report = check(&source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "number")
            .unwrap()
            .ty,
        "number"
    );
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "string")
            .unwrap()
            .ty,
        "string"
    );
}

#[test]
fn nested_generic_argument_demands_keep_outer_callback_parameter_context() {
    let source = format!(
        "---@generic T\n---@param value T\n---@return T\nlocal function identity(value) return value end\n{TRANSFORM}local result=transform({{id=42}},function(value) return identity(value).id end)\n---@type number\nlocal accepted=result"
    );
    let report = check(&source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "result")
            .unwrap()
            .ty,
        "number"
    );
}

#[test]
fn incompatible_callback_body_reports_original_member_location() {
    let source = format!(
        "{TRANSFORM}local result=transform({{id=42}},function(value) return value.missing end)"
    );
    let report = check(&source);
    let missing = report
        .diagnostics
        .iter()
        .find(|error| error.code == "TS2339")
        .unwrap_or_else(|| panic!("{:#?}", report.diagnostics));
    assert_eq!(&source[missing.span.start..missing.span.end], "missing");
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS7006"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn explicit_callback_annotations_remain_constraints_under_context() {
    let source = format!(
        "{TRANSFORM}---@param callback fun(value: string): string\n---@return string\nlocal function use(callback) return callback(\"text\") end\n---@param value number\nlocal invalid=function(value) return value end\nlocal result=use(invalid)"
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
fn returned_contextual_closures_preserve_number_string_and_copied_binding_environments() {
    for body in [
        "return function() return value end",
        "local copied=value return function() return copied end",
    ] {
        let source = format!(
            "{TRANSFORM}local number=transform(42,function(value) {body} end)\nlocal string=transform(\"text\",function(value) {body} end)\n---@type number\nlocal a=number()\n---@type string\nlocal b=string()\n---@type string\nlocal rejected=number()"
        );
        let report = check(&source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{body}: {:#?}",
            report.diagnostics
        );
        assert_eq!(
            report.diagnostics[0].code, "TS2322",
            "{body}: {:#?}",
            report.diagnostics
        );
        assert!(
            !report
                .diagnostics
                .iter()
                .any(|error| matches!(error.code, "LUA2022" | "TS7006")),
            "{:#?}",
            report.diagnostics
        );
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
    }
}

#[test]
fn same_source_generic_returned_closure_is_instantiated_without_cross_call_contamination() {
    for body in [
        "return function() return value end",
        "local copied=value return function() return copied end",
        "\n---@type T\nlocal copied=value return function() return copied end",
    ] {
        let source = format!(
            "{TRANSFORM}---@generic T\n---@param input T\nlocal function make(input) return transform(input,function(value) {body} end) end\nlocal number=make(42)\nlocal string=make(\"text\")\nlocal again=make(7)\n---@type number\nlocal a=number()\n---@type string\nlocal b=string()\n---@type number\nlocal c=again()\n---@type string\nlocal rejected=again()"
        );
        let report = check(&source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{body}: {:#?}",
            report.diagnostics
        );
        assert_eq!(
            report.diagnostics[0].code, "TS2322",
            "{body}: {:#?}",
            report.diagnostics
        );
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
}

#[test]
fn nested_captured_function_values_do_not_fix_unrelated_map_return_variables() {
    for first in [
        "local first=function() return value end",
        "local function first() return value end",
    ] {
        let source = format!(
            "{TRANSFORM}---@generic T\n---@param input T\nlocal function make(input) return transform(input,function(value) {first} return function() return first end end) end\nlocal number=make(42)\nlocal string=make(\"text\")\n---@type number\nlocal a=number()()\n---@type string\nlocal b=string()()\n---@type string\nlocal rejected=number()()"
        );
        let report = check(&source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{first}: {:#?}",
            report.diagnostics
        );
        assert_eq!(
            report.diagnostics[0].code, "TS2322",
            "{first}: {:#?}",
            report.diagnostics
        );
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
    }
}

#[test]
fn nested_contextual_callback_uses_its_own_parameters_inside_a_captured_parent() {
    let source = format!(
        "{TRANSFORM}local result=transform(42,function(value) return transform(\"text\",function(inner) return inner end) end)\n---@type string\nlocal accepted=result\n---@type number\nlocal rejected=result"
    );
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        report.diagnostics[0].code, "TS2322",
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "result")
            .unwrap()
            .ty,
        "string"
    );
    let parameter = source.find("function(inner)").unwrap() + 9;
    assert_eq!(report.type_at(parameter), Some("string"));
}

#[test]
fn captured_signature_and_body_annotations_are_instantiated_once() {
    let source = format!(
        "{TRANSFORM}---@generic T\n---@param input T\nlocal function make(input)\nreturn transform(input,function(value)\n---@param other T\n---@return T\nlocal inner=function(other)\n---@type T\nlocal typed=other\nreturn value\nend\nreturn inner\nend)\nend\nlocal number=make(42)\nlocal string=make(\"text\")\n---@type number\nlocal a=number(7)\n---@type string\nlocal b=string(\"again\")\nnumber(\"wrong\")"
    );
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        report.diagnostics[0].code, "TS2345",
        "{:#?}",
        report.diagnostics
    );
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
}

#[test]
fn recursive_local_function_keeps_the_enclosing_callback_capture_facet() {
    let source = format!(
        "{TRANSFORM}---@generic T\n---@param input T\nlocal function make(input)\nreturn transform(input,function(value)\n---@param count number\n---@return T\nlocal function first(count)\nif count > 0 then return first(count-1) end\nreturn value\nend\nreturn first\nend)\nend\nlocal number=make(42)\nlocal string=make(\"text\")\n---@type number\nlocal a=number(2)\n---@type string\nlocal b=string(2)\n---@type string\nlocal rejected=number(2)"
    );
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        report.diagnostics[0].code, "TS2322",
        "{:#?}",
        report.diagnostics
    );
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
}

#[test]
fn unannotated_vararg_only_callback_receives_the_expected_input_pack() {
    let source = "---@param callback fun(...: number): number...\n\
---@return nil|number\nlocal function use(callback) return (callback(1,2)) end\n\
local result=use(function(...) return ... end)\n\
---@type nil|number\nlocal accepted=result";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn cli_contextual_member_error_disappears_after_original_lua_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis-contextual-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("callback.lua");
    let source = format!(
        "{TRANSFORM}local result=transform({{id=42}},function(value) return value.missing end)"
    );
    fs::write(&path, &source).unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .arg("check")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        rejected.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("TS2339"));
    fs::write(&path, source.replace("value.missing", "value.id")).unwrap();
    let accepted = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .arg("check")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        accepted.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    fs::remove_dir_all(directory).unwrap();
}
