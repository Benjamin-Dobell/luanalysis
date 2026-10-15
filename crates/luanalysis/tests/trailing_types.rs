use luanalysis::check;

#[test]
fn trailing_generic_signature_contextualizes_the_original_function_body() {
    let source = "local identity=(function(value)\nlocal copy=value\nreturn copy\nend) --[=[---@type fun<T>(value:T):T]=]\nlocal numeric=identity(42)\nlocal text=identity(\"word\")\nlocal again=identity(42)";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    for (name, expected) in [
        ("numeric", "number"),
        ("text", "string"),
        ("again", "number"),
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
fn trailing_union_contract_keeps_mutation_constraints_and_next_local_independent() {
    let source = "local value=42 ---@type number|string\nvalue=\"word\"\nlocal other=true\n---@type string\nlocal current=value\n---@type boolean\nlocal separate=other";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let wrong = source.replace("value=\"word\"", "value=true");
    let report = check(&wrong);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2322"
                && &wrong[error.span.start..error.span.end] == "true"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn trailing_multi_type_annotation_checks_nil_padding_of_an_actual_rhs() {
    let wrong = "local first,second=42 ---@type number,string";
    let report = check(wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    let corrected = wrong.replace("=42 ", "=42,\"word\" ");
    let report = check(&corrected);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}
