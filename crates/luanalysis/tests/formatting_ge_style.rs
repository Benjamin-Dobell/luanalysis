//! Focused layout regressions reduced from authored ge_tts (MIT) conventions.
//! The corpus itself is read-only; these cases verify actual applied edits.
use luanalysis::formatting::{FormatEdit, FormattingOptions, format, on_type};
use luanalysis_syntax::{Span, TokenKind};

fn apply(source: &str, edits: &[FormatEdit]) -> String {
    let mut result = String::new();
    let mut offset = 0;
    for edit in edits {
        assert!(offset <= edit.span.start && edit.span.start <= edit.span.end && edit.span.end <= source.len());
        assert!(source[edit.span.start..edit.span.end].chars().all(char::is_whitespace));
        assert!(edit.new_text.chars().all(char::is_whitespace));
        result.push_str(&source[offset..edit.span.start]);
        result.push_str(&edit.new_text);
        offset = edit.span.end;
    }
    result.push_str(&source[offset..]);
    result
}

fn tokens(source: &str) -> Vec<String> {
    luanalysis_syntax::lex(source).tokens.into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::Eof))
        .map(|token| source[token.span.start..token.span.end].to_owned()).collect()
}

fn check(source: &str, expected: &str) {
    let options = FormattingOptions::default();
    let edits = format(source, &options, None).expect("valid Lua formats safely");
    let result = apply(source, &edits);
    assert_eq!(result, expected);
    assert_eq!(tokens(source), tokens(&result));
    assert_eq!(format(&result, &options, None), Some(Vec::new()), "second format changes layout:\n{result}");
    assert!(!luanalysis_syntax::parse(&result).has_lua_syntax_errors());
}

#[test]
fn callback_body_adds_one_level_without_a_call_parenthesis_level() {
    // Coroutine.ttslua:59-67 and DieInstance.ttslua:71-81.
    let source = "local function wait(timeout)\n    if timeout then\n        Wait.time(function()\n            if timeout then\n                return\n            end\n        end, timeout)\n    end\nend\n";
    check(source, source);
}

#[test]
fn inline_table_argument_owns_its_fields_and_callback_bodies() {
    // Vector2.ttslua:51-63 and Http.ttslua:73-85.
    let source = "setmetatable(Vector, {\n    __call = function(_, x)\n        local self = {x = x}\n        setmetatable(self, {\n            __index = index,\n            __tostring = function(_)\n                return self.toString()\n            end,\n        })\n        return self\n    end,\n})\n";
    check(source, source);
    let nested = "consume({\n    complete = function(value)\n        callback({\n            body = value,\n        }, nil)\n    end,\n})\n";
    check(nested, nested);
}

#[test]
fn compact_simple_anonymous_functions_remain_compact() {
    // TableUtils.ttslua:697-711 deliberately uses compact dispatch callbacks.
    let source = "local dispatch = {\n    boolean = function(v) return tostring(v) end,\n    number = function(v) return tostring(v) end,\n}\nreturn select(values, function(v, k) return not predicate(v, k) end)\n";
    check(source, source);
    check("local f=function(x) return x+1 end", "local f = function(x) return x + 1 end");
    // A compound callback still receives block layout, on its first format.
    check("run(function(x) if x then return 1 end end)", "run(function(x)\n    if x then\n        return 1\n    end\nend)");
}

#[test]
fn table_interior_edges_preserve_tight_and_spaced_authored_styles() {
    // Vector2 uses tight records; Class and TableUtils also use spaced tables.
    check("local a={x=1,y=2}\nlocal b={ x=1 }\nlocal c={}\n", "local a = {x = 1, y = 2}\nlocal b = { x = 1 }\nlocal c = {}\n");
}

#[test]
fn casts_stay_tight_after_delimiters_and_leading_semicolons_stay_attached() {
    // Class.ttslua:49, EventManager.ttslua:20 and Http.ttslua:104.
    let source = "local function assign(object, key)\n    ;(--[[---@type table]] object)[key] = 1\n    return object[--[[---@not nil]] key] -- explanation\nend\n";
    check(source, source);
    // A trailing separator still separates sibling statements normally.
    check("local x=1;local y=2", "local x = 1;\nlocal y = 2");
}

#[test]
fn unparenthesized_hanging_expression_chain_gets_one_level() {
    // Instance.ttslua:515-517 and TableUtils.ttslua:318-323.
    let source = "local function select(x)\n    return x\n        and left(x)\n        or right(x)\nend\n";
    check(source, source);
    // `and` binds more tightly than `or`, but indentation follows one expression
    // chain rather than accumulating a level for each binary AST node.
    let precedence = "local function select(x)\n    return x\n        or left(x)\n        and right(x)\nend\n";
    check(precedence, precedence);
    check("local n=1+\n2+\n3", "local n = 1 +\n    2 +\n    3");
}

#[test]
fn explicit_multiline_parentheses_and_index_hanging_operators_compose() {
    // TableUtils.ttslua:681-685 intentionally adds another hanging level for `or`.
    let source = "local function compare(a, b)\n    if a and (\n        b\n            or a\n    ) then\n        return true\n    end\nend\n";
    check(source, source);
    let index = "local function copy(t)\n    local value = t[type(t) == 'table'\n        and 'k'\n        or 'v'\n    ]\n    return value\nend\n";
    check(index, index);
}

