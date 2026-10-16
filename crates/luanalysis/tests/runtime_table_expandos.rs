use luanalysis::{CheckReport, check};
fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}
#[test]
fn ordinary_table_stores_infer_structural_scalar_and_function_members() {
    let source = "local Api={}\nApi.count=42\n---@param value number\n---@return number\nfunction Api.twice(value) return value*2 end\nlocal count=Api.count\nlocal result=Api.twice(3)\n";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("count=Api").unwrap()),
        Some("number")
    );
    assert_eq!(
        report.type_at(source.find("result=Api").unwrap()),
        Some("number")
    );
    let wrong = source.replace("Api.twice(3)", "Api.twice('wrong')");
    assert!(
        check(&wrong)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2345")
    );
    clean(&wrong.replace("Api.twice('wrong')", "Api.twice(3)"));
}
#[test]
fn a_known_future_store_does_not_make_the_member_present_before_execution() {
    let source =
        "local value={}\nlocal before=value.count\nvalue.count=1\nlocal after=value.count\n";
    let report = clean(source);
    assert_eq!(report.type_at(source.find("before=").unwrap()), Some("nil"));
    assert_eq!(
        report.type_at(source.find("after=").unwrap()),
        Some("number")
    );
    let unknown = check("local value={}\nlocal unknown=value.neverStored\n");
    assert!(
        unknown
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2339")
    );
}

// A copy is another spelling: its stores do not update the original's view.
#[test]
fn plain_table_aliases_do_not_share_stores_and_rebound_roots_stay_separate() {
    let source = "local value={}\nlocal alias=value\nalias.count=1\nlocal shared=value.count\nvalue={}\nvalue.other='new'\nlocal old=alias.count\nlocal fresh=value.other\n";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("shared=").unwrap()),
        Some("nil")
    );
    assert_eq!(report.type_at(source.find("old=").unwrap()), Some("number"));
    assert_eq!(
        report.type_at(source.find("fresh=").unwrap()),
        Some("string")
    );
    assert!(
        check(&format!("{source}local wrong=value.count\n"))
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2339")
    );
}
#[test]
fn branch_store_presence_remains_optional_and_narrows_at_the_original_read() {
    let source = "---@param flag boolean\nlocal function example(flag)\n local value={}\n if flag then value.count=1 end\n local optional=value.count\n if value.count~=nil then\n  ---@type number\n  local accepted=value.count\n end\nend\n";
    let report = clean(source);
    let optional = report.type_at(source.find("optional=").unwrap()).unwrap();
    assert!(
        optional.contains("number") && optional.contains("nil"),
        "{optional}"
    );
    let bad = source.replace(
        "local optional=value.count",
        "---@type number\n local optional=value.count",
    );
    assert!(
        check(&bad)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2322")
    );
}
#[test]
fn shadowed_plain_tables_keep_distinct_original_allocations() {
    let source = "local value={}\ndo\n local value={}\n value.item='inside'\n local inner=value.item\nend\nvalue.item=1\nlocal outer=value.item\n";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("inner=").unwrap()),
        Some("string")
    );
    assert_eq!(
        report.type_at(source.find("outer=").unwrap()),
        Some("number")
    );
}
#[test]
fn two_factory_calls_share_schema_but_do_not_share_actual_member_presence() {
    let source = "local function fresh() return {} end\nlocal first=fresh()\nlocal second=fresh()\nfirst.item=1\nlocal populated=first.item\nlocal absent=second.item\n";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("populated=").unwrap()),
        Some("number")
    );
    assert_eq!(report.type_at(source.find("absent=").unwrap()), Some("nil"));
}

#[test]
fn independent_flow_builds_do_not_change_factory_alias_presence() {
    // Flow construction is a public original-source operation, independent of
    // native declaration allocation. Their snapshot numbers need not align.
    let syntax = luanalysis_syntax::parse("local original={}\n");
    let bound = luanalysis::bind::bind(&syntax);
    let original = luanalysis::flow::build(&syntax, &bound);
    assert_eq!(original.graphs[0].entry.snapshot, original.snapshot);
    let source = "local function fresh() return {} end\nlocal first=fresh()\nlocal second=fresh()\nfirst.item=1\nlocal populated=first.item\nlocal absent=second.item\n";
    let report = clean(source);
    assert_eq!(report.type_at(source.find("populated=").unwrap()), Some("number"));
    assert_eq!(report.type_at(source.find("absent=").unwrap()), Some("nil"));
    // The two result columns are different spellings: a store through one
    // does not update the other (Bun's syntactic store matching).
    let shared = "local function pair() local value={} return value,value end\nlocal first,second=pair()\nfirst.item=1\nlocal shared=second.item\n";
    assert_eq!(clean(shared).type_at(shared.find("shared=second").unwrap()), Some("nil"));
}
#[test]
fn written_structural_annotations_keep_existing_closed_member_contracts() {
    let source = "---@type {count:number}\nlocal value={count=1}\nvalue.extra='no'\n";
    assert!(
        check(source)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2339")
    );
}

#[test]
fn deferred_plain_table_methods_use_the_original_prospective_structural_schema() {
    let source = "local Graph={}\n---@param remaining number\n---@return number\nfunction Graph.walk(remaining)\n if remaining==0 then return 0 end\n return Graph.walk(remaining-1)\nend\nlocal result=Graph.walk(2)\n";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("result=").unwrap()),
        Some("number")
    );
    let invalid = source.replace("Graph.walk(remaining-1)", "Graph.walk('wrong')");
    assert!(
        check(&invalid)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2345")
    );
    clean(&invalid.replace("Graph.walk('wrong')", "Graph.walk(remaining-1)"));
}

