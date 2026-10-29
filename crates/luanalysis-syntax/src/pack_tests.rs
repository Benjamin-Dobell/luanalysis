use crate::*;

fn function(file: &File) -> &Function {
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!("expected local function")
    };
    function
}

fn annotation(file: &File) -> &TypeAnnotation {
    let StmtKind::Local {
        annotation: Some(annotation),
        ..
    } = &file.statements[0].kind
    else {
        panic!("expected annotated local")
    };
    annotation
}

#[test]
fn coroutine_return_alternatives_keep_discriminants_and_canonical_slots() {
    // Coroutine.ttslua:27: false | (true, R).
    let source = "-- café\r\n---@generic R\r\n---@return false | (true, R) @ Correlated results\r\nlocal function resume() return false end\r\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let returns = function(&file).annotations.returns.as_ref().unwrap();
    assert_eq!(returns.types.len(), 3);
    assert_eq!(
        returns.types[0].kind,
        TypeExprKind::Literal(TypeLiteral::Boolean(false))
    );
    assert_eq!(
        returns.types[1].kind,
        TypeExprKind::Literal(TypeLiteral::Boolean(true))
    );
    assert_eq!(returns.types[2].kind, TypeExprKind::Named("R".to_owned()));
    let packs = returns.packs.as_ref().unwrap();
    assert_eq!(packs.alternatives.len(), 2);
    assert_eq!(packs.alternatives[0].fixed, 0..1);
    assert_eq!(packs.alternatives[1].fixed, 1..3);
    assert!(
        packs
            .alternatives
            .iter()
            .all(|pack| pack.variadic.is_none())
    );
    assert_eq!(
        &source[packs.span.start..packs.span.end],
        "false | (true, R)"
    );
    assert_eq!(
        &source[packs.alternatives[0].span.start..packs.alternatives[0].span.end],
        "false"
    );
    assert_eq!(
        &source[packs.alternatives[1].span.start..packs.alternatives[1].span.end],
        "(true, R)"
    );
    assert_eq!(
        &source[returns.types[2].span.start..returns.types[2].span.end],
        "R"
    );
}

#[test]
fn scalar_union_and_grouped_scalar_remain_scalar_types() {
    for ty in [
        "number | nil",
        "(number | nil)",
        "(fun(): number, string) | nil",
    ] {
        let source = format!("---@return {ty}\nlocal function get() end\n");
        let file = parse(&source);
        assert!(file.diagnostics.is_empty(), "{ty}: {:?}", file.diagnostics);
        let returns = function(&file).annotations.returns.as_ref().unwrap();
        assert_eq!(returns.types.len(), 1, "{ty}");
        assert!(returns.packs.is_none(), "{ty}");
        if ty == "(number | nil)" {
            let TypeExprKind::Parenthesized(inner) = &returns.types[0].kind else {
                panic!("parenthesized scalar was treated as a tuple pack")
            };
            assert!(matches!(inner.kind, TypeExprKind::Union(_)));
        } else {
            assert!(matches!(returns.types[0].kind, TypeExprKind::Union(_)));
        }
    }
}