#[test]
fn actual_multiline_argument_lists_keep_their_own_continuation_level() {
    let source = "local function run()\n    call(first,\n        second,\n        function()\n            return second\n        end)\nend\n";
    check(source, source);
    let arguments = "call(\n    first,\n    second\n)\n";
    check(arguments, arguments);
}

#[test]
fn callback_layout_remains_bounded_for_range_and_consistent_for_on_type() {
    let source = "local before=1\nrun(function()\nreturn 1\nend)\nlocal after=2\n";
    let start = source.find("run(").unwrap();
    let end = source.find("local after").unwrap();
    let options = FormattingOptions::default();
    let edits = format(source, &options, Some(Span::new(start, end))).unwrap();
    assert!(edits.iter().all(|edit| start <= edit.span.start && edit.span.end <= end));
    let result = apply(source, &edits);
    assert_eq!(result, "local before=1\nrun(function()\n    return 1\nend)\nlocal after=2\n");
    assert_eq!(format(&result, &options, Some(Span::new(start, result.find("local after").unwrap()))), Some(Vec::new()));
    let at = source.find("return").unwrap();
    let typed = apply(source, &on_type(source, at, "\n", &options).unwrap());
    assert_eq!(typed, result);
}

#[test]
fn unfinished_callback_and_table_on_type_avoid_an_extra_call_level() {
    let options = FormattingOptions::default();
    for (source, expected) in [
        ("run(function()\n", "run(function()\n    "),
        ("consume({\n", "consume({\n    "),
        ("call(first,\n", "call(first,\n    "),
        ("call(\n", "call(\n    "),
    ] {
        let result = apply(source, &on_type(source, source.len(), "\n", &options).unwrap());
        assert_eq!(result, expected);
        assert_eq!(tokens(source), tokens(&result));
    }
}

#[test]
fn unfinished_callback_statement_commas_do_not_indent_the_enclosing_call() {
    let options = FormattingOptions::default();
    for (source, expected, completed) in [
        (
            "run(function()\n    local a,\n",
            "run(function()\n    local a,\n    ",
            "run(function()\n    local a,\n    b = 1, 2\nend)",
        ),
        (
            "run(function()\n    return one,\n",
            "run(function()\n    return one,\n    ",
            "run(function()\n    return one,\n    two\nend)",
        ),
    ] {
        check(completed, completed);
        let edits = on_type(source, source.len(), "\n", &options).unwrap();
        let result = apply(source, &edits);
        assert_eq!(result, expected, "statement comma inside a callback must not activate its enclosing call list");
        assert_eq!(tokens(source), tokens(&result));
    }
}

#[test]
fn callback_nested_call_arguments_and_table_fields_keep_their_continuation() {
    let options = FormattingOptions::default();
    for (source, expected) in [
        ("run(function()\n    nested(first,\n", "run(function()\n    nested(first,\n        "),
        ("run(function()\n    local t = {\n        first = 1,\n", "run(function()\n    local t = {\n        first = 1,\n        "),
        ("run(function()\n    return one\nend,\n", "run(function()\n    return one\nend,\n    "),
    ] {
        let result = apply(source, &on_type(source, source.len(), "\n", &options).unwrap());
        assert_eq!(result, expected);
        assert_eq!(tokens(source), tokens(&result));
    }
}

#[test]
fn compact_call_body_keeps_a_space_after_the_parameter_list() {
    // Json.ttslua:44 contains a compact call statement, rather than a return.
    let source = "local NULL = setmetatable({}, {\n    __index = {},\n    __newindex = function() error(\"Attempt to modify JSON.null()\") end,\n    __metatable = false\n})\n";
    check(source, source);
    check("local f=function() log(1) end", "local f = function() log(1) end");
}

#[test]
fn fluent_member_and_method_chain_gets_one_hanging_level() {
    // ObjectUtils.ttslua:238-245 deliberately hangs each rotation in the chain.
    let source = "local function rotate(rotation)\n    local direction = Vector(1, 0, 0)\n        .rotateZ(rotation.z)\n        .rotateX(rotation.x)\n        .rotateY(rotation.y)\n    return direction\nend\n";
    check(source, source);
    let mixed = "local result = source()\n    :first()\n    .next[key]\n    :last()\n";
    check(mixed, mixed);
    check("local result=source()\n.first()\n.second()", "local result = source()\n    .first()\n    .second()");
}

#[test]
fn unfinished_function_parameters_do_not_activate_the_function_body_level() {
    let options = FormattingOptions::default();
    for source in ["local function f(\n", "run(function(\n", "local function f(x,\n", "run(function(x,\n"] {
        let result = apply(source, &on_type(source, source.len(), "\n", &options).unwrap());
        assert_eq!(result, format!("{source}    "), "unfinished function parameters have one list level");
        assert_eq!(tokens(source), tokens(&result));
    }
    for completed in [
        "local function f(\n    x\n)\nend",
        "run(function(\n    x\n)\n    return x\nend)",
    ] {
        check(completed, completed);
    }
    // Closing the parameter list transfers its indentation to the body, so
    // ordinary named/anonymous callback bodies retain their existing level.
    for source in ["local function f()\n", "run(function()\n"] {
        let result = apply(source, &on_type(source, source.len(), "\n", &options).unwrap());
        assert_eq!(result, format!("{source}    "));
        assert_eq!(tokens(source), tokens(&result));
    }
}
