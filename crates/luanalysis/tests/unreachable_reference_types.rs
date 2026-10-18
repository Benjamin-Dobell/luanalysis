use luanalysis::project_check::{
    ProjectCheckOptions, check_project_with_options, static_require_requests,
};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis::{check, location};
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

const DEAD_CONTINUATION: &str = r#"
---@param callback fun(value:number):void
---@param record {value:number}
local function single(callback,record)
    if true then return end
    callback(record.value)
end
---@param flag boolean
---@param callback fun(value:number):void
---@param record {value:number}
local function joined(flag,callback,record)
    if flag then return else return end
    callback(record.value)
end
"#;

// Reduced from lunajson/encoder.lua: the written indexer contains true, so its
// truthy guard can make the continuation unreachable to native CFA. Checking
// that continuation must retain the unrelated builder and callback contracts.
const ENCODER_VISITED: &str = r#"
local error=error
---@type number, string[], table<table,true>
local i,builder,visited=1,{},{}
---@param text string
local function emit(text)
    builder[i]=text
end
---@param o table
local function encode(o)
    if visited[o] then error('loop detected') end
    visited[o]=true
    builder[i]='['
    emit(']')
end
"#;

static NEXT_ENCODER: AtomicU64 = AtomicU64::new(0);

// The low-level check() API has no implicit error builtin. Use the real Lua
// declarations so the saved alias retains error's nonreturning result type.
fn encoder_report(source: &str) -> luanalysis::CheckReport {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis encoder reference API {} {}",
        std::process::id(),
        NEXT_ENCODER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("encoder.lua");
    fs::write(&path, source).unwrap();
    let resolver = Resolver::new(ProjectConfig::new([directory.clone()])).unwrap();
    let project = Project::load_plan(
        &resolver,
        ProjectLoadPlan::new([path]),
        static_require_requests,
    );
    assert_eq!(project.entries.len(), 1, "{project:#?}");
    let entry = project.entries[0];
    let report = check_project_with_options(
        project,
        ProjectCheckOptions {
            environment: StandardEnvironmentProfile::Lua52,
            ..Default::default()
        },
    );
    assert!(report.environment_errors.is_empty(), "{report:#?}");
    assert!(report.project.entry_errors.is_empty(), "{report:#?}");
    assert!(report.project.library_errors.is_empty(), "{report:#?}");
    assert!(report.project.role_conflicts.is_empty(), "{report:#?}");
    let mut encoder = None;
    for file in &report.files {
        if file.file == entry {
            encoder = Some(file.report.clone());
        } else {
            assert!(file.report.diagnostics.is_empty(), "{file:#?}");
        }
    }
    fs::remove_dir_all(&directory).unwrap();
    encoder.expect("original encoder source was checked")
}

#[test]
fn encoder_cycle_guard_preserves_visited_builder_and_callback_reference_types() {
    let report = encoder_report(ENCODER_VISITED);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    for (reference, expected) in [
        ("visited[o]=true", "table<table, true>"),
        ("builder[i]='['", "string[]"),
        ("emit(']')", "fun(text: string): void"),
    ] {
        assert_eq!(
            report.type_at(ENCODER_VISITED.rfind(reference).unwrap()),
            Some(expected),
            "{reference}"
        );
    }
}

#[test]
fn encoder_cycle_guard_keeps_the_original_visited_and_builder_write_contracts() {
    for (before, after, original, expected) in [
        ("visited[o]=true", "visited[o]=false", "false", "true"),
        ("builder[i]='['", "builder[i]=123", "123", "string"),
    ] {
        let source = ENCODER_VISITED.replace(before, after);
        let report = encoder_report(&source);
        assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
        let diagnostic = &report.diagnostics[0];
        assert_eq!(diagnostic.code, "TS2322", "{diagnostic:#?}");
        assert_eq!(
            &source[diagnostic.span.start..diagnostic.span.end],
            original
        );
        assert!(diagnostic.message.contains(expected), "{diagnostic:#?}");
        assert!(!diagnostic.message.contains("'never'"), "{diagnostic:#?}");
    }
    let corrected = encoder_report(ENCODER_VISITED);
    assert!(
        corrected.diagnostics.is_empty(),
        "{:#?}",
        corrected.diagnostics
    );
}

#[test]
fn encoder_cycle_guard_keeps_callback_argument_checking() {
    let source = ENCODER_VISITED.replace("emit(']')", "emit(123)");
    let report = encoder_report(&source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.code, "TS2345", "{diagnostic:#?}");
    assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], "123");
    assert!(diagnostic.message.contains("string"), "{diagnostic:#?}");
    assert!(!diagnostic.message.contains("'never'"), "{diagnostic:#?}");
    let corrected = encoder_report(ENCODER_VISITED);
    assert!(
        corrected.diagnostics.is_empty(),
        "{:#?}",
        corrected.diagnostics
    );
}