#[test]
fn scalar_union_before_tuple_pack_preserves_whole_slot_union() {
    let file = parse("---@return number | nil | (string, boolean)\nlocal function get() end\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let returns = function(&file).annotations.returns.as_ref().unwrap();
    assert_eq!(returns.types.len(), 3);
    assert!(matches!(returns.types[0].kind, TypeExprKind::Union(_)));
    let packs = returns.packs.as_ref().unwrap();
    assert_eq!(packs.alternatives[0].fixed, 0..1);
    assert_eq!(packs.alternatives[1].fixed, 1..3);
}

#[test]
fn simple_fixed_return_lists_keep_the_existing_vector_representation() {
    for ty in ["number, string", "(number, string)"] {
        let file = parse(&format!("---@return {ty}\nlocal function get() end\n"));
        assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
        let returns = function(&file).annotations.returns.as_ref().unwrap();
        assert_eq!(returns.types.len(), 2);
        assert!(returns.packs.is_none());
        assert_eq!(
            returns.types[0].kind,
            TypeExprKind::Named("number".to_owned())
        );
        assert_eq!(
            returns.types[1].kind,
            TypeExprKind::Named("string".to_owned())
        );
    }
}

#[test]
fn variadic_return_tail_excludes_its_type_from_required_fixed_slots() {
    // varreturn.lua and Coroutine.start: number, boolean... / boolean, any....
    for (ty, fixed_count) in [
        ("boolean...", 0),
        ("number, boolean...", 1),
        ("(number, boolean...)", 1),
    ] {
        let source = format!("---@return {ty}\nlocal function get() end\n");
        let file = parse(&source);
        assert!(file.diagnostics.is_empty(), "{ty}: {:?}", file.diagnostics);
        let returns = function(&file).annotations.returns.as_ref().unwrap();
        let pack = &returns.packs.as_ref().unwrap().alternatives[0];
        assert_eq!(pack.fixed, 0..fixed_count);
        let tail = pack.variadic.as_ref().unwrap();
        assert_eq!(tail.type_index, fixed_count);
        assert_eq!(
            returns.types[tail.type_index].kind,
            TypeExprKind::Named("boolean".to_owned())
        );
        assert_eq!(
            &source[tail.ellipsis_span.start..tail.ellipsis_span.end],
            "..."
        );
    }
}

#[test]
fn alternative_variadic_packs_use_absolute_indices_in_one_type_vector() {
    let file =
        parse("---@return (number, string...) | (nil, boolean...)\nlocal function get() end\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let returns = function(&file).annotations.returns.as_ref().unwrap();
    assert_eq!(returns.types.len(), 4);
    let packs = returns.packs.as_ref().unwrap();
    assert_eq!(packs.alternatives[0].fixed, 0..1);
    assert_eq!(
        packs.alternatives[0].variadic.as_ref().unwrap().type_index,
        1
    );
    assert_eq!(packs.alternatives[1].fixed, 2..3);
    assert_eq!(
        packs.alternatives[1].variadic.as_ref().unwrap().type_index,
        3
    );
    assert_eq!(
        returns.types[3].kind,
        TypeExprKind::Named("boolean".to_owned())
    );
}

#[test]
fn nested_iterator_signature_keeps_packs_on_the_inner_function() {
    // TableUtils.fixedLengthIterator uses the same return form with Packed<T>.
    let file = parse(
        "---@generic T\n---@return fun(arr: Packed, i: number): nil | (number, T)\nlocal function iterator() end\n",
    );
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let returns = function(&file).annotations.returns.as_ref().unwrap();
    assert_eq!(returns.types.len(), 1);
    assert!(returns.packs.is_none());
    let TypeExprKind::Function(iterator) = &returns.types[0].kind else {
        panic!("expected returned function")
    };
    assert_eq!(iterator.params.len(), 2);
    assert_eq!(iterator.returns.len(), 3);
    let packs = iterator.packs.as_ref().unwrap();
    assert_eq!(packs.alternatives[0].fixed, 0..1);
    assert_eq!(packs.alternatives[1].fixed, 1..3);
}

#[test]
fn parenthesized_callback_does_not_swallow_outer_result_slots() {
    // TableUtils.iterate: (fun(...): nil | (K, V)), table<K, V>, K.
    let file =
        parse("---@return (fun(key: K): nil | (K, V)), Map, K\nlocal function iterate() end\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let returns = function(&file).annotations.returns.as_ref().unwrap();
    assert_eq!(returns.types.len(), 3);
    assert!(returns.packs.is_none());
    let TypeExprKind::Parenthesized(callback) = &returns.types[0].kind else {
        panic!("expected parenthesized callback")
    };
    let TypeExprKind::Function(callback) = &callback.kind else {
        panic!("expected callback function")
    };
    assert_eq!(callback.returns.len(), 3);
    assert!(callback.packs.is_some());
    assert_eq!(returns.types[1].kind, TypeExprKind::Named("Map".to_owned()));
    assert_eq!(returns.types[2].kind, TypeExprKind::Named("K".to_owned()));
}

#[test]
fn commas_in_parenthesized_callback_returns_do_not_form_an_outer_pack() {
    for ty in [
        "(fun(): number, string), boolean",
        "nil | (fun(): number, string), boolean",
    ] {
        let file = parse(&format!("---@return {ty}\nlocal function get() end\n"));
        assert!(file.diagnostics.is_empty(), "{ty}: {:?}", file.diagnostics);
        let returns = function(&file).annotations.returns.as_ref().unwrap();
        assert_eq!(returns.types.len(), 2, "{ty}");
        assert!(returns.packs.is_none(), "{ty}");
        assert_eq!(
            returns.types[1].kind,
            TypeExprKind::Named("boolean".to_owned())
        );
        let first = if let TypeExprKind::Union(types) = &returns.types[0].kind {
            assert_eq!(types.len(), 2);
            &types[1]
        } else {
            &returns.types[0]
        };
        let TypeExprKind::Parenthesized(callback) = &first.kind else {
            panic!("expected parenthesized scalar callback")
        };
        let TypeExprKind::Function(callback) = &callback.kind else {
            panic!("expected callback")
        };
        assert_eq!(callback.returns.len(), 2);
    }
}

#[test]
fn deeply_nested_function_return_annotations_report_bounds_without_overflow() {
    for ty in [
        format!("{}number", "fun(): ".repeat(1024)),
        format!("{}number{}", "(".repeat(1024), ")".repeat(1024)),
    ] {
        let file = parse(&format!(
            "---@return {ty}\nlocal function get() end\nlocal recovery = 1\n"
        ));
        assert!(
            file.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message == "type annotation nesting exceeds 128")
        );
        assert_eq!(file.statements.len(), 2);
        drop(file.clone());
        drop(file);
    }
}

#[test]
fn callback_pack_returns_respect_record_parameter_and_generic_commas() {
    let source = "---@type {callback: fun(): nil | (number, string), count: number}\nlocal record\n---@type fun(callback: fun(): (number, string...), count: number): boolean\nlocal apply\n---@generic F: fun(): nil | (number, string), T\nlocal function constrained() end\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let TypeExprKind::Record(fields) = &annotation(&file).types[0].kind else {
        panic!("expected record")
    };
    assert_eq!(fields.len(), 2);
    let StmtKind::Local {
        annotation: Some(annotation),
        ..
    } = &file.statements[1].kind
    else {
        panic!("expected function type")
    };
    let TypeExprKind::Function(apply) = &annotation.types[0].kind else {
        panic!("expected function type")
    };
    assert_eq!(apply.params.len(), 2);
    let TypeExprKind::Function(callback) = &apply.params[0].ty.as_ref().unwrap().kind else {
        panic!("expected callback")
    };
    assert_eq!(callback.packs.as_ref().unwrap().alternatives[0].fixed, 0..1);
    let StmtKind::LocalFunction { function, .. } = &file.statements[2].kind else {
        panic!("expected constrained function")
    };
    assert_eq!(function.annotations.generics.len(), 2);
    assert_eq!(function.annotations.generics[1].name.text, "T");
}

#[test]
fn type_annotation_variadic_lists_have_one_pack_and_never_alternatives() {
    let source = "---@type number, string...\nlocal first, second\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let annotation = annotation(&file);
    assert_eq!(annotation.types.len(), 2);
    let packs = annotation.packs.as_ref().unwrap();
    assert_eq!(packs.alternatives.len(), 1);
    assert_eq!(packs.alternatives[0].fixed, 0..1);
    assert_eq!(
        packs.alternatives[0].variadic.as_ref().unwrap().type_index,
        1
    );
    let invalid = parse("---@type nil | (number, string)\nlocal value\n");
    assert!(!invalid.diagnostics.is_empty());
}

#[test]
fn both_function_vararg_spellings_preserve_the_type_and_original_spans() {
    for vararg in ["...: boolean | nil", "vararg boolean | nil"] {
        let source = format!("---@type fun(value: number, {vararg}): string...\nlocal accepts\n");
        let file = parse(&source);
        assert!(
            file.diagnostics.is_empty(),
            "{vararg}: {:?}",
            file.diagnostics
        );
        let TypeExprKind::Function(function) = &annotation(&file).types[0].kind else {
            panic!("expected function type")
        };
        assert_eq!(function.params.len(), 1);
        let parameter = function.vararg.as_ref().unwrap();
        assert_eq!(&source[parameter.span.start..parameter.span.end], vararg);
        assert!(matches!(parameter.ty.kind, TypeExprKind::Union(_)));
        assert_eq!(parameter.ellipsis_span.is_some(), vararg.starts_with("..."));
        assert!(function.packs.is_some());
    }
    let file = parse("---@type fun(vararg: number): number\nlocal named\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let TypeExprKind::Function(named) = &annotation(&file).types[0].kind else {
        panic!("expected function type")
    };
    assert!(named.vararg.is_none());
    assert_eq!(named.params[0].name.text, "vararg");
}

#[test]
fn vararg_tag_attaches_to_native_variadic_functions_and_reports_duplicates() {
    let source = "---@generic T\n---@vararg T\n---@return T...\nlocal function forward(...) return ... end\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let native_function = function(&file);
    let parameter = native_function.annotations.vararg.as_ref().unwrap();
    assert_eq!(parameter.ty.kind, TypeExprKind::Named("T".to_owned()));
    assert!(parameter.ellipsis_span.is_none());
    assert_eq!(
        &source[parameter.span.start..parameter.span.end],
        "@vararg T"
    );
    assert_eq!(
        &source[native_function.vararg.unwrap().start..native_function.vararg.unwrap().end],
        "..."
    );
    let duplicate = parse("---@vararg number\n---@vararg string\nlocal function f(...) end\n");
    assert!(duplicate.diagnostics.iter().any(|diagnostic| {
        diagnostic.message == "multiple @vararg annotations for one function"
    }));
    assert_eq!(
        function(&duplicate)
            .annotations
            .vararg
            .as_ref()
            .unwrap()
            .ty
            .kind,
        TypeExprKind::Named("number".to_owned())
    );
}

#[test]
fn literal_annotation_types_retain_lua_values_and_byte_spans() {
    let source = "-- café\r\n---@type -0x2a, 'é\\255@tag', true, false @ Literal types\r\nlocal number, string, yes, no\r\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let types = &annotation(&file).types;
    assert_eq!(types.len(), 4);
    assert_eq!(
        types[0].kind,
        TypeExprKind::Literal(TypeLiteral::Number("-0x2a".to_owned()))
    );
    let mut expected = "é".as_bytes().to_vec();
    expected.extend([255, b'@', b't', b'a', b'g']);
    assert_eq!(
        types[1].kind,
        TypeExprKind::Literal(TypeLiteral::String(expected))
    );
    assert_eq!(
        types[2].kind,
        TypeExprKind::Literal(TypeLiteral::Boolean(true))
    );
    assert_eq!(
        types[3].kind,
        TypeExprKind::Literal(TypeLiteral::Boolean(false))
    );
    assert_eq!(&source[types[0].span.start..types[0].span.end], "-0x2a");
    assert_eq!(
        &source[types[1].span.start..types[1].span.end],
        "'é\\255@tag'"
    );
}

#[test]
fn literal_scalar_return_and_tuple_alternative_remain_distinct() {
    let file = parse("---@return 1 | (nil, string)\nlocal function get() end\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let returns = function(&file).annotations.returns.as_ref().unwrap();
    assert_eq!(
        returns.types[0].kind,
        TypeExprKind::Literal(TypeLiteral::Number("1".to_owned()))
    );
    assert_eq!(returns.packs.as_ref().unwrap().alternatives[0].fixed, 0..1);
    assert_eq!(returns.packs.as_ref().unwrap().alternatives[1].fixed, 1..3);
}

#[test]
fn malformed_vararg_and_pack_annotations_are_diagnosed_and_recover() {
    for ty in [
        "fun(... number): number",
        "fun(...: number, value: string): number",
        "fun(vararg): number",
        "fun(...: number,): number",
    ] {
        let file = parse(&format!("---@type {ty}\nlocal bad\nlocal recovery = 1\n"));
        assert!(
            !file.diagnostics.is_empty(),
            "invalid function type accepted: {ty}"
        );
        assert_eq!(file.statements.len(), 2);
    }
    for ty in [
        "()",
        "number..., string",
        "(number..., string)",
        "(number,)",
        "number | (string,)",
    ] {
        let file = parse(&format!(
            "---@return {ty}\nlocal function bad() end\nlocal recovery = 1\n"
        ));
        assert!(
            !file.diagnostics.is_empty(),
            "invalid return pack accepted: {ty}"
        );
        assert_eq!(file.statements.len(), 2);
    }
    let missing = parse("---@vararg\nlocal function bad(...) end\n");
    assert!(!missing.diagnostics.is_empty());
}


#[test]
fn v1_bare_callback_results_take_precedence_over_untyped_outer_parameters() {
    for (ty, outer_params, outer_returns) in [
        ("fun<T, RK, RV>(arr: T[], callback: fun(obj: T, i: number): RK, RV): table<RK, RV>", 2, 1),
        ("fun<A, B>(func: fun(): A, B): A, B", 1, 2),
    ] {
        let file = parse(&format!("---@type {ty}\nlocal apply\n"));
        assert!(file.diagnostics.is_empty(), "{ty}: {:?}", file.diagnostics);
        let TypeExprKind::Function(apply) = &annotation(&file).types[0].kind else {
            panic!("expected outer function")
        };
        assert_eq!(apply.params.len(), outer_params, "{ty}");
        assert_eq!(apply.returns.len(), outer_returns, "{ty}");
        let TypeExprKind::Function(callback) = &apply.params.last().unwrap().ty.as_ref().unwrap().kind else {
            panic!("expected callback")
        };
        assert_eq!(callback.returns.len(), 2, "{ty}");
    }
    let file = parse("---@type fun(callback: (fun(): number), untyped): string\nlocal apply\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let TypeExprKind::Function(apply) = &annotation(&file).types[0].kind else {
        panic!("expected outer function")
    };
    assert_eq!(apply.params.len(), 2);
    assert_eq!(apply.params[1].name.text, "untyped");
    assert!(apply.params[1].ty.is_none());
}

#[test]
fn bare_callback_results_stop_at_named_outer_parameters_and_varargs() {
    for (tail, outer_params, has_vararg) in [
        ("count: number", 2, false),
        ("count?: number", 2, false),
        ("...: number", 1, true),
        ("...", 1, true),
        ("vararg number", 1, true),
    ] {
        let source = format!("---@type fun(callback: fun(): number, string, {tail}): boolean\nlocal apply\n");
        let file = parse(&source);
        assert!(file.diagnostics.is_empty(), "{tail}: {:?}", file.diagnostics);
        let TypeExprKind::Function(apply) = &annotation(&file).types[0].kind else {
            panic!("expected outer function")
        };
        assert_eq!(apply.params.len(), outer_params, "{tail}");
        assert_eq!(apply.vararg.is_some(), has_vararg, "{tail}");
        assert_eq!(apply.returns.len(), 1, "{tail}");
        let TypeExprKind::Function(callback) = &apply.params[0].ty.as_ref().unwrap().kind else {
            panic!("expected callback")
        };
        assert_eq!(callback.returns.len(), 2, "{tail}");
    }
}

#[test]
fn callback_parameter_compatibility_does_not_capture_nested_container_commas() {
    for ty in [
        "fun(callback: {fun(): number, string}): boolean",
        "fun(callback: Pair<fun(): number, string>): boolean",
        "fun(callback: [fun(): number, string]): boolean",
        "fun(callback: ({fun(): number, string})): boolean",
        "fun(callback: fun<T: fun(): number, U>(): boolean): string",
    ] {
        let file = parse(&format!("---@type {ty}\nlocal apply\n"));
        assert!(file.diagnostics.is_empty(), "{ty}: {:?}", file.diagnostics);
        let TypeExprKind::Function(apply) = &annotation(&file).types[0].kind else {
            panic!("expected outer function")
        };
        assert_eq!(apply.params.len(), 1, "{ty}");
        let mut inner = apply.params[0].ty.as_ref().unwrap();
        if let TypeExprKind::Parenthesized(value) = &inner.kind {
            inner = value;
        }
        let callback = match &inner.kind {
            TypeExprKind::Record(fields) => {
                assert_eq!(fields.len(), 2, "{ty}");
                &fields[0].ty
            }
            TypeExprKind::Generic { args, .. } => {
                assert_eq!(args.len(), 2, "{ty}");
                &args[0]
            }
            TypeExprKind::Function(callback) => {
                assert_eq!(callback.generics.len(), 2, "{ty}");
                callback.generics[0].constraint.as_ref().unwrap()
            }
            other => panic!("unexpected container: {other:?}"),
        };
        let TypeExprKind::Function(callback) = &callback.kind else {
            panic!("expected nested callback")
        };
        assert_eq!(callback.returns.len(), 1, "{ty}");
    }
}

#[test]
fn v1_nested_returned_function_keeps_its_own_bare_result_list() {
    let file = parse("---@type fun(callback: fun(): fun(): number, string): boolean\nlocal apply\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let TypeExprKind::Function(apply) = &annotation(&file).types[0].kind else {
        panic!("expected outer function")
    };
    assert_eq!(apply.params.len(), 1);
    let TypeExprKind::Function(callback) = &apply.params[0].ty.as_ref().unwrap().kind else {
        panic!("expected callback")
    };
    assert_eq!(callback.returns.len(), 1);
    let TypeExprKind::Function(returned) = &callback.returns[0].kind else {
        panic!("expected returned function")
    };
    assert_eq!(returned.returns.len(), 2);
}


#[test]
fn callback_type_predicates_remain_single_results_with_named_outer_parameters() {
    let valid = parse("---@type fun(callback: fun(value: unknown): value is number, count: number): void\nlocal apply\n");
    assert!(valid.diagnostics.is_empty(), "{:?}", valid.diagnostics);
    let TypeExprKind::Function(apply) = &annotation(&valid).types[0].kind else {
        panic!("expected outer function")
    };
    assert_eq!(apply.params.len(), 2);
    let TypeExprKind::Function(callback) = &apply.params[0].ty.as_ref().unwrap().kind else {
        panic!("expected callback")
    };
    assert_eq!(callback.returns.len(), 1);
    assert!(matches!(callback.returns[0].kind, TypeExprKind::TypePredicate { .. }));
    let invalid = parse("---@type fun(callback: fun(value: unknown): value is number, string): void\nlocal apply\n");
    assert_eq!(invalid.diagnostics.len(), 1, "{:?}", invalid.diagnostics);
    assert_eq!(invalid.diagnostics[0].message, "a type predicate must be the sole function return type");
}

#[test]
fn varargs_written_with_a_trailing_ellipsis_spread_a_sequence() {
    let source = "---@generic T\n---@param ... T...\n---@return fun(...: T...), fun(...: T)\nlocal function bind(...) end\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let annotations = &function(&file).annotations;
    let vararg = annotations.vararg.as_ref().unwrap();
    assert!(vararg.spread.is_some() && vararg.spreads("T"), "{vararg:?}");
    let returns = &annotations.returns.as_ref().unwrap().types;
    let [TypeExpr { kind: TypeExprKind::Function(spread), .. }, TypeExpr { kind: TypeExprKind::Function(each), .. }] = returns.as_slice() else {
        panic!("{returns:?}")
    };
    assert!(spread.vararg.as_ref().unwrap().spreads("T"));
    assert!(each.vararg.as_ref().unwrap().spread.is_none());
    assert!(returns[0].spreads("T") && !returns[1].spreads("T"));
    // `infer P...` in a conditional's extends type is a spread `P` too.
    let file = parse("---@alias Params<F> F extends fun(...: infer P...) and P or never\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let NamedTypeDeclarationKind::Alias { target } = &file.type_declarations[0].kind else { panic!() };
    assert!(target.spreads("P"));
}
