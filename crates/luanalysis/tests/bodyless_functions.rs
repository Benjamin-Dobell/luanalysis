use luanalysis::{check, CheckReport};
use std::{fs, path::PathBuf, process::{Command, Output}, sync::atomic::{AtomicU64,Ordering}};

fn clean(source:&str)->CheckReport {
    let report=check(source);
    assert!(report.diagnostics.is_empty(),"{source}\n{:#?}",report.diagnostics);
    report
}

#[test]
fn bodyless_global_headers_are_public_native_overloads_without_implementations() {
    let source="---@function convert(value:number):string\n---@function convert(value:string):number\nlocal text=convert(1)\nlocal count=convert('x')\n";
    let report=clean(source);
    assert_eq!(report.type_at(source.find("text=").unwrap()),Some("string"));
    assert_eq!(report.type_at(source.find("count=").unwrap()),Some("number"));
    for call in ["convert(true)","convert()","convert(1,2)"] {
        let report=check(&format!("{source}{call}\n"));
        assert!(report.diagnostics.iter().any(|error|matches!(error.code,"TS2345"|"TS2554"|"TS2769")),"{:#?}",report.diagnostics);
        assert!(report.diagnostics.iter().all(|error|error.code!="TS2394"),"{:#?}",report.diagnostics);
    }
}

#[test]
fn original_long_form_doc_tags_supply_missing_header_types_and_generics() {
    let source="---@generic T:string\n---@param value T\n---@return T\n---@function identity(value)\nlocal result=identity('literal')\n";
    let report=clean(source);
    assert_eq!(report.type_at(source.find("result=").unwrap()),Some("\"literal\""));
    let wrong=format!("{source}identity(1)\n");
    assert!(check(&wrong).diagnostics.iter().any(|error|matches!(error.code,"TS2345"|"TS2769")));
    let untyped="---@function untyped(value):number\n";
    let report=check(untyped);
    assert!(report.diagnostics.iter().any(|error|error.code=="TS7006" && &untyped[error.span.start..error.span.end]=="value"),"{:#?}",report.diagnostics);
    clean(&untyped.replace("value)","value:number)"));
}

static NEXT:AtomicU64=AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new()->Self { let path=std::env::temp_dir().join(format!("luanalysis-bodyless-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));fs::create_dir_all(&path).unwrap();Self(path) }
    fn file(&self,name:&str,source:&str){fs::write(self.0.join(name),source).unwrap();}
    fn run(&self,arguments:&[&str])->Output {Command::new(env!("CARGO_BIN_EXE_luanalysis")).current_dir(&self.0).arg("check").args(["--no-stdlib","--dump-types"]).args(arguments).output().unwrap()}
}
impl Drop for Fixture {fn drop(&mut self){fs::remove_dir_all(&self.0).unwrap();}}
fn output(result:&Output)->String {format!("{}{}",String::from_utf8_lossy(&result.stdout),String::from_utf8_lossy(&result.stderr))}

#[test]
fn cli_merges_original_cross_file_doc_heads_and_reports_original_call_errors() {
    let fixture=Fixture::new();
    fixture.file("first.lua","---@function convert(value:number):string\n");
    fixture.file("second.lua","---@function convert<T:string>(value:T):T\n");
    fixture.file("main.lua","local first=convert(1)\nlocal second=convert('literal')\nconvert(true)\n");
    let bad=fixture.run(&["first.lua","second.lua","main.lua"]);
    assert_eq!(bad.status.code(),Some(1),"{}",output(&bad));
    assert!(output(&bad).contains("main.lua:3:") && (output(&bad).contains("TS2769") || output(&bad).contains("TS2345")),"{}",output(&bad));
    fixture.file("main.lua","local first=convert(1)\nlocal second=convert('literal')\n");
    let corrected=fixture.run(&["first.lua","second.lua","main.lua"]);
    assert_eq!(corrected.status.code(),Some(0),"{}",output(&corrected));
    assert!(output(&corrected).contains("string") && output(&corrected).contains("literal"),"{}",output(&corrected));
}

#[test]
fn mixed_doc_runtime_declarations_are_explicitly_unsupported() {
    let source="---@function mixed(value:number):number\nfunction mixed(value) return value end\nlocal value=mixed(1)\n";
    let report=check(source);
    assert!(report.diagnostics.iter().any(|error|error.code=="LUA2034"&&error.message.contains("mixes bodyless")),"{:#?}",report.diagnostics);
}
