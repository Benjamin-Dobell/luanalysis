use crate::{
    Comment, Diagnostic, Name, Span, Token, TokenKind, TypeAnnotation, TypeExpr, TypeExprKind,
    TypeField, lex,
};

/// Parse only documentation tags. Ordinary prose remains a retained comment.
pub(crate) fn annotation(comment: &Comment) -> (Option<TypeAnnotation>, Vec<Diagnostic>) {
    let mut result = None;
    let mut diagnostics = Vec::new();
    let text = &comment.text;
    let (body, base) = if text.starts_with("---") {
        (text.as_str(), comment.span.start)
    } else if text.starts_with("--[") {
        let mut opening_end = 3;
        while text.as_bytes().get(opening_end) == Some(&b'=') {
            opening_end += 1;
        }
        if text.as_bytes().get(opening_end) != Some(&b'[') {
            return (None, diagnostics);
        }
        let close = format!("]{}]", "=".repeat(opening_end - 3));
        let end = if text.ends_with(&close) {
            text.len() - close.len()
        } else {
            text.len()
        };
        (
            &text[opening_end + 1..end],
            comment.span.start + opening_end + 1,
        )
    } else {
        return (None, diagnostics);
    };
    let mut line_offset = 0;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_start_matches([' ', '\t', '\r']);
        let stripped = trimmed
            .trim_start_matches('-')
            .trim_start_matches([' ', '\t']);
        if let Some(tag) = stripped.strip_prefix('@') {
            let tag_offset = base + line_offset + line.len() - stripped.len();
            let tag_len = tag
                .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                .unwrap_or(tag.len());
            let tag_name = &tag[..tag_len];
            if tag_name != "type" {
                diagnostics.push(Diagnostic {
                    span: Span::new(tag_offset, tag_offset + 1 + tag_len),
                    message: format!(
                        "unsupported Luanalysis annotation @{tag_name}; this slice supports @type"
                    ),
                });
            } else {
                let raw_types = &tag[tag_len..];
                let types = raw_types.trim_start_matches([' ', '\t']);
                let types = types.trim_end_matches(['\r', '\n']);
                // `@` explicitly begins annotation documentation in doc.flex.
                let types = types.split_once('@').map_or(types, |(ty, _)| ty).trim_end();
                let types_offset = tag_offset + 1 + tag_len + raw_types.len()
                    - raw_types.trim_start_matches([' ', '\t']).len();
                let lexed = lex(types);
                let tokens = lexed
                    .tokens
                    .into_iter()
                    .map(|mut token| {
                        token.span.start += types_offset;
                        token.span.end += types_offset;
                        token
                    })
                    .collect();
                diagnostics.extend(lexed.diagnostics.into_iter().map(|mut diagnostic| {
                    diagnostic.span.start += types_offset;
                    diagnostic.span.end += types_offset;
                    diagnostic
                }));
                let mut parser = TypeParser {
                    tokens,
                    pos: 0,
                    diagnostics: Vec::new(),
                    depth: 0,
                };
                let parsed_types = parser.list();
                if !parser.eof() {
                    parser.error("unsupported or unexpected syntax after @type".to_owned());
                }
                diagnostics.extend(parser.diagnostics);
                let annotation = TypeAnnotation {
                    span: Span::new(tag_offset, types_offset + types.len()),
                    types: parsed_types,
                };
                if result.is_some() {
                    diagnostics.push(Diagnostic {
                        span: annotation.span,
                        message: "multiple @type annotations for one declaration".to_owned(),
                    });
                } else {
                    result = Some(annotation);
                }
            }
        }
        line_offset += line.len();
    }
    (result, diagnostics)
}

struct TypeParser {
    tokens: Vec<Token>,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
    depth: usize,
}

