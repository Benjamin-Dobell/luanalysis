use luanalysis::formatting::{FormattingOptions, Newline, format};
use luanalysis_syntax::{CommentKind, Span, TokenKind};

fn apply(source: &str, options: &FormattingOptions, range: Option<Span>) -> String {
    let edits = format(source, options, range).expect("valid Lua formats safely");
    let mut result = source.to_owned();
    for edit in edits.into_iter().rev() {
        if let Some(bounds) = range {
            assert!(edit.span.start >= bounds.start && edit.span.end <= bounds.end);
        }
        assert!(!(source.as_bytes().get(edit.span.start) == Some(&b'\n')
            && source.as_bytes().get(edit.span.start.wrapping_sub(1)) == Some(&b'\r')));
        assert!(!(source.as_bytes().get(edit.span.end) == Some(&b'\n')
            && source.as_bytes().get(edit.span.end.wrapping_sub(1)) == Some(&b'\r')));
        result.replace_range(edit.span.start..edit.span.end, &edit.new_text);
    }
    let executable = |text: &str| {
        luanalysis_syntax::lex(text).tokens.into_iter().filter(|token| !matches!(token.kind,
            TokenKind::Comment(_) | TokenKind::Newline | TokenKind::Eof))
            .map(|token| text[token.span.start..token.span.end].to_owned()).collect::<Vec<_>>()
    };
    assert_eq!(executable(source), executable(&result));
    assert!(!luanalysis_syntax::parse(&result).has_lua_syntax_errors());
    if range.is_none() { assert!(format(&result, options, None).unwrap().is_empty(), "{result}"); }
    result
}

#[test]
fn long_overloads_use_multiline_annotations_by_default() {
    // The parameterized overload style used by ge_tts's TableUtils.
    let source = "---@overload fun<V, PV : V, CovariantA : PV, CovariantB : PV, CovariantC : PV>(arr: V[], predicate: (fun(value: PV, index: number): boolean), a: CovariantA[], b: CovariantB[], c: CovariantC[]): void\nlocal function partition(arr, predicate, a, b, c)\nend\n";
    let result = apply(source, &FormattingOptions::default(), None);
    assert!(result.contains("\n---    "), "{result}");
    assert!(result.lines().all(|line| line.chars().count() <= 120), "{result}");
    let file = luanalysis_syntax::parse(&result);
    let signature = &file.overload_annotations[0].signature;
    assert_eq!(signature.generics.len(), 5);
    assert_eq!(signature.params.len(), 5);
    assert_eq!(signature.returns.len(), 1);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    assert_eq!(apply(source, &FormattingOptions { max_line_length: 0, ..FormattingOptions::default() }, None), source);
}

#[test]
fn long_prose_uses_a_real_lua_block_without_touching_code_or_strings() {
    let source = "-- This module operates on tables that contain only positive consecutive integer keys starting at one and preserves the spelling of every value in those tables.\nlocal t = { 'a  quoted string', [=[a long string\nwith exact line breaks]=] }\n";
    let result = apply(source, &FormattingOptions::default(), None);
    assert!(result.starts_with("--[[\n    This module"), "{result}");
    let file = luanalysis_syntax::parse(&result);
    assert_eq!(file.comments.len(), 1);
    assert_eq!(file.comments[0].kind, CommentKind::Long);
}

#[test]
fn crlf_annotations_keep_unicode_descriptions_and_strict_range_bounds() {
    let source = "---@param callback fun(value: number, index: number, context: string): boolean # Vérifie chaque élément et conserve exactement les mots de cette description.\r\nlocal function each(callback) end\r\n";
    let options = FormattingOptions { max_line_length: 80, newline: Some(Newline::CrLf), ..FormattingOptions::default() };
    let result = apply(source, &options, None);
    assert!(result.contains("\r\n---    "), "{result}");
    assert!(!result.replace("\r\n", "").contains(['\r', '\n']));
    let before = luanalysis_syntax::parse(source);
    let after = luanalysis_syntax::parse(&result);
    let prose = |file: &luanalysis_syntax::File, text: &str| {
        let docs = luanalysis_syntax::documentation_above(file, text, text.find("local function").unwrap());
        docs.param("callback").unwrap().split_whitespace().collect::<Vec<_>>().join(" ")
    };
    assert_eq!(prose(&before, source), prose(&after, &result));
    assert_eq!(apply(source, &options, Some(Span::new(3, source.find("\r\n").unwrap()))), source);
}

#[test]
fn structured_comments_and_comment_directives_keep_their_physical_lines() {
    let source = "-- region a long title containing several words and preserving the native region marker\n-- luacheck: globals these names belong to the original line for the external linter\n---@diagnostic disable-next-line: missing-fields, undefined-global, unused-local\nlocal value = 1 -- This inline comment also needs its original place beside its value\n-- endregion a long title containing several words and preserving the native region marker\n";
    assert_eq!(apply(source, &FormattingOptions { max_line_length: 40, ..FormattingOptions::default() }, None), source);
}

