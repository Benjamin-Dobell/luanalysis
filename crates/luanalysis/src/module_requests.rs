//! Original require calls selected syntactically from binder-owned names and their
//! lexical aliases. As in Bun's program load, the targets of these calls are the
//! project's files; no source text scanning, module execution or type
//! construction happens here.
use crate::bind::{BindingId, BoundFile, BuiltinIntrinsic};
use luanalysis_syntax::{Expr, ExprKind, File, Span, Stmt, StmtKind, TableFieldKind};
use crate::collections::HashSet;

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
    let mut locals = Vec::new();
    local_value_candidates(&file.statements, &mut locals);
    let mut loaders = HashSet::default();
    let mut environments = HashSet::default();
    // This is candidate collection, not value propagation. A later assignment
    // never erases a request candidate; the actual native callee must still
    // retain the original require capability at that evaluation's flow input.
    loop {
        let before = loaders.len() + environments.len();
        for (name, value) in &locals {
            let Some(binding) = bound.binding_at(name.span.start) else { continue; };
            if environment_candidate(value, bound, &environments) { environments.insert(binding); }
            if require_candidate(value, bound, &loaders, &environments) { loaders.insert(binding); }
        }
        if before == loaders.len() + environments.len() { break; }
    }
    statements(&file.statements, &mut |expression| {
        let ExprKind::Call { callee, args } = &expression.kind else {
            return;
        };
        let callee = unparenthesized(callee);
        if !require_candidate(callee, bound, &loaders, &environments) { return; }
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
fn environment_candidate(expression: &Expr, bound: &BoundFile, aliases: &HashSet<BindingId>) -> bool {
    let ExprKind::Name(name) = &unparenthesized(expression).kind else { return false; };
    if let Some(binding) = bound.binding_at(name.span.start) { return aliases.contains(&binding); }
    matches!(name.text.as_str(), "_G" | "_ENV") && bound.environment_binding_at(name.span.start).is_none()
}
fn require_candidate(expression: &Expr, bound: &BoundFile, aliases: &HashSet<BindingId>, environments: &HashSet<BindingId>) -> bool {
    let expression = unparenthesized(expression);
    match &expression.kind {
        ExprKind::Name(name) => bound.binding_at(name.span.start).map_or_else(
            || bound.standard_candidate_at(name.span.start) == Some(BuiltinIntrinsic::LuaRequire)
                || (name.text == "require" && bound.environment_binding_at(name.span.start).is_some_and(|binding| environments.contains(&binding))),
            |binding| aliases.contains(&binding)),
        ExprKind::Field { base, name } => name.text == "require" && environment_candidate(base, bound, environments),
        ExprKind::Index { base, index } => matches!(&unparenthesized(index).kind, ExprKind::String(key) if key == b"require") && environment_candidate(base, bound, environments),
        _ => false,
    }
}
fn local_value_candidates<'a>(body: &'a [Stmt], result: &mut Vec<(&'a luanalysis_syntax::Name, &'a Expr)>) {
    for statement in body {
        match &statement.kind {
            StmtKind::Local { names, values, .. } => {
                result.extend(names.iter().zip(values));
                for value in values { expression_value_candidates(value, result); }
            }
            StmtKind::Assign { targets, values, .. } => {
                // Every original bare local store is a candidate. Binding IDs
                // below separate shadows; runtime flow still proves the callee.
                for (target, value) in targets.iter().zip(values) {
                    if let ExprKind::Name(name) = &unparenthesized(target).kind {
                        result.push((name, value));
                    }
                }
                for value in targets.iter().chain(values) { expression_value_candidates(value, result); }
            }
            StmtKind::LocalFunction { function, .. } => local_value_candidates(&function.body, result),
            StmtKind::FunctionDeclaration { target, function, .. } => {
                expression_value_candidates(target, result);
                local_value_candidates(&function.body, result);
            }
            StmtKind::Return(values) => for value in values { expression_value_candidates(value, result); },
            StmtKind::Call(value) => expression_value_candidates(value, result),
            StmtKind::Block { body, .. } => local_value_candidates(body, result),
            StmtKind::While { condition, body, .. } | StmtKind::Repeat { condition, body, .. } => {
                expression_value_candidates(condition, result);
                local_value_candidates(body, result);
            }
            StmtKind::If { branches, else_branch } => {
                for branch in branches {
                    expression_value_candidates(&branch.condition, result);
                    local_value_candidates(&branch.body, result);
                }
                if let Some(branch) = else_branch { local_value_candidates(&branch.body, result); }
            }
            StmtKind::NumericFor { start, limit, step, body, .. } => {
                expression_value_candidates(start, result);
                expression_value_candidates(limit, result);
                if let Some(step) = step { expression_value_candidates(step, result); }
                local_value_candidates(body, result);
            }
            StmtKind::GenericFor { values, body, .. } => {
                for value in values { expression_value_candidates(value, result); }
                local_value_candidates(body, result);
            }
            StmtKind::Break => {}
        }
    }
}
fn expression_value_candidates<'a>(value: &'a Expr, result: &mut Vec<(&'a luanalysis_syntax::Name, &'a Expr)>) {
    match &value.kind {
        ExprKind::Function(function) => local_value_candidates(&function.body, result),
        ExprKind::Parenthesized(inner) | ExprKind::Unary { value: inner, .. } => expression_value_candidates(inner, result),
        ExprKind::Field { base, .. } => expression_value_candidates(base, result),
        ExprKind::Index { base, index } => {
            expression_value_candidates(base, result);
            expression_value_candidates(index, result);
        }
        ExprKind::Binary { left, right, .. } => {
            expression_value_candidates(left, result);
            expression_value_candidates(right, result);
        }
        ExprKind::Table(fields) => for field in fields {
            if let TableFieldKind::Keyed(key) = &field.kind { expression_value_candidates(key, result); }
            expression_value_candidates(&field.value, result);
        },
        ExprKind::Call { callee, args } => {
            expression_value_candidates(callee, result);
            for argument in args { expression_value_candidates(argument, result); }
        }
        ExprKind::MethodCall { receiver, args, .. } => {
            expression_value_candidates(receiver, result);
            for argument in args { expression_value_candidates(argument, result); }
        }
        ExprKind::Nil | ExprKind::Boolean(_) | ExprKind::Number(_) | ExprKind::String(_) | ExprKind::Vararg | ExprKind::Name(_) | ExprKind::Error => {}
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn requested(source: &str) -> Vec<String> {
        let file = luanalysis_syntax::parse(source);
        assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
        let bound = crate::bind::bind(&file);
        require_calls(&file, &bound).into_iter().map(|call| {
            let RequireTarget::Static(name) = call.target else { panic!("expected a static original request") };
            name
        }).collect()
    }

    #[test]
    fn assignments_collect_original_local_loader_and_environment_alias_candidates() {
        let source = "local load, chained, env\nchained=load\nenv=_G\nload=env.require\nchained('chained')\nlocal bracket\nbracket=env['require']\nbracket('bracket')\n";
        // Candidate collection deliberately accepts the pre-store chain. The
        // actual call's native flow query must still reject a nil/custom value.
        assert_eq!(requested(source), ["chained", "bracket"]);
    }

    #[test]
    fn assignment_candidates_keep_distinct_original_shadow_bindings() {
        let source = "local load\nload=require\ndo\nlocal load=function(_) return 1 end\nload('custom')\nend\nload('original')\ndo\nlocal require=function(_) return 1 end\nlocal own\nown=require\nown('shadowed')\nend\n";
        assert_eq!(requested(source), ["original"]);
    }

    #[test]
    fn assignments_inside_anonymous_expression_bodies_retain_original_requests() {
        let source = "accept(function()\nlocal load\nload=require\nload('callback')\nend)\nlocal nested={ [make(function()\nlocal env\nenv=_G\nenv.require('key')\nend)]=wrap(function()\nlocal load\nload=require\nreturn load('value')\nend) }\n";
        assert_eq!(requested(source), ["callback", "key", "value"]);
    }
}
