use luanalysis::{CheckReport, check};

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}

#[test]
fn boolean_keys_have_canonical_original_flow_references() {
    let source = "---@type {[true]: number|string, [false]: number}\nlocal flags={[true]=1,[false]=3}\nif type(flags[true])==\"number\" then\nflags[(true)]=\"changed\"\n---@type string\nlocal changed=flags[true]\n---@type number\nlocal distinct=flags[false]\nend";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("changed=flags[true]").unwrap() + "changed=flags[true".len()),
        Some("string")
    );
}

#[test]
fn dynamic_writes_reset_potentially_aliased_member_narrowing() {
    let source = "---@param values table<string, number|string>\n---@param key string\nlocal function mutate(values,key)\nif type(values.slot)==\"number\" then\nvalues[key]=\"changed\"\nreturn values.slot+1\nend\nreturn 0\nend";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.code, "TS2362");
    assert_eq!(
        &source[diagnostic.span.start..diagnostic.span.end],
        "values.slot"
    );
    let ty = report
        .type_at(source.find("return values.slot").unwrap() + "return values.".len())
        .unwrap();
    assert!(ty.contains("number") && ty.contains("string"), "{ty}");
    clean(&source.replace("return values.slot+1", "return \"value: \"..values.slot"));
}

#[test]
fn dynamic_writes_preserve_unrelated_static_member_prefixes() {
    let source = "---@param record {values: table<string, number|string>, stable: number|string}\n---@param key string\nlocal function mutate(record,key)\nif type(record.stable)==\"number\" then\nrecord.values[key]=\"changed\"\nreturn record.stable+1\nend\nreturn 0\nend";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("return record.stable").unwrap() + "return record.".len()),
        Some("number")
    );
}

#[test]
fn compatible_written_child_contracts_do_not_hide_possible_self_aliases() {
    let source = "---@param record {values: table<string, number|string|table>, stable: number|string}\n---@param key string\nlocal function mutate(record,key)\nif type(record.stable)==\"number\" then\nrecord.values[key]=\"changed\"\nreturn record.stable+1\nend\nreturn 0\nend";
    let report = check(source);
    assert!(report.diagnostics.iter().any(|error| error.code == "TS2362"), "{:#?}", report.diagnostics);
    clean(&source.replace("return record.stable+1", "return \"value: \"..record.stable"));
}

#[test]
fn structural_callable_fields_do_not_disprove_shared_table_aliases() {
    let source = "---@class Callable\n---@overload fun():number\nlocal Callable={}\n---@param record {child:fun():number, stable:number|string}\n---@param other {child:table, stable:number|string}\nlocal function mutate(record,other)\n if type(record.stable)==\"number\" then\n  other.stable=\"changed\"\n  return record.stable+1\n end\n return 0\nend\nlocal shared={child=Callable,stable=1}\nlocal result=mutate(shared,shared)\n";
    let report = check(source);
    assert!(report.diagnostics.iter().any(|error| error.code == "TS2362"), "{:#?}", report.diagnostics);
    assert!(report.diagnostics.iter().all(|error| error.code != "TS2345"), "the same callable-table child satisfies both public contracts: {:#?}", report.diagnostics);
    clean(&source.replace("return record.stable+1", "return \"value: \"..record.stable"));
}

#[test]
fn simultaneous_discriminant_mutation_preserves_aliasing_across_frozen_epochs() {
    let source = "---@param record {kind: 'old'|'new', stable: number|string}\n---@param key string\nlocal function mutate(record,key)\n---@type table<string, number|string>\nlocal alias=record\nif type(record.stable)==\"number\" then\nrecord.kind,alias[key]=\"new\",\"changed\"\nreturn record.stable+1\nend\nreturn 0\nend";
    let report = check(source);
    assert!(report.diagnostics.iter().any(|error| error.code == "TS2362"), "{:#?}", report.diagnostics);
    clean(&source.replace("return record.stable+1", "return \"value: \"..record.stable"));
}

#[test]
fn runtime_table_keys_do_not_poison_later_root_reads() {
    clean(
        "---@type table<{id: number}, string>\nlocal entries={}\nlocal first={id=1}\nlocal second={id=1}\nentries[first]=\"first\"\nentries[second]=\"second\"\n---@type string\nlocal read=entries[first]",
    );
}
