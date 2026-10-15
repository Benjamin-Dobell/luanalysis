use luanalysis_syntax::{
    parse, ExprKind, File, Function, NamedTypeDeclarationKind, Span, StmtKind, TypeExprKind,
};

fn local_function(file: &File) -> &Function {
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!("expected original local function")
    };
    function
}

#[test]
fn encoder_parenthesized_callbacks_own_their_nested_return_commas() {
    // ge_tts/lunajson/encoder.lua:18, unchanged original annotation.
    let source = "-- café\r\n---@alias lunajson__GenerateValueEncode fun(nullv: any, dispatcher: lunajson__EncodeDispatcher, push: (fun(component: string): void), replace: (fun(replacer: (fun(builder: string[], next: number): string[], number)): void)): (fun(v: any): void)\r\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let NamedTypeDeclarationKind::Alias { target } = &file.type_declarations[0].kind else {
        panic!("expected original alias")
    };
    let TypeExprKind::Function(outer) = &target.kind else {
        panic!("expected original callable")
    };
    assert_eq!(outer.params.len(), 4);
    let TypeExprKind::Parenthesized(replace) = &outer.params[3].ty.as_ref().unwrap().kind else {
        panic!("expected original parentheses")
    };
    let TypeExprKind::Function(replace) = &replace.kind else {
        panic!("expected replace callback")
    };
    assert_eq!(replace.params.len(), 1);
    let TypeExprKind::Parenthesized(replacer) = &replace.params[0].ty.as_ref().unwrap().kind else {
        panic!("expected replacer parentheses")
    };
    let TypeExprKind::Function(replacer) = &replacer.kind else {
        panic!("expected replacer callback")
    };
    assert_eq!(replacer.returns.len(), 2);
    assert!(replacer.packs.is_none());
    assert_eq!(
        &source[replacer.returns[0].span.start..replacer.returns[0].span.end],
        "string[]"
    );
    assert_eq!(
        &source[replacer.returns[1].span.start..replacer.returns[1].span.end],
        "number"
    );
    assert_eq!(
        &source[target.span.start..target.span.end],
        source.split("Encode ").nth(1).unwrap().trim_end()
    );
}

#[test]
fn callback_parentheses_release_outer_parameter_and_generic_delimiters() {
    for ty in [
        "fun(callback: (fun(): number, string), after: boolean): boolean",
        "Box<(fun(): number, string), boolean>",
    ] {
        let source = format!("---@type {ty}\nlocal callback\n");
        let file = parse(&source);
        assert!(file.diagnostics.is_empty(), "{ty}: {:?}", file.diagnostics);
        let StmtKind::Local {
            annotation: Some(annotation),
            ..
        } = &file.statements[0].kind
        else {
            panic!("expected original annotation")
        };
        match &annotation.types[0].kind {
            TypeExprKind::Function(function) => assert_eq!(function.params.len(), 2),
            TypeExprKind::Generic { args, .. } => assert_eq!(args.len(), 2),
            _ => panic!("expected callable or generic"),
        }
    }
    let file =
        parse("---@type fun(callback: fun(): number, after: string): boolean\nlocal callback\n");
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let StmtKind::Local {
        annotation: Some(annotation),
        ..
    } = &file.statements[0].kind
    else {
        panic!("expected annotation")
    };
    let TypeExprKind::Function(outer) = &annotation.types[0].kind else {
        panic!("expected callable")
    };
    assert_eq!(outer.params.len(), 2);
    let TypeExprKind::Function(inner) = &outer.params[0].ty.as_ref().unwrap().kind else {
        panic!("expected inner callable")
    };
    assert_eq!(inner.returns.len(), 1);
    for ty in ["(number, string)", "Box<(fun(): number, string), boolean"] {
        let source =
            format!("---@type {ty}\nlocal invalid\n---@type number\nlocal recovered = 1\n");
        let file = parse(&source);
        assert!(!file.diagnostics.is_empty(), "{ty}");
        let StmtKind::Local {
            names,
            annotation: Some(annotation),
            ..
        } = &file.statements[1].kind
        else {
            panic!("expected recovered declaration")
        };
        assert_eq!(names[0].text, "recovered");
        assert_eq!(
            annotation.types[0].kind,
            TypeExprKind::Named("number".to_owned())
        );
    }
}

