use luanalysis_syntax::{StmtKind, TypeExpr, TypeExprKind, parse, type_names};

fn predicate(ty: &TypeExpr) -> (&luanalysis_syntax::Name, &TypeExpr) {
    let TypeExprKind::TypePredicate { parameter, ty } = &ty.kind else {
        panic!("expected type predicate: {ty:#?}")
    };
    (parameter, ty)
}

#[test]
fn return_predicate_preserves_parameter_target_and_original_spans() {
    let source = "-- café\r\n---@param value any\r\n---@return value is string | number[] # a scalar or list\r\nlocal function valid(value) return true end";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!("function")
    };
    let returns = function.annotations.returns.as_ref().unwrap();
    assert_eq!(returns.types.len(), 1);
    assert!(returns.packs.is_none());
    let (parameter, target) = predicate(&returns.types[0]);
    assert_eq!(parameter.text, "value");
    assert_eq!(&source[parameter.span.start..parameter.span.end], "value");
    assert_eq!(&source[target.span.start..target.span.end], "string | number[]");
    assert_eq!(&source[returns.types[0].span.start..returns.types[0].span.end], "value is string | number[]");
    assert!(matches!(target.kind, TypeExprKind::Union(_)));
}

#[test]
fn function_types_overloads_and_declarations_use_the_same_predicate_node() {
    let source = "---@type fun<T>(value:T|string): value is T\nlocal guard\n---@overload fun(value:any): value is string\nlocal function valid(value) end\n---@function isNumber(value:any): value is number\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let StmtKind::Local { annotation: Some(annotation), .. } = &file.statements[0].kind else {
        panic!("annotated local")
    };
    let TypeExprKind::Function(signature) = &annotation.types[0].kind else {
        panic!("function type")
    };
    assert_eq!(predicate(&signature.returns[0]).0.text, "value");
    assert!(matches!(&predicate(&signature.returns[0]).1.kind, TypeExprKind::Named(name) if name == "T"));
    assert!(signature.packs.is_none());
    assert_eq!(predicate(&file.overload_annotations[0].signature.returns[0]).0.text, "value");
    assert_eq!(predicate(&file.function_declarations[0].signature.returns[0]).0.text, "value");
}

#[test]
fn predicate_callbacks_preserve_the_enclosing_parameter_comma() {
    let source = "---@type fun(test:fun(value:any):value is string, other:number):boolean\nlocal test";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let StmtKind::Local { annotation: Some(annotation), .. } = &file.statements[0].kind else {
        panic!("annotated local")
    };
    let TypeExprKind::Function(outer) = &annotation.types[0].kind else { panic!("outer function") };
    assert_eq!(outer.params.len(), 2);
    let TypeExprKind::Function(inner) = &outer.params[0].ty.as_ref().unwrap().kind else { panic!("callback") };
    assert_eq!(inner.returns.len(), 1);
    assert_eq!(predicate(&inner.returns[0]).0.text, "value");
    assert_eq!(outer.params[1].name.text, "other");
}

#[test]
fn malformed_and_combined_predicates_report_annotation_errors() {
    for returns in [
        "value is",
        "value is string, number",
        "value is string, ...",
        "value is string...",
        "(value is string, number)",
        "value is (string, number)",
        "value is string | (true, number)",
    ] {
        let source = format!("---@return {returns}\nlocal function valid(value) end\nlocal recovered=42");
        let file = parse(&source);
        assert!(!file.diagnostics.is_empty(), "{returns}");
        assert!(file.diagnostics.iter().all(|diagnostic| diagnostic.origin == luanalysis_syntax::DiagnosticOrigin::Annotation), "{returns}: {:#?}", file.diagnostics);
        assert!(matches!(&file.statements[1].kind, StmtKind::Local { names, .. } if names[0].text == "recovered"));
    }
}

#[test]
fn predicate_syntax_is_rejected_outside_function_results() {
    for tag in [
        "type fun(value: value is string):boolean",
        "type {guard:value is string}",
        "type table<value is string>",
        "param guard fun(value:value is string):boolean",
        "alias Guard (value is string)",
        "field guard {value:value is string}",
    ] {
        let source = format!("---@{tag}\nlocal value");
        let file = parse(&source);
        assert!(!file.diagnostics.is_empty(), "{tag}");
        assert!(file.diagnostics.iter().all(|diagnostic| diagnostic.origin == luanalysis_syntax::DiagnosticOrigin::Annotation), "{tag}: {:#?}", file.diagnostics);
    }
}

#[test]
fn multiple_return_tags_cannot_combine_a_predicate_with_other_results() {
    for tags in [
        "---@return value is string\n---@return number",
        "---@return number\n---@return value is string",
        "---@return value is string\n---@return value is number",
    ] {
        let source = format!("{tags}\nlocal function valid(value) end");
        let file = parse(&source);
        assert!(file.diagnostics.iter().any(|diagnostic| diagnostic.message.contains("sole function return type")), "{tags}: {:#?}", file.diagnostics);
    }
}

#[test]
fn predicate_parameter_is_not_a_type_reference() {
    let source = "---@return value is Widget\nlocal function valid(value) end";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let names = type_names(&file, source);
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].name, "Widget");
}

#[test]
fn contextual_is_remains_a_named_type_in_return_annotations() {
    for returns in ["is", "is | nil", "is[]", "fun():is"] {
        let source = format!("---@alias is string\n---@return {returns}\nlocal function valid() end");
        let file = parse(&source);
        assert!(file.diagnostics.is_empty(), "{returns}: {:#?}", file.diagnostics);
        assert_eq!(type_names(&file, &source).iter().filter(|name| name.name == "is").count(), 2);
    }
    let file = parse("---@type fun(is:unknown):is is string\nlocal guard");
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let StmtKind::Local { annotation: Some(annotation), .. } = &file.statements[0].kind else { panic!("annotated local") };
    let TypeExprKind::Function(signature) = &annotation.types[0].kind else { panic!("function type") };
    assert_eq!(predicate(&signature.returns[0]).0.text, "is");
}

#[test]
fn deeply_nested_predicate_targets_obey_the_existing_type_depth_bound() {
    for prefix in ["value is fun(value:any): ", "is is fun(is:any): "] {
        let target = format!("{}string", prefix.repeat(1024));
        let source = format!("---@return {target}\nlocal function valid(value) end\nlocal recovered=42");
        let file = parse(&source);
        assert!(file.diagnostics.iter().any(|diagnostic| diagnostic.message == "type annotation nesting exceeds 128"));
        assert!(matches!(&file.statements[1].kind, StmtKind::Local { names, .. } if names[0].text == "recovered"));
    }
}
