use crate::*;

fn parsed(source: &str) -> File {
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    file
}

#[test]
fn generic_for_inline_assertion_targets_the_complete_iterator_factory_call() {
    let source = "for item in --[[---@type fun(): string]] string.gmatch(value, '([^ ]+)') do end";
    let file = parsed(source);
    let assertion = &file.expression_assertions[0];
    assert_eq!(&source[assertion.expression.start..assertion.expression.end], "string.gmatch(value, '([^ ]+)')");
    let StmtKind::GenericFor { values, .. } = &file.statements[0].kind else { panic!() };
    assert!(matches!(values[0].kind, ExprKind::Call { .. }));
}

#[test]
fn inline_assertions_keep_original_call_and_lvalue_syntax() {
    let source = "local a,b=--[[---@type number,string]] pair()\n;(--[[---@not nil]] object).field=--[[---@type number]] value";
    let file = parsed(source);
    assert_eq!(file.expression_assertions.len(), 3);
    let assertion = &file.expression_assertions[0];
    assert_eq!(assertion.kind, ExpressionAssertionKind::Type);
    assert_eq!(assertion.annotation.types.len(), 2);
    assert_eq!(
        &source[assertion.expression.start..assertion.expression.end],
        "pair()"
    );
    let StmtKind::Local { values, .. } = &file.statements[0].kind else {
        panic!()
    };
    assert!(matches!(values[0].kind, ExprKind::Call { .. }));
    let StmtKind::Assign { targets, .. } = &file.statements[1].kind else {
        panic!()
    };
    assert!(matches!(targets[0].kind, ExprKind::Field { .. }));
}

#[test]
fn line_assertions_nested_arguments_and_variadic_not_keep_byte_spans() {
    let source = "local é=invoke(\r\n---@type number @cast documentation\r\nvalue, --[=[---@not nil,string...]=] other())";
    let file = parsed(source);
    assert_eq!(file.expression_assertions.len(), 2);
    assert_eq!(
        &source[file.expression_assertions[0].expression.start
            ..file.expression_assertions[0].expression.end],
        "value"
    );
    let exclusion = &file.expression_assertions[1];
    assert_eq!(exclusion.kind, ExpressionAssertionKind::Not);
    assert!(exclusion.annotation.packs.is_some());
    assert_eq!(
        &source[exclusion.annotation.types[0].span.start..exclusion.annotation.types[0].span.end],
        "nil"
    );
}

#[test]
fn inline_assertions_do_not_turn_statement_annotations_into_casts() {
    let file =
        parsed("--[[---@type number]] local value=42\n---@type string\nlocal text=\"value\"");
    assert!(file.expression_assertions.is_empty());
    assert!(matches!(
        &file.statements[0].kind,
        StmtKind::Local {
            annotation: Some(_),
            ..
        }
    ));
    let file = parse("---@not nil\nlocal value=42");
    assert!(
        file.diagnostics
            .iter()
            .any(|error| error.message.contains("must annotate an expression"))
    );
}

#[test]
fn assertion_operand_precedence_and_parentheses_remain_lua() {
    let source = "local value=--[[---@type number]] x+f()\nlocal one=(--[[---@type number,string]] f())\nlocal key=tab[--[[---@type number]] other]";
    let file = parsed(source);
    assert_eq!(file.expression_assertions.len(), 3);
    // As in v1, the cast types the operand after it.
    assert_eq!(
        &source[file.expression_assertions[0].expression.start
            ..file.expression_assertions[0].expression.end],
        "x"
    );
    let StmtKind::Local { values, .. } = &file.statements[1].kind else {
        panic!()
    };
    assert!(matches!(values[0].kind, ExprKind::Parenthesized(_)));
}
