use luanalysis::check;

#[test]
fn annotated_declarations_without_rhs_keep_their_contract_in_bodies_and_captures() {
    let source = "---@type number\nlocal value\nvalue=42\n---@param text string\nlocal function outer(text)\n---@type string\nlocal later\nlocal function capture() return later end\nlater=text\nreturn capture\nend\nlocal read=outer(\"word\")\n---@type string\nlocal result=read()";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    for (name, expected) in [
        ("value", "number"),
        ("later", "string"),
        ("result", "string"),
    ] {
        assert!(
            report
                .bindings
                .iter()
                .any(|binding| binding.name == name && binding.ty == expected),
            "{:#?}",
            report.bindings
        );
    }
}

#[test]
fn explicit_nil_and_short_rhs_lists_still_check_the_declared_contract() {
    for source in [
        "---@type string\nlocal value=nil",
        "---@type number,string\nlocal first,second=42",
        "---@param text string\nlocal function use(text)\n---@type string\nlocal value=nil\nend",
    ] {
        let report = check(source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{source}\n{:#?}",
            report.diagnostics
        );
        assert_eq!(report.diagnostics[0].code, "TS2322");
    }
}
