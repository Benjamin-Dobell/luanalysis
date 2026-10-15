use luanalysis::{CheckReport, check};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

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
fn bodyless_global_headers_are_public_native_overloads_without_implementations() {
    let source = "---@function convert(value:number):string\n---@function convert(value:string):number\nlocal text=convert(1)\nlocal count=convert('x')\n";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("text=").unwrap()),
        Some("string")
    );
    assert_eq!(
        report.type_at(source.find("count=").unwrap()),
        Some("number")
    );
    for call in ["convert(true)", "convert()", "convert(1,2)"] {
        let report = check(&format!("{source}{call}\n"));
        assert!(
            report
                .diagnostics
                .iter()
                .any(|error| matches!(error.code, "TS2345" | "TS2554" | "TS2769")),
            "{:#?}",
            report.diagnostics
        );
        assert!(
            report
                .diagnostics
                .iter()
                .all(|error| error.code != "TS2394"),
            "{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn original_long_form_doc_tags_supply_missing_header_types_and_generics() {
    let source = "---@generic T:string\n---@param value T\n---@return T\n---@function identity(value)\nlocal result=identity('literal')\n";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("result=").unwrap()),
        Some("\"literal\"")
    );
    let wrong = format!("{source}identity(1)\n");
    assert!(
        check(&wrong)
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2345" | "TS2769"))
    );
    let untyped = "---@function untyped(value):number\n";
    let report = check(untyped);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS7006"
                && &untyped[error.span.start..error.span.end] == "value"),
        "{:#?}",
        report.diagnostics
    );
    clean(&untyped.replace("value)", "value:number)"));
}

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis-bodyless-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, source: &str) {
        fs::write(self.0.join(name), source).unwrap();
    }
    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .arg("check")
            .args(["--no-stdlib", "--dump-types"])
            .args(arguments)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn output(result: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    )
}

#[test]
fn cli_merges_original_cross_file_doc_heads_and_reports_original_call_errors() {
    let fixture = Fixture::new();
    fixture.file("first.lua", "---@function convert(value:number):string\n");
    fixture.file("second.lua", "---@function convert<T:string>(value:T):T\n");
    fixture.file(
        "main.lua",
        "local first=convert(1)\nlocal second=convert('literal')\nconvert(true)\n",
    );
    let bad = fixture.run(&["first.lua", "second.lua", "main.lua"]);
    assert_eq!(bad.status.code(), Some(1), "{}", output(&bad));
    assert!(
        output(&bad).contains("main.lua:3:")
            && (output(&bad).contains("TS2769") || output(&bad).contains("TS2345")),
        "{}",
        output(&bad)
    );
    fixture.file(
        "main.lua",
        "local first=convert(1)\nlocal second=convert('literal')\n",
    );
    let corrected = fixture.run(&["first.lua", "second.lua", "main.lua"]);
    assert_eq!(corrected.status.code(), Some(0), "{}", output(&corrected));
    assert!(
        output(&corrected).contains("string") && output(&corrected).contains("literal"),
        "{}",
        output(&corrected)
    );
}

#[test]
fn mixed_doc_runtime_declarations_are_explicitly_unsupported() {
    let source = "---@function mixed(value:number):number\nfunction mixed(value) return value end\nlocal value=mixed(1)\n";
    let report = check(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "LUA2034" && error.message.contains("mixes bodyless")),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn bodyless_parameter_defaults_rest_and_result_packs_use_native_queries() {
    let source = "---@param value? number\n---@vararg string\n---@return (number,string)\n---@function pack(value)\nlocal first,second=pack()\nlocal other,text=pack(1,'x','y')\n";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("first,").unwrap()),
        Some("number")
    );
    assert_eq!(
        report.type_at(source.find("second=").unwrap()),
        Some("string")
    );
    let wrong = check(&format!("{source}pack(1,true)\n"));
    assert!(
        wrong
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2345" | "TS2769")),
        "{:#?}",
        wrong.diagnostics
    );
}

#[test]
fn bodyless_generic_scopes_are_local_to_the_original_header() {
    let source = "---@function identity<T>(value:T):T\n---@function unrelated(value:T):number\n";
    let report = check(source);
    let errors: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|error| error.code == "TS2304")
        .collect();
    assert_eq!(errors.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(&source[errors[0].span.start..errors[0].span.end], "T");
    clean(&source.replace("unrelated(value:T)", "unrelated(value:number)"));
}

