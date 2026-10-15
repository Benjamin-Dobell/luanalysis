use luanalysis_fixtures::{
    CaseConfig, CaseStatus, DiagnosticMatch, FixtureOrigin, Severity, Span, compare_exact, import,
};

const CHECKED_SEVERITIES: &[Severity] = &[Severity::Error, Severity::Warning];
const CASES: &[CaseConfig] = &[
    CaseConfig {
        id: "legacy/local_def_assignment",
        origin_path: "src/test/resources/inspections/local_def_assignment.lua",
        lua_version: "5.3",
        strict_nil: true,
        unknown_callable: false,
        unknown_indexable: false,
        checked_severities: CHECKED_SEVERITIES,
        status: CaseStatus::SharedSemantics,
    },
    CaseConfig {
        id: "legacy-reduction/number-assignment",
        origin_path: "src/test/resources/inspections/numbers.lua:1,8",
        lua_version: "5.3",
        strict_nil: true,
        unknown_callable: false,
        unknown_indexable: false,
        checked_severities: CHECKED_SEVERITIES,
        status: CaseStatus::SharedSemantics,
    },
    CaseConfig {
        id: "legacy-reduction/boolean-assignment",
        origin_path: "src/test/resources/inspections/boolean.lua:1,16,17",
        lua_version: "5.3",
        strict_nil: true,
        unknown_callable: false,
        unknown_indexable: false,
        checked_severities: CHECKED_SEVERITIES,
        status: CaseStatus::SharedSemantics,
    },
    CaseConfig {
        id: "legacy-reduction/string-assignment",
        origin_path: "src/test/resources/inspections/string_literals.lua:1,8",
        lua_version: "5.3",
        strict_nil: true,
        unknown_callable: false,
        unknown_indexable: false,
        checked_severities: CHECKED_SEVERITIES,
        status: CaseStatus::SharedSemantics,
    },
];

fn native_diagnostics(source: &str) -> Vec<DiagnosticMatch> {
    luanalysis::check(source)
        .diagnostics
        .iter()
        .map(|diagnostic| DiagnosticMatch {
            severity: Severity::Error,
            span: Span {
                start: diagnostic.span.start,
                end: diagnostic.span.end,
            },
            code: Some(diagnostic.code.to_owned()),
        })
        .collect()
}

#[test]
fn original_legacy_fixture_matches_native_error_and_corrected_source() {
    let case = CASES[0];
    case.require_supported().unwrap();
    let marked = include_str!("fixtures/local_def_assignment.lua");
    let fixture = import(marked)
        .unwrap()
        .with_origin(FixtureOrigin::legacy(case.origin_path));
    assert_eq!(
        fixture.source,
        "---@type number\nlocal aNumber = \"not a number\"\n"
    );
    assert_eq!(fixture.diagnostics.len(), 1);
    let expected = &fixture.diagnostics[0];
    assert_eq!(expected.span, Span { start: 32, end: 46 });
    assert_eq!(fixture.snippet(expected), "\"not a number\"");
    assert_eq!(fixture.origin.as_ref().unwrap().path, case.origin_path);
    let expected_native = [DiagnosticMatch {
        severity: expected.severity,
        span: expected.span,
        code: Some("TS2322".into()),
    }];
    let observed = native_diagnostics(&fixture.source);
    let comparison = compare_exact(&expected_native, &observed);
    assert!(comparison.is_match(), "{comparison:?}; {observed:?}");
    let corrected = fixture.source.replace("\"not a number\"", "42");
    let report = luanalysis::check(&corrected);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report.type_at(corrected.find("aNumber").unwrap()),
        Some("number")
    );
}

