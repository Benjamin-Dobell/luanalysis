//! LuaLS (sumneko) annotation forms, as its own sources write them.

use crate::*;

fn checked(source: &str) -> File {
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{source}\n{:#?}", file.diagnostics);
    file
}

fn messages(source: &str) -> Vec<String> {
    parse(source).diagnostics.into_iter().map(|diagnostic| diagnostic.message).collect()
}

fn named(ty: &TypeExpr) -> &str {
    match &ty.kind {
        TypeExprKind::Named(name) => name,
        kind => panic!("expected a named type, found {kind:?}"),
    }
}

// The first function expression in a statement's call arguments.
fn callback(statement: &Stmt) -> &Function {
    let StmtKind::Call(call) = &statement.kind else { panic!("expected a call statement") };
    let mut pending = vec![call];
    while let Some(expr) = pending.pop() {
        match &expr.kind {
            ExprKind::Function(function) => return function,
            ExprKind::Call { callee, args } => {
                pending.extend(args.iter().rev());
                pending.push(callee);
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                pending.extend(args.iter().rev());
                pending.push(receiver);
            }
            _ => {}
        }
    }
    panic!("expected a callback")
}

#[test]
fn qualified_optional_and_tuple_types() {
    let file = checked(concat!(
        "---@class parser.object\n",
        "---@field type? string|string[] # the node's kind\n",
        "---@field range [integer, integer] 'start and finish'\n",
        "---@field handle file*\n",
        "---@alias require-manager.result parser.object?\n",
        "---@param ... any\n",
        "---@return boolean success\n",
        "---@return ...\n",
        "local function f(...) end\n",
    ));
    let NamedTypeDeclarationKind::Record { fields, .. } = &file.type_declarations[0].kind else { panic!("a class") };
    let TypeExprKind::Union(members) = &fields[0].ty.kind else { panic!("an optional field") };
    assert_eq!(named(members.last().unwrap()), "nil");
    assert_eq!(named(&fields[2].ty), "file");
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else { panic!("a function") };
    assert_eq!(named(&function.annotations.vararg.as_ref().unwrap().ty), "any");
    let returns = function.annotations.returns.as_ref().unwrap();
    assert_eq!(returns.types.len(), 2);
    let pack = &returns.packs.as_ref().unwrap().alternatives[0];
    assert_eq!(pack.fixed, 0..1);
    assert_eq!(pack.variadic.as_ref().unwrap().type_index, 1);
    checked("---@return boolean, ...\nlocal function g() end\n");
    checked("---@return (parser.object|string)?\n---@see string.format\nlocal function h() end\n");
    // Results after a variadic one have no slot.
    assert_eq!(
        messages("---@return ...\n---@return boolean\nlocal function i() end\n"),
        ["multiple @return annotations for one function"],
    );
    checked("---@type fun(...):...\nlocal h\n");
}

#[test]
fn alias_members_continue_on_following_lines() {
    let file = checked(concat!(
        "---@alias mode\n",
        "---| 'r' # read\n",
        "  ---| 'w' # write\n",
        "---| \"a\"\n",
        "local m = 'r'\n",
    ));
    let NamedTypeDeclarationKind::Alias { target } = &file.type_declarations[0].kind else { panic!("an alias") };
    let TypeExprKind::Union(members) = &target.kind else { panic!("a union of members") };
    assert_eq!(members.len(), 3);
}

#[test]
fn commented_out_code_is_not_documentation() {
    checked("--[[\n---@class A\nlocal mt\n---@type X\nmt._x = nil\n]]\nlocal y = 1\n");
    // Long comments made only of documentation lines still document.
    let file = checked("--[[\n---@type number\n]]\nlocal x\n");
    let StmtKind::Local { annotation, .. } = &file.statements[0].kind else { panic!("a local") };
    assert!(annotation.is_some());
}

#[test]
fn signature_documentation_binds_to_a_callback_on_the_next_line() {
    let file = checked(concat!(
        "---@param param brave.param.compile\n",
        "brave.on('compile', function (param) end)\n",
        "---@return table\n",
        "m.snapshot = private(function () return {} end)\n",
        "chain()\n",
        "    : case 'local'\n",
        "    ---@async\n",
        "    ---@param source parser.object\n",
        "    : call(function (source) end)\n",
    ));
    assert_eq!(callback(&file.statements[0]).annotations.params[0].name.text, "param");
    let StmtKind::Assign { values, .. } = &file.statements[1].kind else { panic!("an assignment") };
    let ExprKind::Call { args, .. } = &values[0].kind else { panic!("a call") };
    let ExprKind::Function(function) = &args[0].kind else { panic!("a callback") };
    assert!(function.annotations.returns.is_some());
    assert_eq!(callback(&file.statements[2]).annotations.params[0].name.text, "source");
    // Documentation still needs a function on the line after it.
    assert_eq!(
        messages("---@param value number\nrun(\n  function (value) end)\n"),
        ["Luanalysis annotation is not attached to a compatible declaration or function"],
    );
    assert_eq!(
        messages("value\n  ---@param x number\n  .field = 1\n"),
        ["Luanalysis annotation is not attached to a compatible declaration or function"],
    );
}

#[test]
fn a_label_repeats_only_one_declared_before_it() {
    checked("for _, set in ipairs(sets) do\n  for _, field in ipairs(set) do\n    goto CONTINUE\n    ::CONTINUE::\n  end\n  ::CONTINUE::\nend\n");
    assert_eq!(messages("::again::\ndo ::again:: end\n"), ["label 'again' already defined"]);
}

#[test]
fn dotted_owners_qualify_function_and_field_declarations() {
    let file = checked(concat!(
        "---@function parser.object:visit(node: parser.object): boolean\n",
        "---@function parser.object.create(): parser.object\n",
        "---@field parser.object.kind string\n",
    ));
    let owners: Vec<_> = file.function_declarations.iter().map(|declaration| {
        let Some(DocFunctionOwner::Explicit(owner)) = &declaration.owner else { panic!("an explicit owner") };
        (owner.text.as_str(), declaration.name.text.as_str())
    }).collect();
    assert_eq!(owners, [("parser.object", "visit"), ("parser.object", "create")]);
}

#[test]
fn return_lines_continue_only_within_one_documentation_group() {
    let file = checked("---@return boolean ok\n---@return string message\nlocal function f() end\n");
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else { panic!("a function") };
    assert_eq!(function.annotations.returns.as_ref().unwrap().types.len(), 2);
    // Commented-out documentation ends its group; its @return still conflicts, as in v1.
    assert_eq!(
        messages("-----@return void\n--function old() end\n-----@return void\n--\n---@return number\nlocal function g() return 1 end\n"),
        ["multiple @return annotations for one function", "multiple @return annotations for one function"],
    );
}

#[test]
fn operators_attach_to_their_named_type_with_one_operand_parameter() {
    let file = checked(concat!(
        "---@class Vector\n",
        "---@operator add(Vector): Vector\n",
        "---@operator unm: Vector\n",
        "---@operator mul: number # any right operand\n",
        "---@operator call(string): Vector\n",
        "---@operator eq(Vector): boolean\n",
        "local Vector = {}\n",
    ));
    let operators: Vec<_> = file.operator_annotations.iter().map(|operator| {
        assert_eq!(operator.named_candidates, [file.type_declarations[0].id]);
        (operator.operator, operator.signature.params.len())
    }).collect();
    assert_eq!(operators, [(LuaOperator::Add, 1), (LuaOperator::Negate, 0), (LuaOperator::Multiply, 1)]);
    // `call` is a class overload; `eq` declares nothing, since `==` compares identity.
    assert_eq!(file.overload_annotations.len(), 1);
    assert_eq!(file.overload_annotations[0].named_candidates, [file.type_declarations[0].id]);
    assert_eq!(messages("---@class A\n---@operator unm(A): A\nlocal A = {}\n"), ["unary operator 'unm' takes no operand"]);
}

#[test]
fn async_functions_and_named_results_are_ordinary_function_types() {
    checked("---@type fun(): (ok: boolean, message: string?)\nlocal f\n---@type fun(): ok: boolean, string\nlocal g\n---@param callback async fun(value: number): string\n---@param data lightuserdata\n---@return integer\nlocal function h(callback, data) return 1 end\n");
}
