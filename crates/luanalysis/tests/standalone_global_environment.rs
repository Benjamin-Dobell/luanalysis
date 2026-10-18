use luanalysis::{CheckReport, check};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis standalone globals {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        let fixture = Self(path);
        fixture.source(source);
        fixture
    }
    fn source(&self, source: &str) {
        fs::write(self.0.join("main.lua"), source).unwrap();
    }
    fn check(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .current_dir(&self.0)
            .args(["check", "--no-stdlib", "--dump-types", "main.lua"])
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn pass(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{}", text(output));
}
fn fail(output: &Output, diagnostic: &str) {
    assert_eq!(output.status.code(), Some(1), "{}", text(output));
    assert!(text(output).contains(diagnostic), "{}", text(output));
}
fn rejects_at(source: &str, expected: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "TS2322"
                && &source[diagnostic.span.start..diagnostic.span.end] == expected),
        "{:#?}",
        report.diagnostics
    );
    report
}

#[test]
fn single_source_and_cli_global_function_queries_infer_the_original_body_and_accept_correction() {
    let source = "function global() return 1 end\nlocal answer = global()\n---@type number\nlocal checked = answer\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert!(
        report
            .bindings
            .iter()
            .any(|binding| binding.name == "answer" && binding.ty == "number"),
        "{:#?}",
        report.bindings
    );
    let fixture = Fixture::new(source);
    pass(&fixture.check());
    let invalid = source.replace("---@type number", "---@type string");
    rejects_at(&invalid, "answer");
    fixture.source(&invalid);
    fail(&fixture.check(), "main.lua:4:17: error[TS2322]");
    fixture.source(source);
    pass(&fixture.check());
}

#[test]
fn single_source_and_cli_global_writes_keep_the_first_original_inferred_contract() {
    let valid = "answer = 1\nanswer = 2\n---@type number\nlocal checked = answer\n";
    assert!(check(valid).diagnostics.is_empty());
    let fixture = Fixture::new(valid);
    pass(&fixture.check());
    let invalid = valid.replace("answer = 2", "answer = 'wrong'");
    rejects_at(&invalid, "'wrong'");
    fixture.source(&invalid);
    fail(&fixture.check(), "main.lua:2:10: error[TS2322]");
    fixture.source(valid);
    pass(&fixture.check());
}

#[test]
fn original_environment_qualified_and_bare_names_share_the_global_write_contract() {
    for invalid in [
        "answer = 1\n_ENV.answer = 'wrong'\n",
        "answer = 1\nanswer = 'wrong'\n",
    ] {
        rejects_at(invalid, "'wrong'");
        let fixture = Fixture::new(invalid);
        fail(&fixture.check(), "TS2322");
        let corrected = invalid.replace("'wrong'", "2");
        let report = check(&corrected);
        assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
        fixture.source(&corrected);
        pass(&fixture.check());
    }
}
