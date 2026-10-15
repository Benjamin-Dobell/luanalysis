use luanalysis_syntax::{StmtKind, TypeExprKind, parse};

#[test]
fn ordinary_function_overloads_have_one_canonical_head_each_in_source_order() {
    let source = "-- α\r\n---@overload fun(value: string): string\r\n---@overload fun(value: number): number\r\n---@param value string|number\r\n---@return string|number\r\nlocal function echo(value) return value end";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    assert_eq!(file.overload_annotations.len(), 2);
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!("function")
    };
    assert_eq!(function.annotations.overloads.len(), 2);
    for (index, reference) in function.annotations.overloads.iter().enumerate() {
        let head = &file.overload_annotations[reference.id.0 as usize];
        assert_eq!(head.id.0 as usize, index);
        assert_eq!(&source[head.tag_span.start..head.tag_span.end], "@overload");
        assert!(
            source[head.signature.span.start..head.signature.span.end].starts_with("fun(value:")
        );
        assert_eq!(head.span, reference.span);
        assert!(head.named_candidates.is_empty());
    }
    assert!(
        matches!(&file.overload_annotations[0].signature.params[0].ty.as_ref().unwrap().kind, TypeExprKind::Named(name) if name=="string")
    );
}

#[test]
fn callable_class_candidates_are_neutral_and_do_not_depend_on_tag_order() {
    for group in [
        "---@class Callable\n---@overload fun(value:number):number",
        "---@overload fun(value:number):number\n---@class Callable",
    ] {
        let source = format!("{group}\nlocal Callable={{}}");
        let file = parse(&source);
        assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
        assert_eq!(file.overload_annotations.len(), 1);
        let head = &file.overload_annotations[0];
        assert_eq!(head.named_candidates, vec![file.type_declarations[0].id]);
        assert_eq!(head.group_span, file.type_declarations[0].group_span);
    }
    let ambiguous =
        parse("---@class First\n---@overload fun():number\n---@class Second\nlocal value={}");
    assert!(
        ambiguous.diagnostics.is_empty(),
        "{:?}",
        ambiguous.diagnostics
    );
    assert_eq!(ambiguous.overload_annotations[0].named_candidates.len(), 2);
}

#[test]
fn each_overload_retains_signature_local_generics_and_correlated_return_pack_indices() {
    let source = "---@overload fun<R>(callback:fun(value:R):R, timeout?:nil|number):false|(true,R)\n---@overload fun<R:number>(callback:fun(value:R):R):R,R\n---@generic R:string\n---@param callback any\n---@return any\nlocal function apply(callback) return callback end";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let first = &file.overload_annotations[0].signature;
    let second = &file.overload_annotations[1].signature;
    assert_eq!(first.generics[0].name.text, "R");
    assert_eq!(second.generics[0].name.text, "R");
    assert_ne!(first.generics[0].name.span, second.generics[0].name.span);
    let rows = &first.packs.as_ref().unwrap().alternatives;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].fixed.len(), 1);
    assert_eq!(rows[1].fixed.len(), 2);
    assert_eq!(first.returns.len(), 3);
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!("body")
    };
    assert!(
        matches!(&function.annotations.generics[0].constraint.as_ref().unwrap().kind, TypeExprKind::Named(name) if name=="string")
    );
}

#[test]
fn direct_and_parenthesized_literal_parameter_syntax_remains_distinct() {
    let file = parse(
        "---@overload fun(value:'x'):number\n---@overload fun(value:('x')):string\n---@overload fun(value:'x'|'y'):boolean\n---@param value string\nlocal function choose(value) end",
    );
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let kinds: Vec<_> = file
        .overload_annotations
        .iter()
        .map(|head| &head.signature.params[0].ty.as_ref().unwrap().kind)
        .collect();
    assert!(matches!(kinds[0], TypeExprKind::Literal(_)));
    assert!(matches!(kinds[1], TypeExprKind::Parenthesized(_)));
    assert!(matches!(kinds[2], TypeExprKind::Union(_)));
}

#[test]
fn anonymous_field_and_initializer_heads_share_one_original_site_in_source_order() {
    let source="local callbacks={\n---@overload fun(value:number):\"first\"\n---@param value number\n---@return \"first\"|\"second\"\nchoose=\n---@overload fun(value:number):\"second\"\nfunction(value)return \"first\" end\n}";
    let file=luanalysis_syntax::parse(source);
    assert!(file.diagnostics.is_empty(),"{:#?}",file.diagnostics);
    assert_eq!(file.overload_annotations.len(),2);
    let luanalysis_syntax::StmtKind::Local{values,..}=&file.statements[0].kind else {panic!("local")};
    let luanalysis_syntax::ExprKind::Table(fields)=&values[0].kind else {panic!("table")};
    let luanalysis_syntax::ExprKind::Function(function)=&fields[0].value.kind else {panic!("function")};
    assert_eq!(function.annotations.overloads.iter().map(|head|head.id.0).collect::<Vec<_>>(),vec![0,1]);
    let first=&file.overload_annotations[0];let second=&file.overload_annotations[1];
    assert_eq!(first.group_span,second.group_span);
    assert!(first.span.start<second.span.start);
    assert_eq!(&source[function.params[0].span.start..function.params[0].span.end],"value");
}
