use luanalysis::check;
fn accepted(source: &str) {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
}
fn rejected(source: &str) -> luanalysis::CheckReport {
    let report = check(source);
    assert!(
        !report.diagnostics.is_empty(),
        "unexpected clean result: {source}"
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code != "LUA1001" && !d.code.starts_with("LUA2")),
        "{:#?}",
        report.diagnostics
    );
    report
}
#[test]
fn original_generic_builder_return_contract_contextualizes_and_publishes_the_table() {
    let source = "---@generic T\n---@param builder fun():T\n---@return T\nlocal function build(builder) return builder() end\nlocal value=build(function()\n---@type table<string,number>\nreturn {a=1}\nend)\n---@type table<string,number>\nlocal typed=value\n---@type number\nlocal member=value.a";
    accepted(source);
    let wrong = source.replace("{a=1}", "{a='wrong'}");
    assert!(
        rejected(&wrong)
            .diagnostics
            .iter()
            .any(|d| d.code == "TS2322")
    );
}
#[test]
fn written_fixed_pack_controls_value_types_and_checks_every_original_slot() {
    let source = "local function values()\n---@type number,string\nreturn 1,'text'\nend\nlocal number,text=values()\n---@type number\nlocal valid=number\n---@type string\nlocal also=text";
    accepted(source);
    let wrong = source.replace("return 1,'text'", "return 1,true");
    assert!(
        rejected(&wrong)
            .diagnostics
            .iter()
            .any(|d| d.code == "TS2322")
    );
    let wrong = source.replace(
        "---@type string\nlocal also",
        "---@type boolean\nlocal also",
    );
    assert!(
        rejected(&wrong)
            .diagnostics
            .iter()
            .any(|d| d.code == "TS2322")
    );
}
#[test]
fn whole_return_contract_preserves_expansion_and_parenthesized_scalar_adjustment() {
    let source = "---@return number,string\nlocal function pair() return 1,'text' end\nlocal function expanded()\n---@type number,string\nreturn pair()\nend\n---@param first number\n---@param second string\nlocal function consume(first,second) end\nconsume(expanded())";
    accepted(source);
    let wrong = source.replace("return pair()", "return (pair())");
    assert!(
        rejected(&wrong)
            .diagnostics
            .iter()
            .any(|d| d.code == "TS2322")
    );
}
#[test]
fn function_result_contract_is_checked_at_the_written_statement_annotation() {
    let source = "---@return number\nlocal function result()\n---@type string\nreturn 'text'\nend";
    let report = rejected(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "TS2322" && &source[d.span.start..d.span.end] == "@type string"),
        "{:#?}",
        report.diagnostics
    );
    accepted(
        &source
            .replace("@type string", "@type number")
            .replace("'text'", "42"),
    );
}
#[test]
fn outer_parameter_used_only_by_return_annotation_survives_distinct_captures() {
    let source = "---@generic T\n---@param value T\nlocal function make(value)\nreturn function()\n---@type T|nil\nreturn nil\nend\nend\nlocal number=make(1)\nlocal text=make('text')\n---@type number|nil\nlocal a=number()\n---@type string|nil\nlocal b=text()\n---@type number|nil\nlocal again=number()";
    accepted(source);
    let wrong = source.replace(
        "---@type string|nil\nlocal b",
        "---@type number|nil\nlocal b",
    );
    assert!(
        rejected(&wrong)
            .diagnostics
            .iter()
            .any(|d| d.code == "TS2322")
    );
}
#[test]
fn nil_statement_contract_does_not_invent_a_physical_result_for_empty_return() {
    let prefix = "local function empty()\n---@type nil\nreturn\nend\n---@param value nil\nlocal function consume(value) end\n";
    accepted(&format!("{prefix}consume((empty()))"));
    assert!(
        rejected(&format!("{prefix}consume(empty())"))
            .diagnostics
            .iter()
            .any(|d| d.code == "TS2554")
    );
}
#[test]
fn variadic_and_prefix_packs_keep_their_actual_tail_cardinality() {
    let source = "---@vararg string\nlocal function values(...)\n---@type number,string...\nreturn 1,...\nend\nlocal number,text=values('text')\n---@type number\nlocal first=number\n---@type string|nil\nlocal second=text";
    accepted(source);
    let wrong = source.replace(
        "---@type string|nil\nlocal second",
        "---@type boolean|nil\nlocal second",
    );
    assert!(
        rejected(&wrong)
            .diagnostics
            .iter()
            .any(|d| d.code == "TS2322")
    );
}
#[test]
fn return_annotations_preserve_branch_rows_and_unused_contract_diagnostics() {
    let source = "---@param flag boolean\nlocal function branch(flag)\nif flag then\n---@type number\nreturn 1\nelse\n---@type string,boolean\nreturn 'text',true\nend\nend\nlocal value,second=branch(true)\n---@type number|string\nlocal first=value\n---@type boolean|nil\nlocal tail=second";
    accepted(source);
    let invalid = "local function unused()\n---@type MissingReturnType\nreturn 1\nend";
    let report = rejected(invalid);
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code == "TS2304")
            .count(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        report.diagnostics.iter().any(
            |d| d.code == "TS2304" && &invalid[d.span.start..d.span.end] == "MissingReturnType"
        )
    );
}
#[test]
fn returned_function_headers_keep_original_parameter_and_signature_scopes() {
    accepted(
        "local function make()\n---@type fun<T>(value:T):T\nreturn function(value) return value end\nend\nlocal identity=make()\n---@type number\nlocal number=identity(1)\n---@type string\nlocal text=identity('text')",
    );
}
#[test]
fn cli_module_return_contract_reports_original_utf8_crlf_then_accepts_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis return contract {}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("module.lua"),
        "-- résumé\r\n---@type {count:number}\r\nreturn {count='wrong'}\r\n",
    )
    .unwrap();
    fs::write(
        directory.join("main.lua"),
        "local module=require('module')\n---@type number\nlocal count=module.count",
    )
    .unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&directory)
            .args(["check", "--format", "json", "main.lua"])
            .output()
            .unwrap()
    };
    let output = run();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stderr.is_empty());
    let json = String::from_utf8(output.stdout).unwrap();
    assert!(
        json.contains("\"code\":\"TS2322\"")
            && json.contains("\"position\":{\"line\":3,\"column\":8}"),
        "{json}"
    );
    fs::write(
        directory.join("module.lua"),
        "-- résumé\r\n---@type {count:number}\r\nreturn {count=42}\r\n",
    )
    .unwrap();
    let output = run();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"success\":true"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn function_types_in_statement_packs_keep_canonical_generic_signature_owners() {
    accepted(
        "local function make()\n---@type (fun<T>(value:T):T),number\nreturn function(value) return value end,1\nend\nlocal identity,count=make()\n---@type number\nlocal number=identity(1)\n---@type string\nlocal text=identity('text')\n---@type number\nlocal n=count",
    );
}

