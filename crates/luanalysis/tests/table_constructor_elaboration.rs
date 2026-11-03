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

#[test]
fn a_field_an_array_or_dictionary_cannot_have_is_reported_at_its_key() {
    // As TypeScript reports an object literal's excess property, where the table would be
    // assignable without it: a table may have more fields than its type has, but an array has
    // no name, gap or key short of 1, and a dictionary of numbers no name.
    let source = "---@type number[]\nlocal a = {1, 2, [4] = 4}\n---@type number[]\nlocal b = {[0] = 0, [1] = 1}\n---@type number[]\nlocal c = {1, one = 1, two = 2}\n---@type table<number, number>\nlocal d = {word = 1}\n";
    assert_eq!(reported(source), [
        ("TS2322", "[4]".into(), "Index 4 leaves a gap in 'number[]': its next index is 3.".into()),
        ("TS2322", "[0]".into(), "Key 0 is not an index of 'number[]': an array's indexes are the whole numbers from 1.".into()),
        ("TS2353", "one".into(), "Object literal may only specify known properties, and 'one' does not exist in type 'number[]'.".into()),
        ("TS2353", "two".into(), "Object literal may only specify known properties, and 'two' does not exist in type 'number[]'.".into()),
        ("TS2353", "word".into(), "Object literal may only specify known properties, and 'word' does not exist in type 'table<number, number>'.".into()),
    ]);
    // A shape may have more fields than it declares, and a union of tables is not elaborated.
    assert!(reported(&format!("{SHAPES}---@type Point\nlocal p = {{x = 1, y = 2, z = 3}}\n")).is_empty());
    let union = "---@alias Value table<string, Value> | Value[] | number\n---@type Value\nlocal v = {[3] = 1}\n";
    let diagnostics = reported(union);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].1, "{[3] = 1}");
}

#[test]
fn a_class_definition_table_is_elaborated_but_an_inherited_field_is_reported_once() {
    // An inherited field is the check of the class's overrides' to report (TS2416); the table is
    // not reported as well. An own field is reported at its name.
    let source = "---@class Base\n---@field value number\nlocal Base = {}\n---@class Child : Base\n---@field own string\nlocal Child = { value = 'x', own = 2 }\n";
    assert_eq!(reported(source), [
        ("TS2416", "'x'".into(), "Declared member type '\"x\"' is not assignable to the inherited member type 'number'.".into()),
        ("TS2322", "own".into(), "Type '2' is not assignable to type 'nil | string'.".into()),
    ]);
}
