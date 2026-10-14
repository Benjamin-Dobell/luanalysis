use crate::*;

fn locals(file: &File, index: usize) -> (&[Name], &[Expr], Option<&TypeAnnotation>) {
    let StmtKind::Local {
        names,
        values,
        annotation,
    } = &file.statements[index].kind
    else {
        panic!("expected local")
    };
    (names, values, annotation.as_ref())
}

#[test]
fn annotation_and_mismatched_lua_values_preserve_native_spans() {
    let source = include_str!("../tests/fixtures/structural.lua");
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let (names, values, annotation) = locals(&file, 0);
    assert_eq!(names[0].text, "item");
    let annotation = annotation.unwrap();
    let TypeExprKind::Record(fields) = &annotation.types[0].kind else {
        panic!("expected record")
    };
    assert_eq!(
        fields
            .iter()
            .map(|field| field.name.text.as_str())
            .collect::<Vec<_>>(),
        ["title", "count"]
    );
    assert_eq!(
        &source[fields[0].ty.span.start..fields[0].ty.span.end],
        "string"
    );
    let TypeExprKind::Union(types) = &fields[1].ty.kind else {
        panic!("expected union")
    };
    assert_eq!(&source[types[1].span.start..types[1].span.end], "nil");
    let ExprKind::Table(fields) = &values[0].kind else {
        panic!("expected table")
    };
    assert_eq!(fields[0].value.kind, ExprKind::Number("42".to_owned()));
    assert_eq!(fields[1].value.kind, ExprKind::String(b"three".to_vec()));
    let StmtKind::Assign {
        targets, values, ..
    } = &file.statements[1].kind
    else {
        panic!("expected assignment")
    };
    assert_eq!(
        &source[targets[0].span.start..targets[0].span.end],
        "item.count"
    );
    assert_eq!(
        &source[values[0].span.start..values[0].span.end],
        "\"wrong\""
    );
}

#[test]
fn preserves_repeated_local_declarations_and_initializer_name() {
    let file = parse(include_str!("../tests/fixtures/declaration-order.lua"));
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    assert_eq!(file.statements.len(), 4);
    let (names, values, _) = locals(&file, 1);
    assert_eq!(
        names
            .iter()
            .map(|name| name.text.as_str())
            .collect::<Vec<_>>(),
        ["before", "after"]
    );
    let ExprKind::Name(name) = &values[0].kind else {
        panic!("initializer reference lost")
    };
    assert_eq!(name.text, "before");
    assert!(names[0].span.start < name.span.start);
    assert_eq!(locals(&file, 2).0[0].text, "before");
}

#[test]
fn unicode_crlf_offsets_are_utf8_bytes_in_original_source() {
    let source = "-- café\r\n---@type string | nil\r\nlocal 日本語 = \"é\"\r\n日本語 = nil\r\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let (names, values, annotation) = locals(&file, 0);
    assert_eq!(&source[names[0].span.start..names[0].span.end], "日本語");
    assert_eq!(names[0].span.end - names[0].span.start, 9);
    assert_eq!(values[0].kind, ExprKind::String("é".as_bytes().to_vec()));
    assert_eq!(file.comments[0].text, "-- café");
    let TypeExprKind::Union(types) = &annotation.unwrap().types[0].kind else {
        panic!("expected union")
    };
    assert_eq!(&source[types[0].span.start..types[0].span.end], "string");
    assert_eq!(file.span.end, source.len());
    let newlines = lex(source)
        .tokens
        .into_iter()
        .filter(|token| token.kind == TokenKind::Newline)
        .collect::<Vec<_>>();
    assert_eq!(newlines.len(), 4);
    assert!(
        newlines
            .iter()
            .all(|token| &source[token.span.start..token.span.end] == "\r\n")
    );
}

#[test]
fn multiline_tables_and_parentheses_keep_annotations_on_their_declarations() {
    let source = "---@type {value: (number | nil)}\nlocal object = {\n value = (42);\n}\n---@type boolean\nlocal flag = true\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    assert_eq!(file.statements.len(), 2);
    let (_, _, annotation) = locals(&file, 1);
    assert_eq!(
        annotation.unwrap().types[0].kind,
        TypeExprKind::Named("boolean".to_owned())
    );
}

