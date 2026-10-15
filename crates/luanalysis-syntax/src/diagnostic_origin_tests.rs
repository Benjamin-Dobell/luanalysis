use crate::{
    DiagnosticOrigin, ExprKind, NamedTypeDeclarationKind, StmtKind, TypeExprKind, lex, parse,
};

fn annotation_errors(source: &str) -> crate::File {
    let file = parse(source);
    assert!(!file.diagnostics.is_empty(), "{source}");
    assert!(
        file.diagnostics
            .iter()
            .all(|diagnostic| diagnostic.origin == DiagnosticOrigin::Annotation),
        "{source}\n{:#?}",
        file.diagnostics
    );
    assert!(!file.has_lua_syntax_errors(), "{source}");
    file
}

#[test]
fn incomplete_field_keeps_its_error_node_and_original_declarations_on_both_sides() {
    let source = "---@alias Before number\n---@class Broken\n---@field missing nil |\nlocal object={}\n---@alias After string\nlocal after=42\n";
    let file = annotation_errors(source);
    assert_eq!(file.type_declarations.len(), 3);
    assert_eq!(file.type_declarations[0].name.text, "Before");
    assert_eq!(file.type_declarations[2].name.text, "After");
    let NamedTypeDeclarationKind::Record { fields, .. } = &file.type_declarations[1].kind else {
        panic!("original class retained");
    };
    assert_eq!(fields.len(), 1);
    let TypeExprKind::Union(parts) = &fields[0].ty.kind else {
        panic!("original incomplete union retained");
    };
    assert_eq!(parts.len(), 2);
    assert!(matches!(parts[1].kind, TypeExprKind::Error));
    assert_eq!(file.statements.len(), 2);
    let StmtKind::Local { names, values, .. } = &file.statements[1].kind else {
        panic!("following local retained");
    };
    assert_eq!(names[0].text, "after");
    assert!(matches!(&values[0].kind, ExprKind::Number(value) if value == "42"));
    assert_eq!(
        &source[file.type_declarations[1].name.span.start..file.type_declarations[1].name.span.end],
        "Broken"
    );
}

#[test]
fn incomplete_parameter_keeps_written_name_type_error_function_and_adjacent_statements() {
    let source =
        "local before=1\n---@param v\nlocal function take(v) return v end\nlocal after=2\n";
    let file = annotation_errors(source);
    assert_eq!(file.statements.len(), 3);
    let StmtKind::LocalFunction { function, .. } = &file.statements[1].kind else {
        panic!("original function retained");
    };
    assert_eq!(function.annotations.params.len(), 1);
    let parameter = &function.annotations.params[0];
    assert_eq!(parameter.name.text, "v");
    assert!(matches!(parameter.ty.kind, TypeExprKind::Error));
    assert_eq!(
        &source[parameter.span.start..parameter.span.end],
        "@param v"
    );
    assert_eq!(function.body.len(), 1);
    assert!(matches!(function.body[0].kind, StmtKind::Return(_)));
}

#[test]
fn mixed_lua_and_annotation_failures_keep_independent_origins() {
    let file = parse("---@param v\nlocal function take(v) return v end\nlocal = 42\n");
    assert!(file.has_lua_syntax_errors());
    assert!(
        file.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.origin == DiagnosticOrigin::Annotation),
        "{:#?}",
        file.diagnostics
    );
    assert!(
        file.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.origin == DiagnosticOrigin::LuaSyntax),
        "{:#?}",
        file.diagnostics
    );
}

#[test]
fn annotation_lexing_errors_remain_annotation_failures_at_original_source_offsets() {
    for source in [
        "-- λ😀\r\n---@type number|``\r\nlocal value=42\r\n",
        "---@type number|`unfinished\nlocal value=42\n",
        "---@type 'unterminated\nlocal value=42\n",
        "---@type number|$\nlocal value=42\n",
    ] {
        let file = annotation_errors(source);
        assert_eq!(file.statements.len(), 1);
        assert!(file.diagnostics.iter().all(|diagnostic| {
            diagnostic.span.start <= diagnostic.span.end
                && diagnostic.span.end <= source.len()
                && source.is_char_boundary(diagnostic.span.start)
                && source.is_char_boundary(diagnostic.span.end)
        }));
    }
    let source = "---@type number|``\nlocal value=42\n";
    let file = annotation_errors(source);
    let empty_hint = file
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("empty"))
        .expect("original empty-hint diagnostic");
    assert_eq!(&source[empty_hint.span.start..empty_hint.span.end], "``");
}

#[test]
fn shared_lexer_uses_the_grammar_mode_for_backticks_and_string_errors() {
    for source in ["``", "`unfinished", "'unterminated", "$"] {
        let annotation = crate::lexer::lex_annotation(source);
        assert!(!annotation.diagnostics.is_empty(), "{source}");
        assert!(
            annotation
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.origin == DiagnosticOrigin::Annotation)
        );
        let lua = lex(source);
        assert!(!lua.diagnostics.is_empty(), "{source}");
        assert!(
            lua.diagnostics
                .iter()
                .all(|diagnostic| diagnostic.origin == DiagnosticOrigin::LuaSyntax)
        );
    }
    let valid_hint = crate::lexer::lex_annotation("`Choice.OK`");
    assert!(valid_hint.diagnostics.is_empty());
    assert!(
        lex("`Choice.OK`")
            .diagnostics
            .iter()
            .all(|diagnostic| { diagnostic.origin == DiagnosticOrigin::LuaSyntax })
    );
}

#[test]
fn metadata_validation_and_attachment_failures_have_annotation_origin() {
    for source in [
        "---@field orphan number\n",
        "---@param value number\n",
        "---@not number\nlocal value=42\n",
        "---@overload number\n",
        "---@unsupported number\nlocal value=42\n",
        "---@alias Hint `Choice.OK`\n",
        "---@param value number\n---@param value string\nlocal function take(value) end\n",
    ] {
        annotation_errors(source);
    }
}

#[test]
fn lua_grammar_lexing_and_control_diagnostics_have_lua_syntax_origin() {
    for source in [
        "local = 42",
        "local value='unfinished",
        "local value=`runtime`",
        "break",
        "local function take() return ... end",
        "if true local value=42 end",
    ] {
        let file = parse(source);
        assert!(file.has_lua_syntax_errors(), "{source}");
        assert!(
            file.diagnostics
                .iter()
                .all(|diagnostic| diagnostic.origin == DiagnosticOrigin::LuaSyntax),
            "{source}\n{:#?}",
            file.diagnostics
        );
    }
    let valid = parse("---@type number\nlocal value=42\n");
    assert!(valid.diagnostics.is_empty());
    assert!(!valid.has_lua_syntax_errors());
}
