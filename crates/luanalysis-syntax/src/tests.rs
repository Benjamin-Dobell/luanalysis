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
    let source = "local x, y, z = - 0x1.fp-2, .25, 1.e+2\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let values = locals(&file, 0).1;
    let ExprKind::Unary { op, op_span, value } = &values[0].kind else { panic!("expected unary negation") };
    assert_eq!(*op, UnaryOperator::Negate);
    assert_eq!(&source[op_span.start..op_span.end], "-");
    assert_eq!(value.kind, ExprKind::Number("0x1.fp-2".to_owned()));
    assert_eq!(values[1].kind, ExprKind::Number(".25".to_owned()));
    assert_eq!(values[2].kind, ExprKind::Number("1.e+2".to_owned()));
}

#[test]
fn unsupported_lua_and_annotation_forms_are_reported_and_recover() {
    for unsupported in [
        "local x = {1, 2}",
        "local x = {[1] = 'one'}",
        "local function f(..., value) end",
        "---@type number[]",
        "---@type fun<T>(x: T): T",
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

fn expression_shape(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Name(name) => name.text.clone(),
        ExprKind::Number(spelling) => spelling.clone(),
        ExprKind::Unary { op, value, .. } => format!("{op:?}({})", expression_shape(value)),
        ExprKind::Binary { op, left, right, .. } => format!("{op:?}({},{})", expression_shape(left), expression_shape(right)),
        ExprKind::Parenthesized(value) => format!("Paren({})", expression_shape(value)),
        ExprKind::Field { base, name } => format!("Field({},{})", expression_shape(base), name.text),
        ExprKind::Call { callee, args } => format!("Call({};{})", expression_shape(callee), args.iter().map(expression_shape).collect::<Vec<_>>().join(",")),
        other => panic!("unexpected expression for shape assertion: {other:?}"),
    }
}

#[test]
fn operators_follow_lua_precedence_and_associativity() {
    for (expression, expected) in [
        ("-2^2", "Negate(Power(2,2))"),
        ("2^-2", "Power(2,Negate(2))"),
        ("2^3^2", "Power(2,Power(3,2))"),
        ("2^-2^3", "Power(2,Negate(Power(2,3)))"),
        ("a..b..c", "Concat(a,Concat(b,c))"),
        ("a-b-c", "Subtract(Subtract(a,b),c)"),
        ("not a == b", "Equal(Not(a),b)"),
        ("a or b and c", "Or(a,And(b,c))"),
        ("a+b..c+d", "Concat(Add(a,b),Add(c,d))"),
        ("a|b~c&d<<e", "BitwiseOr(a,BitwiseXor(b,BitwiseAnd(c,ShiftLeft(d,e))))"),
        ("a<<b..c", "ShiftLeft(a,Concat(b,c))"),
        ("a<b<c", "Less(Less(a,b),c)"),
        ("#f().x^2", "Length(Power(Field(Call(f;),x),2))"),
        ("a - -b", "Subtract(a,Negate(b))"),
        ("a--b", "a"),
    ] {
        let source = format!("local result = {expression}\n");
        let file = parse(&source);
        assert!(file.diagnostics.is_empty(), "{expression}: {:?}", file.diagnostics);
        assert_eq!(expression_shape(&locals(&file, 0).1[0]), expected, "{expression}");
    }
}

#[test]
fn every_binary_operator_retains_its_original_token_span() {
    for (token, expected) in [
        ("+", BinaryOperator::Add), ("-", BinaryOperator::Subtract), ("*", BinaryOperator::Multiply),
        ("/", BinaryOperator::Divide), ("//", BinaryOperator::FloorDivide), ("%", BinaryOperator::Modulo),
        ("^", BinaryOperator::Power), ("..", BinaryOperator::Concat), ("==", BinaryOperator::Equal),
        ("~=", BinaryOperator::NotEqual), ("<", BinaryOperator::Less), ("<=", BinaryOperator::LessEqual),
        (">", BinaryOperator::Greater), (">=", BinaryOperator::GreaterEqual), ("&", BinaryOperator::BitwiseAnd),
        ("|", BinaryOperator::BitwiseOr), ("~", BinaryOperator::BitwiseXor), ("<<", BinaryOperator::ShiftLeft),
        (">>", BinaryOperator::ShiftRight), ("and", BinaryOperator::And), ("or", BinaryOperator::Or),
    ] {
        let source = format!("local result = left {token} right\n");
        let file = parse(&source);
        assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
        let ExprKind::Binary { op, op_span, .. } = &locals(&file, 0).1[0].kind else { panic!("expected binary {token}") };
        assert_eq!(*op, expected);
        assert_eq!(&source[op_span.start..op_span.end], token);
    }
}

#[test]
fn indexed_access_assignment_and_method_calls_preserve_lua_spans() {
    let source = "local value = object[key + 1].name\nobject['x'] = value\nlocal result = object:method(value, key)\nobject:notify()\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let ExprKind::Field { base, .. } = &locals(&file, 0).1[0].kind else { panic!("expected field") };
    let ExprKind::Index { index, .. } = &base.kind else { panic!("expected index") };
    assert_eq!(&source[index.span.start..index.span.end], "key + 1");
    let StmtKind::Assign { targets, .. } = &file.statements[1].kind else { panic!("expected index assignment") };
    assert!(matches!(targets[0].kind, ExprKind::Index { .. }));
    let ExprKind::MethodCall { receiver, name, args } = &locals(&file, 2).1[0].kind else { panic!("expected method call") };
    assert_eq!(&source[receiver.span.start..receiver.span.end], "object");
    assert_eq!(&source[name.span.start..name.span.end], "method");
    assert_eq!(args.len(), 2);
    assert!(matches!(file.statements[3].kind, StmtKind::Call(Expr { kind: ExprKind::MethodCall { .. }, .. })));
}

#[test]
fn native_vector_arithmetic_fixture_parses_without_translation() {
    let file = parse(include_str!("../tests/fixtures/vector-operators.lua"));
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    assert_eq!(file.statements.len(), 7);
}

#[test]
fn lua_rejects_adjoining_numeric_dots_before_concatenation() {
    for numeral in ["1..2", "1.. 2", ".1..2", "0x1..2", "1...2"] {
        let file = parse(&format!("local value = {numeral}\n"));
        assert!(file.diagnostics.iter().any(|diagnostic| diagnostic.message.contains("numeric literal")), "{numeral}: {:?}", file.diagnostics);
    }
    for expression in ["1 ..2", "1. ..2", "0xe+1"] {
        let file = parse(&format!("local value = {expression}\n"));
        assert!(file.diagnostics.is_empty(), "{expression}: {:?}", file.diagnostics);
    }
}

#[test]
fn malformed_operators_and_index_suffixes_recover_at_the_next_statement() {
    for expression in ["1 +", "not", "object[1 + ]", "object:method", "1 + )", "#"] {
        let source = format!("local value = {expression}\nlocal recovery = 7\n");
        let file = parse(&source);
        assert!(!file.diagnostics.is_empty(), "{expression}");
        assert!(file.statements.iter().any(|stmt| matches!(&stmt.kind, StmtKind::Local { names, .. } if names.iter().any(|name| name.text == "recovery"))), "{expression}: {:?}", file.diagnostics);
    }
}

#[test]
fn constructed_operator_and_index_chains_remain_bounded() {
    for expression in [
        format!("root{}", " + value".repeat(8192)),
        format!("root{}", " ^ value".repeat(8192)),
        format!("{}root", "not ".repeat(1024)),
        format!("root{}", "[index]".repeat(8192)),
        format!("root{}", ":method()".repeat(8192)),
    ] {
        let source = format!("local value = {expression}\nlocal recovery = 7\n");
        let file = parse(&source);
        assert!(file.diagnostics.iter().any(|diagnostic| diagnostic.message.contains("nesting exceeds")));
        assert_eq!(locals(&file, 1).0[0].text, "recovery");
        drop(file.clone());
        drop(file);
    }
}

#[test]
fn native_functions_blocks_calls_and_annotations_keep_source_locations() {
    let source = include_str!("../tests/fixtures/functions.lua");
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    assert_eq!(file.statements.len(), 5);
    let StmtKind::LocalFunction { name, function } = &file.statements[0].kind else { panic!("expected local function") };
    assert_eq!(name.text, "identity");
    assert_eq!(&source[name.span.start..name.span.end], "identity");
    assert_eq!(&source[function.params[0].span.start..function.params[0].span.end], "value");
    assert_eq!(source.as_bytes()[function.body_span.start - 1], b')');
    assert!(source[function.body_span.end..].starts_with("end"));
    let param = &function.annotations.params[0];
    assert_eq!(&source[param.name.span.start..param.name.span.end], "value");
    assert_eq!(&source[param.ty.span.start..param.ty.span.end], "number");
    assert_eq!(function.annotations.returns.as_ref().unwrap().types[0].kind, TypeExprKind::Named("number".to_owned()));
    let StmtKind::Block { body, body_span } = &function.body[0].kind else { panic!("expected lexical block") };
    assert_eq!(body.len(), 1);
    assert!(source[body_span.start..body_span.end].contains("local copied = value"));
    assert!(matches!(function.body[1].kind, StmtKind::Return(_)));
    let ExprKind::Function(function) = &locals(&file, 1).1[0].kind else { panic!("expected function expression") };
    assert_eq!(function.annotations.params[0].name.text, "flag");
    assert_eq!(function.annotations.returns.as_ref().unwrap().types[0].kind, TypeExprKind::Named("string".to_owned()));
    let ExprKind::Call { callee, args } = &locals(&file, 3).1[0].kind else { panic!("expected call") };
    assert_eq!(&source[callee.span.start..callee.span.end], "boxed.callback");
    assert_eq!(&source[args[0].span.start..args[0].span.end], "identity(1)");
    assert!(matches!(file.statements[4].kind, StmtKind::Call(_)));
}

#[test]
fn function_type_annotations_preserve_optional_parameters_and_return_lists() {
    let source = "---@type fun(value: number, flag?: boolean): (string, nil)\nlocal callback = function(value, flag) return 'yes', nil end\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let (_, values, annotation) = locals(&file, 0);
    let TypeExprKind::Function(function_type) = &annotation.unwrap().types[0].kind else { panic!("expected function type") };
    assert_eq!(function_type.params.len(), 2);
    assert_eq!(function_type.params[0].name.text, "value");
    assert!(function_type.params[1].optional);
    assert_eq!(function_type.returns.len(), 2);
    assert_eq!(&source[function_type.params[1].span.start..function_type.params[1].span.end], "flag?: boolean");
    let ExprKind::Function(function) = &values[0].kind else { panic!("expected function") };
    let StmtKind::Return(values) = &function.body[0].kind else { panic!("expected return") };
    assert_eq!(values.len(), 2);
}

#[test]
fn callback_function_types_respect_surrounding_parameter_and_record_commas() {
    let file = parse("---@type fun(callback: fun(value: number): string, value: number): string\nlocal apply = function(callback, value) return callback(value) end\n---@type {callback: fun(value: number): string, count: number}\nlocal object = {callback = apply, count = 1}\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let TypeExprKind::Function(function) = &locals(&file, 0).2.unwrap().types[0].kind else { panic!("expected function type") };
    assert_eq!(function.params.len(), 2);
    let TypeExprKind::Function(callback) = &function.params[0].ty.as_ref().unwrap().kind else { panic!("expected callback type") };
    assert_eq!(callback.returns.len(), 1);
    let TypeExprKind::Record(fields) = &locals(&file, 1).2.unwrap().types[0].kind else { panic!("expected record") };
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[1].name.text, "count");
}

#[test]
fn annotations_attach_to_the_correct_function_and_do_not_escape_blocks() {
    let source = "do\n---@param x number\n---@return number\nlocal function nested(x) return x end\nend\nlocal function plain(x) return x end\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let StmtKind::Block { body, .. } = &file.statements[0].kind else { panic!("expected block") };
    let StmtKind::LocalFunction { function, .. } = &body[0].kind else { panic!("expected nested function") };
    assert_eq!(function.annotations.params.len(), 1);
    let StmtKind::LocalFunction { function, .. } = &file.statements[1].kind else { panic!("expected plain function") };
    assert_eq!(function.annotations, FunctionAnnotations::default());
    let orphan = parse("do\n---@param x number\nend\nlocal function next(x) return x end\n");
    assert!(!orphan.diagnostics.is_empty());
    let StmtKind::LocalFunction { function, .. } = &orphan.statements[1].kind else { panic!("expected next function") };
    assert!(function.annotations.params.is_empty());
}

#[test]
fn function_type_constraints_and_context_share_the_original_annotation() {
    for initializer in ["function(x) return x end", "(function(x) return x end)", "((function(x) return x end))"] {
        let source = format!("---@type fun(x: number): number\n---@param x number\n---@return number\nlocal identity = {initializer}\nlocal result = identity(1)\n");
        let file = parse(&source);
        assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
        let (_, values, annotation) = locals(&file, 0);
        let annotation = annotation.unwrap();
        let mut value = &values[0];
        while let ExprKind::Parenthesized(inner) = &value.kind { value = inner; }
        let ExprKind::Function(function) = &value.kind else { panic!("expected function initializer") };
        assert_eq!(function.annotations.type_annotation.as_ref(), Some(annotation));
        assert_eq!(function.annotations.params.len(), 1);
        assert!(function.annotations.returns.is_some());
        assert_eq!(&source[function.span.start..function.span.end], "function(x) return x end");
        assert_eq!(&source[annotation.span.start..annotation.span.end], "@type fun(x: number): number");
    }
}

#[test]
fn scalar_constraints_do_not_become_function_context() {
    let file = parse("---@type number\nlocal f = (function(x) return x end)\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let (_, values, annotation) = locals(&file, 0);
    assert!(annotation.is_some());
    let ExprKind::Parenthesized(value) = &values[0].kind else { panic!("expected parentheses") };
    let ExprKind::Function(function) = &value.kind else { panic!("expected function") };
    assert!(function.annotations.type_annotation.is_none());
    let alias = parse("---@type fun(x: number): number\nlocal alias = identity\n---@type fun(x: number): number\nalias = identity\n");
    assert!(alias.diagnostics.is_empty(), "{:?}", alias.diagnostics);
    assert!(locals(&alias, 0).2.is_some());
}

#[test]
fn return_accepts_one_terminal_semicolon_and_diagnoses_additional_ones() {
    for source in ["return;", "local function f() return 1; end", "do return; end", "return 1 --comment\n;"] {
        let file = parse(source);
        assert!(file.diagnostics.is_empty(), "{source}: {:?}", file.diagnostics);
    }
    for source in ["return;;", "local function f() return 1;; end", "return; --comment\n;"] {
        let file = parse(source);
        assert_eq!(file.diagnostics.len(), 1, "{source}: {:?}", file.diagnostics);
        let diagnostic = &file.diagnostics[0];
        assert!(diagnostic.message.contains("at most one terminal semicolon"));
        assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], ";");
        assert_eq!(diagnostic.span.start, source.rfind(';').unwrap());
    }
}

