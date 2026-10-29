//! `A & B` is what is each of its types at once. As TypeScript reads an intersection's member,
//! a member some part declares is read from the parts that declare it, and only another name
//! from the parts' index signatures: a lookup that falls back for unknown names keeps each
//! known member's own type, though a single type's index signature would conflict with them.
use luanalysis::check;

const NAMESPACE: &str = "---@class ColorValue\n---@field r number\n\n---@class ColorStatics\n---@field __isColor true\n---@field Red ColorValue\n---@field list string[]\n---@field fromString fun(value: string): ColorValue\n\n---@alias ColorNamespace ColorStatics & { [string]: ColorValue | nil }\n\n";

#[test]
fn an_intersections_declared_members_keep_their_types_and_other_names_use_its_index_signatures() {
    let source = format!(
        "{NAMESPACE}---@param Color ColorNamespace\n---@param name string\nlocal function probe(Color, name)\n    local red = Color.Red\n    local from = Color.fromString\n    local list = Color.list\n    local lower = Color.red\n    local dynamic = Color[name]\n    local made = Color.fromString(name)\n    return red, from, list, lower, dynamic, made\nend\n"
    );
    let report = check(&source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let at = |name: &str| {
        report
            .type_at(source.find(&format!("local {name} =")).unwrap() + 6)
            .map(str::to_owned)
    };
    assert_eq!(at("red").as_deref(), Some("ColorValue"));
    assert_eq!(
        at("from").as_deref(),
        Some("fun(value: string): ColorValue")
    );
    assert_eq!(at("list").as_deref(), Some("string[]"));
    // Not declared: the index signature's, neither `any` nor any member's type.
    assert_eq!(at("lower").as_deref(), Some("nil | ColorValue"));
    let dynamic = at("dynamic").unwrap();
    for possible in [
        "nil",
        "ColorValue",
        "true",
        "string[]",
        "fun(value: string): ColorValue",
    ] {
        assert!(dynamic.contains(possible), "{dynamic}");
    }
    assert!(
        !dynamic.contains("any") && !dynamic.contains("unknown"),
        "{dynamic}"
    );
    assert_eq!(at("made").as_deref(), Some("ColorValue"));
}

#[test]
fn a_dynamic_namespace_read_cannot_drop_declared_statics() {
    let source = format!(
        "{NAMESPACE}---@param Color ColorNamespace\n---@param name string\nlocal function probe(Color,name)\n---@type ColorValue|nil\nlocal dropped=Color[name]\nend\n"
    );
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    let span = report.diagnostics[0].span;
    assert_eq!(&source[span.start..span.end], "Color[name]");
    let corrected = source.replace(
        "---@type ColorValue|nil",
        "---@type ColorValue|nil|true|string[]|(fun(value:string):ColorValue)",
    );
    let report = check(&corrected);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn an_intersection_is_each_of_its_types() {
    let source = format!(
        "{NAMESPACE}---@param statics ColorStatics\nlocal function takeStatics(statics) return statics end\n---@param lookup table<string, ColorValue | nil>\nlocal function takeLookup(lookup) return lookup end\n---@param Color ColorNamespace\nlocal function probe(Color)\n    takeStatics(Color)\n    takeLookup(Color)\n    ---@type number\n    local wrong = Color.Red\nend\n"
    );
    let report = check(&source);
    let codes: Vec<_> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(codes, ["TS2322"], "{:#?}", report.diagnostics);
    assert!(
        report.diagnostics[0]
            .message
            .contains("'ColorValue' is not assignable to type 'number'"),
        "{:#?}",
        report.diagnostics
    );
}
