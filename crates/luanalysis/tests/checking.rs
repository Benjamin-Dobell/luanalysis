use luanalysis::{check, location};

#[test]
fn annotated_values_use_native_relations_and_original_spans() {
    let source = "local inferred = 42\n---@type number\nlocal accepted = inferred\n---@type string\nlocal rejected = inferred\n";
    let report = check(source);
    assert_eq!(report.bindings[0].ty, "number");
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(location(source, report.diagnostics[0].span), (5, 18));
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "inferred"
    );
    let corrected = source.replace("---@type string", "---@type number");
    assert!(check(&corrected).diagnostics.is_empty());
}

#[test]
fn shadowed_local_initializer_reads_the_previous_binding() {
    let source = "local value = 42\nlocal value = value\n---@type string\nlocal rejected = value";
    let report = check(source);
    assert_eq!(report.bindings[1].ty, "number");
    assert_eq!(
        report
            .diagnostics
            .iter()
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        ["TS2322"]
    );
}

#[test]
fn shapes_accept_additional_members_and_preserve_member_relations() {
    let fresh = check("---@type {x: number}\nlocal value = {x = 1, extra = true}");
    assert!(fresh.diagnostics.is_empty(), "{:#?}", fresh.diagnostics);
    let staged = check(
        "local wider = {x = 1, extra = true}\n---@type {x: number}\nlocal value = wider\nlocal x = value.x",
    );
    assert!(staged.diagnostics.is_empty(), "{:#?}", staged.diagnostics);
    assert_eq!(staged.bindings[2].ty, "number");
    let bad = check("---@type {x: number}\nlocal value = {x = \"wrong\"}");
    assert_eq!(bad.diagnostics.len(), 1);
}

#[test]
fn scalar_lists_observe_lua_simultaneous_assignment() {
    let report = check(
        "---@type number|string, number|string\nlocal a, b = 1, \"text\"\na, b = b, a\n---@type string\nlocal s = a\n---@type number\nlocal n = b",
    );
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(report.bindings[2].ty, "string");
    assert_eq!(report.bindings[3].ty, "number");
}

#[test]
fn unicode_crlf_error_locations_use_original_source() {
    let source = "-- α😀\r\n---@type number\r\nlocal answer = \"wrong\"\r\n";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(location(source, report.diagnostics[0].span), (3, 16));
}

#[test]
fn unresolved_calls_are_reported_and_implicit_parameter_errors_are_opt_in() {
    assert!(!check("local value = identity(42)").diagnostics.is_empty());
    let source = "local function identity(value) return value end";
    assert!(check(source).diagnostics.is_empty());
    let report = luanalysis::check_with_options(source, luanalysis::CheckOptions {
        no_implicit_any: true,
        ..Default::default()
    });
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS7006");
    let span = report.diagnostics[0].span;
    assert_eq!(&source[span.start..span.end], "value");
}

#[test]
fn lua_byte_strings_and_hexadecimal_numbers_check_natively() {
    let report = check(
        "local raw = \"\\255\"\n---@type string\nlocal accepted = raw\nlocal hex = -0x1.fp2\n---@type number\nlocal value = hex",
    );
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(report.bindings[0].ty, "string");
    assert_eq!(report.bindings[2].ty, "number");
}

#[test]
fn annotations_control_exposed_members_and_unknown_values() {
    let record = check(
        "local wide = {x = 1, extra = true}\n---@type {x: number}\nlocal narrow = wide\nlocal invalid = narrow.extra",
    );
    assert_eq!(
        record
            .diagnostics
            .iter()
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        ["TS2339"]
    );
    let unknown =
        check("---@type unknown\nlocal value = 42\n---@type number\nlocal invalid = value");
    assert_eq!(
        unknown
            .diagnostics
            .iter()
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        ["TS2322"]
    );
}

#[test]
fn direct_field_writes_reduce_unions_and_replacing_the_base_resets_facts() {
    let source = "---@type {x: number|string}\nlocal item = {x = 1}\nitem.x = \"text\"\n---@type string\nlocal accepted = item.x\nitem = {x = 2}\n---@type string\nlocal rejected = item.x";
    let report = check(source);
    assert_eq!(
        report
            .diagnostics
            .iter()
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        ["TS2322"]
    );
    assert_eq!(report.bindings[1].ty, "string");
}

#[test]
fn field_writes_use_the_current_receiver_and_the_property_declaration() {
    let narrowed =
        check("---@type {x: number}|{x: string}\nlocal item = {x = 1}\nitem.x = \"wrong\"");
    assert_eq!(
        narrowed
            .diagnostics
            .iter()
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        ["TS2322"]
    );
    let nullable = check("---@type {x: number}|nil\nlocal item = {x = 1}\nitem.x = 2");
    assert!(
        nullable.diagnostics.is_empty(),
        "{:#?}",
        nullable.diagnostics
    );
    let nested = check(
        "---@type {child: {x: number}|{x: string}}\nlocal item = {child = {x = 1}}\nitem.child = {x = 1}\nitem.child.x = \"wrong\"",
    );
    assert_eq!(
        nested
            .diagnostics
            .iter()
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        ["TS2322"]
    );
    let declared_property = check(
        "---@type {child: {x: number|string}|nil}\nlocal item = {child = {x = 1}}\nitem.child = {x = 2}\nitem.child.x = \"text\"\nitem.child.x = 3\n---@type number\nlocal accepted = item.child.x",
    );
    assert!(
        declared_property.diagnostics.is_empty(),
        "{:#?}",
        declared_property.diagnostics
    );
}