#[test]
fn missing_end_bad_parameters_and_annotations_are_diagnosed() {
    for source in [
        "do local x = 1", "local function f(x,) return x end", "local function f(..., value) end",
        "---@param x number\nlocal x = 1", "---@param x number\n---@param x string\nlocal function f(x) end",
        "local function f() return 1 local x = 2 end", "local x = f(1,)",
        "---@generic T = number\nlocal function f(x) return x end", "function global[index]() end",
    ] {
        let file = parse(source);
        assert!(!file.diagnostics.is_empty(), "invalid or unsupported input escaped diagnosis: {source}");
    }
}

#[test]
fn named_global_member_and_colon_definitions_retain_original_targets() {
    let source = "---@param value number\n---@return number\nfunction identity(value) return value end\nfunction namespace.nested.member(value) return value end\nfunction namespace.nested:method(value) return self end\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    assert_eq!(file.statements.len(), 3);
    let StmtKind::FunctionDeclaration { target, function, method_span } = &file.statements[0].kind else { panic!("expected global definition") };
    assert!(matches!(target.kind, ExprKind::Name(_)));
    assert_eq!(&source[target.span.start..target.span.end], "identity");
    assert!(method_span.is_none());
    assert_eq!(function.annotations.params[0].name.text, "value");
    assert!(function.annotations.returns.is_some());
    assert_eq!(function.params.len(), 1);
    assert_eq!(source.as_bytes()[function.body_span.start - 1], b')');
    let StmtKind::FunctionDeclaration { target, function, method_span } = &file.statements[1].kind else { panic!("expected member definition") };
    assert_eq!(&source[target.span.start..target.span.end], "namespace.nested.member");
    assert!(matches!(target.kind, ExprKind::Field { .. }));
    assert!(method_span.is_none());
    assert!(source[function.span.start..function.span.end].starts_with("function namespace.nested.member"));
    let StmtKind::FunctionDeclaration { target, function, method_span } = &file.statements[2].kind else { panic!("expected method definition") };
    assert_eq!(&source[target.span.start..target.span.end], "namespace.nested:method");
    let colon = method_span.unwrap();
    assert_eq!(&source[colon.start..colon.end], ":");
    let ExprKind::Field { base, name } = &target.kind else { panic!("expected method field target") };
    assert_eq!(&source[base.span.start..base.span.end], "namespace.nested");
    assert_eq!(name.text, "method");
    assert_eq!(function.params.iter().map(|name| name.text.as_str()).collect::<Vec<_>>(), ["value"]);
}