#[test]
fn annotations_support_multiple_variable_types_and_explicit_documentation() {
    let file = parse("---@type number, string @ Description\nlocal x, y = 1, 'two'\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let (_, _, annotation) = locals(&file, 0);
    assert_eq!(annotation.unwrap().types.len(), 2);
}

#[test]
fn long_documentation_annotations_retain_original_type_span() {
    let source = "--[=[---\n---@type number\n]=]\nlocal x = 1\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let (_, _, annotation) = locals(&file, 0);
    let ty = &annotation.unwrap().types[0];
    assert_eq!(&source[ty.span.start..ty.span.end], "number");
}

#[test]
fn lua_byte_strings_decode_escapes_and_normalize_long_newlines() {
    let source = "local bytes = '\\255\\x00\\u{1f600}\\z \r\n a'\nlocal long = [=[\r\none\r\ntwo\rthree]=]\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let mut expected = vec![255, 0];
    expected.extend_from_slice("😀a".as_bytes());
    assert_eq!(locals(&file, 0).1[0].kind, ExprKind::String(expected));
    assert_eq!(
        locals(&file, 1).1[0].kind,
        ExprKind::String(b"one\ntwo\nthree".to_vec())
    );
}

#[test]
fn long_comments_match_their_exact_bracket_level() {
    let file = parse("--[==[ not a close: ]=] ]==]\nlocal x = 1\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    assert_eq!(file.comments.len(), 1);
    assert_eq!(file.comments[0].kind, CommentKind::Long);
    assert_eq!(file.statements.len(), 1);
}

#[test]
fn negative_and_lua_hexadecimal_numbers_keep_original_spelling() {
    let file = parse("local x, y, z = - 0x1.fp-2, .25, 1.e+2\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    assert_eq!(
        locals(&file, 0)
            .1
            .iter()
            .map(|value| &value.kind)
            .collect::<Vec<_>>(),
        [
            &ExprKind::Number("-0x1.fp-2".to_owned()),
            &ExprKind::Number(".25".to_owned()),
            &ExprKind::Number("1.e+2".to_owned()),
        ]
    );
}

#[test]
fn unsupported_lua_and_annotation_forms_are_reported_and_recover() {
    for unsupported in [
        "local x = 1 + 2",
        "local x = call(1)",
        "local x = object[1]",
        "local x = {1, 2}",
        "local x = {[1] = 'one'}",
        "local function f() end",
        "if true then end",
        "---@type number[]",
        "---@type fun(x: number): string",
        "---@class MyClass",
        "---@type { [string]: number }",
        "---@type number |",
    ] {
        let source = format!("{unsupported}\nlocal recovery = 7\n");
        let file = parse(&source);
        assert!(
            !file.diagnostics.is_empty(),
            "unsupported form escaped diagnosis: {unsupported}"
        );
        assert!(file.statements.iter().any(|stmt| {
            matches!(&stmt.kind, StmtKind::Local { names, .. } if names.iter().any(|name| name.text == "recovery"))
        }), "did not recover after {unsupported}: {:?}", file.diagnostics);
    }
}

#[test]
fn trailing_annotations_do_not_silently_attach_to_the_next_local() {
    let file = parse("local x = 1 ---@type string\nlocal y = true\n");
    assert!(!file.diagnostics.is_empty());
    assert!(locals(&file, 1).2.is_none());
}

#[test]
fn unclosed_short_string_recovers_at_the_following_line() {
    let file = parse("local x = 'missing\nlocal recovery = 7\n");
    assert!(
        file.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unterminated string"))
    );
    assert_eq!(locals(&file, 1).0[0].text, "recovery");
}

#[test]
fn malformed_long_doc_comments_never_panic() {
    for source in ["--[==[---", "--[[---@type number", "--[=[---@type number"] {
        let file = parse(source);
        assert!(!file.diagnostics.is_empty());
    }
}

#[test]
fn missing_initializer_and_bad_escape_have_source_diagnostics() {
    for source in [
        "local x =",
        "local x = '\\256'",
        "local x = '\\x0'",
        "local x = 1e",
    ] {
        let file = parse(source);
        assert!(!file.diagnostics.is_empty(), "{source}");
        assert!(
            file.diagnostics
                .iter()
                .all(|diagnostic| diagnostic.span.start <= diagnostic.span.end
                    && diagnostic.span.end <= source.len())
        );
    }
}

#[test]
fn excessive_field_chain_is_rejected_at_its_original_lua_span() {
    let chain = format!("root{}", ".field".repeat(8192));
    let source = format!("local value = {chain}\nlocal recovery = 7\n");
    let file = parse(&source);
    let diagnostics = &file.diagnostics;
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].message.contains("nesting exceeds"));
    assert_eq!(
        &source[diagnostics[0].span.start..diagnostics[0].span.end],
        ".field"
    );
    assert_eq!(locals(&file, 1).0[0].text, "recovery");
    assert_eq!(locals(&file, 0).1[0].kind, ExprKind::Error);
    // Exercise cloning and recursive destruction, not just parsing the input.
    drop(file.clone());
    drop(file);
}

#[test]
fn field_nesting_bound_includes_surrounding_parentheses_and_tables() {
    let chain = format!("root{}", ".field".repeat(120));
    let expression = format!("{}{}{}", "({ value = ".repeat(20), chain, " })".repeat(20));
    let source = format!("local value = {expression}\nlocal recovery = 7\n");
    let file = parse(&source);
    assert!(
        file.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("nesting exceeds"))
    );
    assert_eq!(locals(&file, 1).0[0].text, "recovery");
    drop(file.clone());
    drop(file);
}