impl TypeParser {
    fn current(&self) -> &Token {
        &self.tokens[self.pos]
    }
    fn eof(&self) -> bool {
        matches!(self.current().kind, TokenKind::Eof)
    }
    fn symbol(&self, value: &str) -> bool {
        matches!(&self.current().kind, TokenKind::Symbol(symbol) if symbol == value)
    }
    fn take(&mut self, value: &str) -> Option<Span> {
        if self.symbol(value) {
            let span = self.current().span;
            self.pos += 1;
            Some(span)
        } else {
            None
        }
    }
    fn error(&mut self, message: String) {
        self.diagnostics.push(Diagnostic {
            span: self.current().span,
            message,
        });
    }
    fn expect(&mut self, value: &str) -> Span {
        self.take(value).unwrap_or_else(|| {
            self.error(format!("expected {value:?} in type annotation"));
            self.current().span
        })
    }
    fn list(&mut self) -> Vec<TypeExpr> {
        let mut types = vec![self.union()];
        while self.take(",").is_some() {
            types.push(self.union());
        }
        types
    }
    fn union(&mut self) -> TypeExpr {
        let first = self.primary();
        if self.take("|").is_none() {
            return first;
        }
        let mut types = vec![first];
        loop {
            types.push(self.primary());
            if self.take("|").is_none() {
                break;
            }
        }
        let span = types.first().unwrap().span.join(types.last().unwrap().span);
        TypeExpr {
            kind: TypeExprKind::Union(types),
            span,
        }
    }
    fn primary(&mut self) -> TypeExpr {
        let start = self.current().span;
        if self.depth >= 128 {
            self.error("type annotation nesting exceeds 128".to_owned());
            if !self.eof() {
                self.pos += 1;
            }
            return TypeExpr {
                kind: TypeExprKind::Error,
                span: start,
            };
        }
        self.depth += 1;
        let ty = match self.current().kind.clone() {
            TokenKind::Identifier(name) => {
                self.pos += 1;
                TypeExpr {
                    kind: TypeExprKind::Named(name),
                    span: start,
                }
            }
            TokenKind::Keyword(name) if name == "nil" => {
                self.pos += 1;
                TypeExpr {
                    kind: TypeExprKind::Named(name),
                    span: start,
                }
            }
            TokenKind::Symbol(symbol) if symbol == "(" => {
                self.pos += 1;
                let inner = self.union();
                let end = self.expect(")");
                TypeExpr {
                    kind: TypeExprKind::Parenthesized(Box::new(inner)),
                    span: start.join(end),
                }
            }
            TokenKind::Symbol(symbol) if symbol == "{" => self.record(),
            _ => {
                self.error(
                    "expected a named, union, or record type; this annotation form is unsupported"
                        .to_owned(),
                );
                if !self.eof() && !self.symbol(",") && !self.symbol("}") && !self.symbol(")") {
                    self.pos += 1;
                }
                TypeExpr {
                    kind: TypeExprKind::Error,
                    span: start,
                }
            }
        };
        self.depth -= 1;
        ty
    }
    fn record(&mut self) -> TypeExpr {
        let start = self.expect("{");
        let mut fields = Vec::new();
        while !self.eof() && !self.symbol("}") {
            let field_start = self.current().span;
            let name = if let TokenKind::Identifier(text) = self.current().kind.clone() {
                self.pos += 1;
                Name {
                    text,
                    span: field_start,
                }
            } else {
                self.error("record annotations currently require named fields".to_owned());
                self.recover_record();
                if self.take(",").is_some() {
                    continue;
                }
                break;
            };
            self.expect(":");
            let ty = self.union();
            let span = field_start.join(ty.span);
            fields.push(TypeField { name, ty, span });
            if self.take(",").is_some() {
                continue;
            }
            if !self.symbol("}") {
                self.error("expected ',' or '}' after record field".to_owned());
                self.recover_record();
                if self.take(",").is_some() {
                    continue;
                }
            }
            break;
        }
        let end = self.expect("}");
        TypeExpr {
            kind: TypeExprKind::Record(fields),
            span: start.join(end),
        }
    }
    fn recover_record(&mut self) {
        while !self.eof() && !self.symbol(",") && !self.symbol("}") {
            self.pos += 1;
        }
    }
}