#[test]
fn actual_ge_tts_captured_self_definitions_parse_in_constructor_scope() {
    // Reduced from ge_tts Instance.ttslua:124 and Vector2.ttslua:83; named return
    // shapes and constructor dependencies are removed, native bodies retained.
    let source = "local function constructor()\nlocal instanceGuid = 'instance'\nlocal self = {x = 1, y = 2}\n---@return string\nfunction self.getInstanceGuid() return instanceGuid end\n---@return {x: number, y: number}\nfunction self.toData() return {x = self.x, y = self.y} end\nreturn self\nend\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else { panic!("expected constructor") };
    assert_eq!(function.body.len(), 5);
    let StmtKind::FunctionDeclaration { target, function: member, .. } = &function.body[2].kind else { panic!("expected captured member") };
    assert_eq!(&source[target.span.start..target.span.end], "self.getInstanceGuid");
    assert!(member.params.is_empty());
    let StmtKind::Return(values) = &member.body[0].kind else { panic!("expected return") };
    assert!(matches!(&values[0].kind, ExprKind::Name(name) if name.text == "instanceGuid"));
    assert!(matches!(function.body[3].kind, StmtKind::FunctionDeclaration { .. }));
}

#[test]
fn named_definition_utf8_crlf_and_generic_annotation_spans_are_native() {
    let source = "---@generic T\r\n---@param value T\r\n---@return T\r\nfunction 日本語:戻る(value) return value end\r\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let StmtKind::FunctionDeclaration { target, function, method_span } = &file.statements[0].kind else { panic!("expected definition") };
    assert_eq!(&source[target.span.start..target.span.end], "日本語:戻る");
    assert_eq!(&source[method_span.unwrap().start..method_span.unwrap().end], ":");
    assert_eq!(function.annotations.generics.len(), 1);
    assert_eq!(&source[function.annotations.generics[0].name.span.start..function.annotations.generics[0].name.span.end], "T");
    assert_eq!(&source[function.params[0].span.start..function.params[0].span.end], "value");
}

