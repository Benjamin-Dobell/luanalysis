use luanalysis::check;

#[test]
fn a_generic_context_keeps_each_native_mapped_capture_environment() {
    let source = "---@type {identity:fun<T>(value:T):T}\nlocal object\n---@generic T\n---@param value T\n---@return T\nobject.identity=function(value) return value end\nlocal number=object.identity(42)\nlocal text=object.identity(\"yes\")";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let ty = |name| &report.bindings.iter().find(|binding| binding.name==name).unwrap().ty;
    assert!(matches!(ty("number").as_str(), "number"|"42"));
    assert!(matches!(ty("text").as_str(), "string"|"\"yes\""));
}