#[test]
fn invalid_owner_recovery_never_publishes_a_global_function() {
    let source = "---@function :broken(value:number):number\nlocal value=broken(1)\n";
    let report = check(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "LUA1001"),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2304"
                && &source[error.span.start..error.span.end] == "broken"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn bodyless_contextual_members_capture_class_generics_and_colon_self() {
    let source = "---@class Box<T>\n---@function get():T\n---@function set(value:T):self\n---@function Box:replace(value:T):self\n\n---@param box Box<number>\nlocal function inspect(box)\n local value=box.get()\n local again=box:replace(1)\n return value,again\nend\n";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("value=").unwrap()),
        Some("number")
    );
    let ty = report.type_at(source.find("again=").unwrap()).unwrap();
    assert!(ty.contains("Box") && ty.contains("number"), "{ty}");
    let bad = source.replace("box:replace(1)", "box:replace('wrong')");
    let report = check(&bad);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2345" | "TS2769")),
        "{:#?}",
        report.diagnostics
    );
    clean(&bad.replace("box:replace('wrong')", "box:replace(1)"));
}

#[test]
fn cli_doc_members_extend_original_cross_file_owner_and_keep_generic_scope() {
    let fixture = Fixture::new();
    fixture.file("owner.def.lua", "---@class Box<T>\n");
    fixture.file("members.def.lua","---@function Box:read():T\n---@function Box:choose(value:number):number\n---@function Box:choose(value:string):string\n");
    fixture.file("main.lua","---@param box Box<number>\nlocal function inspect(box)\n local value=box:read()\n local count=box:choose(1)\n local text=box:choose('x')\n return value,count,text\nend\n");
    let good = fixture.run(&[
        "--library",
        "owner.def.lua",
        "--library",
        "members.def.lua",
        "main.lua",
    ]);
    assert_eq!(good.status.code(), Some(0), "{}", output(&good));
    assert!(
        output(&good).contains("number") && output(&good).contains("string"),
        "{}",
        output(&good)
    );
    fixture.file(
        "main.lua",
        "---@param box Box<number>\nlocal function inspect(box) return box:choose(true) end\n",
    );
    let bad = fixture.run(&[
        "--library",
        "owner.def.lua",
        "--library",
        "members.def.lua",
        "main.lua",
    ]);
    assert_eq!(bad.status.code(), Some(1), "{}", output(&bad));
    assert!(
        output(&bad).contains("main.lua:2:")
            && (output(&bad).contains("TS2345") || output(&bad).contains("TS2769")),
        "{}",
        output(&bad)
    );
}

#[test]
fn cli_repeated_doc_members_merge_across_original_snapshots_and_inherited_mappers() {
    let fixture = Fixture::new();
    fixture.file(
        "owner.def.lua",
        "---@class Box<T>\n\n---@class Derived<T>:Box<T>\n",
    );
    fixture.file("first.def.lua", "---@function Box:choose(value:T):T\n");
    fixture.file(
        "second.def.lua",
        "---@function Box:choose(value:string):string\n",
    );
    fixture.file("main.lua","---@param box Derived<number>\nlocal function inspect(box)\n local count=box:choose(1)\n local text=box:choose('x')\n return count,text\nend\n");
    let result = fixture.run(&[
        "--library",
        "owner.def.lua",
        "--library",
        "first.def.lua",
        "--library",
        "second.def.lua",
        "main.lua",
    ]);
    assert_eq!(result.status.code(), Some(0), "{}", output(&result));
    assert!(
        output(&result).contains("count: number") && output(&result).contains("text: string"),
        "{}",
        output(&result)
    );
}

#[test]
fn unused_unknown_doc_member_owner_is_reported_at_original_owner() {
    let source = "---@function MissingOwner:read(value:number):number\n";
    let report = check(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| &source[error.span.start..error.span.end] == "MissingOwner"),
        "{:#?}",
        report.diagnostics
    );
    let corrected = source.replace("MissingOwner:read", "globalRead");
    // An explicit real global head has no owner dependency.
    clean(&corrected);
}

#[test]
fn alias_colon_owner_never_uses_error_type_as_a_valid_receiver_contract() {
    let source = "---@class Box\n\n---@alias Alias Box\n\n---@function Alias:read():number\n\n---@param box Box\nlocal function inspect(box) return box.read('wrong receiver') end\n";
    let report = check(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2345"
                && &source[error.span.start..error.span.end] == "'wrong receiver'"),
        "{:#?}",
        report.diagnostics
    );
    let direct = source.replace("Alias:read", "Box:read");
    assert!(
        check(&direct)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2345"),
        "{:#?}",
        check(&direct).diagnostics
    );
    clean(&direct.replace("box.read('wrong receiver')", "box:read()"));
}

#[test]
fn specialized_alias_colon_heads_keep_real_formals_self_and_derived_receivers() {
    let source = "---@class Box<T>\n---@field value T\n\n---@alias Alias Box<string>\n\n---@function Alias:replace(value:T):self\n\n---@class Derived:Box<string>\n---@field extra number\n\n---@param box Derived\nlocal function use(box)\n local result=box:replace('text')\n ---@type Derived\n local kept=result\n ---@type string\n local value=result.value\n return result.extra\nend\n";
    clean(source);
    let invalid = source.replace("box:replace('text')", "box:replace(42)");
    assert!(check(&invalid).diagnostics.iter().any(|error| error.code == "TS2345"), "{:#?}", check(&invalid).diagnostics);
    let unrelated = format!("{source}\n---@param other Box<number>\nlocal function wrong(other) return other:replace(42) end\n");
    assert!(check(&unrelated).diagnostics.iter().any(|error| error.code == "TS2339"), "{:#?}", check(&unrelated).diagnostics);
}

