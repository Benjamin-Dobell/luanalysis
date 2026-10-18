use luanalysis::check;
use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

const REQUIRED: &str = "---@overload fun(fn:fun(resume:fun():void):void):false|(true,nil)\n---@generic R\n---@param fn fun(resume:fun(result:R):void):void\n---@return false|(true,R)\nlocal function yield_once(fn) return false end\nlocal ok,value=yield_once(function(resume) resume() end)\n";

fn corrected() -> String {
    REQUIRED.replace("fun(result:R)", "fun(result?:R)")
}

const PROTOCOL: &str = r#"---@overload fun<R>(fn:fun(resume:fun(value:R):void):void):false|(true,R)
---@overload fun(fn:fun(resume:fun():void):void):false|(true,nil)
---@generic R
---@param fn fun(resume:fun(value?:R):void):void
---@return false|(true,nil|R)
local function run(fn)
---@type nil|R
local result
---@type fun(value?:R):void
local resume=function(value) result=value end
fn(resume)
return true,result
end
local ok,value=run(
---@param resume fun(value:number):void
function(resume) resume(42) end)
if ok then
---@type number
local checked=value
end
local ok_empty,empty=run(
---@param resume fun():void
function(resume) resume() end)
if ok_empty then
---@type nil
local checked=empty
end
"#;

#[test]
fn private_nil_capable_storage_preserves_distinct_typed_and_result_free_public_rows() {
    let report = check(PROTOCOL);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn private_nil_admission_does_not_allow_typed_public_resume_omission_or_wrong_payloads() {
    for call in ["resume()", "resume('wrong')"] {
        let source = PROTOCOL.replace("resume(42)", call);
        let report = check(&source);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|error| matches!(error.code, "TS2554" | "TS2345")),
            "{call}\n{:#?}",
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
fn a_required_resume_argument_cannot_implement_a_result_free_public_callback() {
    let report = check(REQUIRED);
    let errors: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|error| error.code == "TS2394")
        .collect();
    assert_eq!(errors.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(errors[0].span.start, 3, "{:#?}", errors[0]);
}

#[test]
fn an_optional_private_resume_argument_accepts_the_result_free_public_callback() {
    let source = corrected();
    let report = check(&source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
}

#[test]
fn result_free_public_callbacks_do_not_gain_the_private_resume_parameter() {
    let source = corrected().replace("resume() end)", "resume('wrong') end)");
    let report = check(&source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2554"),
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

#[test]
fn iterator_style_resume_callbacks_keep_independent_outer_generics() {
    let source = "---@overload fun<T>(arr:T[],fn:fun(resume:fun():void,value:T,index:number):void):false|(true,nil[])\n---@generic T,R\n---@param arr T[]\n---@param fn fun(resume:fun(result:R):void,value:T,index:number):void\n---@return false|(true,R[])\nlocal function yield_all(arr,fn) return false end\n";
    assert!(
        check(source)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2394")
    );
    let fixed = source.replace("fun(result:R)", "fun(result?:R)");
    let report = check(&fixed);
    assert!(
        report.diagnostics.is_empty(),
        "{fixed}\n{:#?}",
        report.diagnostics
    );
}

#[test]
fn cli_reports_the_original_void_callback_overload_then_accepts_its_type_correction() {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let directory = std::env::temp_dir().join(format!(
        "luanalysis void resume {} {}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();
    let source = directory.join("resume.lua");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .current_dir(&directory)
            .args(["check", "--no-stdlib", "resume.lua"])
            .output()
            .unwrap()
    };
    fs::write(&source, REQUIRED).unwrap();
    let invalid = run();
    assert_eq!(invalid.status.code(), Some(1), "{invalid:?}");
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains("resume.lua:1:4: error[TS2394]"),
        "{invalid:?}"
    );
    fs::write(&source, corrected()).unwrap();
    let valid = run();
    assert_eq!(valid.status.code(), Some(0), "{valid:?}");
    fs::remove_dir_all(&directory).unwrap();
}
