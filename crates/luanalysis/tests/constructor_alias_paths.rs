use luanalysis::check;

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


// As in Bun (`isMatchingReference`), a store updates only the reference it is
// spelled through. Copies, nested constructor children and rebound roots are
// different spellings, so their stores do not update another spelling's view.
#[test]
fn stores_update_only_their_own_spelling() {
    for (source, other, own) in [
        ("local root={middle={child={}}}\nlocal alias=root.middle.child\nalias.value=1\n", "root.middle.child.value", "alias.value"),
        ("local original={child={}}\nlocal root=original\nlocal alias=root.child\nalias.value=1\n", "root.child.value", "alias.value"),
        ("local original={child={}}\nlocal root=original\noriginal={child={}}\nlocal alias=root.child\nalias.value=1\n", "original.child.value", "alias.value"),
    ] {
        let report = check(&format!("{source}---@type number\nlocal result={other}\n"));
        assert!(
            report.diagnostics.iter().any(|error| matches!(error.code, "TS2322" | "TS2339")),
            "{source}\n{:#?}",
            report.diagnostics
        );
        let report = check(&format!("{source}---@type number\nlocal result={own}\n"));
        assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    }
}

// Membership follows Bun's expando rule: a store declares a member only on the
// table its receiver names syntactically. `selected` has no initializer, so the
// copy owns no table: its store is an error and neither alternative gains it.
#[test]
fn a_branch_selected_copy_owns_no_table() {
    let source = "---@param flag boolean\nlocal function update(flag)\nlocal left={child={}}\nlocal right={child={}}\nlocal selected\nif flag then selected=left else selected=right end\nlocal root=selected\nlocal alias=root.child\nalias.value=1\nlocal read=left.child.value\nend\n";
    let report = check(source);
    for statement in ["alias.value=1", "local read=left.child.value"] {
        let start = source.find(statement).unwrap() + statement.rfind("value").unwrap();
        assert!(
            report.diagnostics.iter().any(|error| error.code == "TS2339" && error.span.start == start),
            "{statement}\n{:#?}",
            report.diagnostics
        );
    }
}
