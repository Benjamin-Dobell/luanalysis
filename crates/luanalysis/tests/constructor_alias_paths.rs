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
fn copied_rhs_projection_uses_the_original_reference_without_inventing_a_spelling() {
    let source = "local original={child={}}\nlocal root=original\nlocal alias=root.child\nalias.value=1\n---@type number\nlocal result=root.child.value\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn copied_chains_keep_nested_fixed_suffixes_and_each_frozen_input() {
    let source = "local original={middle={child={}}}\nlocal root=original\nlocal second=root\nlocal alias=second.middle.child\nalias.value=1\n---@type number\nlocal result=second.middle.child.value\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn a_copied_rhs_observes_the_latest_child_replacement_instead_of_replaying_the_seed() {
    for source in [
        "local original={child={}}\nlocal old=original.child\noriginal.child={}\nlocal root=original\nlocal alias=root.child\nalias.value=1\n---@type number\nlocal result=root.child.value\n",
        "local original={child={}}\nlocal old=original.child\nlocal writer=original\nwriter.child={}\nlocal root=original\nlocal alias=root.child\nalias.value=1\n---@type number\nlocal result=root.child.value\n",
        "local original={child={}}\nlocal root=original\nlocal old=root.child\noriginal.child={}\nlocal alias=root.child\nalias.value=1\n---@type number\nlocal result=root.child.value\n",
    ] {
        let report = check(source);
        assert!(
            report.diagnostics.is_empty(),
            "{source}\n{:#?}",
            report.diagnostics
        );
        let report = check(&format!("{source}---@type number\nlocal wrong=old.value\n"));
        assert!(
            report
                .diagnostics
                .iter()
                .any(|error| matches!(error.code, "TS2322" | "TS2339")),
            "{source}\n{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn rebinding_the_original_cell_does_not_change_a_previously_copied_table() {
    let source = "local original={child={}}\nlocal root=original\noriginal={child={}}\nlocal alias=root.child\nalias.value=1\n---@type number\nlocal result=root.child.value\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let report = check(&format!(
        "{source}---@type number\nlocal wrong=original.child.value\n"
    ));
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2322" | "TS2339")),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_branch_selected_copy_cannot_prove_an_unconditional_store_to_one_alternative() {
    let source = "---@param flag boolean\nlocal function update(flag)\nlocal left={child={}}\nlocal right={child={}}\nlocal selected\nif flag then selected=left else selected=right end\nlocal root=selected\nlocal alias=root.child\nalias.value=1\n---@type number\nlocal unproven=left.child.value\nend\n";
    let report = check(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2322"),
        "{:#?}",
        report.diagnostics
    );
    let corrected = source.replace(
        "local unproven=left.child.value",
        "local unproven=1\nleft.child.value=2\n---@type number\nlocal proven=left.child.value",
    );
    let report = check(&corrected);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn an_intervening_call_can_change_projected_heap_values_while_base_cells_stay_stable() {
    let source = "local original={child={}}\nlocal old=original.child\nlocal function replace() original.child={} end\nreplace()\nlocal root=original\nlocal alias=root.child\nalias.value=1\n---@type number\nlocal unproven=old.value\n";
    let report = check(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2322" | "TS2339")),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_parallel_root_rebind_does_not_move_its_captured_child_store() {
    let source = "local original={child={}}\nlocal root=original\nlocal old=root.child\noriginal,original.child={child={}},{}\nlocal alias=root.child\nalias.value=1\n---@type number\nlocal result=root.child.value\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    for rejected in ["old.value", "original.child.value"] {
        let report = check(&format!(
            "{source}---@type number\nlocal wrong={rejected}\n"
        ));
        assert!(
            report
                .diagnostics
                .iter()
                .any(|error| matches!(error.code, "TS2322" | "TS2339")),
            "{rejected}\n{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn a_rhs_call_keeps_its_child_receiver_at_the_original_frozen_input() {
    let source = "local original={child={}}\nlocal root=original\nlocal old=root.child\nlocal function replace() original.child={} return 1 end\nroot.child.value=replace()\n---@type number\nlocal previous=old.value\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let report = check(&format!(
        "{source}---@type number\nlocal wrong=root.child.value\n"
    ));
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2322" | "TS2339")),
        "{:#?}",
        report.diagnostics
    );
}
