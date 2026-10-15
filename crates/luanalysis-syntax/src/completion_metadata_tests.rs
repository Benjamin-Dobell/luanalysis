use crate::*;

#[test]
fn completion_text_retains_original_unicode_spans_without_becoming_names() {
    let source = "---@param é number | `Logger.ERROR` | `Factory(\"@name\")` @prose\r\nlocal function take(é) end";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!()
    };
    let TypeExprKind::Union(parts) = &function.annotations.params[0].ty.kind else {
        panic!()
    };
    assert_eq!(parts.len(), 3);
    assert_eq!(
        parts[1].kind,
        TypeExprKind::CompletionHint("Logger.ERROR".into())
    );
    assert_eq!(
        &source[parts[1].span.start..parts[1].span.end],
        "`Logger.ERROR`"
    );
    assert_eq!(
        parts[2].kind,
        TypeExprKind::CompletionHint("Factory(\"@name\")".into())
    );
}

#[test]
fn escaped_backticks_and_literal_backticks_remain_distinct() {
    let file = parse("---@alias Choice number|`emit(\\`value\\`)`\n---@alias Literal \"`value`\"");
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let NamedTypeDeclarationKind::Alias { target } = &file.type_declarations[0].kind else {
        panic!()
    };
    let TypeExprKind::Union(parts) = &target.kind else {
        panic!()
    };
    assert_eq!(
        parts[1].kind,
        TypeExprKind::CompletionHint("emit(\\`value\\`)".into())
    );
    let NamedTypeDeclarationKind::Alias { target } = &file.type_declarations[1].kind else {
        panic!()
    };
    assert!(
        matches!(&target.kind, TypeExprKind::Literal(TypeLiteral::String(text)) if text==b"`value`")
    );
}

#[test]
fn hints_require_an_independent_semantic_type_in_each_slot() {
    for source in [
        "---@alias Only `Choice.OK`",
        "---@type (`First`|`Second`)\nlocal value",
        "---@alias Record {value:`Choice.OK`}",
        "---@alias Callback fun(value:`Choice.OK`):number",
        "---@return number,`Choice.OK`\nlocal function pair() end",
        "---@generic T:`Choice.OK`\nlocal function take() end",
    ] {
        let file = parse(source);
        assert!(
            file.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("accompanying value type")),
            "{source}\n{:#?}",
            file.diagnostics
        );
    }
    let file = parse(
        "---@alias Mixed number|(`First`|`Second`)\n---@alias Callback fun(value:number|`Choice.OK`):string|`Choice.NAME`",
    );
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
}

#[test]
fn invalid_hints_and_runtime_backticks_cannot_pass_as_valid_lua() {
    for source in [
        "---@type number|``\nlocal value=42",
        "---@type number|`unfinished\nlocal recovery=42",
        "local value=`runtime`",
    ] {
        assert!(!parse(source).diagnostics.is_empty(), "{source}");
    }
    let lexed = lex("`runtime`");
    assert!(!lexed.diagnostics.is_empty());
    assert!(
        !lexed
            .tokens
            .iter()
            .any(|token| matches!(token.kind, TokenKind::CompletionHint(_)))
    );
}