#[test]
fn malformed_named_definition_headers_are_diagnosed_without_losing_next_statement() {
    for invalid in ["function () end", "function module:method.field() end", "function module:method:again() end", "function module[index]() end"] {
        let file = parse(&format!("{invalid}\nlocal recovery = 7\n"));
        assert!(!file.diagnostics.is_empty(), "{invalid}");
        assert!(file.statements.iter().any(|stmt| matches!(&stmt.kind, StmtKind::Local { names, .. } if names.iter().any(|name| name.text == "recovery"))), "{invalid}: {:?}", file.diagnostics);
    }
}

#[test]
fn long_named_definition_targets_do_not_construct_unbounded_trees() {
    let source = format!("function namespace{}() end\nlocal recovery = 7\n", ".member".repeat(8192));
    let file = parse(&source);
    assert!(file.diagnostics.iter().any(|diagnostic| diagnostic.message.contains("function name nesting exceeds")));
    assert!(matches!(file.statements[0].kind, StmtKind::FunctionDeclaration { .. }));
    assert_eq!(locals(&file, 1).0[0].text, "recovery");
    drop(file.clone());
    drop(file);
}

#[test]
fn runtime_vararg_headers_and_expression_lists_retain_exact_ellipses() {
    let source = "local function forward(head, ...) return head, ... end\nlocal result = forward(1, 2, 3)\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else { panic!("expected variadic function") };
    assert_eq!(function.params.iter().map(|name| name.text.as_str()).collect::<Vec<_>>(), ["head"]);
    let header = function.vararg.unwrap();
    assert_eq!(&source[header.start..header.end], "...");
    let StmtKind::Return(values) = &function.body[0].kind else { panic!("expected result list") };
    assert_eq!(values.len(), 2);
    assert_eq!(values[1].kind, ExprKind::Vararg);
    assert_eq!(&source[values[1].span.start..values[1].span.end], "...");
    assert!(header.end < function.body_span.start);
    assert!(function.body_span.start < values[1].span.start);
}

