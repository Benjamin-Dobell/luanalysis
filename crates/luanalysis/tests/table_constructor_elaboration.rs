//! A table constructor that is not assignable has its error moved into the fields that are not,
//! as TypeScript's `elaborateError` moves an object literal's into its properties: at a named
//! field's name, and at a sequence field's value.
use luanalysis::check;

/// Each diagnostic as its code and the text it covers.
fn reported(source: &str) -> Vec<(&'static str, String, String)> {
    check(source)
        .diagnostics
        .into_iter()
        .map(|diagnostic| (diagnostic.code, source[diagnostic.span.start..diagnostic.span.end].to_owned(), diagnostic.message))
        .collect()
}

const SHAPES: &str = "---@shape Point\n---@field x number\n---@field y number\n\n---@shape Segment\n---@field from Point\n---@field to Point\n\n";

#[test]
fn a_field_that_is_not_assignable_is_reported_at_its_name() {
    let source = format!("{SHAPES}---@type Point\nlocal p = {{x = 1, y = 'two'}}\np = {{x = 'one', y = 2}}\n");
    assert_eq!(reported(&source), [
        ("TS2322", "y".into(), "Type '\"two\"' is not assignable to type 'number'.".into()),
        ("TS2322", "x".into(), "Type '\"one\"' is not assignable to type 'number'.".into()),
    ]);
}

#[test]
fn a_constructor_written_as_a_fields_value_is_elaborated_in_turn() {
    let source = format!("{SHAPES}---@type Segment\nlocal s = {{from = {{x = 1, y = 2}}, to = {{x = true, y = 2}}}}\n");
    assert_eq!(reported(&source), [("TS2322", "x".into(), "Type 'true' is not assignable to type 'number'.".into())]);
}

#[test]
fn a_sequence_field_is_reported_at_its_value() {
    let source = "---@type number[]\nlocal numbers = {1, 'two', 3}\n";
    assert_eq!(reported(source), [("TS2322", "'two'".into(), "Type '\"two\"' is not assignable to type 'number'.".into())]);
}

#[test]
fn an_argument_and_a_returned_constructor_are_elaborated() {
    let source = format!("{SHAPES}---@param p Point\nlocal function take(p) end\ntake({{x = 1, y = 'two'}})\n---@return Point\nlocal function make() return {{x = 'one', y = 2}} end\n");
    assert_eq!(reported(&source), [
        ("TS2322", "y".into(), "Type '\"two\"' is not assignable to type 'number'.".into()),
        ("TS2322", "x".into(), "Type '\"one\"' is not assignable to type 'number'.".into()),
    ]);
}

#[test]
fn a_constructor_with_no_field_to_blame_is_reported_whole() {
    // A missing field has no field of the constructor to report it at.
    let source = format!("{SHAPES}---@type Point\nlocal p = {{x = 1}}\n");
    let diagnostics = reported(&source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].1, "{x = 1}");
    let assignable = format!("{SHAPES}---@type Point\nlocal p = {{x = 1, y = 2}}\n");
    assert!(reported(&assignable).is_empty());
    // A class is nominal: a constructor is none of it, whatever its fields.
    let class = "---@class Named\n---@field x number\nlocal Named = {}\n\n---@type Named\nlocal n = {x = 1}\n";
    let diagnostics = reported(class);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].1, "{x = 1}");
}
