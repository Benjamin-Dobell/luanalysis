use luanalysis::formatting::{FormatEdit, FormattingOptions, Newline, format, on_type};
use luanalysis_syntax::{Span, TokenKind};

fn apply(source: &str, edits: &[FormatEdit]) -> String {
    let mut result = String::new();
    let mut offset = 0;
    for edit in edits {
        assert!(edit.span.start >= offset && edit.span.end <= source.len());
        for boundary in [edit.span.start, edit.span.end] {
            assert!(!(boundary > 0 && source.as_bytes().get(boundary - 1) == Some(&b'\r')
                && source.as_bytes().get(boundary) == Some(&b'\n')), "edit splits CRLF: {edit:?}");
        }
        assert!(source[edit.span.start..edit.span.end].chars().all(char::is_whitespace));
        assert!(edit.new_text.chars().all(char::is_whitespace));
        result.push_str(&source[offset..edit.span.start]);
        result.push_str(&edit.new_text);
        offset = edit.span.end;
    }
    result.push_str(&source[offset..]);
    result
}

fn formatted(source: &str, options: &FormattingOptions) -> String {
    let edits = format(source, options, None).expect("valid Lua should format safely");
    let result = apply(source, &edits);
    assert_eq!(format(&result, options, None), Some(Vec::new()), "formatting must be idempotent:\n{result}");
    assert!(!luanalysis_syntax::parse(&result).has_lua_syntax_errors(), "{result}");
    result
}

fn token_spellings(source: &str) -> Vec<String> {
    luanalysis_syntax::lex(source).tokens.into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::Eof))
        .map(|token| source[token.span.start..token.span.end].to_owned()).collect()
}

#[test]
fn splits_legacy_multiple_statements_and_honours_keep_option() {
    let input = "local a = 1 local b = 2";
    assert_eq!(formatted(input, &FormattingOptions::default()), "local a = 1\nlocal b = 2");
    let options = FormattingOptions { keep_multiple_statements_in_one_line: true, ..FormattingOptions::default() };
    assert_eq!(formatted(input, &options), input);
}

#[test]
fn formats_nested_lua_control_flow_and_operators() {
    let source = "local function f ( a,b ) if a>b then return - a+ b elseif a==b then return not false else local t={a=1,b=2}; return t.a end end";
    let result = formatted(source, &FormattingOptions::default());
    assert_eq!(result, "local function f(a, b)\n    if a > b then\n        return -a + b\n    elseif a == b then\n        return not false\n    else\n        local t = { a = 1, b = 2 };\n        return t.a\n    end\nend");
    assert_eq!(token_spellings(source), token_spellings(&result));
}

#[test]
fn preserves_every_string_comment_cast_and_multiline_annotation_lexeme() {
    let source = "#!/usr/bin/env lua\n---@alias Choice\n---| 'a'\n---| 'b'\n---@param x number\nlocal function f(x)\nlocal t={\n[ [=[long key]=] ]='a\\\'b',-- exact comment  \nvalue=[==[first\r\n  literal  \nlast]==],\n}\n---@cast x number\nreturn --[[---@type number]] x+1\nend\n";
    let result = formatted(source, &FormattingOptions::default());
    assert_eq!(token_spellings(source), token_spellings(&result));
    assert!(result.contains("[==[first\r\n  literal  \nlast]==]"));
    assert!(result.contains("---@alias Choice\n---| 'a'\n---| 'b'"));
    assert!(result.contains("-- exact comment  "));
    assert_eq!(luanalysis_syntax::parse(source).expression_assertions.len(), luanalysis_syntax::parse(&result).expression_assertions.len());
}

#[test]
fn retains_lua_54_attributes_labels_bitwise_and_floor_division() {
    let source = "local x < const > = 7\n:: again ::\nx=(x//2) ~ (1<<2) | 3 & 2\nif x>1 then goto again end";
    let result = formatted(source, &FormattingOptions::default());
    assert!(result.contains("local x <const> = 7"));
    assert!(result.contains("::again::"));
    assert!(result.contains("x = (x // 2) ~ (1 << 2) | 3 & 2"), "{result}");
    assert_eq!(token_spellings(source), token_spellings(&result));
}

#[test]
fn respects_tabs_tab_width_crlf_and_whitespace_final_newline_options() {
    let source = "if true then  \r\n x=1  \r\nend  \r\n\r\n  ";
    let options = FormattingOptions { tab_size: 2, insert_final_newline: Some(true), trim_final_newlines: Some(true), ..FormattingOptions::default() };
    assert_eq!(formatted(source, &options), "if true then\r\n  x = 1\r\nend\r\n");
    let tabs = FormattingOptions { insert_spaces: false, newline: Some(Newline::Lf), ..options.clone() };
    assert_eq!(formatted(source, &tabs), "if true then\n\tx = 1\nend\n");
    let keep_trailing = FormattingOptions { trim_trailing_whitespace: Some(false), ..FormattingOptions::default() };
    assert_eq!(formatted("local x=1  \n\n  ", &keep_trailing), "local x = 1  \n\n  ");
}

