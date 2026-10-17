use super::{Parser, statements_depth};
use crate::{Diagnostic, ElseBranch, Expr, ExprKind, IfBranch, Span, Stmt, StmtKind};

impl Parser<'_> {
    pub(super) fn control_statement(&mut self, start: Span) -> Option<Stmt> {
        if self.keyword("if") { return Some(self.if_statement(start)); }
        if self.keyword("while") { return Some(self.while_statement(start)); }
        if self.keyword("repeat") { return Some(self.repeat_statement(start)); }
        if self.keyword("for") { return self.for_statement(start); }
        if self.keyword("break") {
            self.pos += 1;
            if self.loop_depth == 0 {
                self.diagnostics.push(Diagnostic {
                    origin: crate::DiagnosticOrigin::LuaSyntax,
                    span: start,
                    message: "'break' is outside a loop in the current Lua function".to_owned(),
                });
            }
            return Some(Stmt { kind: StmtKind::Break, span: start });
        }
        if self.keyword("goto") {
            self.pos += 1;
            let Some(name) = self.name() else {
                self.diagnostics.push(Diagnostic {
                    origin: crate::DiagnosticOrigin::LuaSyntax,
                    span: start,
                    message: "expected a label name after 'goto'".to_owned(),
                });
                return None;
            };
            let span = start.join(name.span);
            return Some(Stmt { kind: StmtKind::Goto(name), span });
        }
        None
    }

    fn control_expression(&mut self, wrapper_depth: usize) -> Expr {
        let (expr, depth) = self.expr_with_depth();
        if depth + wrapper_depth > 128 {
            self.diagnostics.push(Diagnostic {
                origin: crate::DiagnosticOrigin::LuaSyntax,
                span: expr.span,
                message: "control expression nesting exceeds 128".to_owned(),
            });
            Expr { kind: ExprKind::Error, span: expr.span }
        } else { expr }
    }

    fn control_body(&mut self, terminators: &[&str], loop_body: bool, start: Span, wrapper_depth: usize) -> Vec<Stmt> {
        if self.block_depth + self.depth >= 128 {
            self.diagnostics.push(Diagnostic {
                origin: crate::DiagnosticOrigin::LuaSyntax,
                span: start,
                message: "control block nesting exceeds 128".to_owned(),
            });
            let terminator = if terminators == ["until"] { "until" } else { "end" };
            self.recover_block_until(terminator, false);
            return Vec::new();
        }
        self.block_depth += 1;
        if loop_body { self.loop_depth += 1; }
        let mut body = self.statements(terminators);
        if loop_body { self.loop_depth -= 1; }
        self.block_depth -= 1;
        if statements_depth(&body) + wrapper_depth > 128 {
            self.diagnostics.push(Diagnostic {
                origin: crate::DiagnosticOrigin::LuaSyntax,
                span: start,
                message: "control block nesting exceeds 128".to_owned(),
            });
            body.clear();
        }
        body
    }

    fn if_statement(&mut self, start: Span) -> Stmt {
        let mut branches = Vec::new();
        loop {
            let branch_start = self.current().span;
            self.pos += 1;
            let condition = self.control_expression(2);
            let then = self.expect_keyword("then");
            let body = self.control_body(&["elseif", "else", "end"], false, branch_start, 2);
            let body_end = self.current().span.start.max(then.end);
            branches.push(IfBranch {
                condition, body,
                span: Span::new(branch_start.start, body_end),
                body_span: Span::new(then.end, body_end),
            });
            if !self.keyword("elseif") { break; }
        }
        let else_branch = if self.keyword("else") {
            let else_start = self.current().span;
            self.pos += 1;
            let body = self.control_body(&["end"], false, else_start, 2);
            let body_end = self.current().span.start.max(else_start.end);
            Some(ElseBranch {
                body,
                span: Span::new(else_start.start, body_end),
                body_span: Span::new(else_start.end, body_end),
            })
        } else { None };
        let end = self.expect_end();
        Stmt { kind: StmtKind::If { branches, else_branch }, span: start.join(end) }
    }

    fn while_statement(&mut self, start: Span) -> Stmt {
        self.pos += 1;
        let condition = self.control_expression(1);
        let do_token = self.expect_keyword("do");
        let body = self.control_body(&["end"], true, start, 1);
        let body_end = self.current().span.start.max(do_token.end);
        let end = self.expect_end();
        Stmt {
            kind: StmtKind::While { condition, body, body_span: Span::new(do_token.end, body_end) },
            span: start.join(end),
        }
    }

    fn repeat_statement(&mut self, start: Span) -> Stmt {
        self.pos += 1;
        let body = self.control_body(&["until"], true, start, 1);
        let body_end = self.current().span.start.max(start.end);
        self.expect_keyword("until");
        let condition = self.control_expression(1);
        let span = start.join(condition.span);
        Stmt {
            kind: StmtKind::Repeat { body, condition, body_span: Span::new(start.end, body_end) },
            span,
        }
    }

    fn for_statement(&mut self, start: Span) -> Option<Stmt> {
        self.pos += 1;
        let Some(first) = self.name() else { self.recover_statement(); return None; };
        if self.significant_symbol("=") {
            self.expect("=");
            let initial = self.control_expression(1);
            self.expect(",");
            let limit = self.control_expression(1);
            let step = if self.significant_symbol(",") {
                self.expect(",");
                Some(self.control_expression(1))
            } else { None };
            let do_token = self.expect_keyword("do");
            let body = self.control_body(&["end"], true, start, 1);
            let body_end = self.current().span.start.max(do_token.end);
            let end = self.expect_end();
            return Some(Stmt {
                kind: StmtKind::NumericFor {
                    name: first, start: Box::new(initial), limit: Box::new(limit), step: step.map(Box::new), body,
                    body_span: Span::new(do_token.end, body_end),
                },
                span: start.join(end),
            });
        }
        let mut names = vec![first];
        while self.significant_symbol(",") {
            self.expect(",");
            let Some(name) = self.name() else { self.recover_statement(); return None; };
            names.push(name);
        }
        self.expect_keyword("in");
        let mut values = vec![self.control_expression(1)];
        while self.significant_symbol(",") {
            self.expect(",");
            values.push(self.control_expression(1));
        }
        let do_token = self.expect_keyword("do");
        let body = self.control_body(&["end"], true, start, 1);
        let body_end = self.current().span.start.max(do_token.end);
        let end = self.expect_end();
        Some(Stmt {
            kind: StmtKind::GenericFor {
                names, values, body,
                body_span: Span::new(do_token.end, body_end),
            },
            span: start.join(end),
        })
    }
}


