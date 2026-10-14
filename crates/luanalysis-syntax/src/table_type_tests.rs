use crate::*;

fn annotation(source: &str) -> TypeAnnotation {
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let StmtKind::Local { annotation, .. } = &file.statements[0].kind else { panic!("expected local") };
    annotation.clone().unwrap()
}

#[test]
fn nested_table_generics_retain_distinct_closing_angle_spans() {
    // Same nested table idiom as ge_tts Class.ttslua:22.
    let source = "---@type table<table, nil | table<table, nil | boolean>>\nlocal cache = {}\n";
    let ty = annotation(source).types.remove(0);
    let TypeExprKind::Generic { base, args, args_span } = &ty.kind else { panic!("expected table generic") };
    assert_eq!(base.text, "table");
    assert_eq!(args.len(), 2);
    let TypeExprKind::Union(values) = &args[1].kind else { panic!("expected optional table") };
    let TypeExprKind::Generic { args_span: inner, .. } = &values[1].kind else { panic!("expected nested table") };
    assert_eq!(inner.end + 1, args_span.end);
    assert_eq!(&source[inner.end - 1..inner.end], ">");
    assert_eq!(&source[args_span.end - 1..args_span.end], ">");
}

#[test]
fn array_suffixes_bind_to_their_type_and_preserve_parentheses() {
    let source = "---@type number | string[], (number | string)[][]\nlocal first, second\n";
    let types = annotation(source).types;
    let TypeExprKind::Union(first) = &types[0].kind else { panic!("expected union") };
    assert!(matches!(first[0].kind, TypeExprKind::Named(_)));
    assert!(matches!(first[1].kind, TypeExprKind::Array { .. }));
    let TypeExprKind::Array { element, suffix_span } = &types[1].kind else { panic!("expected second array dimension") };
    assert_eq!(&source[suffix_span.start..suffix_span.end], "[]");
    let TypeExprKind::Array { element, .. } = &element.kind else { panic!("expected first array dimension") };
    assert!(matches!(element.kind, TypeExprKind::Parenthesized(_)));
    let file = parse("---@return (number | nil)[]\nlocal function result() return {} end\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else { panic!("expected function") };
    assert!(matches!(function.annotations.returns.as_ref().unwrap().types[0].kind, TypeExprKind::Array { .. }));
}

#[test]
fn record_keys_preserve_numeric_string_domain_and_sequence_syntax() {
    let source = "---@type {name: string, [1.0]: number, ['1']: boolean, [string]: number, number, boolean, ['\\255']: string}\nlocal value = {}\n";
    let ty = annotation(source).types.remove(0);
    let TypeExprKind::Record(fields) = &ty.kind else { panic!("expected record") };
    assert_eq!(fields.len(), 7);
    assert!(matches!(&fields[0].key, TypeFieldKey::Named(name) if name.text == "name"));
    assert!(matches!(&fields[1].key, TypeFieldKey::Indexed(key) if key.kind == TypeExprKind::Literal(TypeLiteral::Number("1.0".to_owned()))));
    assert!(matches!(&fields[2].key, TypeFieldKey::Indexed(key) if key.kind == TypeExprKind::Literal(TypeLiteral::String(b"1".to_vec()))));
    let TypeFieldKey::Indexed(domain) = &fields[3].key else { panic!("expected index domain") };
    assert_eq!(&source[domain.span.start..domain.span.end], "string");
    assert_eq!(fields[4].key, TypeFieldKey::Sequence { index: 1 });
    assert_eq!(fields[5].key, TypeFieldKey::Sequence { index: 2 });
    assert!(matches!(&fields[6].key, TypeFieldKey::Indexed(key) if key.kind == TypeExprKind::Literal(TypeLiteral::String(vec![255]))));
}

#[test]
fn generic_arguments_keep_callback_pack_commas_with_the_callback() {
    let source = "---@type Pair<fun(value: T): nil | (number, T), string>, std__Packed<T>[]\nlocal callbackPair, array\n";
    let types = annotation(source).types;
    let TypeExprKind::Generic { base, args, args_span } = &types[0].kind else { panic!("expected pair") };
    assert_eq!(base.text, "Pair");
    assert_eq!(args.len(), 2);
    let TypeExprKind::Function(callback) = &args[0].kind else { panic!("expected callback") };
    assert_eq!(callback.returns.len(), 3);
    assert_eq!(callback.packs.as_ref().unwrap().alternatives.len(), 2);
    assert_eq!(&source[args_span.start..args_span.end], "<fun(value: T): nil | (number, T), string>");
    assert!(matches!(&types[1].kind, TypeExprKind::Array { element, .. } if matches!(&element.kind, TypeExprKind::Generic { base, .. } if base.text == "std__Packed")));
}

#[test]
fn generic_constraints_arrays_and_variadic_array_tails_compose() {
    let source = "---@generic T: table<string, number[]>\n---@vararg T[]\n---@return T[]...\nlocal function forward(...) return ... end\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else { panic!("expected function") };
    assert!(matches!(function.annotations.generics[0].constraint.as_ref().unwrap().kind, TypeExprKind::Generic { .. }));
    assert!(matches!(function.annotations.vararg.as_ref().unwrap().ty.kind, TypeExprKind::Array { .. }));
    let returns = function.annotations.returns.as_ref().unwrap();
    let tail = returns.packs.as_ref().unwrap().alternatives[0].variadic.unwrap();
    assert!(matches!(returns.types[tail.type_index].kind, TypeExprKind::Array { .. }));
}

#[test]
fn generic_arity_and_literal_key_normalization_are_not_parser_operations() {
    let types = annotation("---@type table<number>, Pair<number, string, boolean>, {[0x1p+0]: string, [1.0]: number}\nlocal first, second, third\n").types;
    assert!(matches!(&types[0].kind, TypeExprKind::Generic { args, .. } if args.len() == 1));
    assert!(matches!(&types[1].kind, TypeExprKind::Generic { args, .. } if args.len() == 3));
    let TypeExprKind::Record(fields) = &types[2].kind else { panic!("expected record") };
    assert_eq!(fields.len(), 2);
    assert_ne!(fields[0].key, fields[1].key);
}

#[test]
fn malformed_array_generic_and_indexer_syntax_is_diagnosed_with_recovery() {
    for ty in ["table<>", "Pair<T,>", "number[index]", "{[string] number}", "{name?: number}"] {
        let source = format!("---@type {ty}\nlocal value = {{}}\nlocal recovery = 7\n");
        let file = parse(&source);
        assert!(!file.diagnostics.is_empty(), "{ty}");
        assert!(file.statements.iter().any(|stmt| matches!(&stmt.kind, StmtKind::Local { names, .. } if names.iter().any(|name| name.text == "recovery"))));
    }
}

#[test]
fn array_generic_and_indexer_type_depth_is_bounded_without_recursive_drop_failure() {
    for ty in [
        format!("number{}", "[]".repeat(8192)),
        format!("{}number{}", "Box<".repeat(1024), ">".repeat(1024)),
        format!("{}number{}", "{[".repeat(1024), "]: number}".repeat(1024)),
    ] {
        let file = parse(&format!("---@type {ty}\nlocal value = {{}}\nlocal recovery = 7\n"));
        assert!(file.diagnostics.iter().any(|diagnostic| diagnostic.message.contains("nesting exceeds")));
        assert!(file.statements.iter().any(|stmt| matches!(&stmt.kind, StmtKind::Local { names, .. } if names.iter().any(|name| name.text == "recovery"))));
        drop(file.clone());
        drop(file);
    }
}