#[test]
fn parentheses_scalarization_and_final_vararg_expansion_remain_distinct() {
    let source = "local function forward(...) return target((...), ...) end\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else { panic!("expected function") };
    let StmtKind::Return(values) = &function.body[0].kind else { panic!("expected return") };
    let ExprKind::Call { args, .. } = &values[0].kind else { panic!("expected forwarding call") };
    let ExprKind::Parenthesized(first) = &args[0].kind else { panic!("expected scalarized vararg") };
    assert_eq!(first.kind, ExprKind::Vararg);
    assert_eq!(args[1].kind, ExprKind::Vararg);
    assert_eq!(&source[args[0].span.start..args[0].span.end], "(...)");
    assert_eq!(&source[args[1].span.start..args[1].span.end], "...");
}

#[test]
fn vararg_scope_is_reset_by_each_function_and_restored_after_it() {
    let source = "local function outer(...)\nlocal inner = function() return ... end\nreturn ...\nend\nreturn ...\n";
    let file = parse(source);
    assert_eq!(file.diagnostics.len(), 1, "{:?}", file.diagnostics);
    let diagnostic = &file.diagnostics[0];
    assert!(diagnostic.message.contains("outside a variadic"));
    assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], "...");
    assert_eq!(diagnostic.span.start, source.find("return ...").unwrap() + "return ".len());
    // A Lua chunk is itself variadic; it has no fabricated header token.
    assert!(parse("return ...").diagnostics.is_empty());
    assert!(parse("local function f(...) do return ... end end").diagnostics.is_empty());
    assert!(!parse("local function f() return ... end").diagnostics.is_empty());
}

