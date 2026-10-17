use crate::{StmtKind, parse};

#[test]
fn see_links_retain_original_utf8_crlf_ranges_without_type_dependencies() {
    let source = "-- 前置\r\n---@see MissingType#method @ descriptive text\r\n---@see table\r\n---@see 类型#成员\r\n---@param value number\r\nlocal function checked(value) end\r\nchecked(1)";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    assert_eq!(file.documentation_references.len(), 3);
    assert!(file.type_declarations.is_empty());
    let first = &file.documentation_references[0];
    assert_eq!(first.target.text, "MissingType");
    assert_eq!(first.member.as_ref().unwrap().text, "method");
    for reference in &file.documentation_references {
        assert_eq!(
            &source[reference.target.span.start..reference.target.span.end],
            reference.target.text
        );
        if let Some(member) = &reference.member {
            assert_eq!(&source[member.span.start..member.span.end], member.text);
        }
        assert!(source[reference.span.start..reference.span.end].starts_with("@see "));
    }
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!("function")
    };
    assert_eq!(function.annotations.params.len(), 1);
}

#[test]
fn see_metadata_is_collected_once_in_statement_expression_and_table_documentation() {
    let source = "---@see First\nlocal first=1\nlocal callback=--[[---@see Callback#call\n@param value number\n@return number\n]] function(value) return value end\nlocal object={\n---@see FieldOwner#field\n---@param value string\n---@return string\nfield=function(value) return value end\n}\n---@see Last";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let targets = file
        .documentation_references
        .iter()
        .map(|reference| reference.target.text.as_str())
        .collect::<Vec<_>>();
    assert_eq!(targets, ["First", "Callback", "FieldOwner", "Last"]);
    assert_eq!(file.statements.len(), 3);
}

#[test]
fn malformed_see_links_report_original_syntax_and_recover_at_the_next_tag() {
    for malformed in [
        "",
        "Broken#",
        "Broken<number>",
    ] {
        let source =
            format!("---@see {malformed}\n---@type number\nlocal value=1\n---@see Good#member");
        let file = parse(&source);
        assert!(!file.diagnostics.is_empty(), "accepted {malformed:?}");
        assert!(
            file.diagnostics
                .iter()
                .all(|diagnostic| diagnostic.span.start < source.find("---@type").unwrap()),
            "{:#?}",
            file.diagnostics
        );
        assert_eq!(
            file.documentation_references.last().unwrap().target.text,
            "Good"
        );
        assert!(matches!(
            &file.statements[0].kind,
            StmtKind::Local {
                annotation: Some(_),
                ..
            }
        ));
    }
}

#[test]
fn trailing_type_and_see_metadata_retain_their_distinct_original_roles() {
    let source = "local value=1 --[[---@type number\n@see Unresolved#member\n]]";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    assert_eq!(file.documentation_references.len(), 1);
    assert_eq!(file.documentation_references[0].target.text, "Unresolved");
    assert!(matches!(
        &file.statements[0].kind,
        StmtKind::Local {
            annotation: Some(_),
            ..
        }
    ));
}
