use luanalysis::{CheckReport, check};

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    report
}

fn error_at(report: &CheckReport, source: &str, code: &str, statement: &str, expression: &str) {
    let start = source.find(statement).unwrap() + statement.rfind(expression).unwrap();
    let end = start + expression.len();
    assert!(
        report.diagnostics.iter().any(|error| error.code == code
            && error.span.start == start && error.span.end == end),
        "{source}\nexpected {code} at {start}..{end}\n{:#?}",
        report.diagnostics
    );
}

fn has_parts(report: &CheckReport, source: &str, binding: &str, parts: &[&str]) {
    let ty = report.type_at(source.find(binding).unwrap()).unwrap();
    for part in parts {
        let present = ty.contains(part)
            || (*part == "boolean"
                && ty.split(" | ").any(|term| term == "false")
                && ty.split(" | ").any(|term| term == "true"));
        assert!(present, "{binding}: {ty}, missing {part}");
    }
    assert!(!ty.contains("any") && !ty.contains("unknown"), "{binding}: {ty}");
}

#[test]
fn reached_numeric_store_infers_a_sparse_string_domain() {
    let source = "---@param index number\n---@param other number\nlocal function fill(index,other)\n local ary={}\n ary[index]='word'\n local inferred=ary[other]\n ---@type nil|string\n local accepted=inferred\nend\n";
    let report = clean(source);
    has_parts(&report, source, "inferred=", &["nil", "string"]);
    let wrong = source.replace("---@type nil|string", "---@type nil|boolean");
    error_at(&check(&wrong), &wrong, "TS2322", "local accepted=inferred", "inferred");
    clean(&wrong.replace("---@type nil|boolean", "---@type nil|string"));
}

#[test]
fn an_rhs_call_without_the_local_table_still_supplies_its_written_value_type() {
    let source = "---@return string\nlocal function nextValue() return 'word' end\n---@param index number\nlocal function fill(index)\n local ary={}\n ary[index]=nextValue()\n ---@type nil|string\n local reached=ary[2]\nend\n";
    clean(source);
    let wrong = source.replace("---@type nil|string", "---@type nil|number");
    error_at(&check(&wrong), &wrong, "TS2322", "local reached=ary[2]", "ary[2]");
    clean(&wrong.replace("---@type nil|number", "---@type nil|string"));
}

#[test]
fn repeated_numeric_stores_keep_all_reached_values_at_other_keys() {
    let source = "---@param first number\n---@param second number\n---@param other number\nlocal function fill(first,second,other)\n local ary={}\n ary[first]='word'\n ary[second]=true\n local inferred=ary[other]\n ---@type nil|string|boolean\n local accepted=inferred\nend\n";
    let report = clean(source);
    has_parts(&report, source, "inferred=", &["nil", "string", "boolean"]);
    for narrowed in ["nil|string", "nil|boolean"] {
        let wrong = source.replace("---@type nil|string|boolean", &format!("---@type {narrowed}"));
        error_at(&check(&wrong), &wrong, "TS2322", "local accepted=inferred", "inferred");
        clean(&wrong.replace(&format!("---@type {narrowed}"), "---@type nil|string|boolean"));
    }
}

#[test]
fn conditional_numeric_stores_do_not_make_unwritten_keys_present() {
    let source = "---@param index number\n---@param other number\n---@param flag boolean\nlocal function fill(index,other,flag)\n local ary={}\n if flag then ary[index]='word' end\n local optional=ary[other]\n ---@type nil|string\n local accepted=optional\nend\n";
    let report = clean(source);
    has_parts(&report, source, "optional=", &["nil", "string"]);
    let wrong = source.replace("---@type nil|string", "---@type string");
    error_at(&check(&wrong), &wrong, "TS2322", "local accepted=optional", "optional");
    clean(&wrong.replace("---@type string", "---@type nil|string"));
}

#[test]
fn numeric_reads_use_reached_stores_without_future_value_pollution() {
    let source = "---@param index number\n---@param other number\nlocal function fill(index,other)\n local ary={}\n ary[index]='word'\n local earlier=ary[other]\n ---@type nil|string\n local accepted=earlier\n ary[index]=true\n local later=ary[other]\nend\n";
    let report = clean(source);
    has_parts(&report, source, "earlier=", &["nil", "string"]);
    let earlier = report.type_at(source.find("earlier=").unwrap()).unwrap();
    for boolean in ["boolean", "false", "true"] {
        assert!(!earlier.split(" | ").any(|term| term == boolean), "{earlier}");
    }
    has_parts(&report, source, "later=", &["nil", "string", "boolean"]);

    let before = "---@param index number\nlocal function fill(index)\n local ary={}\n local before=ary[index]\n ary[index]='word'\nend\n";
    error_at(&check(before), before, "TS2339", "local before=ary[index]", "index");
    let corrected = before.replace(
        "local before=ary[index]\n ary[index]='word'",
        "ary[index]='word'\n local before=ary[index]",
    );
    has_parts(&clean(&corrected), &corrected, "before=", &["nil", "string"]);
}

