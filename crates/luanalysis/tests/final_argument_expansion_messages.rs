//! A final call argument expands to every value it returns. An error in one of those values names
//! that value and its parameter, as TypeScript reports the elements of a spread tuple, rather than
//! the expansion's list.
use luanalysis::check;

fn messages(source: &str) -> Vec<String> {
    check(source).diagnostics.into_iter().map(|error| format!("{} {}", error.code, error.message)).collect()
}

#[test]
fn a_mismatched_expanded_value_is_reported_as_itself() {
    assert_eq!(
        messages("---@return string\nlocal function g() return \"\" end\n---@param n number\nlocal function f(n) end\nf(g())\n"),
        ["TS2345 Argument of type 'string' is not assignable to parameter of type 'number'."],
    );
    // The first value that does not fit, wherever it is in the list.
    assert_eq!(
        messages("---@return number, string\nlocal function g() return 1, \"\" end\n---@param a number\n---@param b number\nlocal function f(a, b) end\nf(g())\n"),
        ["TS2345 Argument of type 'string' is not assignable to parameter of type 'number'."],
    );
    assert_eq!(
        messages("---@return string\nlocal function g() return \"\" end\n---@param a number\n---@param b number\nlocal function f(a, b) end\nf(0, g())\n"),
        ["TS2345 Argument of type 'string' is not assignable to parameter of type 'number'."],
    );
    // Against an optional parameter, whose absence the expansion's value is not.
    assert_eq!(
        messages("---@return nil\nlocal function none() return nil end\n---@param t? number\nlocal function f(t) end\nf(none())\n"),
        ["TS2345 Argument of type 'nil' is not assignable to parameter of type 'number'."],
    );
}
