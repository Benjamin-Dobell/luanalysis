use luanalysis::{CheckReport, check};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

const SOURCE: &str = r#"---@class Root
local Root={}
Root.Runtime={}
---@param value number
---@return number
function Root.Runtime.set(value)
    ---@type number
    local checked=value
    return value
end
---@type number
local result=Root.Runtime.set(42)
"#;

fn clean(source: &str) {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}
fn error_at(source: &str, code: &str, statement: &str, expression: &str) -> CheckReport {
    let start = source.find(statement).unwrap() + statement.rfind(expression).unwrap();
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert_eq!(error.code, code, "{error:?}");
    assert_eq!(error.span.start, start, "{error:?}");
    assert_eq!(error.span.end, start + expression.len(), "{error:?}");
    report
}

#[test]
fn a_class_namespace_member_allocation_keeps_named_methods_and_checks_their_bodies() {
    clean(SOURCE);
    let constructor = SOURCE.replace("local Root={}\nRoot.Runtime={}", "local Root={Runtime={}}");
    clean(&constructor);

    let wrong_call = SOURCE.replace("set(42)", "set('wrong')");
    error_at(
        &wrong_call,
        "TS2345",
        "local result=Root.Runtime.set('wrong')",
        "'wrong'",
    );
    clean(&wrong_call.replace("set('wrong')", "set(42)"));

    let wrong_body = SOURCE.replace(
        "---@type number\n    local checked",
        "---@type string\n    local checked",
    );
    error_at(&wrong_body, "TS2322", "local checked=value", "value");
    clean(&wrong_body.replace(
        "---@type string\n    local checked",
        "---@type number\n    local checked",
    ));

    let missing = format!("{SOURCE}local absent=Root.Runtime.missing\n");
    error_at(
        &missing,
        "TS2339",
        "local absent=Root.Runtime.missing",
        "missing",
    );
}

#[test]
fn a_replaced_nested_namespace_retains_its_own_allocation_and_not_the_old_alias_methods() {
    let source = SOURCE.replace(
        "Root.Runtime={}\n",
        "Root.Runtime={}\nlocal original=Root.Runtime\nRoot.Runtime={}\n",
    );
    clean(&source);
    let missing = format!("{source}local absent=original.set\n");
    error_at(&missing, "TS2339", "local absent=original.set", "set");
    clean(&missing.replace(
        "local absent=original.set",
        "local present=Root.Runtime.set",
    ));
}

#[test]
fn written_nested_namespace_contracts_still_reject_undeclared_methods() {
    let source = r#"---@shape RuntimeContract
---@field set fun(value:number):number

---@class Root
---@field Runtime RuntimeContract
local Root={Runtime={set=function(value) return value end}}
---@param value number
---@return number
function Root.Runtime.set(value) return value end
Root.Runtime.set(42)
"#;
    clean(source);
    let wrong = format!("{source}\nfunction Root.Runtime.extra() end\n");
    error_at(
        &wrong,
        "TS2339",
        "function Root.Runtime.extra() end",
        "extra",
    );
    clean(&wrong.replace("\nfunction Root.Runtime.extra() end\n", ""));
}

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis nested namespace {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn run(&self, source: &str, exit: i32) -> String {
        let path = self.0.join("main.lua");
        fs::write(&path, source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--lua-version", "5.2", "--format", "json"])
            .arg(path)
            .output()
            .unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(exit), "{stdout}\n{stderr}");
        assert!(stderr.is_empty(), "{stderr}");
        stdout
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn cli_preserves_nested_method_call_ranges_and_accepts_the_original_correction() {
    let fixture = Fixture::new();
    assert!(fixture.run(SOURCE, 0).contains("\"diagnostics\":[]"));
    let wrong = SOURCE.replace("set(42)", "set('wrong')");
    let start = wrong.find("'wrong'").unwrap();
    let range = format!(
        "\"range\":{{\"start\":{start},\"end\":{}}}",
        start + "'wrong'".len()
    );
    let rejected = fixture.run(&wrong, 1);
    assert!(
        rejected
            .split("},{\"path\":")
            .any(|diagnostic| diagnostic.contains("\"code\":\"TS2345\"")
                && diagnostic.contains(&range)),
        "{rejected}"
    );
    assert!(
        fixture
            .run(&wrong.replace("set('wrong')", "set(42)"), 0)
            .contains("\"diagnostics\":[]")
    );
}
