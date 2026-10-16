use luanalysis::{CheckReport, check};
fn passes(source: &str) -> CheckReport {
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
        .unwrap()
        .ty
}
#[test]
fn implicit_shape_checks_initializer_and_preserves_declared_member_constraints() {
    let source = "---@shape Attached\n---@field value number\nlocal attached={value=42}\nlocal result=attached.value\nattached.value=\"wrong\"";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "\"wrong\""
    );
    assert_eq!(ty(&report, "attached"), "Attached");
    assert_eq!(ty(&report, "result"), "number");
    let report =
        check("---@shape Attached\n---@field value number\nlocal attached={value=\"wrong\"}");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "TS2322"),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "LUA2031")
    );
}
#[test]
fn tagged_shapes_permit_extra_members_and_check_required_members() {
    passes("---@shape Point\n---@field value number\nlocal point={value=1,extra=true}");
    for source in [
        "---@shape Point\n---@field value number\nlocal point={}",
        "---@shape Point\n---@field value number\nlocal point={value=\"wrong\",extra=true}",
    ] {
        let report = check(source);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "TS2322"),
            "{:#?}",
            report.diagnostics
        );
    }
}
#[test]
fn initializer_binds_the_old_shadowed_local_before_shape_activation() {
    let report = passes(
        "local point=42\n---@shape Point\n---@field value number\nlocal point={value=point}\nlocal result=point.value",
    );
    assert_eq!(ty(&report, "result"), "number");
}
#[test]
fn original_shape_names_need_not_match_runtime_names() {
    let report = passes(
        "---@shape DeclaredName\n---@field value number\nlocal differentName={value=42}\nlocal result=differentName.value",
    );
    assert_eq!(ty(&report, "differentName"), "DeclaredName");
    assert_eq!(ty(&report, "result"), "number");
}
#[test]
fn explicit_type_and_transparent_alias_retain_the_same_shape_owner() {
    passes("---@shape Point\n---@field value number\n---@type Point\nlocal point={value=42}");
    passes(
        "---@shape Point\n---@field value number\n---@alias Alias Point\n---@type Alias\nlocal point={value=42}",
    );
    passes("---@shape Box<T>\n---@field value T\n---@type Box<number>\nlocal box={value=42}");
}
#[test]
fn incompatible_and_broad_explicit_annotation_ownership_stays_gated() {
    for annotation in ["any", "unknown", "number", "{value:any}", "{value:number}"] {
        let source = format!(
            "---@shape Point\n---@field value number\n---@type {annotation}\nlocal point={{value=42}}"
        );
        let report = check(&source);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "LUA2031"
                    && &source[diagnostic.span.start..diagnostic.span.end] == "Point"),
            "{source}\n{:#?}",
            report.diagnostics
        );
    }
}
#[test]
fn ambiguous_multi_destination_nonlocal_and_nonliteral_attachments_are_explicit() {
    for source in [
        "---@shape One\n---@field value number\n---@shape Two\n---@field value number\nlocal point={value=42}",
        "---@shape Point\n---@field value number\nlocal first,second={value=1},{value=2}",
        "local point={}\n---@shape Point\n---@field value number\npoint={value=42}",
        "local function make() return {value=42} end\n---@shape Point\n---@field value number\nlocal point=make()",
        "---@shape Point<T>\n---@field value T\nlocal point={value=42}",
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
fn parentheses_preserve_literal_context_and_original_locations() {
    let report = passes(
        "-- café\r\n---@shape Point\r\n---@field value number\r\nlocal point=({value=42})\r\nlocal result=point.value",
    );
    assert_eq!(ty(&report, "result"), "number");
}
#[test]
fn attached_shapes_keep_native_branch_and_loop_field_flow() {
    let report = passes(
        "---@param flag boolean\nlocal function read(flag)\n---@shape Point\n---@field value number|string\nlocal point={value=42}\nif flag then point.value=\"branch\" end\nwhile flag do point.value=42;break end\nreturn point.value\nend\nlocal result=read(true)",
    );
    assert!(ty(&report, "result").contains("number"));
}
#[test]
fn lazy_captured_read_uses_the_declared_shape_before_diagnostic_traversal() {
    let report = passes(
        "---@shape Point\n---@field value number|string\nlocal point={value=42}\nlocal function read() return point.value end\npoint.value=\"later\"\nlocal result=read()",
    );
    let result = ty(&report, "result");
    assert!(
        result.contains("number") && result.contains("string"),
        "{result}"
    );
}
#[test]
fn literal_callback_fields_receive_context_in_normal_and_lazy_loop_demands() {
    let report = passes(
        "---@param flag boolean\nlocal function make(flag)\n---@shape Callbacks\n---@field read fun(value:{x:number}):number\nlocal callbacks={read=function(value) return value.x end}\nwhile flag do local copy=callbacks.read;break end\nreturn callbacks.read({x=42})\nend\nlocal result=make(true)",
    );
    assert_eq!(ty(&report, "result"), "number");
}
#[test]
fn outer_generic_shape_members_keep_captured_instance_mappers() {
    let report = passes(
        "---@generic T\n---@param value T\nlocal function outer(value)\n---@shape Captured\n---@field value T\nlocal captured={value=value}\nreturn function() return captured.value end\nend\nlocal number=outer(42)\nlocal string=outer(\"word\")\nlocal a=number()\nlocal b=string()",
    );
    assert_eq!(ty(&report, "a"), "number");
    assert_eq!(ty(&report, "b"), "string");
}
#[test]
fn actual_cli_rejects_the_original_shape_field_then_accepts_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis-shape-attach-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("shape.ttslua");
    let wrong = include_str!("fixtures/named/implicit_shape_wrong.lua");
    fs::write(&path, wrong).unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--dump-types"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        rejected.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        stderr.contains("shape.ttslua:3:18: error[TS2322]"),
        "{stderr}"
    );
    assert!(!stderr.contains("LUA2031"), "{stderr}");
    fs::write(
        &path,
        include_str!("fixtures/named/implicit_shape_corrected.lua"),
    )
    .unwrap();
    let corrected = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--dump-types"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    fs::remove_dir_all(&directory).unwrap();
}