#[test]
fn all_alias_member_scopes_are_prepared_before_a_class_captures_outer_parameters() {
    let fixture = Fixture::new();
    fixture.file("member.def.lua", "---@function Alias:read():T\n---@function Alias:again():{value:T}\n");
    fixture.file("main.lua", "---@generic T\n---@param value T\nlocal function make(value)\n ---@class Captured\n local Captured={}\n ---@alias Alias Captured\n ---@type Captured\n local instance\n local first=instance:read()\n local second=instance:again()\n return first,second.value\nend\nlocal count,other=make(1)\nlocal text,more=make('x')\n");
    let result = fixture.run(&["--library", "member.def.lua", "main.lua"]);
    assert_eq!(result.status.code(), Some(0), "{}", output(&result));
    for expected in ["count: number", "other: number", "text: string", "more: string"] {
        assert!(output(&result).contains(expected), "{}", output(&result));
    }
}

#[test]
fn alias_owner_cycles_fail_without_publishing_a_receiver_signature() {
    let source = "---@alias First Second\n---@alias Second First\n---@function First:read():self\n";
    let report = check(source);
    assert!(report.diagnostics.iter().any(|error| matches!(error.code, "TS2456" | "TS2589")), "{:#?}", report.diagnostics);
    assert!(!report.diagnostics.iter().any(|error| error.code == "TS7006"), "{:#?}", report.diagnostics);
}

#[test]
fn alias_constraints_can_demand_a_sibling_original_signature_after_scope_preparation() {
    clean("---@shape HasRead\n---@field read fun(value:number):number\n\n---@shape Good\n\n---@class Box<T:HasRead>\n\n---@alias Alias Box<Good>\n\n---@function Alias:copy():self\n---@function Good.read(value:number):number\n\n---@param value Alias\nlocal function use(value) return value:copy() end\n");
}

#[test]
fn cli_specialized_alias_colon_receiver_rejects_original_wrong_value_and_accepts_correction() {
    let fixture = Fixture::new();
    fixture.file("owner.def.lua", "---@class Box<T>\n---@field value T\n\n---@class Derived:Box<string>\n---@field extra number\n");
    fixture.file("alias.def.lua", "---@alias Alias Box<string>\n");
    fixture.file("members.def.lua", "---@function Alias:replace(value:T):self\n");
    fixture.file("main.lua", "-- café\r\n---@param box Derived\r\nlocal function use(box)\r\n return box.replace('wrong receiver','text')\r\nend\r\n");
    let args = ["--library", "owner.def.lua", "--library", "alias.def.lua", "--library", "members.def.lua", "main.lua"];
    let invalid = fixture.run(&args);
    assert_eq!(invalid.status.code(), Some(1), "{}", output(&invalid));
    assert!(output(&invalid).contains("main.lua:4:21: error[TS2345]"), "{}", output(&invalid));
    fixture.file("main.lua", "---@param box Derived\nlocal function use(box)\n local result=box:replace('text')\n ---@type Derived\n local kept=result\n ---@type string\n local value=result.value\n ---@type number\n local extra=result.extra\n return extra\nend\n");
    let corrected = fixture.run(&args);
    assert_eq!(corrected.status.code(), Some(0), "{}", output(&corrected));
}

#[test]
fn long_form_bodyless_literal_heads_keep_native_overload_priority() {
    let source = "---@param value string\n---@return number\n---@function choose(value)\n\n---@param value \"literal\"\n---@return string\n---@function choose(value)\nlocal special=choose('literal')\nlocal ordinary=choose('other')\n";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("special=").unwrap()),
        Some("string")
    );
    assert_eq!(
        report.type_at(source.find("ordinary=").unwrap()),
        Some("number")
    );
}

#[test]
fn foreign_doc_members_capture_original_enclosing_generic_scopes() {
    let fixture = Fixture::new();
    fixture.file("member.def.lua", "---@function Captured:read():{value:T}\n");
    fixture.file("main.lua","---@generic T\n---@param value T\nlocal function make(value)\n ---@class Captured\n local Captured={}\n ---@type Captured\n local instance\n local result=instance:read()\n return result.value\nend\nlocal count=make(1)\nlocal text=make('x')\n");
    let result = fixture.run(&["--library", "member.def.lua", "main.lua"]);
    assert_eq!(result.status.code(), Some(0), "{}", output(&result));
    assert!(
        output(&result).contains("count: number") && output(&result).contains("text: string"),
        "{}",
        output(&result)
    );
}