#[test]
fn written_record_literal_members_do_not_widen_before_return_validation() {
    let source = "local function value()\n---@type {tag:'fixed',count:1}\nreturn {tag='fixed',count=1}\nend\nlocal record=value()\n---@type 'fixed'\nlocal tag=record.tag\n---@type 1\nlocal count=record.count";
    accepted(source);
    let wrong = source.replace(
        "return {tag='fixed',count=1}",
        "return {tag='wrong',count=2}",
    );
    assert!(
        rejected(&wrong)
            .diagnostics
            .iter()
            .any(|d| d.code == "TS2322")
    );
}

#[test]
fn unreachable_return_annotations_still_validate_original_written_types() {
    let source =
        "local function unused()\ndo return end\n---@type MissingReturnType\nreturn 1\nend";
    let report = rejected(source);
    assert!(report.diagnostics.iter().any(|d| d.code=="TS2304" && &source[d.span.start..d.span.end]=="MissingReturnType"), "{:#?}", report.diagnostics);
    accepted(&source.replace("MissingReturnType", "number"));
}

#[test]
fn copied_written_literal_return_keeps_its_declared_type_while_inferred_values_widen() {
    let source = "local function one()\n---@type 1\nreturn 1\nend\nlocal copied=one()\ncopied=2";
    let report = rejected(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "TS2322" && d.message.contains("type '1'")),
        "{:#?}",
        report.diagnostics
    );
    accepted(&source.replace("@type 1", "@type number"));
    accepted("local inferred=1\ninferred=2");
}
