//! Original call recipes selected by binder-owned runtime intrinsic identity.
//! No source text scanning, module execution or type construction happens here.
use crate::bind::{BoundFile, BuiltinIntrinsic};
use luanalysis_syntax::{Expr, ExprKind, File, Span, Stmt, StmtKind, TableFieldKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RequireTarget {
    Static(String),
    Dynamic,
    NonUtf8,
    Arity(usize),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RequireCall {
    pub span: Span,
    pub argument_span: Span,
    pub target: RequireTarget,
}

pub(crate) fn require_calls(file: &File, bound: &BoundFile) -> Vec<RequireCall> {
    let mut calls = Vec::new();
    statements(&file.statements, &mut |expression| {
        let ExprKind::Call { callee, args } = &expression.kind else {
            return;
        };
        let callee = unparenthesized(callee);
        if bound.intrinsic_at(callee.span.start) != Some(BuiltinIntrinsic::LuaRequire) {
            return;
        }
        let argument_span = args.first().map_or(callee.span, |argument| argument.span);
        let target = match args.as_slice() {
            [argument] => match &unparenthesized(argument).kind {
                ExprKind::String(name) => match String::from_utf8(name.clone()) {
                    Ok(name) => RequireTarget::Static(name),
                    Err(_) => RequireTarget::NonUtf8,
                },
                _ => RequireTarget::Dynamic,
            },
            _ => RequireTarget::Arity(args.len()),
        };
        calls.push(RequireCall {
            span: expression.span,
            argument_span,
            target,
        });
    });
    calls.sort_by_key(|call| (call.span.start, call.span.end));
    calls
}
fn unparenthesized(mut expression: &Expr) -> &Expr {
    while let ExprKind::Parenthesized(inner) = &expression.kind {
        expression = inner;
    }
    expression
}
fn statements(body: &[Stmt], visit: &mut impl FnMut(&Expr)) {
    for statement in body {
        match &statement.kind {
            StmtKind::Local { values, .. } | StmtKind::Return(values) => {
                for value in values {
                    expression(value, visit);
                }
            }
            StmtKind::Assign {
                targets, values, ..
            } => {
                for value in targets.iter().chain(values) {
                    expression(value, visit);
                }
            }
            StmtKind::LocalFunction { function, .. } => statements(&function.body, visit),
            StmtKind::FunctionDeclaration {
                target, function, ..
            } => {
                expression(target, visit);
                statements(&function.body, visit);
            }
            StmtKind::Call(value) => expression(value, visit),
            StmtKind::Block { body, .. } => statements(body, visit),
            StmtKind::While {
                condition, body, ..
            } => {
                expression(condition, visit);
                statements(body, visit);
            }
            StmtKind::Repeat {
                body, condition, ..
            } => {
                statements(body, visit);
                expression(condition, visit);
            }
            StmtKind::If {
                branches,
                else_branch,
            } => {
                for branch in branches {
                    expression(&branch.condition, visit);
                    statements(&branch.body, visit);
                }
                if let Some(branch) = else_branch {
                    statements(&branch.body, visit);
                }
            }
            StmtKind::NumericFor {
                start,
                limit,
                step,
                body,
                ..
            } => {
                expression(start, visit);
                expression(limit, visit);
                if let Some(step) = step {
                    expression(step, visit);
                }
                statements(body, visit);
            }
            StmtKind::GenericFor { values, body, .. } => {
                for value in values {
                    expression(value, visit);
                }
                statements(body, visit);
            }
            StmtKind::Break => {}
        }
    }
}
fn expression(value: &Expr, visit: &mut impl FnMut(&Expr)) {
    visit(value);
    match &value.kind {
        ExprKind::Parenthesized(inner) | ExprKind::Unary { value: inner, .. } => {
            expression(inner, visit)
        }
        ExprKind::Field { base, .. } => expression(base, visit),
        ExprKind::Index { base, index } => {
            expression(base, visit);
            expression(index, visit);
        }
        ExprKind::Binary { left, right, .. } => {
            expression(left, visit);
            expression(right, visit);
        }
        ExprKind::Table(fields) => {
            for field in fields {
                if let TableFieldKind::Keyed(key) = &field.kind {
                    expression(key, visit);
                }
                expression(&field.value, visit);
            }
        }
        ExprKind::Function(function) => statements(&function.body, visit),
        ExprKind::Call { callee, args } => {
            expression(callee, visit);
            for argument in args {
                expression(argument, visit);
            }
        }
        ExprKind::MethodCall { receiver, args, .. } => {
            expression(receiver, visit);
            for argument in args {
                expression(argument, visit);
            }
        }
        ExprKind::Nil
        | ExprKind::Boolean(_)
        | ExprKind::Number(_)
        | ExprKind::String(_)
        | ExprKind::Vararg
        | ExprKind::Name(_)
        | ExprKind::Error => {}
    }
}
