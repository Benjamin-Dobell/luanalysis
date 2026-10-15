#[test]
fn an_untrusted_local_named_assert_does_not_narrow_its_argument_after_a_call() {
    let source = "---@param value number|nil\nlocal function assert(value) return value end\n---@param value number|nil\n---@return number\nlocal function use(value)\n assert(value)\n return value\nend";
    let report = luanalysis::check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "value"
    );
}