#[test]
fn simultaneous_writes_do_not_attach_old_receiver_facts_to_replaced_roots() {
    for assignment in [
        "item, item.x = {x = \"new\"}, 2",
        "item.x, item = 2, {x = \"new\"}",
    ] {
        let source = format!(
            "---@type {{x: number}}|{{x: string}}\nlocal item = {{x = 1}}\n{assignment}\n---@type string\nlocal accepted = item.x\n---@type number\nlocal rejected = item.x"
        );
        let report = check(&source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{assignment}: {:#?}",
            report.diagnostics
        );
        assert_eq!(
            report.diagnostics[0].message, "Type 'string' is not assignable to type 'number'.",
            "{assignment}"
        );
        assert_eq!(location(&source, report.diagnostics[0].span), (7, 18));
    }
}

#[test]
fn simultaneous_writes_do_not_attach_old_receiver_facts_to_replaced_members() {
    for assignment in [
        "item.child, item.child.x = {x = \"new\"}, 2",
        "item.child.x, item.child = 2, {x = \"new\"}",
    ] {
        let source = format!(
            "---@type {{child: {{x: number}}|{{x: string}}}}\nlocal item = {{child = {{x = 1}}}}\nitem.child = {{x = 1}}\n{assignment}\n---@type string\nlocal accepted = item.child.x\n---@type number\nlocal rejected = item.child.x"
        );
        let report = check(&source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{assignment}: {:#?}",
            report.diagnostics
        );
        assert_eq!(
            report.diagnostics[0].message, "Type 'string' is not assignable to type 'number'.",
            "{assignment}"
        );
        assert_eq!(location(&source, report.diagnostics[0].span), (8, 18));
    }
}

#[test]
fn duplicate_assignment_destinations_follow_lua_54_reverse_stores() {
    for (declaration, assignment, read) in [
        (
            "---@type number|string\nlocal item = 1",
            "item, item = \"left\", 2",
            "item",
        ),
        (
            "---@type {x: number|string}\nlocal item = {x = 1}",
            "item.x, item.x = \"left\", 2",
            "item.x",
        ),
    ] {
        let source = format!(
            "{declaration}\n{assignment}\n---@type string\nlocal accepted = {read}\n---@type number\nlocal rejected = {read}"
        );
        let report = check(&source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{assignment}: {:#?}",
            report.diagnostics
        );
        assert_eq!(
            report.diagnostics[0].message, "Type 'string' is not assignable to type 'number'.",
            "{assignment}"
        );
        assert_eq!(location(&source, report.diagnostics[0].span), (7, 18));
    }
}

#[test]
fn duplicate_constructor_candidates_form_a_union_in_both_source_orders() {
    for constructor in ["{x = 1, x = \"text\"}", "{x = \"text\", x = 1}"] {
        let source = format!("local item = {constructor}\n---@type string\nlocal value = item.x");
        let report = check(&source);
        assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
        assert_eq!(report.diagnostics[0].code, "TS2322");
        assert_eq!(
            &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
            "item.x"
        );
        let actual = report.type_at(source.find("item.x").unwrap() + "item.".len()).unwrap();
        let alternatives: std::collections::BTreeSet<_> = actual.split(" | ").collect();
        assert_eq!(alternatives, ["1", "\"text\""].into_iter().collect());
        let corrected = source.replace("---@type string", "---@type number|string");
        assert!(check(&corrected).diagnostics.is_empty());
    }
}

#[test]
fn bare_cr_and_crlf_locations_count_logical_lines_and_unicode_columns() {
    let source = "-- α😀\r---@type number\r\nlocal 日本語 = \"wrong\"\r";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(location(source, report.diagnostics[0].span), (3, 13));
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "\"wrong\""
    );
}

#[test]
fn cli_checks_captured_receivers_and_reports_bare_cr_locations() {
    use std::fs;
    use std::process::Command;
    let directory = std::env::temp_dir().join(format!(
        "luanalysis-captured-receiver-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("input.lua");
    let source = "---@type {x: number}|{x: string}\rlocal item = {x = 1}\ritem, item.x = {x = \"new\"}, 2\r---@type string\rlocal accepted = item.x\r";
    fs::write(&file, source).unwrap();
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
    fs::write(&file, source.replace("---@type string", "---@type number")).unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .arg("check")
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    let error = String::from_utf8(rejected.stderr).unwrap();
    assert!(
        error.contains(
            "input.lua:5:18: error[TS2322]: Type 'string' is not assignable to type 'number'."
        ),
        "{error}"
    );
    fs::remove_dir_all(&directory).unwrap();
}