#[test]
fn range_edits_are_strictly_bounded_and_idempotent_after_application() {
    let source = "local untouched=1\nif true then\n local café=2+3\nend\nlocal untouched2=4";
    let start = source.find(" local").unwrap();
    let end = source.find("\nend").unwrap();
    let range = Span::new(start, end);
    let edits = format(source, &FormattingOptions::default(), Some(range)).unwrap();
    assert!(!edits.is_empty());
    assert!(edits.iter().all(|edit| edit.span.start >= start && edit.span.end <= end));
    let result = apply(source, &edits);
    assert!(result.starts_with(&source[..start]));
    assert!(result.contains("\n    local café = 2 + 3\n"));
    assert!(result.ends_with(&source[end..]));
    let new_end = result.find("\nend").unwrap();
    assert_eq!(format(&result, &FormattingOptions::default(), Some(Span::new(start, new_end))), Some(Vec::new()));
    assert_eq!(format(source, &FormattingOptions::default(), Some(Span::new(start, start))), Some(Vec::new()));
}

#[test]
fn refuses_invalid_or_unterminated_lua_without_any_edits() {
    for source in ["local x =", "if true then\n", "local x = 'unterminated", "local x=[=[unterminated", "--[=[unterminated"] {
        assert!(format(source, &FormattingOptions::default(), None).is_none(), "{source}");
    }
    let source = "local café=1";
    let middle = source.find('é').unwrap() + 1;
    assert!(format(source, &FormattingOptions::default(), Some(Span::new(middle, source.len()))).is_none());
}

#[test]
fn spacing_cannot_create_comments_or_long_string_openers() {
    let source = "local x=1- -2\nlocal a={ [ [=[k]=] ]= x}\nreturn a[ [=[k]=] ]";
    let result = formatted(source, &FormattingOptions::default());
    assert!(result.contains("1 - -2"));
    assert!(result.contains("[ [=[k]=]]"));
    assert_eq!(token_spellings(source), token_spellings(&result));
}

#[test]
fn on_type_newline_indents_complete_and_incomplete_blocks_without_inserting_tokens() {
    let options = FormattingOptions { tab_size: 2, ..FormattingOptions::default() };
    for (source, at, expected) in [
        ("if true then\nend", 13, "if true then\nend"),
        ("if true then\n", 13, "if true then\n  "),
        ("local function f()\n", 19, "local function f()\n  "),
        ("if true then\n  while true do\n", 29, "if true then\n  while true do\n    "),
    ] {
        let edits = on_type(source, at, "\n", &options).unwrap();
        let result = apply(source, &edits);
        assert_eq!(result, expected);
        assert_eq!(token_spellings(source), token_spellings(&result));
    }
}

#[test]
fn on_type_protects_long_strings_comments_and_non_indentation_text() {
    for (source, at) in [
        ("local x=[=[a\nb]=]", 13),
        ("--[=[a\nb]=]", 7),
        ("local x='bad\n", 13),
        ("local x=1\nreturn x", 15),
    ] {
        assert_eq!(on_type(source, at, "\n", &FormattingOptions::default()), Some(Vec::new()), "{source}");
    }
    assert_eq!(on_type("end", 3, ")", &FormattingOptions::default()), None);
}

#[test]
fn expanded_function_bodies_inside_calls_and_tables_are_idempotent() {
    for source in [
        "invoke(function(a) return a+1 end)",
        "local t={f=function(x) if x then return 1 else return 2 end end}",
        "return (function() return 1 end)()",
        "local function empty() end",
        "if true then else end",
        "invoke(function() end)",
        "local t={a={x=1,y=2},b=function() local x=1 local y=2 return x+y end}",
    ] {
        let result = formatted(source, &FormattingOptions::default());
        assert_eq!(token_spellings(source), token_spellings(&result));
    }
}

#[test]
fn newline_conversion_never_returns_an_edit_inside_a_crlf_pair() {
    for source in ["local x=1\r\n", "local x=1  \r\n\r\n", "\r\nlocal x=1\r\n"] {
        let options = FormattingOptions { newline: Some(Newline::Lf), insert_final_newline: Some(true), trim_final_newlines: Some(true), ..FormattingOptions::default() };
        let result = formatted(source, &options);
        assert!(!result.contains('\r'));
        let midpoint = source.find("\r\n").unwrap() + 1;
        let edits = format(source, &options, Some(Span::new(midpoint, source.len()))).unwrap();
        assert!(edits.iter().all(|edit| edit.span.start >= midpoint));
        apply(source, &edits);
    }
    assert_eq!(formatted("local x=1\n", &FormattingOptions { newline: Some(Newline::CrLf), ..FormattingOptions::default() }), "local x = 1\r\n");
}
