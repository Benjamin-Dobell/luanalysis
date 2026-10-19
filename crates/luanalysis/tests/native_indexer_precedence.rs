use luanalysis::{CheckReport, check};

const VECTOR: &str = "---@shape Vector\n---@field [1] number\n---@field [2] number\n---@field [3] number\n---@field [number] boolean\n\n";

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

fn error_at(source: &str, code: &str, expression: &str) {
    let report = check(source);
    assert_eq!(
        report.diagnostics.len(),
        1,
        "{source}\n{:#?}",
        report.diagnostics
    );
    let error = &report.diagnostics[0];
    assert_eq!(error.code, code, "{error:#?}");
    assert_eq!(&source[error.span.start..error.span.end], expression);
}

#[test]
fn exact_fields_override_the_same_declarations_fallback_indexer() {
    for kind in ["shape", "class"] {
        let declaration = VECTOR.replace("---@shape", &format!("---@{kind}"));
        let source = format!(
            "{declaration}---@param vector Vector\nlocal function inspect(vector)\n---@type number\nlocal first=vector[1]\n---@type number\nlocal second=vector[2]\n---@type number\nlocal third=vector[3]\n---@type boolean\nlocal fourth=vector[4]\nvector[1]=42\nvector[4]=true\nend\n"
        );
        clean(&source);
        error_at(
            &source.replace("vector[1]=42", "vector[1]=false"),
            "TS2322",
            "false",
        );
        error_at(
            &source.replace("vector[4]=true", "vector[4]=42"),
            "TS2322",
            "42",
        );
        error_at(
            &source.replace(
                "---@type number\nlocal first",
                "---@type boolean\nlocal first",
            ),
            "TS2322",
            "vector[1]",
        );
    }
}

#[test]
fn union_keys_select_each_exact_field_or_fallback_independently() {
    let source = format!(
        "{VECTOR}---@param vector Vector\n---@param exact 1|2\n---@param mixed 1|4\nlocal function inspect(vector,exact,mixed)\n---@type number\nlocal fixed=vector[exact]\n---@type number|boolean\nlocal either=vector[mixed]\nvector[exact]=42\nend\n"
    );
    clean(&source);
    error_at(
        &source.replace(
            "---@type number|boolean\nlocal either",
            "---@type number\nlocal either",
        ),
        "TS2322",
        "vector[mixed]",
    );
    for value in ["42", "true"] {
        let wrong = source.replace("vector[exact]=42", &format!("vector[mixed]={value}"));
        error_at(&wrong, "TS2322", value);
    }
}

#[test]
fn broad_reads_include_exact_exceptions_and_broad_writes_preserve_every_contract() {
    for (generic, key) in [("", "number"), ("---@generic K:number\n", "K")] {
        let source = format!(
            "{VECTOR}{generic}---@param vector Vector\n---@param key {key}\nlocal function inspect(vector,key)\n---@type number|boolean\nlocal value=vector[key]\nend\n"
        );
        clean(&source);
        for narrower in ["number", "boolean"] {
            error_at(
                &source.replace("---@type number|boolean", &format!("---@type {narrower}")),
                "TS2322",
                "vector[key]",
            );
        }
        for value in ["42", "true", "nil"] {
            error_at(
                &source.replace("end\n", &format!("vector[key]={value}\nend\n")),
                "TS2322",
                value,
            );
        }
        let compatible = source.replace("---@field [number] boolean", "---@field [number] number");
        clean(&compatible.replace("end\n", "vector[key]=42\nend\n"));
    }
}

#[test]
fn constructor_and_dictionary_relations_check_exact_exceptions_and_fallback_entries() {
    let source = format!("{VECTOR}---@type Vector\nlocal vector={{[1]=1,[2]=2,[3]=3,[4]=true}}\n");
    clean(&source);
    error_at(
        &source.replace("[1]=1", "[1]=false"),
        "TS2322",
        "{[1]=false,[2]=2,[3]=3,[4]=true}",
    );
    error_at(
        &source.replace("[4]=true", "[4]=4"),
        "TS2322",
        "{[1]=1,[2]=2,[3]=3,[4]=4}",
    );
    let source = format!(
        "{VECTOR}---@param vector Vector\nlocal function inspect(vector)\n---@type table<number,boolean>\nlocal dictionary=vector\nend\n"
    );
    error_at(&source, "TS2322", "vector");
    clean(&source.replace("table<number,boolean>", "table<number,number|boolean>"));
}

#[test]
fn own_exact_precedence_does_not_remove_inherited_indexer_conformance() {
    let wrong = "---@shape Base\n---@field [number] boolean\n\n---@shape Child:Base\n---@field [1] number\n";
    error_at(wrong, "TS2416", "number");
    clean(&wrong.replace("---@field [1] number", "---@field [1] boolean"));
    let inherited_field = "---@shape Base\n---@field [1] number\n\n---@shape Child:Base\n---@field [number] boolean\n";
    error_at(inherited_field, "TS2416", "boolean");
    clean(&inherited_field.replace("---@field [number] boolean", "---@field [number] number"));
    let source = format!(
        "{VECTOR}---@shape Child:Vector\n\n---@param child Child\n---@param key number\nlocal function inspect(child,key)\n---@type number\nlocal exact=child[1]\n---@type number|boolean\nlocal broad=child[key]\nend\n"
    );
    clean(&source);
}

#[test]
fn broad_intersection_reads_include_exact_fields_from_other_constituents() {
    for (generic, key) in [("", "string"), ("---@generic K:string\n", "K")] {
        let source = format!(
            "---@alias Bag {{special:string}}&table<string,number>\n\n{generic}---@param bag Bag\n---@param key {key}\nlocal function inspect(bag,key)\n---@type string\nlocal exact=bag.special\n---@type number\nlocal fallback=bag.other\n---@type number|string\nlocal dynamic=bag[key]\nend\n"
        );
        clean(&source);
        for narrower in ["number", "string"] {
            error_at(
                &source.replace("---@type number|string", &format!("---@type {narrower}")),
                "TS2322",
                "bag[key]",
            );
        }
    }
    clean(
        "---@alias Bag {special:string|nil}&table<string,number>\n\n---@param bag Bag\n---@param key string\nlocal function inspect(bag,key)\n---@type number|string|nil\nlocal dynamic=bag[key]\nend\n",
    );
}

#[test]
fn intersection_writes_use_exact_precedence_and_preserve_every_possible_broad_target() {
    let source = "---@alias Bag {special:string}&table<string,number>\n\n---@param bag Bag\n---@param key string\nlocal function inspect(bag,key)\nbag.special='changed'\nbag.other=42\nend\n";
    clean(source);
    error_at(
        &source.replace("bag.special='changed'", "bag.special=42"),
        "TS2322",
        "42",
    );
    error_at(
        &source.replace("bag.other=42", "bag.other='wrong'"),
        "TS2322",
        "'wrong'",
    );
    for value in ["42", "'wrong'", "nil"] {
        error_at(
            &source.replace("end\n", &format!("bag[key]={value}\nend\n")),
            "TS2322",
            value,
        );
    }
    let compatible = "---@alias Bag {special:number}&table<string,number>\n\n---@param bag Bag\n---@param key string\nlocal function inspect(bag,key)\nbag[key]=42\nend\n";
    clean(compatible);
    error_at(
        &compatible.replace("bag[key]=42", "bag[key]='wrong'"),
        "TS2322",
        "'wrong'",
    );
}