#[test]
fn bare_returns_stop_at_every_original_lua_block_terminator() {
    // Zone.ttslua:504-508 has bare return immediately before elseif.
    let source = "local function choose(value)\n if value == 1 then return\n elseif value == 2 then return\n else return\n end\nend\nlocal function repeat_once() repeat return until true end\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let StmtKind::If {
        branches,
        else_branch,
        ..
    } = &local_function(&file).body[0].kind
    else {
        panic!("expected original conditional")
    };
    let returns = branches
        .iter()
        .flat_map(|branch| &branch.body)
        .chain(&else_branch.as_ref().unwrap().body);
    for statement in returns {
        let StmtKind::Return(values) = &statement.kind else {
            panic!("expected bare return")
        };
        assert!(values.is_empty());
        assert_eq!(&source[statement.span.start..statement.span.end], "return");
    }
    let invalid = parse("local function invalid() return\n local after = 1\nend\n");
    assert!(!invalid.diagnostics.is_empty());
}

#[test]
fn a_literal_rhs_does_not_consume_the_next_parenthesized_assignment() {
    // TableUtils.ttslua:132-133; Lua does not require a semicolon here.
    for separator in ["\n", " ", "\r\n"] {
        let source = format!("local selected = {{}}\nlocal i = 0\ni = i + 1{separator}(--[[---@type number[] ]] selected)[i] = 4\n");
        let file = parse(&source);
        assert!(
            file.diagnostics.is_empty(),
            "{separator:?}: {:?}",
            file.diagnostics
        );
        assert_eq!(file.statements.len(), 4);
        let StmtKind::Assign { targets, .. } = &file.statements[3].kind else {
            panic!("expected separate indexed assignment")
        };
        assert!(matches!(targets[0].kind, ExprKind::Index { .. }));
        assert_eq!(
            &source[file.statements[2].span.start..file.statements[2].span.end],
            "i = i + 1"
        );
        assert_eq!(file.expression_assertions.len(), 1);
        assert_eq!(
            &source[file.expression_assertions[0].expression.start
                ..file.expression_assertions[0].expression.end],
            "selected"
        );
    }
}

#[test]
fn declaration_docs_before_a_returned_closure_keep_the_original_function() {
    // TableUtils.ttslua:11-12.
    let source = "local function iterator()\n---@type fun(value: number): number\nreturn function(value) return value end\nend\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let StmtKind::Return(values) = &local_function(&file).body[0].kind else {
        panic!("expected original return")
    };
    let ExprKind::Function(function) = &values[0].kind else {
        panic!("expected original closure")
    };
    let annotation = function.annotations.type_annotation.as_ref().unwrap();
    assert_eq!(
        &source[annotation.types[0].span.start..annotation.types[0].span.end],
        "fun(value: number): number"
    );
    assert_eq!(
        &source[function.span.start..function.span.end],
        "function(value) return value end"
    );
    assert_eq!(values[0].span, function.span);
    let scalar = parse("local function valid()\n---@type number\nreturn 1\nend\n");
    assert!(scalar.diagnostics.is_empty(), "{:#?}", scalar.diagnostics);
    assert_eq!(scalar.return_annotations.len(), 1);
}

#[test]
fn genuinely_incomplete_corpus_annotations_remain_errors_and_recover() {
    // UI.def.lua:109 is missing the union RHS; Json.ttslua:308 is missing the
    // mandatory complex_ty from doc.bnf's tag_param rule.
    let source = "---@shape UI\n---@field tooltipPosition nil |\n---@alias Recovery number\nlocal recovery = 1\n";
    let file = parse(source);
    let missing = source.find("nil |").unwrap() + "nil |".len();
    assert!(file
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.span == Span::new(missing, missing)));
    assert!(file
        .type_declarations
        .iter()
        .any(|declaration| declaration.name.text == "Recovery"));
    assert!(file.statements.iter().any(|statement| matches!(&statement.kind, StmtKind::Local { names, .. } if names[0].text == "recovery")));
    let source = "local function factory()\n---@param v\nreturn function(v) return v end\nend\nlocal recovery = 1\n";
    let file = parse(source);
    assert_eq!(file.diagnostics.len(), 1, "{:?}", file.diagnostics);
    let missing = source.find("@param v").unwrap() + "@param v".len();
    assert_eq!(file.diagnostics[0].span, Span::new(missing, missing));
    assert!(
        matches!(&file.statements[1].kind, StmtKind::Local { names, .. } if names[0].text == "recovery")
    );
}

#[test]
fn invalid_nonprefix_postfix_syntax_is_rejected_without_losing_following_declarations() {
    for invalid in [
        "function() return 1 end()",
        "{x = 1}.x",
        "true[1]",
        "'value':method()",
        "nil.field",
    ] {
        let source = format!("local value = {invalid}\nlocal recovery = 1\n");
        let file = parse(&source);
        assert!(!file.diagnostics.is_empty(), "{invalid}");
        assert!(file.statements.iter().any(|statement| matches!(&statement.kind, StmtKind::Local { names, .. } if names[0].text == "recovery")), "{invalid}: {:?}", file.diagnostics);
    }
}
