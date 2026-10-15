use luanalysis_syntax::{StmtKind, parse};

#[test]
fn original_return_lists_own_written_contracts_without_rewriting_expressions() {
    let source = "-- 前置\r\nlocal function values(...)\r\n---@type number,string...\r\nreturn 1,...\r\nend\r\n---@type nil\r\nreturn";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    assert_eq!(file.return_annotations.len(), 2);
    for annotation in &file.return_annotations {
        assert!(source[annotation.statement.start..annotation.statement.end].starts_with("return"));
        assert!(
            source[annotation.annotation.span.start..annotation.annotation.span.end]
                .starts_with("@type")
        );
    }
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!("function")
    };
    let StmtKind::Return(values) = &function.body[0].kind else {
        panic!("return")
    };
    assert_eq!(values.len(), 2);
    assert!(function.annotations.type_annotation.is_none());
    assert!(
        file.return_annotations[0]
            .annotation
            .packs
            .as_ref()
            .unwrap()
            .alternatives[0]
            .variadic
            .is_some()
    );
}
#[test]
fn returned_function_docs_stay_function_owned_and_list_contracts_stay_statement_owned() {
    let source = "local function builder()\n---@type fun<T>(value:T):T\nreturn function(value) return value end\nend\nlocal function pair()\n---@type function,number\nreturn function() end,1\nend";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    assert_eq!(file.return_annotations.len(), 1);
    assert_eq!(file.return_annotations[0].annotation.types.len(), 2);
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!("function")
    };
    let StmtKind::Return(values) = &function.body[0].kind else {
        panic!("return")
    };
    let luanalysis_syntax::ExprKind::Function(function) = &values[0].kind else {
        panic!("closure")
    };
    assert!(function.annotations.type_annotation.is_some());
}
#[test]
fn malformed_return_contracts_preserve_error_ranges_and_following_declarations() {
    let source = "local function invalid()\n---@type number|\nreturn 1\nend\nlocal recovered=42";
    let file = parse(source);
    assert!(!file.diagnostics.is_empty());
    assert!(
        file.diagnostics
            .iter()
            .all(|d| d.span.start >= source.find("number|").unwrap()
                && d.span.end <= source.find("return 1").unwrap())
    );
    assert!(
        matches!(&file.statements[1].kind, StmtKind::Local { names, .. } if names[0].text=="recovered")
    );
}
