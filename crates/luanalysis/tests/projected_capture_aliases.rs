use luanalysis::check;

#[test]
fn uncertain_projected_capture_identity_does_not_hide_a_later_local_store() {
    for later in ["", "root.other={}\n"] {
        let source = format!("local root={{child={{}}}}\nlocal function mutate()\nlocal copied=root\nlocal alias=copied.child\nalias.value=1\n---@type number\nlocal proven=alias.value\nend\n{later}");
        let report = check(&source);
        assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    }
}

// A captured read uses the table's prospective schema. Its members currently
// come from every store whose receiver has that table's type, stores through
// copies included, so the closure's read is typed by the copy's store, where
// binder-level ownership would type it by the table's own stores.
#[test]
fn captured_reads_use_the_prospective_schema_of_stores_through_copies() {
    for later in ["root.child={}\nmutate()\n", "local writer=root\nwriter.child={}\nmutate()\n", "root.other={}\n"] {
        let source = format!("local old={{}}\nlocal root={{child=old}}\nlocal function mutate()\nlocal copied=root\nlocal alias=copied.child\nalias.value=1\n---@type number\nlocal read=old.value\nend\n{later}");
        let report = check(&source);
        assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    }
}

// A function result has no syntactic owner, so a store through it declares no
// member (Bun's expando rule) and is reported where it is written.
#[test]
fn stores_through_function_results_declare_no_member() {
    for source in [
        "local root={child={}}\nlocal function get()\nlocal copied=root\nreturn copied.child\nend\nlocal first=get()\nfirst.value=1\n",
        "local function make()\nlocal root={child={}}\nreturn function()\nlocal copied=root\nreturn copied.child\nend\nend\nlocal left=make()\nlocal first=left()\nfirst.value=1\n",
    ] {
        let report = check(source);
        let start = source.find("first.value=1").unwrap() + "first.".len();
        assert!(
            report.diagnostics.iter().any(|error| error.code == "TS2339" && error.span.start == start),
            "{source}\n{:#?}",
            report.diagnostics
        );
    }
}