#[test]
fn full_legacy_lambda_params_matches_original_errors_and_corrected_bodies() {
    let case = CaseConfig {
        id: "legacy/lambda_params",
        origin_path: "src/test/resources/inspections/lambda_params.lua",
        lua_version: "5.3",
        strict_nil: true,
        unknown_callable: false,
        unknown_indexable: false,
        checked_severities: CHECKED_SEVERITIES,
        status: CaseStatus::SharedSemantics,
    };
    case.require_supported().unwrap();
    let marked = include_str!("fixtures/lambda_params.lua");
    let fixture = import(marked)
        .unwrap()
        .with_origin(FixtureOrigin::legacy(case.origin_path));
    assert_eq!(fixture.diagnostics.len(), 2);
    let expected: Vec<_> = fixture
        .diagnostics
        .iter()
        .map(|diagnostic| DiagnosticMatch {
            severity: diagnostic.severity,
            span: diagnostic.span,
            code: Some("TS2322".into()),
        })
        .collect();
    assert_eq!(
        expected[0].span,
        Span {
            start: 289,
            end: 294
        }
    );
    assert_eq!(
        expected[1].span,
        Span {
            start: 365,
            end: 366
        }
    );
    let observed = native_diagnostics(&fixture.source);
    let comparison = compare_exact(&expected, &observed);
    assert!(comparison.is_match(), "{comparison:?}; {observed:?}");
    let mut corrected = fixture.source.clone();
    for (diagnostic, replacement) in fixture.diagnostics.iter().zip(["42", "\"valid\""]).rev() {
        corrected.replace_range(diagnostic.span.start..diagnostic.span.end, replacement);
    }
    let report = luanalysis::check(&corrected);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn valid_primitive_reductions_have_no_unexpected_diagnostics_and_owned_types() {
    let inputs = [
        (
            include_str!("fixtures/number-assignment.lua"),
            "aNumber",
            "number",
        ),
        (
            include_str!("fixtures/boolean-assignment.lua"),
            "unknownBoolean",
            "boolean",
        ),
        (
            include_str!("fixtures/string-assignment.lua"),
            "str",
            "string",
        ),
    ];
    for (case, (marked, name, expected_type)) in CASES[1..].iter().zip(inputs) {
        case.require_supported().unwrap();
        let fixture = import(marked)
            .unwrap()
            .with_origin(FixtureOrigin::legacy(case.origin_path));
        assert!(fixture.diagnostics.is_empty());
        let report = luanalysis::check(&fixture.source);
        assert!(
            report.diagnostics.is_empty(),
            "{}: {:#?}",
            case.id,
            report.diagnostics
        );
        let declaration = fixture.source.find(&format!("local {name}")).unwrap() + "local ".len();
        assert_eq!(
            report.type_at(declaration),
            Some(expected_type),
            "{}",
            case.id
        );
    }
}

#[test]
fn full_legacy_trailing_type_reports_its_original_initializer_error_and_correction() {
    let case = CaseConfig {
        id: "legacy/trailing_type",
        origin_path: "src/test/resources/inspections/trailing_type.lua",
        lua_version: "5.3",
        strict_nil: true,
        unknown_callable: false,
        unknown_indexable: false,
        checked_severities: CHECKED_SEVERITIES,
        status: CaseStatus::SharedSemantics,
    };
    case.require_supported().unwrap();
    let fixture = import(include_str!("fixtures/trailing_type.lua"))
        .unwrap()
        .with_origin(FixtureOrigin::legacy(case.origin_path));
    assert_eq!(fixture.diagnostics.len(), 1);
    let expected = &fixture.diagnostics[0];
    assert_eq!(fixture.snippet(expected), "\"hello\"");
    let observed = native_diagnostics(&fixture.source);
    let comparison = compare_exact(
        &[DiagnosticMatch {
            severity: expected.severity,
            span: expected.span,
            code: Some("TS2322".into()),
        }],
        &observed,
    );
    assert!(comparison.is_match(), "{comparison:?}; {observed:?}");
    let mut corrected = fixture.source.clone();
    corrected.replace_range(expected.span.start..expected.span.end, "42");
    let report = luanalysis::check(&corrected);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn unsupported_feature_diagnostic_cannot_satisfy_a_legacy_type_error() {
    let expected = [DiagnosticMatch {
        severity: Severity::Error,
        span: Span { start: 0, end: 1 },
        code: Some("TS2322".into()),
    }];
    let unsupported = [DiagnosticMatch {
        severity: Severity::Error,
        span: Span { start: 0, end: 1 },
        code: Some("LUA2014".into()),
    }];
    let comparison = compare_exact(&expected, &unsupported);
    assert_eq!(comparison.missing, [0]);
    assert_eq!(comparison.unexpected, [0]);
}
