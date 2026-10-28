//! The pinned corpus uses the existing marker reader; only the host and diagnostic
//! vocabulary are adapted here. Original Lua and expectations are never rewritten.
use luanalysis::incremental::ProjectWorkspace;
use luanalysis::language_service::{DocumentSymbol, complete, completion_site, document_symbols};
use luanalysis::project_check::{
    ProjectCheckOptions, check_project_with_options, static_require_requests,
};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis::{Diagnostic, DiagnosticCategory, OverloadMode};
use luanalysis_fixtures::{Fixture, Severity, Span, import};
use luanalysis_project::{Project, ProjectConfig, Resolver};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub const MANIFEST: &str = include_str!("../legacy/cases.tsv");
pub const INVENTORY: &str = include_str!("../legacy/inventory.tsv");

#[derive(Clone, Debug)]
pub struct Case {
    pub id: &'static str,
    pub kind: &'static str,
    pub version: &'static str,
    pub strict_nil: bool,
    pub unknown: bool,
    pub weak: bool,
    pub required: Vec<&'static str>,
    pub not_all: Vec<&'static str>,
    pub inputs: Vec<&'static str>,
    pub origin: &'static str,
    pub notes: &'static str,
}
fn list(value: &'static str) -> Vec<&'static str> {
    value.split('|').filter(|value| !value.is_empty()).collect()
}
pub fn cases() -> Vec<Case> {
    MANIFEST
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            assert_eq!(fields.len(), 11, "invalid manifest: {line}");
            let boolean = |index: usize| match fields[index] {
                "true" => true,
                "false" => false,
                other => panic!("invalid boolean: {other}"),
            };
            Case {
                id: fields[0],
                kind: fields[1],
                version: fields[2],
                strict_nil: boolean(3),
                unknown: boolean(4),
                weak: boolean(5),
                required: list(fields[6]),
                not_all: list(fields[7]),
                inputs: list(fields[8]),
                origin: fields[9],
                notes: fields[10],
            }
        })
        .collect()
}
pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/legacy")
}
static NEXT: AtomicU64 = AtomicU64::new(0);
pub struct Temp(pub PathBuf);
impl Temp {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis-legacy-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[derive(Debug)]
pub struct Result {
    pub passed: bool,
    pub expected: usize,
    pub missing: usize,
    pub unexpected: usize,
    pub detail: String,
}
impl Result {
    fn new() -> Self {
        Self {
            passed: true,
            expected: 0,
            missing: 0,
            unexpected: 0,
            detail: String::new(),
        }
    }
    fn fail(&mut self, message: impl AsRef<str>) {
        self.passed = false;
        self.detail.push_str(message.as_ref());
        self.detail.push('\n');
    }
}
fn options(case: &Case) -> ProjectCheckOptions {
    let environment = match case.version {
        "5.1" => StandardEnvironmentProfile::Lua51,
        "5.2" => StandardEnvironmentProfile::Lua52,
        "5.3" => StandardEnvironmentProfile::Lua53,
        "5.4" => StandardEnvironmentProfile::Lua54,
        other => panic!("unsupported environment {other}"),
    };
    ProjectCheckOptions {
        jobs: 1,
        environment,
        strict_nil: case.strict_nil,
        overload_mode: OverloadMode::LegacyExposeImplementation,
        describe_types: false,
        inspections: matches!(case.id, "LocalNameShadowedTest/testLocalNameHidesPrevious" | "TypeSafetyTest/testFunctionMultipleReturns"
            | "TypeSafetyTest/testFunctionGenerics" | "TypeSafetyTest/testGenericClassScope"),
        ..Default::default()
    }
}

/// Explicit semantic diagnostic families, not equality with obsolete IntelliJ
/// wording. Unsupported features and syntax errors can never satisfy a type error.
fn semantic_code(description: Option<&str>, code: &str) -> bool {
    let description = description.unwrap_or("");
    if description.starts_with("Undeclared variable") {
        return code == "TS2304";
    }
    if description.starts_with("No such member") {
        return matches!(code, "TS2339" | "TS2551");
    }
    if description.starts_with("No such indexer") {
        return matches!(code, "TS7053" | "TS7015" | "TS2538");
    }
    if description.starts_with("Unknown function") {
        return matches!(code, "TS2349" | "TS2721" | "TS2722");
    }
    if description.starts_with("Missing argument")
        || description.starts_with("Missing self argument")
    {
        return matches!(code, "TS2554" | "TS2555");
    }
    if description.starts_with("Return type") {
        return matches!(code, "TS2355" | "TS2366");
    }
    if description.starts_with("Alias '") {
        return code == "TS2456";
    }
    if description.starts_with("Illegal inheritance")
        || description.starts_with("Illegal self inheritance")
        || description.starts_with("Illegal cyclical inheritance")
    {
        return matches!(code, "TS2310" | "TS2312" | "TS2506" | "LUA2018");
    }
    if description.starts_with("Generic parameters cannot be shadowed") {
        return code == "LUA4005";
    }
    if description.starts_with("Required parameters cannot follow optional") {
        return code == "TS1016";
    }
    if description.starts_with("Illegal override") {
        return matches!(code, "TS2416" | "TS2417" | "TS2430");
    }
    if description.starts_with("Type mismatch") || description.contains("type mismatch") {
        return matches!(
            code,
            "TS2322"
                | "TS2345"
                | "TS2344"
                | "TS2352"
                | "TS2739"
                | "TS2740"
                | "TS2741"
                | "TS2538"
                | "TS2353"
                | "TS2411"
                | "TS2413"
                | "TS2684"
        );
    }
    if description.starts_with("Too many assignees") {
        return matches!(code, "LUA4003" | "TS2322" | "TS2554" | "TS2555");
    }
    if description.starts_with("Incorrect number of values") {
        return matches!(code, "TS2322" | "TS2554" | "TS2555");
    }
    if description.starts_with("Insufficient assignees") {
        return code == "LUA4002";
    }
    if description.contains("excess argument") {
        // Strict public call arity remains a checker error, independent of
        // assignment-result hints. Do not let one substitute for the other.
        return false;
    }
    // Bare markers mean any *language* diagnostic in the original test. Do not
    // let parser failures or a LUA20xx unsupported-feature report stand in for it.
    description.is_empty() && (code.starts_with("TS") || code == "LUA4001")
}

fn language_level_description(description: &str) -> bool {
    (description.starts_with("The binary operator '")
        || description.starts_with("The unary operator '")
        || description.starts_with("The local variable attribute '"))
        && (description.ends_with("only available in Lua 5.3 and above")
            || description.ends_with("only available in Lua 5.4 and above"))
}

pub fn matches(expected: &luanalysis_fixtures::ExpectedDiagnostic, actual: &Diagnostic) -> bool {
    let severity = match actual.category() {
        DiagnosticCategory::Error => Severity::Error,
        DiagnosticCategory::Warning => Severity::Warning,
        DiagnosticCategory::Suggestion => Severity::WeakWarning,
    };
    // Undeclared variables became checker errors. Keep the warning's exact name
    // range and TS2304 semantic identity while adapting that explicit severity.
    let expected_severity = if actual.code == "LUA4003" && expected.description.as_deref()
        .is_some_and(|description| description.starts_with("Too many assignees"))
    {
        // Count inspection is intentionally a nonfatal hint; nil-to-required
        // type errors still have to match their own original error markers.
        Severity::WeakWarning
    } else if actual.code == "LUA4005" && expected.description.as_deref()
        .is_some_and(|description| description.starts_with("Generic parameters cannot be shadowed"))
    {
        // Shadowing is valid, as in TypeScript; v1's error is a warning inspection.
        Severity::Warning
    } else if expected
        .description
        .as_deref()
        .is_some_and(|d| d.starts_with("Undeclared variable"))
    {
        Severity::Error
    } else {
        expected.severity
    };
    expected.span
        == (Span {
            start: actual.span.start,
            end: actual.span.end,
        })
        && expected_severity == severity
        && if let Some(description) = expected.description.as_deref()
            && language_level_description(description)
        {
            // LUA1001 also identifies parser errors. A version requirement must
            // match the exact original feature, never an arbitrary parse error.
            actual.code == "LUA1001" && actual.message == description
        } else {
            semantic_code(expected.description.as_deref(), actual.code)
        }
}

/// Translate only documented PSI display ranges to exact native syntax ranges.
/// This is driven by the original expectation and AST, never by observed errors.
/// The marked source, original span and diagnostic multiplicity remain intact.
pub fn translated_expectations(fixture: &Fixture) -> Vec<luanalysis_fixtures::ExpectedDiagnostic> {
    let syntax = luanalysis_syntax::parse(&fixture.source);
    let mut expected = fixture.diagnostics.clone();
    for diagnostic in &mut expected {
        // The Lua54 test uses bare markers on these two attribute tokens.
        // Derive their feature identity from the unchanged marked input.
        if diagnostic.description.is_none() {
            let snippet = &fixture.source[diagnostic.span.start..diagnostic.span.end];
            if matches!(snippet, "<const>" | "<close>") {
                diagnostic.description = Some(format!(
                    "The local variable attribute '{snippet}' is only available in Lua 5.4 and above"
                ));
            }
        }
        let description = diagnostic.description.as_deref().unwrap_or("");
        if description.starts_with("Alias '")
            || description.starts_with("Illegal inheritance")
            || description.starts_with("Illegal self inheritance")
            || description.starts_with("Illegal cyclical inheritance")
        {
            if let Some(declaration) = syntax.type_declarations.iter().find(|declaration| {
                diagnostic.span.start <= declaration.name.span.start
                    && declaration.name.span.end <= diagnostic.span.end
            }) {
                let target = if description.starts_with("Alias '") {
                    declaration.name.span
                } else if let luanalysis_syntax::NamedTypeDeclarationKind::Record {
                    base: Some(base),
                    ..
                } = &declaration.kind
                {
                    base.span
                } else {
                    declaration.name.span
                };
                diagnostic.span = Span {
                    start: target.start,
                    end: target.end,
                };
            }
        }
        if description.starts_with("No such member") || description.starts_with("No such indexer") {
            // IntelliJ underlined an access; the native diagnostic underlines
            // its member name or key. Parse that exact marked access to select
            // the native range, without accepting arbitrary containment.
            let snippet = &fixture.source[diagnostic.span.start..diagnostic.span.end];
            let prefix = "return ";
            let mut access = luanalysis_syntax::parse(&format!("{prefix}{snippet}"));
            if access.has_lua_syntax_errors() && description.starts_with("No such member") {
                access = luanalysis_syntax::parse(&format!("{prefix}{snippet}()"));
            }
            if let [
                luanalysis_syntax::Stmt {
                    kind: luanalysis_syntax::StmtKind::Return(values),
                    ..
                },
            ] = access.statements.as_slice()
            {
                if let [value] = values.as_slice() {
                    let target = match &value.kind {
                        luanalysis_syntax::ExprKind::Field { name, .. }
                        | luanalysis_syntax::ExprKind::MethodCall { name, .. }
                            if description.starts_with("No such member") =>
                        {
                            Some(name.span)
                        }
                        luanalysis_syntax::ExprKind::Index { index, .. }
                            if description.starts_with("No such indexer") =>
                        {
                            Some(index.span)
                        }
                        _ => None,
                    };
                    if let Some(target) = target {
                        diagnostic.span = Span {
                            start: diagnostic.span.start + target.start - prefix.len(),
                            end: diagnostic.span.start + target.end - prefix.len(),
                        };
                    }
                }
            }
        }
        if description.starts_with("Return type") {
            for statement in &syntax.statements {
                let function = match &statement.kind {
                    luanalysis_syntax::StmtKind::LocalFunction { function, .. }
                    | luanalysis_syntax::StmtKind::FunctionDeclaration { function, .. } => function,
                    _ => continue,
                };
                if diagnostic.span.end == function.span.end {
                    if let Some(annotation) = &function.annotations.returns {
                        diagnostic.span = Span {
                            start: annotation.span.start,
                            end: annotation.span.end,
                        };
                    }
                }
            }
        }
    }
    expected
}

pub fn compare(fixture: &Fixture, actual: &[Diagnostic], weak: bool) -> (Vec<usize>, Vec<usize>) {
    let mut used = vec![false; actual.len()];
    let mut missing = Vec::new();
    // Distinct nested markers still require distinct observed diagnostics.
    for (index, expected) in translated_expectations(fixture).iter().enumerate() {
        if expected.severity == Severity::Info
            || expected.severity == Severity::WeakWarning && !weak
        {
            continue;
        }
        if let Some(found) = actual
            .iter()
            .enumerate()
            .position(|(i, actual)| !used[i] && matches(expected, actual))
        {
            used[found] = true;
        } else {
            missing.push(index);
        }
    }
    (
        missing,
        used.iter()
            .enumerate()
            .filter_map(|(i, used)| (!used).then_some(i))
            .collect(),
    )
}

fn location(source: &str, span: Span) -> String {
    let (line, column) = luanalysis::location(
        source,
        luanalysis_syntax::Span {
            start: span.start,
            end: span.end,
        },
    );
    format!("{line}:{column} [{}..{}]", span.start, span.end)
}
fn append_diagnostics(
    result: &mut Result,
    path: &str,
    fixture: &Fixture,
    actual: &[Diagnostic],
    weak: bool,
) {
    let (missing, unexpected) = compare(fixture, actual, weak);
    result.expected += fixture
        .diagnostics
        .iter()
        .filter(|d| d.severity != Severity::Info && (weak || d.severity != Severity::WeakWarning))
        .count();
    result.missing += missing.len();
    result.unexpected += unexpected.len();
    for i in missing {
        let diagnostic = &fixture.diagnostics[i];
        result.fail(format!(
            "  MISSING {path}:{} {:?} {:?}: {}",
            location(&fixture.source, diagnostic.span),
            diagnostic.severity,
            fixture.snippet(diagnostic),
            diagnostic.description.as_deref().unwrap_or("<bare marker>")
        ));
    }
    for i in unexpected {
        let diagnostic = &actual[i];
        let span = Span {
            start: diagnostic.span.start,
            end: diagnostic.span.end,
        };
        result.fail(format!(
            "  EXTRA {path}:{} {} {:?}: {}",
            location(&fixture.source, span),
            diagnostic.code,
            fixture
                .source
                .get(span.start..span.end)
                .unwrap_or("<invalid span>"),
            diagnostic.message
        ));
    }
}

/// Preserve containsAll/contains and assertFalse(containsAll), including the last
/// one's weaker quantifier. Additional completion labels were always allowed.
pub fn missing_labels(case: &Case, labels: &BTreeSet<String>) -> Vec<String> {
    let mut errors: Vec<_> = case
        .required
        .iter()
        .filter(|label| !labels.contains(**label))
        .map(|label| format!("missing completion {label:?}"))
        .collect();
    if !case.not_all.is_empty() && case.not_all.iter().all(|label| labels.contains(*label)) {
        errors.push(format!(
            "expected at least one of {:?} to be absent",
            case.not_all
        ));
    }
    errors
}
/// The structure test asserts a class/method hierarchy, not a flat label set.
pub fn symbol_hierarchy_matches(
    symbols: &[DocumentSymbol],
    source: &str,
    class: &str,
    method: &str,
) -> bool {
    use luanalysis::language_service::SymbolKind;
    symbols.iter().any(|symbol| {
        symbol.name == class
            && symbol.kind == SymbolKind::Class
            && symbol.children.iter().any(|child| {
                child.kind == SymbolKind::Method
                    && source.get(child.selection.start..child.selection.end) == Some(method)
                    && symbol.range.start <= child.range.start
                    && child.range.end <= symbol.range.end
            })
    })
}

pub fn run(case: &Case, directory: &Path) -> Result {
    run_with_sources(case, directory, |_, source| source)
}

/// Controlled counterfactual inputs for investigating a difference. The normal
/// runner always reads the stored case unchanged through `run` above.
pub fn run_with_sources(
    case: &Case,
    directory: &Path,
    replace: impl Fn(&str, String) -> String,
) -> Result {
    let mut result = Result::new();
    let mut fixtures = Vec::new();
    let mut entries = Vec::new();
    let mut caret = None;
    for input in &case.inputs {
        let marked = replace(input, fs::read_to_string(root().join(input)).unwrap());
        let mut fixture = import(&marked).unwrap_or_else(|e| panic!("{input}: {e}"));
        for marker in ["<caret>", "--[[caret]]"] {
            if let Some(offset) = fixture.source.find(marker) {
                assert!(caret.is_none(), "multiple caret inputs in {}", case.id);
                assert_eq!(fixture.source.matches(marker).count(), 1);
                fixture
                    .source
                    .replace_range(offset..offset + marker.len(), "");
                caret = Some((entries.len(), offset));
            }
        }
        let path = directory.join(Path::new(input).file_name().unwrap());
        assert!(
            !entries.contains(&path),
            "colliding project filenames in {}",
            case.id
        );
        fs::write(&path, &fixture.source).unwrap();
        entries.push(path);
        fixtures.push(fixture);
    }
    if case.kind == "document-symbols" {
        let source = &fixtures[0].source;
        let symbols = document_symbols(source);
        assert_eq!(case.required.len(), 2);
        if !symbol_hierarchy_matches(&symbols, source, case.required[0], case.required[1]) {
            result.fail(format!(
                "  Expected class {:?} with child method {:?}; observed outline: {symbols:?}",
                case.required[0], case.required[1]
            ));
        }
        return result;
    }
    if case.kind == "completion" {
        let (index, offset) = caret.expect("completion case without caret");
        let source = &fixtures[index].source;
        let Some(site) = completion_site(source, offset) else {
            result.fail(format!(
                "  No native completion site at {}",
                location(
                    source,
                    Span {
                        start: offset,
                        end: offset
                    }
                )
            ));
            return result;
        };
        fs::write(&entries[index], &site.text).unwrap();
        let resolver = Resolver::new(ProjectConfig::new([directory.to_owned()])).unwrap();
        let project = Project::load(&resolver, entries.clone(), static_require_requests);
        let file = project
            .files
            .iter()
            .find(|f| f.path == entries[index])
            .unwrap()
            .id;
        let mut labels = BTreeSet::new();
        complete(
            project,
            options(case),
            &mut ProjectWorkspace::new(),
            file,
            &site,
            |items| {
                for item in items {
                    if let Some(implementation) = &item.implementation {
                        labels.insert(format!(
                            "{}({}) end",
                            implementation.head,
                            implementation.parameters.join(", ")
                        ));
                    }
                    labels.insert(item.label);
                }
            },
        );
        for error in missing_labels(case, &labels) {
            result.fail(format!("  {error}"));
        }
        if !result.passed {
            result
                .detail
                .push_str(&format!("  Observed labels: {labels:?}\n"));
        }
        return result;
    }
    let resolver = Resolver::new(ProjectConfig::new([directory.to_owned()])).unwrap();
    let project = Project::load(&resolver, entries.clone(), static_require_requests);
    let report = check_project_with_options(project, options(case));
    for (index, path) in entries.iter().enumerate() {
        let source = report
            .project
            .files
            .iter()
            .find(|file| file.path == *path)
            .unwrap();
        let actual = &report
            .file(source.id)
            .expect("entry not checked")
            .report
            .diagnostics;
        // Reproduce the original selected optional inspection family while
        // keeping every checker error visible.
        let selected;
        let actual = if matches!(case.id, "LocalNameShadowedTest/testLocalNameHidesPrevious" | "TypeSafetyTest/testFunctionMultipleReturns"
            | "TypeSafetyTest/testFunctionGenerics" | "TypeSafetyTest/testGenericClassScope")
        {
            selected = actual.iter().filter(|diagnostic| {
                diagnostic.category() == DiagnosticCategory::Error || match case.id {
                    "LocalNameShadowedTest/testLocalNameHidesPrevious" => diagnostic.code == "LUA4001",
                    "TypeSafetyTest/testFunctionMultipleReturns" => matches!(diagnostic.code, "LUA4002" | "LUA4003"),
                    _ => diagnostic.code == "LUA4005",
                }
            }).cloned().collect::<Vec<_>>();
            &selected
        } else { actual };
        append_diagnostics(
            &mut result,
            case.inputs[index],
            &fixtures[index],
            actual,
            case.weak,
        );
    }
    if !report.environment_errors.is_empty()
        || !report.project.entry_errors.is_empty()
        || !report.project.library_errors.is_empty()
    {
        result.fail(format!(
            "  Host errors: {:?}; {:?}; {:?}",
            report.environment_errors, report.project.entry_errors, report.project.library_errors
        ));
    }
    // The legacy unknown-call/index policy has no equivalent native switch.
    // Retain the actual diagnostics as well as that unsupported configuration.
    if case.id == "TypeSafetyTest/testUnknown" {
        result.fail("  UNSUPPORTED SETTING: permissive unknown calls and indexing");
    }
    result
}