/// Lua's rules for one function's labels: a `goto` jumps to a label visible from it (in its
/// block or an enclosing one), and a label may not repeat a visible label's name (Lua 5.4).
/// Nested functions are validated with their own bodies.
pub(crate) fn validate_labels(body: &[Stmt], diagnostics: &mut Vec<Diagnostic>) {
    // A goto sees every label of its open blocks, while a label repeats only
    // one declared before it in an open block (Lua 5.4's findlabel).
    fn block(
        statements: &[Stmt],
        visible: &mut Vec<Vec<String>>,
        declared: &mut Vec<String>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        visible.push(statements.iter().filter_map(|statement| match &statement.kind {
            StmtKind::Label(name) => Some(name.text.clone()),
            _ => None,
        }).collect());
        let outer = declared.len();
        for statement in statements {
            match &statement.kind {
                StmtKind::Label(name) => {
                    if declared.contains(&name.text) {
                        diagnostics.push(Diagnostic {
                            origin: crate::DiagnosticOrigin::LuaSyntax,
                            span: name.span,
                            message: format!("label '{}' already defined", name.text),
                        });
                    }
                    declared.push(name.text.clone());
                }
                StmtKind::Goto(name) if !visible.iter().flatten().any(|label| *label == name.text) => {
                    diagnostics.push(Diagnostic {
                        origin: crate::DiagnosticOrigin::LuaSyntax,
                        span: name.span,
                        message: format!("no visible label '{}' for goto", name.text),
                    });
                }
                StmtKind::Block { body, .. } | StmtKind::While { body, .. } | StmtKind::Repeat { body, .. }
                | StmtKind::NumericFor { body, .. } | StmtKind::GenericFor { body, .. } => block(body, visible, declared, diagnostics),
                StmtKind::If { branches, else_branch } => {
                    for branch in branches { block(&branch.body, visible, declared, diagnostics); }
                    if let Some(branch) = else_branch { block(&branch.body, visible, declared, diagnostics); }
                }
                _ => {}
            }
        }
        declared.truncate(outer);
        visible.pop();
    }
    block(body, &mut Vec::new(), &mut Vec::new(), diagnostics);
}