#[test]
fn cli_checks_encoder_cycle_guard_and_preserves_strict_errors_and_corrections() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis encoder cycle guard {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("encoder.lua");
    for (source, expected) in [
        (ENCODER_VISITED.to_owned(), None),
        (
            ENCODER_VISITED.replace("visited[o]=true", "visited[o]=false"),
            Some(("TS2322", "false", "true")),
        ),
        (
            ENCODER_VISITED.replace("emit(']')", "emit(123)"),
            Some(("TS2345", "123", "string")),
        ),
        (ENCODER_VISITED.to_owned(), None),
    ] {
        fs::write(&file, &source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .args(["check", "--lua-version", "5.2"])
            .arg(&file)
            .output()
            .unwrap();
        let text = String::from_utf8(output.stderr).unwrap();
        if let Some((code, original, expected)) = expected {
            assert_eq!(output.status.code(), Some(1), "{text}");
            let report = encoder_report(&source);
            assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
            let diagnostic = &report.diagnostics[0];
            assert_eq!(diagnostic.code, code, "{diagnostic:#?}");
            assert_eq!(
                &source[diagnostic.span.start..diagnostic.span.end],
                original
            );
            let (line, column) = location(&source, diagnostic.span);
            assert!(
                text.contains(&format!("encoder.lua:{line}:{column}: error[{code}]")),
                "{text}"
            );
            assert!(text.contains(expected), "{text}");
            assert!(!text.contains("'never'"), "{text}");
        } else {
            assert!(output.status.success(), "{text}");
        }
    }
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn entirely_dead_continuations_check_calls_and_members_against_their_declarations() {
    let report = check(DEAD_CONTINUATION);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    for offset in DEAD_CONTINUATION
        .match_indices("callback(record.value)")
        .map(|(offset, _)| offset)
    {
        assert_eq!(report.type_at(offset), Some("fun(value: number): void"));
        assert_eq!(
            report.type_at(offset + "callback(record.".len()),
            Some("number")
        );
    }
}

#[test]
fn dead_continuations_still_check_arguments_instead_of_replacing_the_callee_with_never() {
    let source = DEAD_CONTINUATION.replace("callback(record.value)", "callback('wrong')");
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 2, "{:#?}", report.diagnostics);
    for diagnostic in &report.diagnostics {
        assert_eq!(diagnostic.code, "TS2345", "{diagnostic:#?}");
        assert_eq!(
            &source[diagnostic.span.start..diagnostic.span.end],
            "'wrong'"
        );
        assert!(diagnostic.message.contains("number"), "{diagnostic:#?}");
    }
    let corrected = check(DEAD_CONTINUATION);
    assert!(
        corrected.diagnostics.is_empty(),
        "{:#?}",
        corrected.diagnostics
    );
}

#[test]
fn an_impossible_predicate_on_the_reference_itself_still_narrows_it_to_never() {
    let source = "---@param value number|string\nlocal function impossible(value)\n if type(value)=='boolean' then value() end\nend\n";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.code, "TS2349");
    assert!(diagnostic.message.contains("'never'"), "{diagnostic:#?}");
    assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], "value");
    assert_eq!(report.type_at(diagnostic.span.start), Some("never"));
    let corrected = check(&source.replace("value()", "local ignored=value"));
    assert!(
        corrected.diagnostics.is_empty(),
        "{:#?}",
        corrected.diagnostics
    );
}

#[test]
fn cli_preserves_dead_code_argument_errors_and_accepts_the_original_correction() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis unreachable references {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("main.lua");
    fs::write(&file, DEAD_CONTINUATION).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .args(["check", "--lua-version", "5.2"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let wrong = DEAD_CONTINUATION.replacen("callback(record.value)", "callback('wrong')", 1);
    fs::write(&file, &wrong).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .args(["check", "--lua-version", "5.2"])
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8(output.stderr).unwrap();
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let (line, column) = location(&wrong, report.diagnostics[0].span);
    assert!(
        text.contains(&format!("main.lua:{line}:{column}: error[TS2345]")),
        "{text}"
    );
    assert!(!text.contains("'never'"), "{text}");
    fs::write(&file, DEAD_CONTINUATION).unwrap();
    let corrected = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .args(["check", "--lua-version", "5.2"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        corrected.status.success(),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    fs::remove_dir_all(&directory).unwrap();
}