#[test]
fn runtime_ellipsis_is_the_final_parameter_and_has_crlf_byte_spans() {
    let source = "function 日本語:forward(value, ...)\r\nreturn ...\r\nend\r\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let StmtKind::FunctionDeclaration { function, .. } = &file.statements[0].kind else { panic!("expected definition") };
    let span = function.vararg.unwrap();
    assert_eq!(&source[span.start..span.end], "...");
    assert_eq!(span.start, source.find("...").unwrap());
    for invalid in ["local function f(..., value) end", "local function f(value, ...,) end", "local function f(..., ...) end"] {
        let file = parse(invalid);
        assert!(file.diagnostics.iter().any(|diagnostic| diagnostic.message.contains("final parameter")), "{invalid}: {:?}", file.diagnostics);
    }
}

#[test]
fn control_flow_clauses_and_loop_headers_preserve_original_spans() {
    let source = "if first then\nlocal value = 1\nelseif second then\nlocal value = 2\nelse\nlocal value = 3\nend\nwhile count < 3 do\ncount = count + 1\nbreak\nend\nrepeat\nlocal finished = count > 0\nuntil finished\nfor index = 1, 10, -2 do\nlocal value = index\nend\nfor key, value in iterator(state), initial do\nlocal copy = value\nend\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    assert_eq!(file.statements.len(), 5);
    let StmtKind::If { branches, else_branch } = &file.statements[0].kind else { panic!("expected if") };
    assert_eq!(branches.len(), 2);
    assert_eq!(&source[branches[1].condition.span.start..branches[1].condition.span.end], "second");
    assert!(source[branches[0].body_span.end..].starts_with("elseif"));
    assert!(source[branches[1].body_span.end..].starts_with("else"));
    let else_branch = else_branch.as_ref().unwrap();
    assert!(source[else_branch.body_span.start..else_branch.body_span.end].contains("local value = 3"));
    let StmtKind::While { body, body_span, .. } = &file.statements[1].kind else { panic!("expected while") };
    assert!(matches!(body[1].kind, StmtKind::Break));
    assert!(source[body_span.end..].starts_with("end"));
    let StmtKind::Repeat { body, condition, body_span } = &file.statements[2].kind else { panic!("expected repeat") };
    assert_eq!(body.len(), 1);
    assert!(source[body_span.end..].starts_with("until"));
    assert_eq!(&source[condition.span.start..condition.span.end], "finished");
    assert_eq!(file.statements[2].span.end, condition.span.end);
    let StmtKind::NumericFor { name, start, limit, step, .. } = &file.statements[3].kind else { panic!("expected numeric for") };
    assert_eq!(name.text, "index");
    assert_eq!(&source[start.span.start..start.span.end], "1");
    assert_eq!(&source[limit.span.start..limit.span.end], "10");
    assert_eq!(expression_shape(step.as_ref().unwrap()), "Negate(2)");
    let StmtKind::GenericFor { names, values, body, .. } = &file.statements[4].kind else { panic!("expected generic for") };
    assert_eq!(names.iter().map(|name| name.text.as_str()).collect::<Vec<_>>(), ["key", "value"]);
    assert_eq!(values.len(), 2);
    assert_eq!(body.len(), 1);
}