#[test]
fn captured_constructor_fields_do_not_depend_on_later_store_receiver_discovery() {
    // The uninvoked body must also check: this is a declaration-space field
    // contract, independent of actual call-state replay or normal completion.
    // Writable numeric fields need broad contracts rather than v1 singleton
    // inference from their initial 1/0 values.
    for invocation in ["", "install()\n"] {
        let source = format!("local record={{current=--[[---@type number]]1,saved=--[[---@type number]]1,marker=--[[---@type number]]0}}\nlocal function install()\n record.saved=record.current\n record.marker=1\n ---@type number\n local read=record.current\nend\n{invocation}");
        clean(&source);
    }
}

#[test]
fn captured_constructor_reads_keep_the_original_bound_when_a_later_store_is_wrong() {
    let source = "local record={current=--[[---@type number]]1,saved=--[[---@type number]]1,marker=--[[---@type number]]0}\nlocal function install()\n record.saved=record.current\n record.current='wrong'\n record.marker=1\nend\n";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(report.diagnostics[0].span.start, source.find("'wrong'").unwrap());
    assert_eq!(report.diagnostics[0].span.end, source.find("'wrong'").unwrap() + "'wrong'".len());
    assert_eq!(&source[report.diagnostics[0].span.start..report.diagnostics[0].span.end], "'wrong'");
    clean(&source.replace("'wrong'", "2"));
}

#[test]
fn written_fields_keep_their_storage_bound_across_repeated_narrowing_and_deletion() {
    let source = "local value={\n---@type number|string|nil\ncurrent=1,\n}\nvalue.current=1\nvalue.current='word'\nlocal text=value.current\nvalue.current=nil\nlocal gone=value.current\nvalue.current=2\nlocal restored=value.current\n";
    let report = clean(source);
    assert_eq!(report.type_at(source.find("text=value").unwrap()), Some("string"));
    assert_eq!(report.type_at(source.find("gone=value").unwrap()), Some("nil"));
    assert_eq!(report.type_at(source.find("restored=value").unwrap()), Some("number"));
    let wrong = format!("{source}value.current=true\n");
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(&wrong[report.diagnostics[0].span.start..report.diagnostics[0].span.end], "true");
    clean(&wrong.replace("value.current=true", "value.current='corrected'"));
}

#[test]
fn factory_result_columns_do_not_share_stores_between_spellings() {
    let shared = "local function pair() local value={} return value,value end\nlocal first,second=pair()\nfirst.item=1\nlocal shared=second.item\n";
    assert_eq!(
        clean(shared).type_at(shared.find("shared=second").unwrap()),
        Some("nil")
    );
    let distinct = "local function pair() return {},{} end\nlocal first,second=pair()\nfirst.item=1\nlocal absent=second.item\nsecond.item=2\n";
    assert_eq!(
        clean(distinct).type_at(distinct.find("absent=").unwrap()),
        Some("nil")
    );
}

// The store goes to the frozen old receiver, so the replacement lacks the
// field. The copy `old` is another spelling and does not see the store.
#[test]
fn parallel_assignment_does_not_populate_the_replacement_receiver() {
    let source = "local value={}\nlocal old=value\nvalue,value.item={},1\nlocal oldItem=old.item\nvalue.other='new'\nlocal fresh=value.other\n";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("oldItem=").unwrap()),
        Some("nil")
    );
    assert_eq!(
        report.type_at(source.find("fresh=").unwrap()),
        Some("string")
    );
    assert!(
        check(&format!("{source}local wrong=value.item\n"))
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2339")
    );
}

#[test]
fn cli_exports_actual_plain_table_members_with_checked_calls_and_source_correction() {
    use std::{
        fs,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let directory = std::env::temp_dir().join(format!(
        "luanalysis-runtime-export-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("api.lua"),"local Api={}\nApi.count=42\n---@param value number\n---@return number\nfunction Api.twice(value) return value*2 end\nreturn Api\n").unwrap();
    let source = "local Api=require('api')\n---@type number\nlocal count=Api.count\nlocal result=Api.twice('wrong')\n";
    fs::write(directory.join("main.lua"), source).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&directory)
            .args(["check", "--dump-types", "--module-root", ".", "main.lua"])
            .output()
            .unwrap()
    };
    let failed = run();
    let errors = format!(
        "{}{}",
        String::from_utf8_lossy(&failed.stdout),
        String::from_utf8_lossy(&failed.stderr)
    );
    assert_eq!(failed.status.code(), Some(1), "{errors}");
    assert!(
        errors.contains("main.lua:4:") && errors.contains("TS2345"),
        "{errors}"
    );
    fs::write(
        directory.join("main.lua"),
        source.replace("Api.twice('wrong')", "Api.twice(3)"),
    )
    .unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}{}",
        String::from_utf8_lossy(&corrected.stdout),
        String::from_utf8_lossy(&corrected.stderr)
    );
    let output = String::from_utf8_lossy(&corrected.stdout);
    assert!(
        output.contains("count: number") && output.contains("result: number"),
        "{output}"
    );
    fs::remove_dir_all(directory).unwrap();
}
