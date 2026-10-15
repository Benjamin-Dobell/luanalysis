use crate::*;

fn parsed(source: &str) -> File {
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    file
}

fn call_arguments(statement: &Stmt) -> &[Expr] {
    let StmtKind::Call(Expr {
        kind: ExprKind::Call { args, .. },
        ..
    }) = &statement.kind
    else {
        panic!("expected original call statement");
    };
    args
}

fn function(expression: &Expr) -> &Function {
    match &expression.kind {
        ExprKind::Function(function) => function,
        ExprKind::Parenthesized(inner) => function(inner),
        _ => panic!("expected original function expression"),
    }
}

#[test]
fn anonymous_call_arguments_retain_documentation_and_exact_utf8_crlf_spans() {
    let source = "invoke(\r\n---@param résumé number\r\n---@return number\r\nfunction(résumé) return résumé end,\r\nfunction(plain) return plain end)";
    let file = parsed(source);
    let arguments = call_arguments(&file.statements[0]);
    let callback = function(&arguments[0]);
    let parameter = &callback.annotations.params[0];
    assert_eq!(parameter.name.text, "résumé");
    assert_eq!(
        &source[parameter.name.span.start..parameter.name.span.end],
        "résumé"
    );
    assert_eq!(
        &source[parameter.ty.span.start..parameter.ty.span.end],
        "number"
    );
    assert!(callback.annotations.returns.is_some());
    assert!(source[callback.span.start..callback.span.end].starts_with("function(résumé)"));
    assert_eq!(
        function(&arguments[1]).annotations,
        FunctionAnnotations::default()
    );
}

#[test]
fn named_keyed_and_sequence_table_callbacks_own_only_their_documentation() {
    let source = "local callbacks={\n---@param class self\n---@param value number\n__call=function(class,value) return value end,\n---@param index number\n[\"key\"]=function(index) return index end,\n---@param item string\nfunction(item) return item end,\nplain=function(value) return value end}";
    let file = parsed(source);
    let StmtKind::Local { values, .. } = &file.statements[0].kind else {
        panic!()
    };
    let ExprKind::Table(fields) = &values[0].kind else {
        panic!()
    };
    assert_eq!(fields.len(), 4);
    assert!(matches!(&fields[0].kind, TableFieldKind::Named(name) if name.text == "__call"));
    assert!(matches!(&fields[1].kind, TableFieldKind::Keyed(_)));
    assert!(matches!(
        &fields[2].kind,
        TableFieldKind::Sequence { index: 1 }
    ));
    for (index, expected) in [(0, "class"), (1, "index"), (2, "item")] {
        let callback = function(&fields[index].value);
        assert_eq!(callback.annotations.params[0].name.text, expected);
        let annotation = &callback.annotations.params[0];
        assert_eq!(
            &source[annotation.name.span.start..annotation.name.span.end],
            expected
        );
    }
    assert_eq!(
        function(&fields[3].value).annotations,
        FunctionAnnotations::default()
    );
}

#[test]
fn field_and_value_documentation_merge_into_the_same_original_function() {
    let file = parsed(
        "local callbacks={\n---@generic T\n---@param value T\nidentity=\n---@return T\n(function(value) return value end)}",
    );
    let StmtKind::Local { values, .. } = &file.statements[0].kind else {
        panic!()
    };
    let ExprKind::Table(fields) = &values[0].kind else {
        panic!()
    };
    let callback = function(&fields[0].value);
    assert_eq!(callback.annotations.generics[0].name.text, "T");
    assert_eq!(callback.annotations.params[0].name.text, "value");
    assert!(callback.annotations.returns.is_some());
}

#[test]
fn nested_function_documentation_does_not_escape_to_siblings_or_statements() {
    let file = parsed(
        "---@param outer number\nlocal function owner(outer)\ninvoke(\n---@param inner string\nfunction(inner)\nreturn invoke(\n---@param nested boolean\nfunction(nested) return nested end)\nend, function(plain) end)\nend\nlocal function following(value) end",
    );
    let StmtKind::LocalFunction {
        function: owner, ..
    } = &file.statements[0].kind
    else {
        panic!()
    };
    assert_eq!(owner.annotations.params[0].name.text, "outer");
    let arguments = call_arguments(&owner.body[0]);
    let inner = function(&arguments[0]);
    assert_eq!(inner.annotations.params[0].name.text, "inner");
    let StmtKind::Return(values) = &inner.body[0].kind else {
        panic!()
    };
    let ExprKind::Call { args, .. } = &values[0].kind else {
        panic!()
    };
    assert_eq!(function(&args[0]).annotations.params[0].name.text, "nested");
    assert_eq!(
        function(&arguments[1]).annotations,
        FunctionAnnotations::default()
    );
    let StmtKind::LocalFunction {
        function: following,
        ..
    } = &file.statements[1].kind
    else {
        panic!()
    };
    assert_eq!(following.annotations, FunctionAnnotations::default());
}

#[test]
fn nonfunction_annotation_owners_are_rejected_without_leaking_to_next_callback() {
    for source in [
        "invoke(\n---@param value number\n42, function(value) return value end)",
        "invoke({\n---@param value number\nfield=42, next=function(value) return value end})",
    ] {
        let file = parse(source);
        assert_eq!(file.diagnostics.len(), 1, "{:#?}", file.diagnostics);
        assert!(file.diagnostics[0].message.contains("not attached"));
        let arguments = call_arguments(&file.statements[0]);
        let callback = match &arguments[0].kind {
            ExprKind::Table(fields) => function(&fields[1].value),
            _ => function(&arguments[1]),
        };
        assert_eq!(callback.annotations, FunctionAnnotations::default());
    }
}

#[test]
fn function_documentation_coexists_with_original_inline_type_assertion() {
    let source = "invoke(\n---@param value number\n--[[---@type fun(value:number):number]] function(value) return value end)";
    let file = parsed(source);
    let arguments = call_arguments(&file.statements[0]);
    assert_eq!(
        function(&arguments[0]).annotations.params[0].name.text,
        "value"
    );
    assert_eq!(file.expression_assertions.len(), 1);
    assert_eq!(file.expression_assertions[0].expression, arguments[0].span);
}

#[test]
fn duplicate_field_and_value_annotations_report_the_original_second_tag() {
    let source = "local callbacks={\n---@param value number\nfield=\n---@param value string\nfunction(value) return value end}";
    let file = parse(source);
    assert_eq!(file.diagnostics.len(), 1, "{:#?}", file.diagnostics);
    assert!(file.diagnostics[0].message.contains("multiple @param"));
    assert_eq!(
        &source[file.diagnostics[0].span.start..file.diagnostics[0].span.end],
        "@param value string"
    );
}
