use crate::*;

#[test]
fn bare_fun_and_explicit_headers_retain_distinct_original_byte_spans() {
    let source = "---@type fun\r\nlocal é\r\n---@type fun()\r\nlocal empty\r\n---@type fun: number\r\nlocal typed";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    for (index, expected) in [(0, None), (1, Some("()")), (2, None)] {
        let StmtKind::Local {
            annotation: Some(annotation),
            ..
        } = &file.statements[index].kind
        else {
            panic!()
        };
        let TypeExprKind::Function(function) = &annotation.types[0].kind else {
            panic!()
        };
        assert_eq!(
            function
                .parameters_span
                .map(|span| &source[span.start..span.end]),
            expected
        );
        assert_eq!(function.returns.len(), usize::from(index == 2));
    }
}
