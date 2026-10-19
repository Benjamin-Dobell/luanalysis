//! Result-count hints describe proven Lua adjustment, without replacing type errors.
use luanalysis::project_check::{
    ProjectCheckOptions, ProjectCheckReport, check_project_with_options, static_require_requests,
};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis::{Diagnostic, DiagnosticCategory};
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const PAIR: &str = "---@return number,string\nlocal function pair() return 1,'two' end\n";

fn check(source: &str, inspections: bool) -> (String, ProjectCheckReport) {
    let root = std::env::temp_dir().join(format!(
        "luanalysis result counts {} {}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.lua");
    fs::write(&main, source).unwrap();
    let resolver = Resolver::new(ProjectConfig::new([root.clone()])).unwrap();
    let project = Project::load_plan(
        &resolver,
        ProjectLoadPlan::new([main]),
        static_require_requests,
    );
    let report = check_project_with_options(
        project,
        ProjectCheckOptions {
            environment: StandardEnvironmentProfile::Lua54,
            inspections,
            ..Default::default()
        },
    );
    fs::remove_dir_all(root).unwrap();
    (source.to_owned(), report)
}

fn diagnostics(checked: &(String, ProjectCheckReport)) -> &[Diagnostic] {
    &checked
        .1
        .file(checked.1.project.entries[0])
        .unwrap()
        .report
        .diagnostics
}

fn hints(checked: &(String, ProjectCheckReport)) -> Vec<&Diagnostic> {
    let mut result: Vec<_> = diagnostics(checked)
        .iter()
        .filter(|diagnostic| matches!(diagnostic.code, "LUA4002" | "LUA4003"))
        .collect();
    for diagnostic in &result {
        assert_eq!(diagnostic.category(), DiagnosticCategory::Suggestion);
        assert!(!diagnostic.is_unnecessary(), "{diagnostic:#?}");
    }
    result.sort_by_key(|diagnostic| (diagnostic.span.start, diagnostic.span.end, diagnostic.code));
    result
}

fn found(checked: &(String, ProjectCheckReport)) -> Vec<String> {
    hints(checked)
        .iter()
        .map(|diagnostic| {
            format!(
                "{} {}",
                diagnostic.code,
                &checked.0[diagnostic.span.start..diagnostic.span.end]
            )
        })
        .collect()
}

fn clean(checked: &(String, ProjectCheckReport)) {
    assert!(
        !checked.1.has_errors(),
        "{}\n{:#?}",
        checked.0,
        diagnostics(checked)
    );
}

#[test]
fn direct_discard_and_padding_examples_are_opt_in_nonfatal_hints() {
    let source = format!(
        "{PAIR}local first=pair()\nlocal scalar,padded=7\nlocal one,two,third=pair()\nreturn first,scalar,padded,one,two,third\n"
    );
    let inspected = check(&source, true);
    clean(&inspected);
    assert_eq!(
        found(&inspected),
        ["LUA4002 pair()", "LUA4003 padded", "LUA4003 third"]
    );
    let ordinary = check(&source, false);
    clean(&ordinary);
    assert!(found(&ordinary).is_empty());
}

#[test]
fn assignment_padding_and_excess_literal_rhs_keep_original_utf8_byte_spans() {
    let source = "-- 🦀\r\n---@function one():number\r\nlocal first,second,third=0,nil,nil\r\nfirst,second,third=one()\r\nlocal kept=1,2,3\r\nreturn first,second,third,kept\r\n";
    let checked = check(source, true);
    clean(&checked);
    assert_eq!(
        found(&checked),
        ["LUA4003 second", "LUA4003 third", "LUA4002 2"]
    );
    let assignment = source.find("first,second,third=one()").unwrap();
    let literal = source.find("local kept=1,2,3").unwrap() + "local kept=1,".len();
    let spans: Vec<_> = hints(&checked)
        .iter()
        .map(|diagnostic| diagnostic.span)
        .collect();
    let expected = [
        (assignment + "first,".len(), "second".len()),
        (assignment + "first,second,".len(), "third".len()),
        (literal, 1),
    ]
    .map(|(start, length)| luanalysis_syntax::Span {
        start,
        end: start + length,
    });
    assert_eq!(spans, expected);
    assert!(hints(&checked).last().unwrap().message.contains("2 results"));
}

#[test]
fn scalar_and_table_values_are_one_result_nil_is_a_value_and_void_is_absence() {
    let source = "---@return nil\nlocal function nilValue() return nil end\n---@return void\nlocal function noValues() end\n---@function unknownValue():unknown\nlocal pending\nlocal scalar,scalar_padding=42\nlocal whole={1,2,3}\nlocal table_value,table_padding={1,2,3}\nlocal explicit_nil=nilValue()\nlocal nil_value,nil_padding=nilValue()\nlocal void_padding=noValues()\nlocal opaque,opaque_padding=unknownValue()\nreturn pending,scalar,scalar_padding,whole,table_value,table_padding,explicit_nil,nil_value,nil_padding,void_padding,opaque,opaque_padding\n";
    let checked = check(source, true);
    clean(&checked);
    assert_eq!(
        found(&checked),
        [
            "LUA4003 scalar_padding",
            "LUA4003 table_padding",
            "LUA4003 nil_padding",
            "LUA4003 void_padding",
            "LUA4003 opaque_padding",
        ]
    );
}

#[test]
fn explicit_scalarization_table_expansion_forwarding_and_call_statements_are_quiet() {
    let source = format!(
        "{PAIR}local selected=(pair())\nlocal first,second=pair(),9\nlocal complete,tag,last=pair(),pair()\nlocal boxed={{pair()}}\nlocal function forward() return pair() end\npair()\nreturn selected,first,second,complete,tag,last,boxed,forward\n"
    );
    let checked = check(&source, true);
    clean(&checked);
    assert!(found(&checked).is_empty(), "{:?}", found(&checked));
}

#[test]
fn required_prefixes_prove_discard_but_optional_union_and_open_tail_lengths_do_not() {
    let source = "---@function prefix():number,string,boolean...\n---@function alternative():(number,string)|(number,string,boolean)\n---@function tail():number,string...\n---@function choice():number|(number,string)\nlocal first=prefix()\nlocal second=alternative()\nlocal tail_first,tail_second=tail()\nlocal prefix_first,prefix_second=prefix()\nlocal union_first,union_second=alternative()\nlocal choice_first=choice()\nlocal choice_one,choice_two=choice()\nreturn first,second,tail_first,tail_second,prefix_first,prefix_second,union_first,union_second,choice_first,choice_one,choice_two\n";
    let checked = check(source, true);
    clean(&checked);
    assert_eq!(
        found(&checked),
        ["LUA4002 prefix()", "LUA4002 alternative()"]
    );
}

#[test]
fn finite_union_and_optional_rows_only_hint_destinations_missing_from_every_row() {
    let source = "---@function choice():number|(number,string)\n\n---@param flag boolean\n---@return number|void\nlocal function maybe(flag) if flag then return 1 end end\nlocal first,uncertain,absent=choice()\nlocal possible=maybe(true)\nlocal optional_first,optional_absent=maybe(false)\nreturn first,uncertain,absent,possible,optional_first,optional_absent\n";
    let checked = check(source, true);
    clean(&checked);
    assert_eq!(
        found(&checked),
        ["LUA4003 absent", "LUA4003 optional_absent"]
    );
}

#[test]
fn explicit_underscore_destinations_consume_normal_physical_slots() {
    let source = format!(
        "{PAIR}local kept,_=pair()\nlocal scalar,_=7\nlocal first,tag,_=pair()\nreturn kept,scalar,first,tag\n"
    );
    let checked = check(&source, true);
    clean(&checked);
    assert_eq!(found(&checked), ["LUA4003 _", "LUA4003 _"]);
    let spans: Vec<_> = hints(&checked)
        .iter()
        .map(|diagnostic| diagnostic.span.start)
        .collect();
    assert_eq!(
        spans,
        [
            source.find("local scalar,_").unwrap() + "local scalar,".len(),
            source.find("local first,tag,_").unwrap() + "local first,tag,".len(),
        ]
    );
}

#[test]
fn whole_dynamic_and_symbolic_packs_and_never_prefixes_remain_quiet() {
    let dynamic = "---@param dynamic any\nlocal function consume(dynamic)\nlocal first,second=dynamic()\nreturn first,second\nend\n---@generic T\n---@param producer fun():T\nlocal function symbolic(producer)\nlocal first,second=producer()\nreturn first,second\nend\nreturn consume,symbolic\n";
    let checked = check(dynamic, true);
    clean(&checked);
    assert!(found(&checked).is_empty(), "{:?}", found(&checked));
    let reachable = check("---@function choice():never|(number,string)\nlocal first=choice()\nreturn first\n", true);
    clean(&reachable);
    assert_eq!(found(&reachable), ["LUA4002 choice()"]);
    for prefix in ["stop()", "(stop())"] {
        let source = format!(
            "---@return never\nlocal function stop() error('stop') end\nlocal unreachable={prefix},1\n"
        );
        let checked = check(&source, true);
        clean(&checked);
        assert!(found(&checked).is_empty(), "{:?}", found(&checked));
    }
}

#[test]
fn count_hints_do_not_replace_assignment_or_call_errors_or_explain_failed_callees() {
    let source = format!(
        "{PAIR}---@param value number\n---@return number\nlocal function required(value) return value end\n---@type string\nlocal wrong=pair()\nlocal kept=required(),99\nreturn wrong,kept\n"
    );
    let checked = check(&source, true);
    assert!(checked.1.has_errors());
    assert_eq!(found(&checked), ["LUA4002 pair()"]);
    for code in ["TS2322", "TS2554"] {
        assert_eq!(
            diagnostics(&checked)
                .iter()
                .filter(|diagnostic| diagnostic.code == code)
                .count(),
            1,
            "{:#?}",
            diagnostics(&checked)
        );
    }
    let ordinary = check(&source, false);
    assert!(ordinary.1.has_errors());
    assert!(found(&ordinary).is_empty());
    for code in ["TS2322", "TS2554"] {
        assert_eq!(
            diagnostics(&ordinary)
                .iter()
                .filter(|diagnostic| diagnostic.code == code)
                .count(),
            1,
            "{:#?}",
            diagnostics(&ordinary)
        );
    }
    for source in [
        "local first,second=missing()\nreturn first,second\n",
        "---@param unknown unknown\nlocal function use(unknown) local first,second=unknown() return first,second end\nreturn use\n",
    ] {
        let failed = check(source, true);
        assert!(failed.1.has_errors());
        assert!(found(&failed).is_empty(), "{:#?}", diagnostics(&failed));
    }
}

#[test]
fn instantiated_generics_and_colon_methods_keep_their_actual_result_pack() {
    let source = "---@generic T\n---@param value T\n---@return T,string\nlocal function label(value) return value,'tag' end\nlocal projected=label(1)\n---@class Labels\nlocal Labels={}\n---@generic T\n---@param value T\n---@return T,string\nfunction Labels:label(value) return value,'tag' end\nlocal method_first=Labels:label(true)\nlocal complete,tag,method_padding=Labels:label(2)\nreturn projected,method_first,complete,tag,method_padding\n";
    let checked = check(source, true);
    clean(&checked);
    assert_eq!(
        found(&checked),
        [
            "LUA4002 label(1)",
            "LUA4002 Labels:label(true)",
            "LUA4003 method_padding"
        ]
    );
}
