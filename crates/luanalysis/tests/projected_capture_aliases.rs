use luanalysis::check;

#[test]
fn uncertain_projected_capture_identity_does_not_hide_a_later_local_store() {
    for later in ["", "root.other={}\n"] {
        let source = format!("local root={{child={{}}}}\nlocal function mutate()\nlocal copied=root\nlocal alias=copied.child\nalias.value=1\n---@type number\nlocal proven=alias.value\nend\n{later}");
        let report = check(&source);
        assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    }
}

#[test]
fn separate_calls_cannot_share_an_opaque_projected_entry_origin() {
    let source = "local root={child={}}\nlocal function get()\nlocal copied=root\nreturn copied.child\nend\nlocal first=get()\nroot.child={}\nlocal second=get()\nfirst.value=1\n---@type number\nlocal wrong=second.value\n";
    let report = check(source);
    assert!(
        report.diagnostics.iter().any(|error| matches!(error.code, "TS2322" | "TS2339")),
        "{:#?}",
        report.diagnostics
    );
    let corrected = source.replace("local wrong=second.value", "local proven=first.value");
    let report = check(&corrected);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn distinct_closure_activations_cannot_share_projected_capture_identity() {
    let source = "local function make()\nlocal root={child={}}\nreturn function()\nlocal copied=root\nreturn copied.child\nend\nend\nlocal left=make()\nlocal right=make()\nlocal first=left()\nlocal second=right()\nfirst.value=1\n---@type number\nlocal wrong=second.value\n";
    let report = check(source);
    assert!(
        report.diagnostics.iter().any(|error| matches!(error.code, "TS2322" | "TS2339")),
        "{:#?}",
        report.diagnostics
    );
    let corrected = source.replace("local wrong=second.value", "local proven=first.value");
    let report = check(&corrected);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

// A captured read uses the table's prospective schema. Its members currently
// come from every store whose receiver has that table's type, stores through
// copies included, so the closure's read is typed by the copy's store.
// docs/BUN-CHECK-COMPARISON.md item 4 recommends binder-level ownership.
#[test]
fn captured_reads_use_the_prospective_schema_of_stores_through_copies() {
    for later in ["root.child={}\nmutate()\n", "local writer=root\nwriter.child={}\nmutate()\n", "root.other={}\n"] {
        let source = format!("local old={{}}\nlocal root={{child=old}}\nlocal function mutate()\nlocal copied=root\nlocal alias=copied.child\nalias.value=1\n---@type number\nlocal read=old.value\nend\n{later}");
        let report = check(&source);
        assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    }
}
