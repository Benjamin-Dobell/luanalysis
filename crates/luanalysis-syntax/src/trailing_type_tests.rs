use crate::*;

fn parsed(source: &str) -> File {
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    file
}

#[test]
fn trailing_type_belongs_to_local_and_preserves_original_expression_span() {
    let source = "local a ---@type number\nlocal b=\"hello\" ---@type number";
    let file = parsed(source);
    assert!(file.expression_assertions.is_empty());
    for statement in &file.statements {
        let StmtKind::Local {
            annotation: Some(annotation),
            ..
        } = &statement.kind
        else {
            panic!("{statement:#?}")
        };
        assert!(matches!(&annotation.types[0].kind, TypeExprKind::Named(name) if name=="number"));
        assert!(annotation.span.start >= statement.span.end);
    }
    let StmtKind::Local { values, .. } = &file.statements[1].kind else {
        panic!()
    };
    assert_eq!(
        &source[values[0].span.start..values[0].span.end],
        "\"hello\""
    );
}

#[test]
fn trailing_function_type_retains_generic_scope_and_unicode_crlf_locations() {
    let source = "local é=(function(value) return value end) --[=[---@type fun<T>(value:T):T]=]\r\nlocal next=42";
    let file = parsed(source);
    let StmtKind::Local {
        values,
        annotation: Some(annotation),
        ..
    } = &file.statements[0].kind
    else {
        panic!()
    };
    let TypeExprKind::Function(signature) = &annotation.types[0].kind else {
        panic!()
    };
    assert_eq!(signature.generics[0].name.text, "T");
    assert_eq!(
        &source[signature.params[0].ty.as_ref().unwrap().span.start
            ..signature.params[0].ty.as_ref().unwrap().span.end],
        "T"
    );
    let ExprKind::Parenthesized(inner) = &values[0].kind else {
        panic!()
    };
    let ExprKind::Function(function) = &inner.kind else {
        panic!()
    };
    assert_eq!(
        function.annotations.type_annotation.as_ref(),
        Some(annotation)
    );
    let StmtKind::Local { annotation, .. } = &file.statements[1].kind else {
        panic!()
    };
    assert!(annotation.is_none());
}

#[test]
fn next_line_documentation_keeps_its_own_local_owner() {
    let file = parsed("local first=42\r\n---@type string\r\nlocal second=\"word\"");
    let StmtKind::Local { annotation, .. } = &file.statements[0].kind else {
        panic!()
    };
    assert!(annotation.is_none());
    let StmtKind::Local {
        annotation: Some(annotation),
        ..
    } = &file.statements[1].kind
    else {
        panic!()
    };
    assert!(matches!(&annotation.types[0].kind, TypeExprKind::Named(name) if name=="string"));
}

#[test]
fn duplicate_leading_and_trailing_type_reports_the_later_original_tag() {
    let source = "---@type number\nlocal value=42 ---@type string";
    let file = parse(source);
    assert_eq!(file.diagnostics.len(), 1, "{:#?}", file.diagnostics);
    let diagnostic = &file.diagnostics[0];
    assert!(
        diagnostic.span.start >= source.find("42").unwrap(),
        "{diagnostic:?}"
    );
    assert!(
        diagnostic.message.contains("multiple @type"),
        "{diagnostic:?}"
    );
}