#[test]
fn callback_and_table_field_comments_retain_their_original_owners() {
    use luanalysis_syntax::{ExprKind, StmtKind};
    let source = "invoke(\n---@param callback fun(value: number, index: number, context: string): boolean\nfunction(callback) return callback end,\nfunction(plain) return plain end)\nlocal callbacks = {\n---@param field fun(value: number, index: number, context: string): boolean\nfirst = function(field) return field end,\nsecond = function(plain) return plain end\n}\n";
    let result = apply(source, &FormattingOptions { max_line_length: 60, ..FormattingOptions::default() }, None);
    assert!(result.contains("---    "), "{result}");
    for text in [source, result.as_str()] {
        let file = luanalysis_syntax::parse(text);
        assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
        let StmtKind::Call(call) = &file.statements[0].kind else { panic!() };
        let ExprKind::Call { args, .. } = &call.kind else { panic!() };
        let ExprKind::Function(first) = &args[0].kind else { panic!() };
        let ExprKind::Function(second) = &args[1].kind else { panic!() };
        assert_eq!(first.annotations.params[0].name.text, "callback");
        assert!(second.annotations.params.is_empty());
        let StmtKind::Local { values, .. } = &file.statements[1].kind else { panic!() };
        let ExprKind::Table(fields) = &values[0].kind else { panic!() };
        let ExprKind::Function(first) = &fields[0].value.kind else { panic!() };
        let ExprKind::Function(second) = &fields[1].value.kind else { panic!() };
        assert_eq!(first.annotations.params[0].name.text, "field");
        assert!(second.annotations.params.is_empty());
    }
}

#[test]
fn multiline_casts_keep_the_same_operand_and_runtime_neighbors() {
    use luanalysis_syntax::{ExprKind, StmtKind};
    let source = "local typed =\n---@type { first: number, second: string, third: boolean, fourth: number }\nfoo() + bar()\nlocal excluded =\n---@not nil | string | number | boolean\nfoo() + bar()\nlocal value = nil\n---@cast value { first: number, second: string, third: boolean, fourth: number }\nconsume(value)\n";
    let result = apply(source, &FormattingOptions { max_line_length: 45, ..FormattingOptions::default() }, None);
    assert!(result.contains("---    "), "{result}");
    for text in [source, result.as_str()] {
        let file = luanalysis_syntax::parse(text);
        assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
        assert_eq!(file.expression_assertions.len(), 3);
        for assertion in &file.expression_assertions[..2] {
            assert_eq!(&text[assertion.expression.start..assertion.expression.end], "foo()");
        }
        assert_eq!(file.statements.len(), 5);
        assert!(matches!(&file.statements[2].kind, StmtKind::Local { names, .. } if names[0].text == "value"));
        assert!(matches!(&file.statements[3].kind, StmtKind::Assign { targets, .. }
            if matches!(&targets[0].kind, ExprKind::Name(name) if name.text == "value")));
        assert!(matches!(&file.statements[4].kind, StmtKind::Call(_)));
    }
}

#[test]
fn wrapped_named_declarations_keep_class_fields_and_overload_owners() {
    use luanalysis_syntax::{DocFunctionOwner, NamedTypeDeclarationKind};
    let source = "---@class Owner\n---@field private field fun(value: number, index: number, context: string): boolean\n---@function visit(value: number, index: number, context: string): boolean\n---@overload fun(value: number, index: number, context: string): Owner\nlocal Owner = {}\n";
    let result = apply(source, &FormattingOptions { max_line_length: 55, ..FormattingOptions::default() }, None);
    assert!(result.contains("---    "), "{result}");
    for text in [source, result.as_str()] {
        let file = luanalysis_syntax::parse(text);
        assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
        assert_eq!(file.type_declarations.len(), 1);
        let owner = &file.type_declarations[0];
        assert_eq!(owner.name.text, "Owner");
        assert_eq!(owner.attachment, Some(file.statements[0].span));
        let NamedTypeDeclarationKind::Record { fields, .. } = &owner.kind else { panic!() };
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].visibility.unwrap().kind, luanalysis_syntax::FieldVisibility::Private);
        assert_eq!(file.function_declarations[0].owner, Some(DocFunctionOwner::Contextual(owner.id)));
        assert_eq!(file.function_declarations[0].name.text, "visit");
        assert_eq!(file.function_declarations[0].signature.params.len(), 3);
        assert_eq!(file.overload_annotations[0].named_candidates, [owner.id]);
    }
}

#[test]
fn wrapping_preserves_blank_line_and_ordinary_comment_attachment_barriers() {
    use luanalysis_syntax::StmtKind;
    let source = "---@type { first: number, second: string, third: boolean, fourth: number }\n\nlocal blank = nil\n---@type { first: number, second: string, third: boolean, fourth: number }\n-- This ordinary prose barrier separates the long annotation from the declaration below it and must remain a barrier.\nlocal barrier = nil\n";
    let result = apply(source, &FormattingOptions { max_line_length: 55, ..FormattingOptions::default() }, None);
    for text in [source, result.as_str()] {
        let file = luanalysis_syntax::parse(text);
        assert_eq!(file.statements.len(), 2);
        for statement in &file.statements {
            let StmtKind::Local { annotation, .. } = &statement.kind else { panic!() };
            assert!(annotation.is_none());
        }
    }
}

#[test]
fn unions_write_nil_first_in_every_annotation() {
    let options = FormattingOptions::default();
    let source = "---@class Box\n---@field label string|nil the label, or nil\n---@field size table<string, number | nil> | nil\nlocal Box = {}\n\n---@param value string | nil\n---@return boolean|nil, string? message\nlocal function check(value)\n    return nil\nend\n\nlocal width = --[[---@type number|nil]] (Box.size)\n";
    assert_eq!(
        apply(source, &options, None),
        "---@class Box\n---@field label nil|string the label, or nil\n---@field size nil | table<string, nil | number>\nlocal Box = {}\n\n---@param value nil | string\n---@return nil|boolean, string? message\nlocal function check(value)\n    return nil\nend\n\nlocal width = --[[---@type nil|number]] (Box.size)\n",
    );
    // A union written across `---|` lines keeps its order.
    let rows = "---@alias Mode\n---| \"read\"\n---| nil\nlocal mode = 1\n";
    assert_eq!(apply(rows, &options, None), rows);
}
