use luanalysis::check;

#[test]
fn retained_standalone_declarations_cannot_count_as_semantically_checked() {
    for (source, code, origin) in [
        ("---@alias Unused Missing", "TS2304", "Missing"),
        (
            "---@class Unused\n---@field member Missing",
            "TS2304",
            "Missing",
        ),
        (
            "---@shape Unused\n---@field member Missing",
            "TS2304",
            "Missing",
        ),
    ] {
        let report = check(source);
        assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
        let diagnostic = &report.diagnostics[0];
        assert_eq!(diagnostic.code, code);
        assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], origin);
    }
}

#[test]
fn retained_external_field_augmentations_require_a_named_type_environment() {
    let source = "---@field<Existing> member number";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2304");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "Existing"
    );
}
