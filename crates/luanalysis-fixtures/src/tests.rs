use super::*;

#[test]
fn plain_lua_angles_and_unknown_tags_are_preserved() {
    let source =
        "local x<const> = a < b\n---@type Box<T>\nlocal close<close>\nlocal text = '<unknown>'\n";
    let fixture = import(source).unwrap();
    assert_eq!(fixture.source, source);
    assert!(fixture.diagnostics.is_empty());
    for offset in 0..=source.len() {
        assert_eq!(fixture.source_map.clean_offset(offset), Some(offset));
        assert_eq!(fixture.source_map.original_offset(offset), Some(offset));
    }
}

#[test]
fn nested_unicode_crlf_and_original_clean_byte_mapping() {
    let marked = "<error descr='outer &lt;T&gt;'>α<warning descr=\"quoted: \\\"x\\\"\">😀</warning>\r\n</error>";
    let fixture = import(marked).unwrap();
    assert_eq!(fixture.source, "α😀\r\n");
    assert_eq!(fixture.diagnostics.len(), 2);
    let outer = &fixture.diagnostics[0];
    let inner = &fixture.diagnostics[1];
    assert_eq!(outer.description.as_deref(), Some("outer <T>"));
    assert_eq!(inner.description.as_deref(), Some("quoted: \"x\""));
    assert_eq!(outer.span, Span { start: 0, end: 8 });
    assert_eq!(inner.span, Span { start: 2, end: 6 });
    assert_eq!(fixture.snippet(inner), "😀");
    let alpha = marked.find('α').unwrap();
    let emoji = marked.find('😀').unwrap();
    for byte in 0..2 {
        assert_eq!(fixture.source_map.clean_offset(alpha + byte), Some(byte));
        assert_eq!(fixture.source_map.original_offset(byte), Some(alpha + byte));
    }
    for byte in 0..4 {
        assert_eq!(
            fixture.source_map.clean_offset(emoji + byte),
            Some(2 + byte)
        );
    }
    assert_eq!(
        fixture.source_map.original_span(inner.span),
        Some(Span {
            start: emoji,
            end: emoji + 4
        })
    );
    assert_eq!(fixture.source_map.original_offset(8), Some(marked.len()));
    assert_eq!(fixture.source_map.clean_offset(marked.len()), Some(8));
}

#[test]
fn identical_nested_ranges_and_adjacent_markers_are_distinct() {
    let fixture =
        import("<error><error descr='inner'>a</error></error><warning>b</warning><info></info>")
            .unwrap();
    assert_eq!(fixture.source, "ab");
    assert_eq!(fixture.diagnostics.len(), 4);
    assert_eq!(fixture.diagnostics[0].span, Span { start: 0, end: 1 });
    assert_eq!(fixture.diagnostics[1].span, fixture.diagnostics[0].span);
    assert_eq!(fixture.diagnostics[0].description, None);
    assert_eq!(fixture.diagnostics[2].span, Span { start: 1, end: 2 });
    assert_eq!(fixture.diagnostics[3].span, Span { start: 2, end: 2 });
}

#[test]
fn description_quotes_gt_entities_and_unknown_backslash_escapes() {
    let marked = "<weak_warning descr='it\\'s > &amp; &#34; &#x1F600; \\n'>x</weak_warning>";
    let fixture = import(marked).unwrap();
    assert_eq!(fixture.source, "x");
    assert_eq!(
        fixture.diagnostics[0].description.as_deref(),
        Some("it's > & \" 😀 \\n")
    );
    assert_eq!(fixture.diagnostics[0].severity, Severity::WeakWarning);
}

#[test]
fn backslash_parity_does_not_close_an_escaped_quote() {
    let marked = r#"<error descr="slash \\\" and quote">x</error>"#;
    let fixture = import(marked).unwrap();
    assert_eq!(
        fixture.diagnostics[0].description.as_deref(),
        Some("slash \\\" and quote")
    );
}

#[test]
fn description_and_source_newlines_are_kept() {
    let fixture = import("<error\n descr=\"first\nsecond\">line1\r\nline2</error>").unwrap();
    assert_eq!(fixture.source, "line1\r\nline2");
    assert_eq!(
        fixture.diagnostics[0].description.as_deref(),
        Some("first\nsecond")
    );
    assert_eq!(fixture.diagnostics[0].span, Span { start: 0, end: 12 });
}

