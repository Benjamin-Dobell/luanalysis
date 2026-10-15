use luanalysis::{CheckReport, check};

fn ty<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .unwrap()
        .ty
}
fn passes(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

#[test]
fn primitive_union_and_forward_aliases_are_canonical_native_types() {
    let source = "---@alias Early Later\n---@alias Later number\n---@alias Maybe Early|string\n---@type Early\nlocal value=42\n---@type Maybe\nlocal mixed=\"chosen\"\n---@type string\nlocal rejected=value";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "value"
    );
    assert_eq!(ty(&report, "value"), "number");
}

#[test]
fn shape_fields_are_read_and_written_through_native_named_relations() {
    let source = "---@shape Point\n---@field x number\n---@field y number\n---@type Point\nlocal point={x=1,y=2}\nlocal x=point.x\npoint.y=\"wrong\"";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "\"wrong\""
    );
    assert_eq!(ty(&report, "x"), "number");
}

#[test]
fn recursive_record_aliases_publish_heads_and_flow_narrows_nested_optional_fields() {
    passes(
        "---@alias Node {value:number,next:Node|nil}\n---@param node Node\nlocal function read(node)\nif node.next then return node.next.value end\nreturn node.value\nend",
    );
    passes("---@alias Left {right:Right|nil}\n---@alias Right {left:Left|nil}\n");
}

#[test]
fn recursive_array_and_dictionary_aliases_remain_finite() {
    passes("---@alias Nested Nested[]\n---@alias Lookup table<string,Lookup>\n");
    let report = passes(
        "---@alias Values number[]\n---@alias Dictionary table<string,number>\n---@type Values\nlocal values={1,2}\n---@type Dictionary\nlocal dictionary={answer=42}\nlocal first=values[1]\nlocal answer=dictionary.answer",
    );
    assert_eq!(ty(&report, "first"), "number");
    assert_eq!(ty(&report, "answer"), "number");
}

#[test]
fn alias_only_cycles_report_original_declarations_without_panicking() {
    let source = "---@alias First Second\n---@alias Second First\n";
    let report = check(source);
    assert!(!report.diagnostics.is_empty());
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "TS2456"),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "TS2456")
            .all(|diagnostic| matches!(
                &source[diagnostic.span.start..diagnostic.span.end],
                "First" | "Second"
            ))
    );
}

#[test]
fn unused_bad_alias_targets_and_members_are_semantically_checked() {
    for source in [
        "---@alias Unused Missing\n",
        "---@shape Unused\n---@field value Missing\n",
        "---@alias Unused {value:Missing}\n",
    ] {
        let report = check(source);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "TS2304"
                    && &source[diagnostic.span.start..diagnostic.span.end] == "Missing"),
            "{source}\n{:#?}",
            report.diagnostics
        );
    }
    let report = check("---@alias Unused table<string>\n");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "TS2314"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn generic_alias_instances_keep_distinct_member_mappers() {
    let source = "---@alias Box<T> {value:T}\n---@type Box<number>\nlocal numeric={value=42}\n---@type Box<string>\nlocal text={value=\"word\"}\nlocal a=numeric.value\nlocal b=text.value\n---@type string\nlocal rejected=numeric.value";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(ty(&report, "a"), "number");
    assert_eq!(ty(&report, "b"), "string");
}

#[test]
fn named_generic_constraints_and_argument_arity_use_native_parameter_queries() {
    passes("---@alias Bounded<T:number> T\n---@type Bounded<number>\nlocal value=42");
    let report =
        check("---@alias Bounded<T:number> T\n---@type Bounded<string>\nlocal value=\"wrong\"");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "TS2344"),
        "{:#?}",
        report.diagnostics
    );
    let report = check("---@alias Box<T> {value:T}\n---@type Box\nlocal box={value=1}");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "TS2314"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn named_constraint_errors_pin_the_original_argument_type_and_preserve_unicode_crlf() {
    let source = "-- café\r\n---@alias Bounded<T:number> T\r\n---@type Bounded<string>\r\nlocal value=\"wrong\"";
    let report = check(source);
    let error = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "TS2344")
        .unwrap();
    assert_eq!(&source[error.span.start..error.span.end], "string");
    assert_eq!(luanalysis::location(source, error.span), (3, 18));
}

#[test]
fn f_bounded_named_templates_preserve_the_productive_object_boundary() {
    passes("---@alias Node<T:Node<T>> {value:number,next:T|nil}\n");
}

