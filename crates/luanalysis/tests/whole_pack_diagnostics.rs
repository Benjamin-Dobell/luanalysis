use luanalysis::Diagnostic;
use luanalysis::project_check::{
    ProjectCheckOptions, ProjectCheckReport, check_project_with_options, static_require_requests,
};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const PROFILES: [(StandardEnvironmentProfile, &str); 2] = [
    (StandardEnvironmentProfile::Lua52, "5.2"),
    (StandardEnvironmentProfile::Lua54, "5.4"),
];
const PREFIX: &str = "Lua result pack is not assignable to the declared result pack.";
const WHOLE: &str = "-- 前置\r\n---@param callback any\r\n---@return never\r\nlocal function invoke(callback) return callback() end\r\n";
const WHOLE_MESSAGE: &str = "Lua result pack is not assignable to the declared result pack. Actual: any result pack. Expected: never.";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis whole pack diagnostics {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn write(&self, source: &str) -> PathBuf {
        let path = self.0.join("main.lua");
        fs::write(&path, source).unwrap();
        path
    }
    fn check(&self, source: &str, environment: StandardEnvironmentProfile) -> ProjectCheckReport {
        let main = self.write(source);
        let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
        let project = Project::load_plan(
            &resolver,
            ProjectLoadPlan::new([main]),
            static_require_requests,
        );
        check_project_with_options(
            project,
            ProjectCheckOptions {
                environment,
                ..Default::default()
            },
        )
    }
    fn cli(&self, source: &str, format: &str, version: &str) -> Output {
        self.write(source);
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .current_dir(&self.0)
            .args([
                "check",
                "--format",
                format,
                "--lua-version",
                version,
                "main.lua",
            ])
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn clean(report: &ProjectCheckReport) {
    assert!(!report.has_errors(), "{:#?}", report.files);
}
fn diagnostics(report: &ProjectCheckReport) -> &[Diagnostic] {
    &report
        .file(report.project.entries[0])
        .unwrap()
        .report
        .diagnostics
}
fn span(source: &str, statement: &str, expression: &str) -> (usize, usize) {
    let start = source.find(statement).unwrap() + statement.rfind(expression).unwrap();
    (start, start + expression.len())
}
fn no_opaque_descriptions(text: &str) {
    for opaque in [
        "contextual type",
        "<uninstantiated variadic result pack>",
        "SessionPack",
        "NativeResultPack",
        "undefined",
    ] {
        assert!(!text.contains(opaque), "{opaque}: {text}");
    }
}
fn error_at(diagnostic: &Diagnostic, source: &str, code: &str, statement: &str, expression: &str) {
    let (start, end) = span(source, statement, expression);
    assert_eq!(diagnostic.code, code, "{source}\n{diagnostic:#?}");
    assert_eq!(diagnostic.span.start, start, "{source}\n{diagnostic:#?}");
    assert_eq!(diagnostic.span.end, end, "{source}\n{diagnostic:#?}");
    no_opaque_descriptions(&diagnostic.message);
}
fn return_failure(
    report: &ProjectCheckReport,
    source: &str,
    statement: &str,
    expression: &str,
    actual: &str,
) {
    let diagnostics = diagnostics(report);
    assert_eq!(diagnostics.len(), 1, "{source}\n{diagnostics:#?}");
    error_at(&diagnostics[0], source, "TS2322", statement, expression);
    assert_eq!(
        diagnostics[0].message,
        format!("{PREFIX} Actual: {actual}. Expected: never.")
    );
}

#[test]
fn any_and_untyped_function_call_results_render_their_whole_pack_carrier() {
    let fixture = Fixture::new();
    for parameter in ["any", "function"] {
        let source = WHOLE.replace(
            "---@param callback any",
            &format!("---@param callback {parameter}"),
        );
        let corrected = source.replace("---@return never", "---@return any...");
        for (profile, _) in PROFILES {
            return_failure(
                &fixture.check(&source, profile),
                &source,
                "return callback()",
                "callback()",
                "any result pack",
            );
            clean(&fixture.check(&corrected, profile));
        }
    }
}

#[test]
fn unknown_call_errors_keep_the_original_callee_and_whole_recovery_pack_diagnostics() {
    let fixture = Fixture::new();
    let source = WHOLE.replace("---@param callback any", "---@param callback unknown");
    let corrected_return = source.replace("---@return never", "---@return any...");
    let corrected_all = corrected_return.replace(
        "---@param callback unknown",
        "---@param callback fun():any...",
    );
    for (profile, _) in PROFILES {
        let report = fixture.check(&source, profile);
        let errors = diagnostics(&report);
        assert_eq!(errors.len(), 2, "{errors:#?}");
        let call = errors.iter().find(|error| error.code == "TS2349").unwrap();
        error_at(call, &source, "TS2349", "return callback()", "callback");
        assert_eq!(call.message, "Type 'unknown' has no call signatures.");
        let returns = errors.iter().find(|error| error.code == "TS2322").unwrap();
        error_at(
            returns,
            &source,
            "TS2322",
            "return callback()",
            "callback()",
        );
        assert_eq!(returns.message, WHOLE_MESSAGE);

        let report = fixture.check(&corrected_return, profile);
        let errors = diagnostics(&report);
        assert_eq!(errors.len(), 1, "{errors:#?}");
        error_at(
            &errors[0],
            &corrected_return,
            "TS2349",
            "return callback()",
            "callback",
        );
        clean(&fixture.check(&corrected_all, profile));
    }
}

#[test]
fn whole_any_packs_remain_distinct_from_scalar_any_and_variadic_any_rows() {
    let fixture = Fixture::new();
    let scalar = WHOLE.replace("return callback()", "return (callback())");
    let variadic = "---@vararg any\n---@return never\nlocal function forward(...) return ... end\n";
    let typed = "---@param callback fun():any...\n---@return never\nlocal function invoke(callback) return callback() end\n";
    for (profile, _) in PROFILES {
        for (source, statement, expression, actual) in [
            (WHOLE, "return callback()", "callback()", "any result pack"),
            (
                scalar.as_str(),
                "return (callback())",
                "(callback())",
                "any",
            ),
            (variadic, "return ...", "...", "(any...)"),
            (typed, "return callback()", "callback()", "void | (any...)"),
        ] {
            return_failure(
                &fixture.check(source, profile),
                source,
                statement,
                expression,
                actual,
            );
            clean(&fixture.check(
                &source.replace("---@return never", "---@return any..."),
                profile,
            ));
        }
    }
}

#[test]
fn rendered_return_packs_do_not_suppress_original_typed_callback_argument_errors() {
    let fixture = Fixture::new();
    let source = "---@param callback fun(value:number):any...\n---@return never\nlocal function invoke(callback) return callback('wrong') end\n";
    let corrected_return = source.replace("---@return never", "---@return any...");
    let corrected_all = corrected_return.replace("callback('wrong')", "callback(1)");
    for (profile, _) in PROFILES {
        let report = fixture.check(source, profile);
        let errors = diagnostics(&report);
        assert_eq!(errors.len(), 2, "{errors:#?}");
        let returns = errors.iter().find(|error| error.code == "TS2322").unwrap();
        error_at(
            returns,
            source,
            "TS2322",
            "return callback('wrong')",
            "callback('wrong')",
        );
        assert_eq!(
            returns.message,
            format!("{PREFIX} Actual: void | (any...). Expected: never.")
        );
        let argument = errors.iter().find(|error| error.code == "TS2345").unwrap();
        error_at(argument, source, "TS2345", "callback('wrong')", "'wrong'");
        assert_eq!(
            argument.message,
            "Argument of type '\"wrong\"' is not assignable to parameter of type 'number'."
        );

        let report = fixture.check(&corrected_return, profile);
        let errors = diagnostics(&report);
        assert_eq!(errors.len(), 1, "{errors:#?}");
        error_at(
            &errors[0],
            &corrected_return,
            "TS2345",
            "callback('wrong')",
            "'wrong'",
        );
        clean(&fixture.check(&corrected_all, profile));
    }
}

#[test]
fn decomposed_correlated_return_rows_keep_their_original_slot_relationships() {
    let fixture = Fixture::new();
    let source = "---@param ok boolean\n---@return (true,number)|(false,string)\nlocal function choose(ok)\n    if ok then return true,1 end\n    return false,'word'\nend\n\n---@param ok boolean\n---@return (true,string)|(false,number)\nlocal function invoke(ok) return choose(ok) end\n";
    let corrected = source.replace(
        "---@return (true,string)|(false,number)",
        "---@return (true,number)|(false,string)",
    );
    for (profile, _) in PROFILES {
        let report = fixture.check(source, profile);
        let errors = diagnostics(&report);
        assert_eq!(errors.len(), 1, "{errors:#?}");
        error_at(
            &errors[0],
            source,
            "TS2322",
            "return choose(ok)",
            "choose(ok)",
        );
        let (actual, expected) = errors[0]
            .message
            .strip_prefix(&format!("{PREFIX} Actual: "))
            .unwrap()
            .split_once(". Expected: ")
            .unwrap();
        for row in ["(true, number)", "(false, string)"] {
            assert!(actual.contains(row), "{actual}");
        }
        for row in ["(true, string)", "(false, number)"] {
            assert!(expected.contains(row), "{expected}");
        }
        assert!(actual.contains(" | ") && expected.contains(" | "));
        assert!(!actual.contains("number | string"), "{actual}");
        clean(&fixture.check(&corrected, profile));
    }
}

#[test]
fn text_cli_renders_whole_pack_types_at_the_original_unicode_crlf_location() {
    let fixture = Fixture::new();
    let corrected = WHOLE.replace("---@return never", "---@return any...");
    for (_, version) in PROFILES {
        let invalid = fixture.cli(WHOLE, "text", version);
        let stderr = String::from_utf8(invalid.stderr).unwrap();
        assert_eq!(invalid.status.code(), Some(1), "{stderr}");
        assert!(
            stderr.contains(&format!("main.lua:4:40: error[TS2322]: {WHOLE_MESSAGE}")),
            "{stderr}"
        );
        no_opaque_descriptions(&stderr);
        let corrected = fixture.cli(&corrected, "text", version);
        assert_eq!(corrected.status.code(), Some(0), "{corrected:?}");
        assert!(corrected.stderr.is_empty(), "{corrected:?}");
        assert!(
            String::from_utf8(corrected.stdout)
                .unwrap()
                .contains("No type errors"),
        );
    }
}

#[test]
fn json_cli_keeps_whole_pack_messages_codes_and_original_utf8_byte_ranges() {
    let fixture = Fixture::new();
    let corrected = WHOLE.replace("---@return never", "---@return any...");
    let (start, end) = span(WHOLE, "return callback()", "callback()");
    let range = format!("\"range\":{{\"start\":{start},\"end\":{end}}}");
    for (_, version) in PROFILES {
        let invalid = fixture.cli(WHOLE, "json", version);
        assert_eq!(invalid.status.code(), Some(1), "{invalid:?}");
        assert!(invalid.stderr.is_empty(), "{invalid:?}");
        let stdout = String::from_utf8(invalid.stdout).unwrap();
        assert_eq!(stdout.matches("\"code\":\"TS2322\"").count(), 1, "{stdout}");
        assert!(
            stdout.contains(&format!("\"message\":\"{WHOLE_MESSAGE}\"")),
            "{stdout}"
        );
        assert!(stdout.contains(&range), "{stdout}");
        assert!(
            stdout.contains("\"position\":{\"line\":4,\"column\":40}"),
            "{stdout}"
        );
        assert!(
            stdout.contains("\"rangeEncoding\":\"utf-8-bytes\""),
            "{stdout}"
        );
        no_opaque_descriptions(&stdout);
        let corrected = fixture.cli(&corrected, "json", version);
        assert_eq!(corrected.status.code(), Some(0), "{corrected:?}");
        assert!(corrected.stderr.is_empty(), "{corrected:?}");
        let stdout = String::from_utf8(corrected.stdout).unwrap();
        assert!(
            stdout.contains("\"success\":true") && stdout.contains("\"diagnostics\":[]"),
            "{stdout}"
        );
    }
}