#[test]
fn returns_are_terminal_within_each_if_branch() {
    let file = parse("local function choose(value) if value then return 1; elseif other then return 2 else return 3 end end\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let invalid = parse("if value then return 1 local misplaced = 2 else return 3 end\n");
    assert!(invalid.diagnostics.iter().any(|diagnostic| diagnostic.message.contains("return must be")));
}

#[test]
fn break_scope_does_not_cross_function_boundaries() {
    let source = "while true do\nlocal function invalid() break end\ndo break end\nend\n";
    let file = parse(source);
    assert_eq!(file.diagnostics.len(), 1, "{:?}", file.diagnostics);
    let diagnostic = &file.diagnostics[0];
    assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], "break");
    assert!(diagnostic.message.contains("outside a loop"));
    assert_eq!(diagnostic.span.start, source.find("break").unwrap());
    assert!(!parse("break").diagnostics.is_empty());
}

#[test]
fn malformed_control_headers_report_errors_and_recover_after_the_block() {
    for invalid in [
        "if true local value = 1 end", "while true local value = 1 end",
        "repeat local value = 1 until", "for index = 1 do end", "for key iterator do end",
        "if true then elseif then end",
    ] {
        let source = format!("{invalid}\nlocal recovery = 7\n");
        let file = parse(&source);
        assert!(!file.diagnostics.is_empty(), "{invalid}");
        assert!(file.statements.iter().any(|stmt| matches!(&stmt.kind, StmtKind::Local { names, .. } if names.iter().any(|name| name.text == "recovery"))), "{invalid}: {:?}", file.diagnostics);
    }
}