#[test]
fn generic_shapes_and_recursive_generic_aliases_keep_declaration_space_members() {
    passes(
        "---@shape Box<T>\n---@field value T\n---@type Box<number>\nlocal value={value=42}\n---@type number\nlocal read=value.value",
    );
    passes(
        "---@alias Node<T> {value:T,next:Node<T>|nil}\n---@param node Node<number>\nlocal function read(node) return node.value end",
    );
}

#[test]
fn transparent_alias_owner_augmentations_resolve_in_the_core() {
    let report = passes(
        "---@alias Original {x:number}\n---@alias Owner Original\n---@field <Owner> y string\n---@type Original\nlocal value={x=1,y=\"yes\"}\nlocal read=value.y\n",
    );
    assert_eq!(ty(&report, "read"), "string");
}

#[test]
fn unknown_primitive_ambiguous_and_specialized_augmentation_owners_are_rejected() {
    for source in [
        "---@field <Missing> x number\n",
        "---@alias Primitive number\n---@field <Primitive> x number\n",
        "---@shape Same\n---@shape Same\n---@field <Same> x number\n",
        "---@shape Box<T>\n---@field value T\n---@alias Specialized Box<number>\n---@field <Specialized> extra number\n",
    ] {
        let report = check(source);
        assert!(!report.diagnostics.is_empty(), "{source}");
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| matches!(diagnostic.code, "TS2304" | "TS2300" | "LUA2031")),
            "{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn class_runtime_module_base_and_visibility_semantics_stay_explicit_gates() {
    for source in [
        "---@class Nominal\nlocal Nominal={}\n",
        "---@module Module\n",
        "---@shape Derived:Base\n---@shape Base\n",
        "---@shape Private\n---@field private value number\n",
    ] {
        let report = check(source);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "LUA2031"),
            "{source}\n{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn implicit_shape_attachments_check_the_actual_initializer() {
    let wrong = include_str!("fixtures/named/implicit_shape_wrong.lua");
    let corrected = include_str!("fixtures/named/implicit_shape_corrected.lua");
    let report = check(wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &wrong[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "{value = \"wrong\"}"
    );
    passes(corrected);
    let explicit = wrong.replace("local attached", "---@type Attached\nlocal attached");
    let report = check(&explicit);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    passes(&explicit.replace("\"wrong\"", "42"));
}

#[test]
fn named_generic_scope_inside_function_captures_original_outer_parameter_identity() {
    passes(
        "---@generic T\n---@param value T\nlocal function outer(value)\n---@alias Box {value:T}\n---@type Box\nlocal box={value=value}\nreturn box.value\nend\nlocal number=outer(42)\nlocal string=outer(\"word\")",
    );
}

#[test]
fn returned_annotation_only_alias_closures_retain_the_hidden_outer_mapper() {
    let report = passes(
        "---@generic T\n---@param value T\nlocal function outer(value)\n---@alias Box {value:T}\n---@type any\nlocal payload={value=value}\n---@return Box\nlocal function inner() return payload end\nreturn inner\nend\nlocal number=outer(42)\nlocal string=outer(\"word\")\nlocal a=number().value\nlocal b=string().value",
    );
    assert_eq!(ty(&report, "a"), "number");
    assert_eq!(ty(&report, "b"), "string");
}

#[test]
fn aliased_function_return_annotations_retain_free_outer_parameter_identity() {
    let report = passes(
        "---@generic T\n---@param value T\nlocal function outer(value)\n---@alias Callback fun():T\n---@type any\nlocal payload=function() return value end\n---@return Callback\nlocal function inner() return payload end\nreturn inner\nend\nlocal number=outer(42)\nlocal string=outer(\"word\")\nlocal a=number()()\nlocal b=string()()",
    );
    assert_eq!(ty(&report, "a"), "number");
    assert_eq!(ty(&report, "b"), "string");
}

#[test]
fn cli_checks_original_alias_annotation_then_accepts_a_source_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis-native-named-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("alias.ttslua");
    let source = "---@alias Count number\n---@type Count\nlocal value=\"wrong\"\n";
    fs::write(&path, source).unwrap();
    let failed = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--dump-types"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        failed.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("alias.ttslua:3:13: error[TS2322]"),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    fs::write(&path, source.replace("\"wrong\"", "42")).unwrap();
    let passed = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--dump-types"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        passed.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&passed.stderr)
    );
    fs::remove_dir_all(&directory).unwrap();
}