#[test]
fn malformed_markers_are_explicit_errors() {
    for marked in [
        "<error>",
        "</error>",
        "<error><warning>x</error></warning>",
        "<error",
        "<error descr='unfinished>",
        "<error descr bare>x</error>",
        "<error descr=bare>x</error>",
        "<error descr='a' descr='b'>x</error>",
        "<error extra='x'>x</error>",
        "<error/>",
        "<error>x</error descr='x'>",
        "<error descr='&unknown;'>x</error>",
        "<error descr='&#xD800;'>x</error>",
        "<error descr='&#xZZ;'>x</error>",
        "<error descr='&#1114112;'>x</error>",
    ] {
        assert!(import(marked).is_err(), "{marked}");
    }
}

#[test]
fn source_mapping_rejects_out_of_range_and_reversed_spans() {
    let fixture = import("<error>x</error>").unwrap();
    assert_eq!(fixture.source_map.clean_offset(0), Some(0));
    assert_eq!(
        fixture.source_map.original_span(Span { start: 1, end: 0 }),
        None
    );
    assert_eq!(
        fixture.source_map.original_span(Span { start: 0, end: 2 }),
        None
    );
    assert_eq!(fixture.source_map.original_offset(2), None);
}

#[test]
fn malformed_description_reports_original_offset_after_escape_removal() {
    let marked = "<error descr=\"α \\\"quoted\\\" &unknown;\">x</error>";
    let error = import(marked).unwrap_err();
    assert_eq!(error.offset, marked.find("&unknown;").unwrap());
}

#[test]
fn json_artifact_escapes_controls_quotes_and_preserves_origin() {
    let fixture = import("<error descr='x &quot; y'>\"α\"\r\n\u{1}</error>")
        .unwrap()
        .with_origin(FixtureOrigin::legacy(
            "src/test/resources/inspections/example.lua",
        ));
    let json = fixture.to_json();
    assert!(json.contains("\"format\":1"));
    assert!(json.contains(LEGACY_REVISION));
    assert!(json.contains("\\\"α\\\"\\r\\n\\u0001"));
    assert!(json.contains("\"description\":\"x \\\" y\""));
}

fn matched(span: Span, code: Option<&str>) -> DiagnosticMatch {
    DiagnosticMatch {
        severity: Severity::Error,
        span,
        code: code.map(str::to_owned),
    }
}

#[test]
fn exact_comparison_rejects_containment_wrong_code_and_missing_multiplicity() {
    let span = Span { start: 2, end: 5 };
    let expected = [matched(span, Some("TS2322")), matched(span, Some("TS2322"))];
    assert!(compare_exact(&expected, &expected).is_match());
    assert_eq!(compare_exact(&expected, &expected[..1]).missing, [1]);
    let aggregate = [matched(Span { start: 0, end: 9 }, Some("TS2322"))];
    assert_eq!(compare_exact(&expected[..1], &aggregate).unexpected, [0]);
    let wrong_code = [matched(span, Some("LUA2001"))];
    assert!(!compare_exact(&expected[..1], &wrong_code).is_match());
}

#[test]
fn code_specific_expectations_match_before_wildcards() {
    let span = Span { start: 0, end: 1 };
    let expected = [matched(span, None), matched(span, Some("TS2322"))];
    let actual = [matched(span, Some("TS2322")), matched(span, Some("TS2355"))];
    assert!(compare_exact(&expected, &actual).is_match());
}

#[test]
fn pending_case_is_not_a_successful_checking_result() {
    let config = CaseConfig {
        id: "legacy/class",
        origin_path: "src/test/resources/inspections/class.lua",
        lua_version: "5.3",
        strict_nil: true,
        unknown_callable: false,
        unknown_indexable: false,
        checked_severities: &[Severity::Error, Severity::Warning],
        status: CaseStatus::Pending {
            feature: "nominal class declarations",
        },
    };
    assert_eq!(
        config.require_supported(),
        Err("nominal class declarations")
    );
}