#[test]
fn nested_control_statements_are_bounded_and_recover_without_stack_overflow() {
    for (opening, closing) in [("if true then\n", "end\n"), ("while true do\n", "end\n"), ("repeat\n", "until true\n")] {
        let source = format!("{}local value = 1\n{}local recovery = 7\n", opening.repeat(1024), closing.repeat(1024));
        let file = parse(&source);
        assert!(file.diagnostics.iter().any(|diagnostic| diagnostic.message.contains("nesting exceeds")), "{opening}");
        assert!(file.statements.iter().any(|stmt| matches!(&stmt.kind, StmtKind::Local { names, .. } if names.iter().any(|name| name.text == "recovery"))), "{opening}: {:?}", file.diagnostics);
        drop(file.clone());
        drop(file);
    }
}

#[test]
fn postfix_operations_require_lua_prefix_expressions() {
    for invalid in ["function() return 1 end()", "{x = 1}.x", "true[1]", "'value':method()", "nil.field"] {
        let file = parse(&format!("local value = {invalid}\n"));
        assert!(file.diagnostics.iter().any(|diagnostic| diagnostic.message.contains("postfix access")), "{invalid}: {:?}", file.diagnostics);
    }
    for valid in ["(function() return 1 end)()", "({x = 1}).x", "(true)[1]", "('value'):method()", "(nil).field"] {
        let file = parse(&format!("local value = {valid}\n"));
        assert!(file.diagnostics.is_empty(), "{valid}: {:?}", file.diagnostics);
    }
}


#[test]
fn excessive_call_chains_and_mixed_function_trees_are_bounded() {
    let source = format!("local value = root{}\nlocal recovery = 7\n", "()".repeat(8192));
    let file = parse(&source);
    assert_eq!(file.diagnostics.len(), 1);
    assert!(file.diagnostics[0].message.contains("nesting exceeds"));
    assert_eq!(&source[file.diagnostics[0].span.start..file.diagnostics[0].span.end], "()");
    assert_eq!(locals(&file, 1).0[0].text, "recovery");
    drop(file.clone());
    drop(file);
    let nested = format!("local value = {}root{}{}\nlocal recovery = 7\n", "function() return ".repeat(40), ".field".repeat(120), " end".repeat(40));
    let file = parse(&nested);
    assert!(file.diagnostics.iter().any(|diagnostic| diagnostic.message.contains("nesting exceeds")));
    assert_eq!(locals(&file, 1).0[0].text, "recovery");
    drop(file.clone());
    drop(file);
}

#[test]
fn deeply_nested_do_blocks_and_functions_recover_without_stack_overflow() {
    for source in [
        format!("{}local value = 1\n{}\nlocal recovery = 7\n", "do\n".repeat(1024), "end\n".repeat(1024)),
        format!("local value = {}1{}\nlocal recovery = 7\n", "function() return ".repeat(1024), " end".repeat(1024)),
    ] {
        let file = parse(&source);
        assert!(file.diagnostics.iter().any(|diagnostic| diagnostic.message.contains("nesting exceeds")));
        assert!(file.statements.iter().any(|stmt| matches!(&stmt.kind, StmtKind::Local { names, .. } if names.iter().any(|name| name.text == "recovery"))));
        drop(file.clone());
        drop(file);
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
