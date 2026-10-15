use luanalysis::check;

#[test]
fn written_callback_fields_keep_context_through_assignment_member_paths() {
    for target in ["callbacks.map", "callbacks['map']", "(callbacks).map"] {
        let source = format!(
            "local callbacks={{\n---@type fun(value:number):number\nmap=function(value) return value end,\n}}\n{target}=function(replacement) return replacement+1 end\nlocal result=callbacks.map(42)\n"
        );
        let report = check(&source);
        assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
        assert_eq!(
            report.type_at(source.find("function(replacement)").unwrap() + "function(".len()),
            Some("number")
        );
        let bad = source.replace("replacement+1", "'wrong'");
        let report = check(&bad);
        assert!(
            report.diagnostics.iter().any(|error| error.code == "TS2322"),
            "{bad}\n{:#?}",
            report.diagnostics
        );
        assert!(report.diagnostics.iter().all(|error| error.code != "TS7006"));
    }
}

#[test]
fn replacing_the_root_binding_does_not_contextualize_a_distinct_allocation() {
    let source = "local record={old=1}\nrecord={new='word'}\n---@type string\nlocal result=record.new\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let bad = source.replace("---@type string", "---@type number");
    let report = check(&bad);
    assert!(
        report.diagnostics.iter().any(|error| error.code == "TS2322"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn invalid_constructors_keep_only_the_original_key_diagnostic() {
    for source in ["local value={[nil]=42}", "local key=nil\nlocal value={[key]=42}"] {
        let report = check(source);
        assert_eq!(report.diagnostics.len(), 1, "{source}\n{:#?}", report.diagnostics);
        assert_eq!(report.diagnostics[0].code, "TS2538");
    }
}