#[test]
fn written_numeric_storage_bounds_keep_rejecting_wrong_values_and_keys() {
    for (header, annotation) in [
        ("", "{[number]:string}"),
        ("---@shape Values\n---@field [number] string\n\n", "Values"),
        ("", "string[]"),
        ("", "table<number,string>"),
    ] {
        let source = format!("{header}---@param index number\nlocal function fill(index)\n ---@type {annotation}\n local ary={{}}\n ary[index]=true\nend\n");
        error_at(&check(&source), &source, "TS2322", "ary[index]=true", "true");
        clean(&source.replace("ary[index]=true", "ary[index]='word'"));
    }
    for key in ["nil", "true"] {
        let source = format!("---@type table<number,string>\nlocal ary={{}}\nary[{key}]='word'\n");
        error_at(&check(&source), &source, "TS2538", &format!("ary[{key}]='word'"), key);
        clean(&source.replace(&format!("ary[{key}]"), "ary[1]"));
    }
}

#[test]
fn string_unknown_any_and_unconstrained_generic_keys_do_not_infer_numeric_storage() {
    for (generic, key_type) in [("", "string"), ("", "unknown"), ("", "any"), ("---@generic K\n", "K")] {
        let source = format!("{generic}---@param key {key_type}\nlocal function fill(key)\n local ary={{}}\n ary[key]='word'\n local absent=ary[1]\nend\n");
        let report = check(&source);
        error_at(&report, &source, "TS2339", "local absent=ary[1]", "1");
        if key_type == "string" {
            error_at(&report, &source, "TS2339", "ary[key]='word'", "key");
        }
        let corrected = source.replace(&format!("---@param key {key_type}"), "---@param key number");
        clean(&corrected);
    }
}

#[test]
fn fixed_numeric_fields_remain_exact_and_sparse_numeric_domains_do_not_become_arrays() {
    let fixed = "local ary={}\nary[1]='word'\nlocal exact=ary[1]\n";
    assert_eq!(clean(fixed).type_at(fixed.find("exact=").unwrap()), Some("string"));
    let source = "---@param index number\nlocal function fill(index)\n local ary={}\n ary[index]='word'\n ---@type string[]\n local sequence=ary\nend\n";
    error_at(&check(source), source, "TS2322", "local sequence=ary", "ary");
    clean(&source.replace("---@type string[]", "---@type {[number]:nil|string}"));
}

#[test]
fn numeric_stores_join_overlapping_literal_values_without_widening_other_key_domains() {
    let source = "---@param index number\nlocal function fill(index)\n local value={[1]=1,named=false,[false]='flag'}\n value[index]='word'\n local overlap=value[1]\n ---@type number|string\n local accepted=overlap\n local named=value.named\n local flag=value[false]\n value[index]=nil\n local deleted=value[1]\n ---@type nil|number|string\n local optional=deleted\n local stillNamed=value.named\n local stillFlag=value[false]\nend\n";
    let report = clean(source);
    has_parts(&report, source, "overlap=", &["1", "string"]);
    assert!(!report.type_at(source.find("overlap=").unwrap()).unwrap().contains("nil"));
    has_parts(&report, source, "deleted=", &["nil", "1", "string"]);
    for binding in ["named=", "stillNamed="] {
        let local = format!("local {binding}");
        assert_eq!(report.type_at(source.find(&local).unwrap() + "local ".len()), Some("boolean"));
    }
    for binding in ["flag=", "stillFlag="] {
        assert_eq!(report.type_at(source.find(binding).unwrap()), Some("string"));
    }
    let wrong = source.replace("---@type nil|number|string", "---@type number|string");
    error_at(&check(&wrong), &wrong, "TS2322", "local optional=deleted", "deleted");
    clean(&wrong.replace("---@type number|string\n local optional", "---@type nil|number|string\n local optional"));
}

#[test]
fn computed_key_rebinding_evolves_the_frozen_receiver_without_populating_its_replacement() {
    let source = "local value={}\nlocal original=value\n---@return number\nlocal function key()\n value={}\n return 1\nend\nvalue[key()]='word'\nlocal reached=original[2]\nlocal missing=value[2]\n";
    let report = check(source);
    has_parts(&report, source, "reached=", &["nil", "string"]);
    error_at(&report, source, "TS2339", "local missing=value[2]", "2");
    let corrected = source.replace("local missing=value[2]", "local missing=original[2]");
    has_parts(&clean(&corrected), &corrected, "missing=", &["nil", "string"]);
}
