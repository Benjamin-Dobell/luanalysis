use crate::{
    Comment, Diagnostic, FunctionAnnotations, FunctionType, FunctionTypeParam, GenericParameter,
    Name, ParamAnnotation, Span, Token, TokenKind, TypeAnnotation, TypeExpr, TypeExprKind,
    TypeField, TypeFieldKey, TypeLiteral, TypePack, TypePacks, VarargAnnotation, VariadicTypeIndex, lex,
    FieldDeclaration, FieldVisibility, FieldVisibilityAnnotation, NamedRecordKind,
    NamedTypeDeclarationKind,
};

#[derive(Default)]
pub(crate) struct ParsedDocumentation {
    pub functions: FunctionAnnotations,
    pub declarations: Vec<DeclarationItem>,
}

pub(crate) enum DeclarationItem {
    Named {
        name: Name,
        generics: Vec<GenericParameter>,
        kind: NamedTypeDeclarationKind,
        span: Span,
    },
    Field(FieldDeclaration),
}

/// Parse only documentation tags. Ordinary prose remains a retained comment.
pub(crate) fn annotations(comment: &Comment) -> (ParsedDocumentation, Vec<Diagnostic>) {
    let mut result = ParsedDocumentation::default();
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
            return (result, diagnostics);
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
        return (result, diagnostics);
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
            if !matches!(tag_name, "type" | "param" | "return" | "generic" | "vararg" | "alias" | "shape" | "class" | "module" | "field") {
                diagnostics.push(Diagnostic {
                    span: Span::new(tag_offset, tag_offset + 1 + tag_len),
                    message: format!(
                        "unsupported Luanalysis annotation @{tag_name}"
                    ),
                });
            } else {
                let raw_types = &tag[tag_len..];
                let types = raw_types.trim_start_matches([' ', '\t']);
                let types = types.trim_end_matches(['\r', '\n']);
                // `@` begins trailing documentation in doc.flex's type state,
                // but an @ inside a quoted literal belongs to that literal.
                let types = annotation_type_text(types);
                let types_offset = tag_offset + 1 + tag_len + raw_types.len()
                    - raw_types.trim_start_matches([' ', '\t']).len();
                let lexed = lex(types);
                let tokens = lexed
                    .tokens
                    .into_iter()
                    .flat_map(|mut token| {
                        token.span.start += types_offset;
                        token.span.end += types_offset;
                        // Annotation generic closers are individual source `>`
                        // tokens, while the shared Lua lexer recognizes `>>`.
                        if matches!(&token.kind, TokenKind::Symbol(symbol) if symbol == ">>") {
                            let middle = token.span.start + 1;
                            vec![Token { kind: TokenKind::Symbol(">".to_owned()), span: Span::new(token.span.start, middle) },
                                Token { kind: TokenKind::Symbol(">".to_owned()), span: Span::new(middle, token.span.end) }]
                        } else { vec![token] }
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
                    comma_ends_type: 0,
                };
                let annotation_span = Span::new(tag_offset, types_offset + types.len());
                let mut group = FunctionAnnotations::default();
                match tag_name {
                    "alias" | "shape" | "class" | "module" => {
                        if let Some(name) = parser.declaration_name("named type") {
                            let generics = parser.declaration_parameters();
                            let kind = if tag_name == "alias" {
                                NamedTypeDeclarationKind::Alias { target: parser.union() }
                            } else {
                                let base = parser.take(":").map(|_| parser.primary());
                                NamedTypeDeclarationKind::Record {
                                    flavor: match tag_name {
                                        "shape" => NamedRecordKind::Shape,
                                        "class" => NamedRecordKind::Class,
                                        "module" => NamedRecordKind::Module,
                                        _ => unreachable!(),
                                    },
                                    base,
                                    fields: Vec::new(),
                                }
                            };
                            result.declarations.push(DeclarationItem::Named {
                                name, generics, kind, span: annotation_span,
                            });
                            parser.documentation_tail();
                        }
                    }
                    "field" => {
                        if let Some(field) = parser.field_declaration(annotation_span) {
                            result.declarations.push(DeclarationItem::Field(field));
                            parser.documentation_tail();
                        }
                    }
                    "generic" => {
                        group.generics = parser.generic_parameters();
                    }
                    "param" => {
                        if let Some(name) = parser.name() {
                            let optional = parser.take("?").is_some();
                            let ty = parser.union();
                            group.params.push(ParamAnnotation {
                                name,
                                ty,
                                optional,
                                span: annotation_span,
                            });
                        }
                    }
                    "vararg" => {
                        group.vararg = Some(Box::new(VarargAnnotation {
                            ty: parser.union(),
                            span: annotation_span,
                            ellipsis_span: None,
                        }));
                    }
                    "return" => {
                        let (types, packs) = parser.return_packs();
                        group.returns = Some(TypeAnnotation {
                            span: annotation_span,
                            types,
                            packs,
                        });
                    }
                    "type" => {
                        let (types, packs) = parser.annotation_type_list();
                        group.type_annotation = Some(TypeAnnotation {
                            span: annotation_span,
                            types,
                            packs,
                        });
                    }
                    _ => unreachable!(),
                }
                if !parser.eof() {
                    parser.error(format!(
                        "unsupported or unexpected syntax after @{tag_name}"
                    ));
                }
                diagnostics.extend(parser.diagnostics);
                merge_annotations(&mut result.functions, group, &mut diagnostics);
            }
        }
        line_offset += line.len();
    }
    (result, diagnostics)
}

