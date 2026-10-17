//! As TypeScript checks JavaScript with JSDoc, a type parameter that a call infers nothing for
//! is `any` (`InferenceFlags.AnyDefault`), not `unknown`: an `any` argument, such as a value
//! from an unresolved module, does not make everything read through the call unknown.
use luanalysis::check;

#[test]
fn an_uninferred_type_parameter_is_any() {
    let source = "---@type any\nlocal list = nil\n---@generic T\n---@param values T[]\n---@return T\nlocal function first(values) return values[1] end\nlocal head = first(list)\nlocal length = head.length\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(report.type_at(source.find("head =").unwrap()).unwrap(), "any");
}

#[test]
fn an_inferred_type_parameter_keeps_its_type() {
    let source = "---@generic T\n---@param values T[]\n---@return T\nlocal function first(values) return values[1] end\nlocal head = first({ 1, 2 })\nlocal wrong = head.length\n";
    let codes: Vec<_> = check(source).diagnostics.iter().map(|diagnostic| diagnostic.code).collect();
    assert_eq!(codes, ["TS2339"]);
}
