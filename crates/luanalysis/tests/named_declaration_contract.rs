use luanalysis::check;

#[test]
fn retained_standalone_declarations_cannot_count_as_semantically_checked() {
    for source in [
        "---@alias Unused Missing",
        "---@class Unused\n---@field member Missing",
        "---@shape Unused\n---@field member Missing",
    ] {
        let report = check(source);
        assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
        let diagnostic = &report.diagnostics[0];
        assert_eq!(diagnostic.code, "LUA2031");
        assert_eq!(
            &source[diagnostic.span.start..diagnostic.span.end],
            "Unused"
        );
    }
}

#[test]
fn retained_external_field_augmentations_require_a_named_type_environment() {
    let source = "---@field<Existing> member number";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "LUA2031");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "Existing"
    );
}
