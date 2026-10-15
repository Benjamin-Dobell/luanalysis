use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const SOURCE: &str = r#"---@param src string | fun(): nil|string
local function make(src)
    if type(src)=='string' then
        return function() return src end
    else
        return function() return src() end
    end
end
"#;
const ORIGINAL_READER: &str = include_str!("fixtures/sax/reader-creation-flow.lua.fixture");

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis captured creation narrowing {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn check(&self, source: &str, version: &str, exit: i32) -> String {
        let path = self.0.join("main.lua");
        fs::write(&path, source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--format", "json", "--lua-version", version])
            .arg(path)
            .output()
            .unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(exit), "{text}\n{stderr}");
        assert!(stderr.is_empty(), "{stderr}");
        assert!(
            !text.contains("LUA2041") && !text.contains("TS2589"),
            "{text}"
        );
        if exit == 0 {
            assert!(text.contains("\"diagnostics\":[]"), "{text}");
        }
        text
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn anchored(text: &str, source: &str, occurrence: &str, expression: &str, code: &str) {
    let start = source.rfind(occurrence).unwrap() + occurrence.find(expression).unwrap();
    let range = format!(
        "\"range\":{{\"start\":{start},\"end\":{}}}",
        start + expression.len()
    );
    assert!(
        text.split("},{\"path\":").any(|diagnostic| {
            diagnostic.contains(&format!("\"code\":\"{code}\"")) && diagnostic.contains(&range)
        }),
        "{text}"
    );
}

#[test]
fn immutable_parameters_keep_branch_narrowing_in_direct_and_saved_type_closures() {
    let fixture = Fixture::new();
    let saved = format!(
        "local kind=type\nlocal _ENV=nil\n{}",
        SOURCE.replace("type(src)", "kind(src)")
    );
    for version in ["5.2", "5.4"] {
        fixture.check(SOURCE, version, 0);
        fixture.check(&saved, version, 0);
    }
}

#[test]
fn annotated_local_captures_and_same_scope_reads_keep_their_native_flow_facts() {
    let fixture = Fixture::new();
    let local = SOURCE.replace("---@param src", "---@param input").replace(
        "local function make(src)",
        "local function make(input)\n---@type string | fun(): nil|string\nlocal src=input",
    );
    let saved = format!(
        "local kind=type\n{}",
        local.replace("type(src)", "kind(src)")
    );
    let same_scope = SOURCE
        .replace("return function() return src end", "return src")
        .replace("return function() return src() end", "return src()");
    for version in ["5.2", "5.4"] {
        fixture.check(&local, version, 0);
        fixture.check(&saved, version, 0);
        fixture.check(&same_scope, version, 0);
    }
}

#[test]
fn a_closure_after_the_last_assignment_uses_its_creation_flow() {
    let fixture = Fixture::new();
    let source = "---@param src string | fun(): nil|string\nlocal function make(src)\nif type(src)=='string' then\nsrc=function() return 'converted' end\nend\nreturn function() return src() end\nend\n";
    for version in ["5.2", "5.4"] {
        fixture.check(source, version, 0);
    }
}

#[test]
fn narrowed_captured_calls_still_check_original_arguments_and_accept_correction() {
    let fixture = Fixture::new();
    let source = SOURCE
        .replace("fun(): nil|string", "fun(value:number): nil|string")
        .replace("return src()", "return src(1)");
    let wrong = source.replace("return src(1)", "return src('wrong')");
    for version in ["5.2", "5.4"] {
        fixture.check(&source, version, 0);
        let rejected = fixture.check(&wrong, version, 1);
        anchored(&rejected, &wrong, "src('wrong')", "'wrong'", "TS2345");
        fixture.check(&source, version, 0);

        let wrong_result = source.replace(
            "return function() return src(1) end",
            "return function()\n---@type number\nlocal checked=src(1)\nreturn checked end",
        );
        let rejected = fixture.check(&wrong_result, version, 1);
        anchored(
            &rejected,
            &wrong_result,
            "local checked=src(1)",
            "src(1)",
            "TS2322",
        );
        fixture.check(
            &wrong_result.replace("---@type number", "---@type nil|string"),
            version,
            0,
        );
    }
}

#[test]
fn writes_inside_or_after_creation_prevent_stale_captured_branch_facts() {
    let fixture = Fixture::new();
    for wrong in [
        SOURCE.replace(
            "return function() return src() end",
            "return function() src='changed'; return src() end",
        ),
        SOURCE.replace(
            "return function() return src() end",
            "local next=function() return src() end; src='changed'; return next",
        ),
    ] {
        for version in ["5.2", "5.4"] {
            let rejected = fixture.check(&wrong, version, 1);
            anchored(&rejected, &wrong, "return src()", "src", "TS2349");
            fixture.check(SOURCE, version, 0);
        }
    }
}

#[test]
fn replaced_direct_and_saved_type_functions_cannot_invent_captured_predicates() {
    let fixture = Fixture::new();
    let fake = "---@param value unknown\n---@return string\nlocal function fake(value) return 'string' end\n";
    let replaced = format!("{fake}type=fake\n{SOURCE}");
    let saved_replaced = format!(
        "{fake}local kind=type\nkind=fake\n{}",
        SOURCE.replace("type(src)", "kind(src)")
    );
    for wrong in [replaced, saved_replaced] {
        for version in ["5.2", "5.4"] {
            let rejected = fixture.check(&wrong, version, 1);
            anchored(&rejected, &wrong, "return src()", "src", "TS2349");
            fixture.check(SOURCE, version, 0);
        }
    }
}

#[test]
fn preexisting_closures_keep_their_environment_while_later_ones_capture_nil() {
    let fixture = Fixture::new();
    let source = "local savedError=error\n---@param message string\nlocal function before(message) error(message) end\nlocal _ENV=nil\n---@param message string\nlocal function after(message) error(message) end\nlocal function callBefore() before('expected') end\nlocal function callAfter() after('expected') end\n";
    let corrected = source.replace(
        "function after(message) error(message)",
        "function after(message) savedError(message)",
    );
    for version in ["5.2", "5.4"] {
        let rejected = fixture.check(source, version, 1);
        anchored(
            &rejected,
            source,
            "after(message) error(message)",
            "error",
            "TS2339",
        );
        assert!(
            rejected.contains("Property 'error' does not exist on type 'nil'."),
            "{rejected}"
        );
        fixture.check(&corrected, version, 0);
    }
}

#[test]
fn original_sax_reader_keeps_creation_narrowing_through_its_saved_type_and_loop() {
    let fixture = Fixture::new();
    for version in ["5.2", "5.4"] {
        fixture.check(ORIGINAL_READER, version, 0);
        for (wrong, occurrence, expression, code) in [
            (
                ORIGINAL_READER.replace("src()", "src(42)"),
                "src(42)",
                "src(42)",
                "TS2554",
            ),
            (
                ORIGINAL_READER.replace("json = --[[---@not nil]] src()", "json = 42"),
                "json = 42",
                "42",
                "TS2322",
            ),
        ] {
            let rejected = fixture.check(&wrong, version, 1);
            assert_eq!(
                rejected.matches("\"severity\":\"error\"").count(),
                1,
                "{rejected}"
            );
            anchored(&rejected, &wrong, occurrence, expression, code);
            fixture.check(ORIGINAL_READER, version, 0);
        }
    }
}
