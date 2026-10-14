use crate::{
    CommentKind, Diagnostic, Expr, ExprKind, File, Name, Span, Stmt, StmtKind, TableField, Token,
    TokenKind, TypeAnnotation, lex, types,
};

pub fn parse(source: &str) -> File {
    let lexed = lex(source);
    let comments = lexed
        .tokens
        .iter()
        .filter_map(|token| {
            if let TokenKind::Comment(comment) = &token.kind {
                Some(comment.clone())
            } else {
                None
            }
        })
        .collect();
    let mut parser = Parser {
        source,
        tokens: lexed.tokens,
        pos: 0,
        diagnostics: lexed.diagnostics,
        pending_annotation: None,
        depth: 0,
    };
    let mut statements = Vec::new();
    loop {
        parser.leading_trivia();
        if parser.eof() {
            break;
        }
        if parser.take_symbol(";").is_some() {
            continue;
        }
        let before = parser.pos;
        if let Some(stmt) = parser.statement() {
            statements.push(stmt);
        }
        if parser.pos == before {
            parser.pos += 1;
        }
    }
    if let Some(annotation) = parser.pending_annotation.take() {
        parser.diagnostics.push(Diagnostic {
            span: annotation.span,
            message: "@type annotation is not attached to a declaration or assignment".to_owned(),
        });
    }
    File {
        statements,
        comments,
        diagnostics: parser.diagnostics,
        span: Span::new(0, source.len()),
    }
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
    pending_annotation: Option<TypeAnnotation>,
    depth: usize,
}

