use luanalysis::check;

#[test]
fn a_captured_copy_cannot_replay_a_child_replaced_after_closure_creation() {
    let source = "local old={}\nlocal root={child=old}\nlocal function mutate()\nlocal copied=root\nlocal alias=copied.child\nalias.value=1\n---@type number\nlocal wrong=old.value\nend\nroot.child={}\nmutate()\n";
    let report = check(source);
    let wrong = source.find("old.value").unwrap();
    assert!(
        report.diagnostics.iter().any(|error| {
            error.code == "TS2322" && error.span.start <= wrong && wrong < error.span.end
        }),
        "{:#?}",
        report.diagnostics
    );
    let corrected = source.replace("local wrong=old.value", "local proven=alias.value");
    let report = check(&corrected);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn a_postcreation_replacement_through_an_alias_cannot_replay_a_captured_child() {
    let source = "local old={}\nlocal root={child=old}\nlocal writer=root\nlocal function mutate()\nlocal copied=root\nlocal alias=copied.child\nalias.value=1\n---@type number\nlocal wrong=old.value\nend\nwriter.child={}\nmutate()\n";
    let report = check(source);
    assert!(
        report.diagnostics.iter().any(|error| error.code == "TS2322"),
        "{:#?}",
        report.diagnostics
    );
    let corrected = source.replace("local wrong=old.value", "local proven=alias.value");
    let report = check(&corrected);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn uncertain_projected_capture_identity_does_not_hide_a_later_local_store() {
    for later in ["", "root.other={}\n"] {
        let source = format!("local root={{child={{}}}}\nlocal function mutate()\nlocal copied=root\nlocal alias=copied.child\nalias.value=1\n---@type number\nlocal proven=alias.value\nend\n{later}");
        let report = check(&source);
        assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    }
}

#[test]
fn unchanged_captured_heap_identity_requires_a_proof_separate_from_cell_immutability() {
    for later in ["", "root.other={}\n"] {
        let source = format!("local old={{}}\nlocal root={{child=old}}\nlocal function mutate()\nlocal copied=root\nlocal alias=copied.child\nalias.value=1\n---@type number\nlocal unproven=old.value\nend\n{later}");
        let report = check(&source);
        assert!(
            report.diagnostics.iter().any(|error| error.code == "TS2322"),
            "{source}\n{:#?}",
            report.diagnostics
        );
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
