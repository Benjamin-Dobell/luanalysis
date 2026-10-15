use luanalysis::check;

#[test]
fn nested_literal_fields_preserve_each_original_constructor_value() {
    let source = "local root={middle={child={}}}\nlocal alias=root.middle.child\nalias.value=1\n---@type number\nlocal result=root.middle.child.value\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report.type_at(source.find("result=").unwrap()),
        Some("number")
    );
}

#[test]
fn replacing_a_nested_child_preserves_its_frozen_old_alias() {
    let source = "local old={}\nlocal root={middle={child=old}}\nlocal alias=root.middle.child\nroot.middle.child={}\nalias.value=1\n---@type number\nlocal previous=old.value\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let report = check(&format!(
        "{source}---@type number\nlocal wrong=root.middle.child.value\n"
    ));
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2339"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn replacing_the_root_preserves_the_old_nested_constructor_alias() {
    let source = "local child={}\nlocal root={middle={child=child}}\nlocal alias=root.middle.child\nroot={middle={child={}}}\nalias.value=1\n---@type number\nlocal previous=child.value\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let report = check(&format!(
        "{source}---@type number\nlocal wrong=root.middle.child.value\n"
    ));
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2339"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn duplicate_and_dynamic_fields_cannot_fabricate_a_unique_constructor_origin() {
    for source in [
        "local shared={}\nlocal root={middle={child=shared,child={}}}\nlocal alias=root.middle.child\nalias.value=1\n---@type number\nlocal unproven=shared.value\n",
        "---@param key string\nlocal function update(key)\nlocal shared={}\nlocal root={middle={child=shared,[key]={}}}\nlocal alias=root.middle.child\nalias.value=1\n---@type number\nlocal unproven=shared.value\nend\n",
    ] {
        let report = check(source);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|error| error.code == "TS2322"),
            "{source}\n{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn copied_rhs_projection_does_not_invent_a_source_reference_or_any() {
    // A future native projected-reference query must make this valid source
    // precise. Until then, an uncertain alias cannot prove member presence.
    let source = "local original={child={}}\nlocal root=original\nlocal alias=root.child\nalias.value=1\n---@type number\nlocal result=root.child.value\n";
    let report = check(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2322"),
        "{:#?}",
        report.diagnostics
    );
}