fn type_depth(root: &TypeExpr) -> usize {
    let mut pending = vec![(root, 1_usize)];
    let mut depth = 0;
    while let Some((ty, current)) = pending.pop() {
        depth = depth.max(current);
        match &ty.kind {
            TypeExprKind::Parenthesized(element) | TypeExprKind::Array { element, .. } => pending.push((element, current + 1)),
            TypeExprKind::Union(types) | TypeExprKind::Generic { args: types, .. } => {
                pending.extend(types.iter().map(|ty| (ty, current + 1)));
            }
            TypeExprKind::Record(fields) => {
                for field in fields {
                    if let TypeFieldKey::Indexed(key) = &field.key { pending.push((key, current + 1)); }
                    pending.push((&field.ty, current + 1));
                }
            }
            TypeExprKind::Function(function) => {
                pending.extend(function.params.iter().filter_map(|param| param.ty.as_ref()).map(|ty| (ty, current + 1)));
                if let Some(vararg) = &function.vararg { pending.push((&vararg.ty, current + 1)); }
                pending.extend(function.returns.iter().map(|ty| (ty, current + 1)));
            }
            TypeExprKind::Named(_) | TypeExprKind::Literal(_) | TypeExprKind::Error => {},
        }
    }
    depth
}

fn annotation_type_text(text: &str) -> &str {
    if !text.contains('@') {
        return text.trim_end();
    }
    let end = lex(text)
        .tokens
        .iter()
        .find_map(|token| {
            matches!(&token.kind, TokenKind::Symbol(symbol) if symbol == "@")
                .then_some(token.span.start)
        })
        .unwrap_or(text.len());
    text[..end].trim_end()
}

pub(crate) fn merge_annotations(
    target: &mut FunctionAnnotations,
    incoming: FunctionAnnotations,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for generic in incoming.generics {
        if target
            .generics
            .iter()
            .any(|existing| existing.name.text == generic.name.text)
        {
            diagnostics.push(Diagnostic {
                span: generic.name.span,
                message: format!("multiple @generic declarations for {}", generic.name.text),
            });
        } else {
            target.generics.push(generic);
        }
    }
    for param in incoming.params {
        if target
            .params
            .iter()
            .any(|existing| existing.name.text == param.name.text)
        {
            diagnostics.push(Diagnostic {
                span: param.span,
                message: format!("multiple @param annotations for {}", param.name.text),
            });
        } else {
            target.params.push(param);
        }
    }
    if let Some(vararg) = incoming.vararg {
        if target.vararg.is_some() {
            diagnostics.push(Diagnostic {
                span: vararg.span,
                message: "multiple @vararg annotations for one function".to_owned(),
            });
        } else {
            target.vararg = Some(vararg);
        }
    }
    if let Some(returns) = incoming.returns {
        if target.returns.is_some() {
            diagnostics.push(Diagnostic {
                span: returns.span,
                message: "multiple @return annotations for one function".to_owned(),
            });
        } else {
            target.returns = Some(returns);
        }
    }
    if let Some(annotation) = incoming.type_annotation {
        if target.type_annotation.is_some() {
            diagnostics.push(Diagnostic {
                span: annotation.span,
                message: "multiple @type annotations for one declaration".to_owned(),
            });
        } else {
            target.type_annotation = Some(annotation);
        }
    }
}

