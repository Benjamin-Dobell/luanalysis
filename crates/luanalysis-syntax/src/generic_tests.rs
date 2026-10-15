use crate::*;

fn local_function(file: &File) -> &Function {
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!("expected local function")
    };
    function
}

#[test]
fn generic_tags_attach_native_names_constraints_and_function_annotations() {
    let source = "-- café\r\n---@generic T, U: {value: T}\r\n---@generic V: (string | number)\r\n---@param value T\r\n---@return T\r\nlocal function identity(value) return value end\r\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let annotations = &local_function(&file).annotations;
    assert_eq!(
        annotations
            .generics
            .iter()
            .map(|generic| generic.name.text.as_str())
            .collect::<Vec<_>>(),
        ["T", "U", "V"]
    );
    let first = &annotations.generics[0];
    assert!(first.constraint.is_none());
    assert_eq!(&source[first.name.span.start..first.name.span.end], "T");
    assert_eq!(&source[first.span.start..first.span.end], "T");
    let second = &annotations.generics[1];
    let constraint = second.constraint.as_ref().unwrap();
    assert_eq!(&source[second.span.start..second.span.end], "U: {value: T}");
    assert_eq!(
        &source[constraint.span.start..constraint.span.end],
        "{value: T}"
    );
    let TypeExprKind::Record(fields) = &constraint.kind else {
        panic!("expected record constraint")
    };
    assert_eq!(fields[0].ty.kind, TypeExprKind::Named("T".to_owned()));
    let TypeExprKind::Parenthesized(constraint) =
        &annotations.generics[2].constraint.as_ref().unwrap().kind
    else {
        panic!("expected parenthesized union constraint")
    };
    assert!(matches!(constraint.kind, TypeExprKind::Union(_)));
    assert_eq!(
        annotations.params[0].ty.kind,
        TypeExprKind::Named("T".to_owned())
    );
    assert_eq!(
        annotations.returns.as_ref().unwrap().types[0].kind,
        TypeExprKind::Named("T".to_owned())
    );
}

#[test]
fn function_constraints_preserve_generic_declaration_commas() {
    let source = "---@generic F: fun(value: string): number, T, G: fun(value: T): (T, string)\nlocal function f() end\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let generics = &local_function(&file).annotations.generics;
    assert_eq!(generics.len(), 3);
    assert_eq!(generics[1].name.text, "T");
    let TypeExprKind::Function(first) = &generics[0].constraint.as_ref().unwrap().kind else {
        panic!("expected function constraint")
    };
    assert_eq!(first.returns.len(), 1);
    assert_eq!(
        first.returns[0].kind,
        TypeExprKind::Named("number".to_owned())
    );
    let TypeExprKind::Function(last) = &generics[2].constraint.as_ref().unwrap().kind else {
        panic!("expected function constraint")
    };
    assert_eq!(last.returns.len(), 2);
    assert_eq!(
        &source[generics[2].span.start..generics[2].span.end],
        "G: fun(value: T): (T, string)"
    );
}

#[test]
fn generic_duplicates_across_lists_and_tags_report_original_names() {
    let source = "---@generic T, T\n---@generic U, T\nlocal function f() end\n";
    let file = parse(source);
    assert_eq!(file.diagnostics.len(), 2, "{:?}", file.diagnostics);
    let generics = &local_function(&file).annotations.generics;
    assert_eq!(generics.len(), 2);
    assert_eq!(generics[0].name.text, "T");
    assert_eq!(generics[1].name.text, "U");
    for diagnostic in &file.diagnostics {
        assert_eq!(diagnostic.message, "multiple @generic declarations for T");
        assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], "T");
    }
    assert!(file.diagnostics[0].span.start < file.diagnostics[1].span.start);
}

#[test]
fn generic_tags_attach_to_parenthesized_function_initializers() {
    let file = parse(
        "---@generic T\n---@param value T\n---@return T\nlocal identity = ((function(value) return value end))\n",
    );
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let StmtKind::Local { values, .. } = &file.statements[0].kind else {
        panic!("expected local initializer")
    };
    let mut value = &values[0];
    while let ExprKind::Parenthesized(inner) = &value.kind {
        value = inner;
    }
    let ExprKind::Function(function) = &value.kind else {
        panic!("expected function initializer")
    };
    assert_eq!(function.annotations.generics[0].name.text, "T");
    assert_eq!(function.annotations.params[0].name.text, "value");
}

#[test]
fn generic_tags_work_in_long_documentation_with_trailing_prose() {
    let source = "--[=[---\n@generic T: {value: string}, U @ Type parameters\n@param value T\n@return U\n]=]\nlocal function convert(value) return value end\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let generics = &local_function(&file).annotations.generics;
    assert_eq!(generics.len(), 2);
    assert_eq!(generics[1].name.text, "U");
    assert_eq!(
        &source[generics[1].name.span.start..generics[1].name.span.end],
        "U"
    );
}

#[test]
fn generic_defaults_and_bare_union_constraints_are_invalid() {
    let defaults = parse("---@generic T = number\nlocal function f() end\n");
    assert!(defaults.diagnostics.iter().any(|diagnostic| {
        diagnostic.message == "generic parameter defaults are invalid Luanalysis annotation syntax"
    }));
    let union = parse("---@generic T: string | number\nlocal function f() end\n");
    assert!(union.diagnostics.iter().any(|diagnostic| {
        diagnostic.message == "unsupported or unexpected syntax after @generic"
    }));
    for tag in [
        "---@generic",
        "---@generic T,",
        "---@generic 42",
        "---@generic T:",
    ] {
        let file = parse(&format!("{tag}\nlocal function f() end\n"));
        assert!(
            !file.diagnostics.is_empty(),
            "invalid generic tag accepted: {tag}"
        );
    }
}

#[test]
fn generic_names_follow_documentation_identifiers_not_lua_keywords() {
    let file = parse("---@generic end, nil\nlocal function f() end\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let generics = &local_function(&file).annotations.generics;
    assert_eq!(generics[0].name.text, "end");
    assert_eq!(generics[1].name.text, "nil");
}

#[test]
fn generic_function_types_preserve_their_own_original_parameter_scope() {
    let source =
        "---@type fun<T: number>(value: T): T\nlocal identity = function(value) return value end\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let StmtKind::Local { values, .. } = &file.statements[0].kind else {
        panic!("local function value")
    };
    let ExprKind::Function(body) = &values[0].kind else {
        panic!("function initializer")
    };
    assert!(body.annotations.generics.is_empty());
    let TypeExprKind::Function(signature) =
        &body.annotations.type_annotation.as_ref().unwrap().types[0].kind
    else {
        panic!("generic function-type head")
    };
    assert_eq!(signature.generics.len(), 1);
    assert_eq!(
        &source[signature.generics[0].name.span.start..signature.generics[0].name.span.end],
        "T"
    );
    assert!(
        matches!(&signature.generics[0].constraint.as_ref().unwrap().kind, TypeExprKind::Named(name) if name=="number")
    );
    assert!(
        matches!(&signature.params[0].ty.as_ref().unwrap().kind, TypeExprKind::Named(name) if name=="T")
    );
    assert!(matches!(&signature.returns[0].kind, TypeExprKind::Named(name) if name=="T"));
}

#[test]
fn generic_annotations_are_diagnosed_when_not_attached_to_a_function() {
    let file = parse("---@generic T\nlocal value = 42\n");
    assert!(!file.diagnostics.is_empty());
    assert!(file.diagnostics.iter().all(|diagnostic| {
        !diagnostic
            .message
            .contains("unsupported Luanalysis annotation @generic")
    }));
}
