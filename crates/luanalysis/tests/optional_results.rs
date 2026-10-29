//! An optional is a value that may be absent, never nil, which is a value. A result or a tuple
//! element is written `T?`, having no name to write the `?` on; a parameter or field writes it on
//! its name. `--dangerously-decay-optionals` reads them as EmmyLua Analyzer does: `nil | T`, and
//! absence as nil.
use luanalysis::{CheckOptions, check, check_with_options};

fn decayed() -> CheckOptions {
    CheckOptions { decay_optionals: true, ..CheckOptions::default() }
}

fn emmylua() -> CheckOptions {
    CheckOptions { emmylua_syntax: true, ..CheckOptions::default() }
}

fn diagnostics(source: &str, options: CheckOptions) -> Vec<(&'static str, String)> {
    check_with_options(source, options).diagnostics.into_iter().map(|error| (error.code, error.message)).collect()
}

fn accepted(source: &str) {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
}

fn rejected(source: &str, message: &str) {
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{source}\n{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert!(error.message.contains(message), "{source}\n{error:#?}");
}

#[test]
fn an_optional_result_may_be_absent() {
    let declaration = "---@param x boolean\n---@return number?\nlocal function f(x)\n";
    accepted(&format!("{declaration}if x then return end\nreturn 1\nend"));
    accepted(&format!("{declaration}if x then return 1 end\nend"));
    accepted("---@return number?\nlocal function f() end");
    // `string?, boolean?` is `void | string | (string, boolean)`, in any row.
    accepted("---@param x boolean\n---@return string?, boolean?\nlocal function f(x)\nif x then return 'a' end\nreturn 'a', true\nend");
    accepted("---@param x boolean\n---@return (true, number?) | (false)\nlocal function f(x)\nif x then return true end\nreturn false\nend");
    // Written void is absence too.
    accepted("---@param x boolean\n---@return void | number\nlocal function f(x)\nif x then return end\nreturn 1\nend");
    // Passing on one optional result as another.
    accepted("---@return number?\nlocal function f() end\n---@return number?\nlocal function g() return f() end");
}

#[test]
fn an_optional_result_is_not_nil() {
    rejected("---@param x boolean\n---@return number?\nlocal function f(x)\nif x then return nil end\nreturn 1\nend", "Actual: nil. Expected: number?.");
    rejected("---@type fun(): number?\nlocal f = function() return nil end", "Actual: nil. Expected: number?.");
    // A tail's values are each present.
    rejected("---@return number?...\nlocal function f() return 1, nil end", "Actual: (1, nil).");
    // Unless its type admits nil.
    accepted("---@return (nil | number)?\nlocal function f() return nil end");
}

#[test]
fn a_nil_result_is_a_value_that_must_be_returned() {
    rejected("---@param x boolean\n---@return nil | number\nlocal function f(x)\nif x then return end\nreturn 1\nend", "Actual: void. Expected: nil | number.");
    rejected("---@param x boolean\n---@return number?\nlocal function f(x) if x then return 1 end end\n---@return nil | number\nlocal function g() return f(true) end", "Actual: number?. Expected: nil | number.");
}

#[test]
fn a_required_value_cannot_follow_an_optional_one() {
    let source = "---@param x boolean\n---@return number?, string\nlocal function f(x)\nif x then return end\nreturn 1, 'a'\nend";
    assert_eq!(diagnostics(source, CheckOptions::default()), [
        ("LUA1001", "a required result cannot follow a `T?` result, which may be absent".to_owned()),
        ("TS2322", "Lua result pack is not assignable to the declared result pack. Actual: void. Expected: (number, string).".to_owned()),
    ], "{source}");
    assert_eq!(diagnostics("---@type [string?, number]\nlocal t\n", CheckOptions::default()), [
        ("LUA1001", "a required element cannot follow a `T?` element, which may be absent".to_owned()),
    ]);
}

#[test]
fn callers_read_an_optional_result_as_nil_or_its_value() {
    let declaration = "---@param x boolean\n---@return number?\nlocal function f(x) if x then return 1 end end\nlocal value = f(true)\n";
    accepted(&format!("{declaration}---@type nil | number\nlocal copy = value"));
    rejected(&format!("{declaration}---@type number\nlocal copy = value"), "Type 'nil | number' is not assignable to type 'number'.");
    // Its type is written as it was declared.
    rejected("---@type fun(): number, string?\nlocal f\n---@type fun(): number, string\nlocal g = f", "Type 'fun(): (number, string?)' is not assignable");
}

#[test]
fn an_optional_parameter_may_be_absent_but_is_not_nil() {
    let declaration = "---@param s string\n---@param t? number\nlocal function f(s, t) if t then return t + 1 end return 0 end\n";
    accepted(&format!("{declaration}f('a')\nf('a', 1)"));
    rejected(&format!("{declaration}f('a', nil)"), "Argument of type 'nil' is not assignable to parameter of type 'number'.");
    rejected(&format!("{declaration}---@param maybe nil | number\nlocal function g(maybe) f('a', maybe) end"), "Argument of type 'nil | number' is not assignable to parameter of type 'number'.");
    rejected(&format!("{declaration}---@return nil\nlocal function none() return nil end\nf('a', none())"), "Argument of type 'nil' is not assignable to parameter of type 'number'.");
    accepted(&format!("{declaration}---@param x boolean\n---@return number?\nlocal function some(x) if x then return 1 end end\nf('a', some(true))"));
    rejected("---@type fun(s: string, t?: number)\nlocal g\ng('a', nil)", "Argument of type 'nil' is not assignable to parameter of type 'number'.");
    // Its own type may admit nil; within its function it may be nil.
    accepted("---@param t? nil | number\nlocal function f(t) end\nf(nil)");
    rejected("---@type fun(s: string, t?: number)\nlocal g = function(s, t) local n = t + 1 end", "got 'nil | number'");
}

#[test]
fn a_tuple_element_written_optional_may_be_absent() {
    accepted("---@type [string, number?]\nlocal t = { 'a' }\n---@type [string, number?]\nlocal u = { 'a', 1 }");
    rejected("---@type [string, number?]\nlocal t = { 'a', 'b' }", "is not assignable");
}

#[test]
fn only_results_and_tuple_elements_are_written_optional() {
    let gated = "`T?` here is EmmyLua Analyzer syntax (--emmylua-syntax): only a result or a tuple element is written `T?`; an optional parameter or field is `name? T`, and a type that admits nil is `nil | T`";
    for source in [
        "---@param x string?\nlocal function f(x) end",
        "---@type fun(x: string?)\nlocal f",
        "---@type { name: string? }\nlocal record",
        "---@type number?\nlocal value",
        "---@type table<string, number?>\nlocal map",
        "---@alias Maybe number?",
    ] {
        assert_eq!(diagnostics(source, CheckOptions::default()), [("LUA1001", gated.to_owned())], "{source}");
        assert!(diagnostics(source, emmylua()).is_empty(), "{source}");
    }
    // With EmmyLua Analyzer's syntax, a parameter's or field's `T?` is its name's.
    let report = check_with_options("---@param x string?\nlocal function f(x) end\nf()\nf(nil)", emmylua());
    assert_eq!(report.diagnostics.iter().map(|error| error.code).collect::<Vec<_>>(), ["TS2345"], "{:#?}", report.diagnostics);
}

#[test]
fn decayed_optionals_are_nil() {
    // `T?` is `nil | T`, and absence is nil, as EmmyLua Analyzer reads them: a final result
    // that admits nil may be nil or left out.
    assert!(diagnostics("---@return number?\nlocal function f() return nil end", decayed()).is_empty());
    assert!(diagnostics("---@param x boolean\n---@return number?\nlocal function f(x) if x then return end return 1 end", decayed()).is_empty());
    assert!(diagnostics("---@return number?\n---@return number?\nlocal function f() return end", decayed()).is_empty());
    assert!(diagnostics("---@return number?\nlocal function f() end", decayed()).is_empty());
    assert!(diagnostics("---@param x boolean\n---@return number | nil\nlocal function f(x) if x then return end return 1 end", decayed()).is_empty());
    assert!(diagnostics("---@param cb fun(): string | nil\nlocal function take(cb) end\ntake(function() end)", decayed()).is_empty());
    assert!(diagnostics("---@return string?, boolean\nlocal function f() return nil, true end", decayed()).is_empty());
    // A result that does not admit nil, or that a required result follows, is still required.
    let codes = |source| diagnostics(source, decayed()).into_iter().map(|error| error.0).collect::<Vec<_>>();
    assert_eq!(codes("---@param x boolean\n---@return number\nlocal function f(x) if x then return end return 1 end"), ["TS2322"]);
    assert_eq!(codes("---@return string?, boolean\nlocal function f() return end"), ["TS2322"]);
    // An optional parameter takes nil, and a final parameter that admits nil may be omitted.
    assert!(diagnostics("---@param t? number\nlocal function f(t) end\nf()\nf(nil)", decayed()).is_empty());
    assert!(diagnostics("---@param a string\n---@param b string | nil\nlocal function f(a, b) end\nf(\"a\")\nf(\"a\", nil)", decayed()).is_empty());
    assert_eq!(codes("---@param a string\n---@param b string | nil\nlocal function f(a, b) end\nf()"), ["TS2554"]);
    // A parameter's `T?` is `nil | T`, and so may be omitted.
    let decayed = CheckOptions { emmylua_syntax: true, ..decayed() };
    assert!(diagnostics("---@param x string?\nlocal function f(x) end\nf(nil)\nf()", decayed).is_empty());
}