pub(crate) fn annotation_spans(group: &FunctionAnnotations) -> Vec<Span> {
    group
        .generics
        .iter()
        .map(|generic| generic.span)
        .chain(group.params.iter().map(|param| param.span))
        .chain(group.vararg.iter().map(|vararg| vararg.span))
        .chain(group.returns.iter().map(|returns| returns.span))
        .chain(
            group
                .type_annotation
                .iter()
                .map(|annotation| annotation.span),
        )
        .collect()
}

struct TypeParser {
    tokens: Vec<Token>,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
    depth: usize,
    // A comma outside a return pack delimits its surrounding record field or
    // function parameter. Parenthesized return packs remove that ambiguity.
    comma_ends_type: usize,
}

struct ParsedReturnPack {
    types: Vec<TypeExpr>,
    ellipsis_span: Option<Span>,
    span: Span,
}

enum ReturnUnit {
    Scalar(TypeExpr),
    Pack(ParsedReturnPack),
}

impl TypeParser {
    fn declaration_name(&mut self, context: &str) -> Option<Name> {
        match self.current().kind.clone() {
            TokenKind::Identifier(text) | TokenKind::Keyword(text) => {
                let span = self.current().span;
                self.pos += 1;
                Some(Name { text, span })
            }
            _ => {
                self.error(format!("expected a name in {context} annotation"));
                None
            }
        }
    }

    fn declaration_parameters(&mut self) -> Vec<GenericParameter> {
        if self.take("<").is_none() {
            return Vec::new();
        }
        let mut parameters = Vec::new();
        while !self.eof() && !self.symbol(">") {
            let Some(name) = self.declaration_name("generic declaration") else {
                break;
            };
            let constraint = if self.take(":").is_some() {
                self.comma_ends_type += 1;
                // generic_def and class extension use one `ty` in doc.bnf;
                // a union constraint must be explicitly parenthesized.
                let constraint = self.primary();
                self.comma_ends_type -= 1;
                Some(constraint)
            } else {
                None
            };
            let span = constraint.as_ref().map_or(name.span, |ty| name.span.join(ty.span));
            parameters.push(GenericParameter { name, constraint, span });
            if self.take(",").is_none() {
                break;
            }
            if self.symbol(">") {
                self.error("expected a generic parameter after ','".to_owned());
            }
        }
        if parameters.is_empty() {
            self.error("named generic declaration requires at least one parameter".to_owned());
        }
        self.expect(">");
        parameters
    }

    fn field_declaration(&mut self, span: Span) -> Option<FieldDeclaration> {
        let visibility = match &self.current().kind {
            TokenKind::Identifier(word) if matches!(word.as_str(), "public" | "protected" | "private") => {
                let kind = match word.as_str() {
                    "public" => FieldVisibility::Public,
                    "protected" => FieldVisibility::Protected,
                    "private" => FieldVisibility::Private,
                    _ => unreachable!(),
                };
                let span = self.current().span;
                self.pos += 1;
                Some(FieldVisibilityAnnotation { kind, span })
            }
            _ => None,
        };
        let explicit_owner = if self.take("<").is_some() {
            let owner = self.declaration_name("field owner");
            self.expect(">");
            owner
        } else { None };
        let key = if self.take("[").is_some() {
            let ty = self.union();
            self.expect("]");
            TypeFieldKey::Indexed(Box::new(ty))
        } else {
            TypeFieldKey::Named(self.declaration_name("field")?)
        };
        let ty = self.union();
        Some(FieldDeclaration { key, ty, visibility, explicit_owner, span })
    }