impl Parser<'_> {
    fn current(&self) -> &Token {
        &self.tokens[self.pos]
    }
    fn eof(&self) -> bool {
        matches!(self.current().kind, TokenKind::Eof)
    }
    fn symbol(&self, value: &str) -> bool {
        matches!(&self.current().kind, TokenKind::Symbol(symbol) if symbol == value)
    }
    fn keyword(&self, value: &str) -> bool {
        matches!(&self.current().kind, TokenKind::Keyword(keyword) if keyword == value)
    }
    fn take_symbol(&mut self, value: &str) -> Option<Span> {
        if self.symbol(value) {
            let span = self.current().span;
            self.pos += 1;
            Some(span)
        } else {
            None
        }
    }
    fn error(&mut self, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            span: self.current().span,
            message: message.into(),
        });
    }
    fn expect(&mut self, symbol: &str) -> Span {
        self.inner_trivia();
        self.take_symbol(symbol).unwrap_or_else(|| {
            self.error(format!("expected {symbol:?}"));
            self.current().span
        })
    }
    fn significant_index(&self) -> usize {
        let mut pos = self.pos;
        while matches!(
            self.tokens[pos].kind,
            TokenKind::Newline | TokenKind::Comment(_)
        ) {
            pos += 1;
        }
        pos
    }
    fn significant_symbol(&self, value: &str) -> bool {
        matches!(&self.tokens[self.significant_index()].kind, TokenKind::Symbol(symbol) if symbol == value)
    }
    fn leading_trivia(&mut self) {
        loop {
            match self.current().kind.clone() {
                TokenKind::Newline => self.pos += 1,
                TokenKind::Comment(comment) => {
                    self.pos += 1;
                    if comment.kind != CommentKind::Documentation {
                        continue;
                    }
                    let (annotation, diagnostics) = types::annotation(&comment);
                    self.diagnostics.extend(diagnostics);
                    if let Some(annotation) = annotation {
                        let line_start = self.source[..comment.span.start]
                            .rfind(['\r', '\n'])
                            .map_or(0, |index| index + 1);
                        let standalone = self.source[line_start..comment.span.start]
                            .chars()
                            .all(|ch| matches!(ch, ' ' | '\t'));
                        if !standalone {
                            self.diagnostics.push(Diagnostic {
                                span: annotation.span, message: "this slice requires @type annotations before a declaration or assignment".to_owned(),
                            });
                        } else if self.pending_annotation.is_some() {
                            self.diagnostics.push(Diagnostic {
                                span: annotation.span,
                                message: "multiple @type annotations for one declaration"
                                    .to_owned(),
                            });
                        } else {
                            self.pending_annotation = Some(annotation);
                        }
                    }
                }
                _ => break,
            }
        }
    }
    fn inner_trivia(&mut self) {
        loop {
            match self.current().kind.clone() {
                TokenKind::Newline => self.pos += 1,
                TokenKind::Comment(comment) => {
                    self.pos += 1;
                    if comment.kind == CommentKind::Documentation {
                        let (annotation, diagnostics) = types::annotation(&comment);
                        self.diagnostics.extend(diagnostics);
                        if let Some(annotation) = annotation {
                            self.diagnostics.push(Diagnostic {
                                span: annotation.span, message: "@type annotations inside expressions are unsupported in this slice".to_owned(),
                            });
                        }
                    }
                }
                _ => break,
            }
        }
    }
    fn name(&mut self) -> Option<Name> {
        self.inner_trivia();
        if let TokenKind::Identifier(text) = self.current().kind.clone() {
            let span = self.current().span;
            self.pos += 1;
            Some(Name { text, span })
        } else {
            self.error("expected a Lua identifier");
            None
        }
    }
    fn statement(&mut self) -> Option<Stmt> {
        let start = self.current().span;
        let annotation = self.pending_annotation.take();
        if self.keyword("local") {
            self.pos += 1;
            if self.tokens[self.significant_index()].kind
                == TokenKind::Keyword("function".to_owned())
            {
                self.error("local functions are unsupported in this first frontend slice");
                self.recover_statement();
                return None;
            }
            let Some(first) = self.name() else {
                self.recover_statement();
                return None;
            };
            let mut names = vec![first];
            while self.significant_symbol(",") {
                self.inner_trivia();
                self.pos += 1;
                let Some(name) = self.name() else {
                    self.recover_statement();
                    return None;
                };
                names.push(name);
            }
            let values = if self.significant_symbol("=") {
                self.inner_trivia();
                self.pos += 1;
                self.expr_list()
            } else {
                Vec::new()
            };
            let end = values
                .last()
                .map_or(names.last().unwrap().span, |value| value.span);
            return Some(Stmt {
                kind: StmtKind::Local {
                    names,
                    values,
                    annotation,
                },
                span: start.join(end),
            });
        }
        if matches!(&self.current().kind, TokenKind::Keyword(_)) {
            self.error(
                "unsupported Lua statement; this slice supports local declarations and assignments",
            );
            self.recover_statement();
            return None;
        }
        let targets = self.expr_list();
        if !self.significant_symbol("=") {
            self.error("expected an assignment; calls and other expression statements are unsupported in this slice");
            self.recover_statement();
            return None;
        }
        self.inner_trivia();
        self.pos += 1;
        for target in &targets {
            if !matches!(target.kind, ExprKind::Name(_) | ExprKind::Field { .. }) {
                self.diagnostics.push(Diagnostic {
                    span: target.span,
                    message: "invalid assignment target".to_owned(),
                });
            }
        }
        let values = self.expr_list();
        let end = values.last().map_or(start, |value| value.span);
        Some(Stmt {
            kind: StmtKind::Assign {
                targets,
                values,
                annotation,
            },
            span: start.join(end),
        })
    }
    fn expr_list(&mut self) -> Vec<Expr> {
        let mut values = vec![self.expr()];
        while self.significant_symbol(",") {
            self.inner_trivia();
            self.pos += 1;
            values.push(self.expr());
        }
        values
    }
    fn expr(&mut self) -> Expr {
        self.expr_with_depth().0
    }
    // Track the depth of the constructed tree as well as recursive parser calls.
    // Postfix access is parsed iteratively but still creates nested boxed nodes.
    fn expr_with_depth(&mut self) -> (Expr, usize) {
        self.inner_trivia();
        let start = self.current().span;
        if self.depth >= 128 {
            self.error("expression nesting exceeds 128");
            if !self.eof() {
                self.pos += 1;
            }
            return (
                Expr {
                    kind: ExprKind::Error,
                    span: start,
                },
                1,
            );
        }
        self.depth += 1;
        let mut built_depth = 1;
        let mut expr = match self.current().kind.clone() {
            TokenKind::Identifier(text) => {
                self.pos += 1;
                Expr {
                    kind: ExprKind::Name(Name { text, span: start }),
                    span: start,
                }
            }
            TokenKind::Number(spelling) => {
                self.pos += 1;
                Expr {
                    kind: ExprKind::Number(spelling),
                    span: start,
                }
            }
            TokenKind::String(value) => {
                self.pos += 1;
                Expr {
                    kind: ExprKind::String(value),
                    span: start,
                }
            }
            TokenKind::Keyword(keyword) if keyword == "nil" => {
                self.pos += 1;
                Expr {
                    kind: ExprKind::Nil,
                    span: start,
                }
            }
            TokenKind::Keyword(keyword) if keyword == "true" || keyword == "false" => {
                self.pos += 1;
                Expr {
                    kind: ExprKind::Boolean(keyword == "true"),
                    span: start,
                }
            }
            TokenKind::Symbol(symbol) if symbol == "-" => {
                self.pos += 1;
                self.inner_trivia();
                if let TokenKind::Number(spelling) = self.current().kind.clone() {
                    let span = start.join(self.current().span);
                    self.pos += 1;
                    Expr {
                        kind: ExprKind::Number(format!("-{spelling}")),
                        span,
                    }
                } else {
                    self.error("unary operators are currently supported only for negative numeric literals");
                    Expr {
                        kind: ExprKind::Error,
                        span: start,
                    }
                }
            }
            TokenKind::Symbol(symbol) if symbol == "(" => {
                self.pos += 1;
                let (inner, inner_depth) = self.expr_with_depth();
                let end = self.expect(")");
                let span = start.join(end);
                if inner_depth >= 128 {
                    self.diagnostics.push(Diagnostic {
                        span,
                        message: "expression nesting exceeds 128".to_owned(),
                    });
                    Expr {
                        kind: ExprKind::Error,
                        span,
                    }
                } else {
                    built_depth = inner_depth + 1;
                    Expr {
                        kind: ExprKind::Parenthesized(Box::new(inner)),
                        span,
                    }
                }
            }
            TokenKind::Symbol(symbol) if symbol == "{" => {
                let (table, table_depth) = self.table();
                built_depth = table_depth;
                table
            }
            _ => {
                self.error(
                    "expected an expression; this Lua expression form is unsupported in this slice",
                );
                if !self.eof() && !self.symbol(",") && !self.symbol("}") && !self.symbol(")") {
                    self.pos += 1;
                }
                Expr {
                    kind: ExprKind::Error,
                    span: start,
                }
            }
        };
        let mut postfix_exceeded = false;
        while self.significant_symbol(".") {
            self.inner_trivia();
            let postfix_start = self.current().span;
            self.pos += 1;
            let Some(name) = self.name() else {
                break;
            };
            let span = expr.span.join(name.span);
            if postfix_exceeded || built_depth >= 128 {
                if !postfix_exceeded {
                    self.diagnostics.push(Diagnostic {
                        span: postfix_start.join(name.span),
                        message: "expression nesting exceeds 128".to_owned(),
                    });
                }
                // Keep consuming the suffix without building or retaining an
                // unbounded tree. Recovery resumes at the next Lua statement.
                expr = Expr {
                    kind: ExprKind::Error,
                    span,
                };
                built_depth = 1;
                postfix_exceeded = true;
                continue;
            }
            expr = Expr {
                kind: ExprKind::Field {
                    base: Box::new(expr),
                    name,
                },
                span,
            };
            built_depth += 1;
        }
        self.depth -= 1;
        (expr, built_depth)
    }
    fn table(&mut self) -> (Expr, usize) {
        let start = self.expect("{");
        let mut fields = Vec::new();
        let mut built_depth = 1;
        loop {
            self.inner_trivia();
            if self.eof() || self.symbol("}") {
                break;
            }
            let before = self.pos;
            let Some(name) = self.name() else {
                self.error("table expressions currently require named fields (name = value)");
                self.recover_table();
                if self.take_symbol(",").is_some() || self.take_symbol(";").is_some() {
                    continue;
                }
                break;
            };
            if !self.significant_symbol("=") {
                self.error("table expressions currently require named fields (name = value)");
                self.recover_table();
                if self.take_symbol(",").is_some() || self.take_symbol(";").is_some() {
                    continue;
                }
                break;
            }
            self.expect("=");
            let (mut value, mut value_depth) = self.expr_with_depth();
            if value_depth >= 128 {
                self.diagnostics.push(Diagnostic {
                    span: value.span,
                    message: "expression nesting exceeds 128".to_owned(),
                });
                value = Expr {
                    kind: ExprKind::Error,
                    span: value.span,
                };
                value_depth = 1;
            }
            built_depth = built_depth.max(value_depth + 1);
            let span = name.span.join(value.span);
            fields.push(TableField { name, value, span });
            self.inner_trivia();
            if self.take_symbol(",").is_some() || self.take_symbol(";").is_some() {
                continue;
            }
            if !self.symbol("}") {
                self.error("expected ',' or '}' after table field");
                self.recover_table();
                if self.take_symbol(",").is_some() || self.take_symbol(";").is_some() {
                    continue;
                }
            }
            if self.pos == before {
                self.pos += 1;
            }
            break;
        }
        let end = self.expect("}");
        (
            Expr {
                kind: ExprKind::Table(fields),
                span: start.join(end),
            },
            built_depth,
        )
    }
    fn recover_statement(&mut self) {
        while !self.eof() && !matches!(self.current().kind, TokenKind::Newline) && !self.symbol(";")
        {
            self.pos += 1;
        }
    }
    fn recover_table(&mut self) {
        let mut nesting = 0_usize;
        while !self.eof() {
            if nesting == 0 && (self.symbol(",") || self.symbol(";") || self.symbol("}")) {
                break;
            }
            if self.symbol("{") || self.symbol("(") || self.symbol("[") {
                nesting += 1;
            }
            if (self.symbol("}") || self.symbol(")") || self.symbol("]")) && nesting > 0 {
                nesting -= 1;
            }
            self.pos += 1;
        }
    }
}
