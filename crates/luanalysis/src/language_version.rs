//! Availability of parsed Lua syntax in the host's selected language version.
//! The parser retains the newest grammar for recovery and editor queries.
use crate::Diagnostic;
use crate::standard_environment::StandardEnvironmentProfile;
use luanalysis_syntax::{
    BinaryOperator, Expr, ExprKind, File, Span, Stmt, StmtKind, TableFieldKind, Token, TokenKind,
    UnaryOperator,
};

pub(crate) fn diagnostics(
    file: &File,
    source: &str,
    profile: StandardEnvironmentProfile,
) -> Vec<Diagnostic> {
    let lua53 = match profile {
        StandardEnvironmentProfile::Lua50
        | StandardEnvironmentProfile::Lua51
        | StandardEnvironmentProfile::Lua52 => false,
        StandardEnvironmentProfile::Lua53 => true,
        StandardEnvironmentProfile::Lua54
        | StandardEnvironmentProfile::LegacyMinimal
        | StandardEnvironmentProfile::None => return Vec::new(),
    };
    let mut validator = Validator {
        source,
        lua53,
        tokens: None,
        diagnostics: Vec::new(),
    };
    validator.body(&file.statements);
    validator
        .diagnostics
        .sort_by_key(|diagnostic| (diagnostic.span.start, diagnostic.span.end));
    validator.diagnostics
}

struct Validator<'a> {
    source: &'a str,
    lua53: bool,
    // Only attribute diagnostics need the delimiters not retained by the AST's Name.
    tokens: Option<Vec<Token>>,
    diagnostics: Vec<Diagnostic>,
}

impl Validator<'_> {
    fn error(&mut self, span: Span, message: String) {
        self.diagnostics.push(Diagnostic {
            code: "LUA1001",
            span,
            message,
        });
    }

    fn attribute_span(&mut self, name: Span) -> Span {
        let tokens = self
            .tokens
            .get_or_insert_with(|| luanalysis_syntax::lex(self.source).tokens);
        let Ok(index) = tokens.binary_search_by_key(&name.start, |token| token.span.start) else {
            return name;
        };
        let significant =
            |token: &&Token| !matches!(token.kind, TokenKind::Comment(_) | TokenKind::Newline);
        let open = tokens[..index].iter().rev().find(significant);
        let close = tokens[index + 1..].iter().find(significant);
        match (open, close) {
            (Some(open), Some(close))
                if matches!(&open.kind, TokenKind::Symbol(symbol) if symbol == "<")
                    && matches!(&close.kind, TokenKind::Symbol(symbol) if symbol == ">") =>
            {
                open.span.join(close.span)
            }
            _ => name,
        }
    }

    fn body(&mut self, body: &[Stmt]) {
        for statement in body {
            match &statement.kind {
                StmtKind::Local {
                    attributes, values, ..
                } => {
                    for attribute in attributes.iter().flatten() {
                        if matches!(attribute.text.as_str(), "const" | "close") {
                            let span = self.attribute_span(attribute.span);
                            self.error(span, format!(
                                "The local variable attribute '<{}>' is only available in Lua 5.4 and above",
                                attribute.text,
                            ));
                        }
                    }
                    self.values(values);
                }
                StmtKind::Assign {
                    targets, values, ..
                } => {
                    self.values(targets);
                    self.values(values);
                }
                StmtKind::Block { body, .. } => self.body(body),
                StmtKind::LocalFunction { function, .. } => self.body(&function.body),
                StmtKind::FunctionDeclaration {
                    target, function, ..
                } => {
                    self.expression(target);
                    self.body(&function.body);
                }
                StmtKind::Return(values) => self.values(values),
                StmtKind::Call(value) => self.expression(value),
                StmtKind::If {
                    branches,
                    else_branch,
                } => {
                    for branch in branches {
                        self.expression(&branch.condition);
                        self.body(&branch.body);
                    }
                    if let Some(branch) = else_branch {
                        self.body(&branch.body);
                    }
                }
                StmtKind::While {
                    condition, body, ..
                }
                | StmtKind::Repeat {
                    condition, body, ..
                } => {
                    self.expression(condition);
                    self.body(body);
                }
                StmtKind::NumericFor {
                    start,
                    limit,
                    step,
                    body,
                    ..
                } => {
                    self.expression(start);
                    self.expression(limit);
                    if let Some(step) = step {
                        self.expression(step);
                    }
                    self.body(body);
                }
                StmtKind::GenericFor { values, body, .. } => {
                    self.values(values);
                    self.body(body);
                }
                StmtKind::Goto(_) | StmtKind::Label(_) | StmtKind::Break => {}
            }
        }
    }

    fn values(&mut self, values: &[Expr]) {
        for value in values {
            self.expression(value);
        }
    }

    fn expression(&mut self, expression: &Expr) {
        match &expression.kind {
            ExprKind::Unary { op, op_span, value } => {
                if !self.lua53 && *op == UnaryOperator::BitwiseNot {
                    self.error(
                        *op_span,
                        "The unary operator '~' only available in Lua 5.3 and above".into(),
                    );
                }
                self.expression(value);
            }
            ExprKind::Binary {
                op,
                op_span,
                left,
                right,
            } => {
                if !self.lua53
                    && matches!(
                        op,
                        BinaryOperator::FloorDivide
                            | BinaryOperator::BitwiseAnd
                            | BinaryOperator::BitwiseOr
                            | BinaryOperator::BitwiseXor
                            | BinaryOperator::ShiftLeft
                            | BinaryOperator::ShiftRight
                    )
                {
                    self.error(
                        *op_span,
                        format!(
                            "The binary operator '{}' only available in Lua 5.3 and above",
                            &self.source[op_span.start..op_span.end],
                        ),
                    );
                }
                self.expression(left);
                self.expression(right);
            }
            ExprKind::Parenthesized(value) => self.expression(value),
            ExprKind::Table(fields) => {
                for field in fields {
                    if let TableFieldKind::Keyed(key) = &field.kind {
                        self.expression(key);
                    }
                    self.expression(&field.value);
                }
            }
            ExprKind::Field { base, .. } => self.expression(base),
            ExprKind::Index { base, index } => {
                self.expression(base);
                self.expression(index);
            }
            ExprKind::Function(function) => self.body(&function.body),
            ExprKind::Call { callee, args } => {
                self.expression(callee);
                self.values(args);
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                self.expression(receiver);
                self.values(args);
            }
            ExprKind::Nil
            | ExprKind::Boolean(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Name(_)
            | ExprKind::Vararg
            | ExprKind::Error => {}
        }
    }
}
