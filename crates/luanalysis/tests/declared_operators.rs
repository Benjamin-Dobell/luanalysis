//! Operators a type declares with `---@operator`, in the notation proposed in LuaLS issue 599:
//! `---@operator add(Other): Result`. The right operand's type selects the overload.
use std::{fs, path::PathBuf, process::{Command, Output}, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn check(source: &str) -> Output {
    let root: PathBuf = std::env::temp_dir().join(format!("luanalysis operators {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("main.lua"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis")).current_dir(&root)
        .args(["check", "--lua-version", "5.4", "main.lua"]).output().unwrap();
    fs::remove_dir_all(&root).unwrap();
    output
}

fn text(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

fn clean(source: &str) {
    let output = check(source);
    assert!(output.status.success(), "{}", text(&output));
}

const VECTOR: &str = "---@class Vector\n---@operator unm: Vector\n---@operator add(Vector): Vector\n---@operator mul(number): Vector\n---@operator lt(Vector): boolean\n---@operator len: integer\n---@operator concat(string): string\n---@operator call(string): Vector\nlocal Vector = {}\n---@class Point : Vector\nlocal Point = {}\n---@type Vector\nlocal a\n---@type Vector\nlocal b\n---@type Point\nlocal p\n";

#[test]
fn declared_operators_type_their_results_by_operand() {
    clean(&format!("{VECTOR}---@type Vector\nlocal sum = a + b\n---@type Vector\nlocal scaled = a * 2\n---@type Vector\nlocal negated = -a\n---@type boolean\nlocal less = a < b\n---@type boolean\nlocal greater = a > b\n---@type number\nlocal length = #a\n---@type string\nlocal text = a .. 'x'\n---@type Vector\nlocal called = a('name')\n"));
}

#[test]
fn either_operand_and_bases_supply_declared_operators() {
    // Lua dispatches a metamethod from either operand, as LuaLS reads `---@operator`.
    clean(&format!("{VECTOR}---@type Vector\nlocal scaled = 2 * a\n---@type Vector\nlocal inherited = p + a\n"));
}

#[test]
fn an_operand_no_declared_overload_accepts_is_an_error() {
    let output = check(&format!("{VECTOR}local wrong = a + 'name'\n"));
    assert!(text(&output).contains("TS2365"), "{}", text(&output));
    assert!(text(&output).contains("Operator '+' cannot be applied to types 'Vector' and '\"name\"'"), "{}", text(&output));
    let undeclared = check(&format!("{VECTOR}local wrong = a - b\n"));
    assert!(text(&undeclared).contains("TS2362"), "{}", text(&undeclared));
}

#[test]
fn the_overload_follows_the_right_operand_type() {
    let output = check("---@class A\n---@operator add(A): A\n---@operator add(number): number\nlocal A = {}\n---@type A\nlocal a\n---@type A\nlocal wrong = a + 1\n");
    assert!(text(&output).contains("Type 'number' is not assignable to type 'A'"), "{}", text(&output));
}

#[test]
fn an_operator_needs_a_named_type() {
    let output = check("---@operator add(number): number\nlocal value = 1\n");
    assert!(text(&output).contains("@operator has no class"), "{}", text(&output));
    let unknown = check("---@class A\n---@operator plus(A): A\nlocal A = {}\n");
    assert!(text(&unknown).contains("unknown operator 'plus'"), "{}", text(&unknown));
}

#[test]
fn an_any_operand_may_declare_the_operator_so_arithmetic_with_it_is_any() {
    // An unresolved lpeg pattern is `any`; its `*` and `+` dispatch through metamethods.
    clean("---@type any\nlocal lpeg\nlocal pattern = 'b' * lpeg.P('a') + lpeg.S('x')\nlocal captured = lpeg.C(pattern) / tostring\n---@type string\nlocal whatever = lpeg.P('x') * 'y'\n");
}