    fn documentation_tail(&mut self) {
        // doc.bnf permits comment_string after named declarations/fields.
        // Punctuation that would continue a malformed type remains an error.
        if matches!(self.current().kind, TokenKind::Identifier(_) | TokenKind::Keyword(_) | TokenKind::String(_)) {
            while !self.eof() { self.pos += 1; }
        }
    }

    fn current(&self) -> &Token {
        &self.tokens[self.pos]
    }
    fn eof(&self) -> bool {
        matches!(self.current().kind, TokenKind::Eof)
    }
    fn symbol(&self, value: &str) -> bool {
        matches!(&self.current().kind, TokenKind::Symbol(symbol) if symbol == value)
    }
    fn symbol_at(&self, pos: usize, value: &str) -> bool {
        self.tokens.get(pos).is_some_and(
            |token| matches!(&token.kind, TokenKind::Symbol(symbol) if symbol == value),
        )
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
    fn annotation_type_list(&mut self) -> (Vec<TypeExpr>, Option<Box<TypePacks>>) {
        // Unlike function returns, @type has only one type list in doc.bnf.
        // Its optional ellipsis repeats the final listed type.
        let start = self.current().span;
        let types = self.list();
        let packs = self.take("...").map(|ellipsis_span| {
            let type_index = types.len() - 1;
            let span = start.join(ellipsis_span);
            Box::new(TypePacks {
                alternatives: vec![TypePack {
                    fixed: 0..type_index,
                    variadic: Some(VariadicTypeIndex {
                        type_index,
                        ellipsis_span,
                    }),
                    span,
                }],
                span,
            })
        });
        (types, packs)
    }
    fn name(&mut self) -> Option<Name> {
        if let TokenKind::Identifier(text) = self.current().kind.clone() {
            let span = self.current().span;
            self.pos += 1;
            Some(Name { text, span })
        } else {
            self.error("expected a parameter name in function annotation".to_owned());
            None
        }
    }
    fn generic_parameters(&mut self) -> Vec<GenericParameter> {
        let mut parameters = Vec::new();
        loop {
            // Documentation's xGENERIC lexer accepts ID without reserving
            // Lua keywords, unlike the lexer shared with Lua expressions.
            let name = if let TokenKind::Identifier(text) | TokenKind::Keyword(text) =
                self.current().kind.clone()
            {
                let span = self.current().span;
                self.pos += 1;
                Name { text, span }
            } else {
                self.error("expected a type parameter name after @generic or ','".to_owned());
                break;
            };
            let constraint = if self.take(":").is_some() {
                // doc.bnf: generic_def ::= ID (EXTENDS ty)?. A constraint is
                // one `ty`, rather than `complex_ty`; unions must be wrapped
                // in parentheses. A function constraint's bare return also
                // cannot consume the comma between generic declarations.
                self.comma_ends_type += 1;
                let ty = self.primary();
                self.comma_ends_type -= 1;
                Some(ty)
            } else {
                None
            };
            let span = constraint
                .as_ref()
                .map_or(name.span, |ty| name.span.join(ty.span));
            parameters.push(GenericParameter {
                name,
                constraint,
                span,
            });
            if self.symbol("=") {
                self.error(
                    "generic parameter defaults are invalid Luanalysis annotation syntax"
                        .to_owned(),
                );
                break;
            }
            if self.take(",").is_none() {
                break;
            }
        }
        parameters
    }
    fn return_packs(&mut self) -> (Vec<TypeExpr>, Option<Box<TypePacks>>) {
        let mut types = Vec::new();
        let mut alternatives = Vec::new();
        let mut pending = None;
        loop {
            let (mut pack, next) = if let Some(pack) = pending.take() {
                (pack, None)
            } else {
                self.return_pack()
            };
            let first = types.len();
            types.append(&mut pack.types);
            let variadic = pack.ellipsis_span.map(|ellipsis_span| VariadicTypeIndex {
                type_index: types.len() - 1,
                ellipsis_span,
            });
            let fixed_end = variadic
                .as_ref()
                .map_or(types.len(), |tail| tail.type_index);
            alternatives.push(TypePack {
                fixed: first..fixed_end,
                variadic,
                span: pack.span,
            });
            if let Some(next) = next {
                pending = Some(next);
                continue;
            }
            if self.take("|").is_none() {
                break;
            }
        }
        let has_tail = alternatives.iter().any(|pack| pack.variadic.is_some());
        if alternatives.len() == 1 && !has_tail {
            return (types, None);
        }
        let span = alternatives
            .first()
            .unwrap()
            .span
            .join(alternatives.last().unwrap().span);
        (types, Some(Box::new(TypePacks { alternatives, span })))
    }
    fn return_pack(&mut self) -> (ParsedReturnPack, Option<ParsedReturnPack>) {
        let first = match self.return_unit() {
            ReturnUnit::Pack(pack) => return (pack, None),
            ReturnUnit::Scalar(ty) => ty,
        };
        let start = first.span;
        let (first, mut pending) = self.return_scalar_union(first);
        let mut types = vec![first];
        if pending.is_none() && self.comma_ends_type == 0 {
            while self.take(",").is_some() {
                let first = match self.return_unit() {
                    ReturnUnit::Scalar(ty) => ty,
                    ReturnUnit::Pack(pack) => {
                        self.diagnostics.push(Diagnostic {
                            span: pack.span,
                            message: "a return pack cannot appear as a scalar result slot"
                                .to_owned(),
                        });
                        TypeExpr {
                            kind: TypeExprKind::Error,
                            span: pack.span,
                        }
                    }
                };
                let (ty, next) = self.return_scalar_union(first);
                types.push(ty);
                pending = next;
                if pending.is_some() {
                    break;
                }
            }
        }
        let ellipsis_span = if pending.is_none() {
            self.take("...")
        } else {
            None
        };
        let end = ellipsis_span.unwrap_or_else(|| types.last().unwrap().span);
        (
            ParsedReturnPack {
                types,
                ellipsis_span,
                span: start.join(end),
            },
            pending,
        )
    }
    fn return_scalar_union(&mut self, first: TypeExpr) -> (TypeExpr, Option<ParsedReturnPack>) {
        let mut types = vec![first];
        let mut pending = None;
        while self.take("|").is_some() {
            match self.return_unit() {
                ReturnUnit::Scalar(ty) => types.push(ty),
                ReturnUnit::Pack(pack) => {
                    pending = Some(pack);
                    break;
                }
            }
        }
        let ty = if types.len() == 1 {
            types.pop().unwrap()
        } else {
            let span = types.first().unwrap().span.join(types.last().unwrap().span);
            TypeExpr {
                kind: TypeExprKind::Union(types),
                span,
            }
        };
        (ty, pending)
    }
    fn return_unit(&mut self) -> ReturnUnit {
        if !self.symbol("(") || self.depth >= 128 {
            return ReturnUnit::Scalar(self.primary());
        }
        let start = self.take("(").unwrap();
        let surrounding_comma_mode = self.comma_ends_type;
        // Parse the contents before deciding whether parentheses enclose a
        // scalar type or a result pack. Commas in a callback's own return list
        // must remain owned by that callback, even at this group's depth.
        self.comma_ends_type = 0;
        self.depth += 1;
        let mut types = self.list();
        let ellipsis_span = self.take("...");
        let end = self.expect(")");
        self.depth -= 1;
        self.comma_ends_type = surrounding_comma_mode;
        let span = start.join(end);
        if types.len() == 1 && ellipsis_span.is_none() {
            let ty = TypeExpr {
                kind: TypeExprKind::Parenthesized(Box::new(types.pop().unwrap())),
                span,
            };
            ReturnUnit::Scalar(self.array_suffix(ty))
        } else {
            ReturnUnit::Pack(ParsedReturnPack {
                types,
                ellipsis_span,
                span,
            })
        }
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
        let mut ty = match self.current().kind.clone() {
            TokenKind::Identifier(name) if name == "fun" => self.function_type(),
            TokenKind::Identifier(name) => {
                self.pos += 1;
                TypeExpr {
                    kind: TypeExprKind::Named(name),
                    span: start,
                }
            }
            TokenKind::Keyword(name) if name == "true" || name == "false" => {
                self.pos += 1;
                TypeExpr {
                    kind: TypeExprKind::Literal(TypeLiteral::Boolean(name == "true")),
                    span: start,
                }
            }
            TokenKind::Keyword(name) => {
                self.pos += 1;
                TypeExpr {
                    kind: TypeExprKind::Named(name),
                    span: start,
                }
            }
            TokenKind::Number(value) => {
                self.pos += 1;
                TypeExpr {
                    kind: TypeExprKind::Literal(TypeLiteral::Number(value)),
                    span: start,
                }
            }
            TokenKind::String(value) => {
                self.pos += 1;
                TypeExpr {
                    kind: TypeExprKind::Literal(TypeLiteral::String(value)),
                    span: start,
                }
            }
            TokenKind::Symbol(symbol) if symbol == "-" => {
                self.pos += 1;
                if let TokenKind::Number(value) = self.current().kind.clone() {
                    let end = self.current().span;
                    self.pos += 1;
                    TypeExpr {
                        kind: TypeExprKind::Literal(TypeLiteral::Number(format!("-{value}"))),
                        span: start.join(end),
                    }
                } else {
                    self.error("expected a numeric literal type after '-'".to_owned());
                    TypeExpr {
                        kind: TypeExprKind::Error,
                        span: start,
                    }
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
        if self.symbol("<") {
            if let TypeExprKind::Named(text) = &ty.kind {
                let base = Name { text: text.clone(), span: ty.span };
                let args_start = self.expect("<");
                let mut args = Vec::new();
                if self.symbol(">") {
                    self.error("generic type argument list must not be empty".to_owned());
                } else {
                    loop {
                        self.comma_ends_type += 1;
                        args.push(self.union());
                        self.comma_ends_type -= 1;
                        if self.take(",").is_none() { break; }
                        if self.symbol(">") {
                            self.error("generic type argument list cannot have a trailing comma".to_owned());
                            break;
                        }
                    }
                }
                let end = self.expect(">");
                let args_span = args_start.join(end);
                let span = base.span.join(end);
                if args.iter().map(type_depth).max().unwrap_or(0) >= 128 {
                    self.diagnostics.push(Diagnostic { span: args_span, message: "generic type nesting exceeds 128".to_owned() });
                    ty = TypeExpr { kind: TypeExprKind::Error, span };
                } else {
                    ty = TypeExpr { kind: TypeExprKind::Generic { base, args, args_span }, span };
                }
            }
        }
        ty = self.array_suffix(ty);
        self.depth -= 1;
        ty
    }
    fn array_suffix(&mut self, mut ty: TypeExpr) -> TypeExpr {
        let mut depth = type_depth(&ty);
        let mut exceeded = false;
        while self.symbol("[") && self.symbol_at(self.pos + 1, "]") {
            let start = self.expect("[");
            let end = self.expect("]");
            let suffix_span = start.join(end);
            let span = ty.span.join(end);
            if exceeded || depth >= 128 {
                if !exceeded {
                    self.diagnostics.push(Diagnostic { span: suffix_span, message: "array type nesting exceeds 128".to_owned() });
                }
                ty = TypeExpr { kind: TypeExprKind::Error, span };
                depth = 1;
                exceeded = true;
            } else {
                ty = TypeExpr { kind: TypeExprKind::Array { element: Box::new(ty), suffix_span }, span };
                depth += 1;
            }
        }
        ty
    }
    fn function_type(&mut self) -> TypeExpr {
        let start = self.current().span;
        self.pos += 1;
        let mut params = Vec::new();
        let mut vararg = None;
        if self.symbol("<") {
            self.error(
                "generic function types are unsupported until generic inference is implemented"
                    .to_owned(),
            );
            while !self.eof() && !self.symbol("(") {
                self.pos += 1;
            }
        }
        if self.take("(").is_some() {
            while !self.eof() && !self.symbol(")") {
                let param_start = self.current().span;
                let ellipsis = self.symbol("...");
                let word = matches!(&self.current().kind, TokenKind::Identifier(name) if name == "vararg")
                    && !self.symbol_at(self.pos + 1, ":")
                    && !self.symbol_at(self.pos + 1, "?");
                if ellipsis || word {
                    self.pos += 1;
                    if ellipsis {
                        self.expect(":");
                    }
                    self.comma_ends_type += 1;
                    let ty = self.union();
                    self.comma_ends_type -= 1;
                    let span = param_start.join(ty.span);
                    vararg = Some(Box::new(VarargAnnotation {
                        ty,
                        span,
                        ellipsis_span: ellipsis.then_some(param_start),
                    }));
                    if self.take(",").is_some() {
                        self.error("variadic function type parameter must be last".to_owned());
                        self.recover_function_params();
                    } else if !self.symbol(")") {
                        self.error(
                            "expected ')' after variadic function type parameter".to_owned(),
                        );
                        self.recover_function_params();
                    }
                    break;
                }
                let Some(name) = self.name() else {
                    self.error(
                        "unnamed function parameters are unsupported in this slice".to_owned(),
                    );
                    while !self.eof() && !self.symbol(",") && !self.symbol(")") {
                        self.pos += 1;
                    }
                    if self.take(",").is_some() {
                        continue;
                    }
                    break;
                };
                let optional = self.take("?").is_some();
                let ty = if self.take(":").is_some() {
                    self.comma_ends_type += 1;
                    let ty = self.union();
                    self.comma_ends_type -= 1;
                    Some(ty)
                } else {
                    None
                };
                let end = ty
                    .as_ref()
                    .map_or(self.tokens[self.pos - 1].span, |ty| ty.span);
                params.push(FunctionTypeParam {
                    name,
                    ty,
                    optional,
                    span: param_start.join(end),
                });
                if self.take(",").is_some() {
                    continue;
                }
                if !self.symbol(")") {
                    self.error("expected ',' or ')' after function type parameter".to_owned());
                    while !self.eof() && !self.symbol(")") {
                        self.pos += 1;
                    }
                }
                break;
            }
            self.expect(")");
        }
        let (returns, packs) = if self.take(":").is_some() {
            self.return_packs()
        } else {
            (Vec::new(), None)
        };
        let end = if self.pos > 0 {
            self.tokens[self.pos - 1].span
        } else {
            start
        };
        let span = start.join(end);
        TypeExpr {
            kind: TypeExprKind::Function(FunctionType {
                params,
                vararg,
                returns,
                packs,
                span,
            }),
            span,
        }
    }
    fn recover_function_params(&mut self) {
        let mut nesting = 0;
        while !self.eof() {
            if self.symbol(")") {
                if nesting == 0 {
                    break;
                }
                nesting -= 1;
            } else if self.symbol("(") {
                nesting += 1;
            }
            self.pos += 1;
        }
    }
    fn record(&mut self) -> TypeExpr {
        let start = self.expect("{");
        let mut fields = Vec::new();
        let mut sequence_index = 0;
        while !self.eof() && !self.symbol("}") {
            let field_start = self.current().span;
            let key = if self.take("[").is_some() {
                self.comma_ends_type += 1;
                let mut key = self.union();
                self.comma_ends_type -= 1;
                if type_depth(&key) >= 128 {
                    self.diagnostics.push(Diagnostic { span: key.span, message: "record key type nesting exceeds 128".to_owned() });
                    key = TypeExpr { kind: TypeExprKind::Error, span: key.span };
                }
                self.expect("]");
                self.expect(":");
                TypeFieldKey::Indexed(Box::new(key))
            } else if let TokenKind::Identifier(text) | TokenKind::Keyword(text) = self.current().kind.clone() {
                if self.symbol_at(self.pos + 1, ":") {
                    self.pos += 1;
                    self.expect(":");
                    TypeFieldKey::Named(Name { text, span: field_start })
                } else {
                    sequence_index += 1;
                    TypeFieldKey::Sequence { index: sequence_index }
                }
            } else {
                sequence_index += 1;
                TypeFieldKey::Sequence { index: sequence_index }
            };
            self.comma_ends_type += 1;
            let mut ty = self.union();
            self.comma_ends_type -= 1;
            if type_depth(&ty) >= 128 {
                self.diagnostics.push(Diagnostic { span: ty.span, message: "record value type nesting exceeds 128".to_owned() });
                ty = TypeExpr { kind: TypeExprKind::Error, span: ty.span };
            }
            let span = field_start.join(ty.span);
            fields.push(TypeField { key, ty, span });
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
