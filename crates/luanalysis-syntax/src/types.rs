use crate::{
    Comment, Diagnostic, FieldDeclaration, FieldVisibility, FieldVisibilityAnnotation,
    FunctionAnnotations, FunctionType, FunctionTypeParam, GenericParameter, MappedModifier, MappedType, Name, NamedRecordKind,
    NamedTypeDeclarationKind, ParamAnnotation, Span, Token, TokenKind, TypeAnnotation, TypeExpr,
    TypeExprKind, TypeField, TypeFieldKey, TypeLiteral, TypePack, TypePacks, VarargAnnotation,
    VariadicTypeIndex,
};

#[derive(Default)]
pub(crate) struct ParsedDocumentation {
    pub functions: FunctionAnnotations,
    pub declarations: Vec<DeclarationItem>,
    pub exclusions: Vec<TypeAnnotation>,
    /// LuaLS `--[[@as T]]`: the type of the expression just before the comment.
    pub as_casts: Vec<TypeAnnotation>,
    /// LuaLS `---@cast name T`, `---@cast name -?` and `---@cast name -T`.
    pub casts: Vec<CastAnnotation>,
    /// The prose lines before the first tag.
    pub prose: Vec<String>,
    /// Each tag's description: the text after its complete type.
    pub descriptions: Vec<TagDescription>,
    /// `---@deprecated`, with what it says (the replacement to use), on its line or those after.
    pub deprecated: Option<String>,
}

/// What a tag says beside its type: `---@param path string the file` describes `path`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TagDescription {
    pub tag: String,
    /// The parameter, field or type the tag names.
    pub subject: Option<String>,
    pub text: String,
}

/// LuaLS `---@cast name T` (`kind` Type) or `---@cast name -T` (`kind` Not, `-?` removing
/// nil): from here on, the variable's value is so typed.
pub(crate) struct CastAnnotation {
    pub name: Name,
    pub kind: crate::ExpressionAssertionKind,
    pub annotation: TypeAnnotation,
    pub tag_span: Span,
}

pub(crate) enum DeclarationItem {
    Reference(crate::DocumentationReference),
    Named {
        name: Name,
        generics: Vec<GenericParameter>,
        kind: NamedTypeDeclarationKind,
        span: Span,
        /// LuaLS's `---@enum`, `Some(true)` for `(key)`: the alias is derived from the table
        /// that the statement it is attached to assigns.
        enumeration: Option<bool>,
    },
    Field(FieldDeclaration),
    Function {
        name: Name,
        explicit_owner: Option<Name>,
        method_colon: Option<Span>,
        signature: Box<FunctionType>,
        annotations: FunctionAnnotations,
        span: Span,
        tag_span: Span,
    },
    Overload {
        signature: Box<FunctionType>,
        span: Span,
        tag_span: Span,
    },
    Operator {
        operator: crate::LuaOperator,
        signature: Box<FunctionType>,
        span: Span,
        tag_span: Span,
    },
}

/// Parse only documentation tags. Ordinary prose remains a retained comment.
pub(crate) fn annotations(comment: &Comment, options: crate::ParseOptions) -> (ParsedDocumentation, Vec<Diagnostic>) {
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
    let mut active_function = None;
    let mut seen_tag = false;
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    for (index, line) in lines.iter().copied().enumerate() {
        if !seen_tag && !is_tag_line(line) {
            let prose = line.trim_start_matches([' ', '\t', '\r']).trim_start_matches('-').trim();
            if !prose.is_empty() || !result.prose.is_empty() { result.prose.push(prose.to_owned()); }
        }
        let trimmed = line.trim_start_matches([' ', '\t', '\r']);
        let dashes = trimmed.len() - trimmed.trim_start_matches('-').len();
        let stripped = trimmed
            .trim_start_matches('-')
            .trim_start_matches([' ', '\t']);
        // More dashes than `---` comment an annotation out (`-----@type T`), as in LuaLS.
        if let Some(tag) = stripped.strip_prefix('@').filter(|_| dashes <= 3) {
            seen_tag = true;
            let tag_offset = base + line_offset + line.len() - stripped.len();
            let tag_len = tag
                .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                .unwrap_or(tag.len());
            let tag_name = &tag[..tag_len];
            // LuaLS annotations that state nothing this checker uses: editor and diagnostic
            // directives, visibility, and operators and casts not yet modelled. Deprecation is
            // documentation: the editor marks what it deprecates.
            if tag_name == "deprecated" {
                let mut text = tag[tag_len..].to_owned();
                for continuation in lines[index + 1..].iter().take_while(|line| !is_tag_line(line)) {
                    text.push_str(&continuation_text(continuation));
                }
                result.deprecated = Some(description_text(&text));
            }
            if matches!(
                tag_name,
                "async" | "nodiscard" | "deprecated" | "package" | "private" | "protected"
                    | "public" | "meta" | "diagnostic" | "format" | "version" | "source"
            ) {
                line_offset += line.len();
                continue;
            }
            if !matches!(
                tag_name,
                "type"
                    | "not"
                    | "param"
                    | "return"
                    | "return_overload"
                    | "readonly"
                    | "generic"
                    | "vararg"
                    | "alias"
                    | "shape"
                    | "class"
                    | "module"
                    | "field"
                    | "function"
                    | "overload"
                    | "operator"
                    | "see"
                    | "enum"
                    | "as"
                    | "cast"
            ) || (tag_name == "return_overload" && !options.emmylua)
            {
                diagnostics.push(Diagnostic {
                    origin: crate::DiagnosticOrigin::Annotation,
                    span: Span::new(tag_offset, tag_offset + 1 + tag_len),
                    message: format!("unsupported Luanalysis annotation @{tag_name}"),
                });
            } else {
                let raw_types = &tag[tag_len..];
                // The `---` lines up to the next tag continue this one: its text spans them,
                // with their comment prefixes blanked so that every offset is the source's.
                let mut joined = raw_types.trim_start_matches([' ', '\t']).to_owned();
                for continuation in lines[index + 1..].iter().take_while(|line| !is_tag_line(line)) {
                    joined.push_str(&continuation_text(continuation));
                }
                let types = joined.trim_end_matches(['\r', '\n']);
                // `@` begins trailing documentation in doc.flex's type state,
                // but an @ inside a quoted literal belongs to that literal.
                let types = annotation_type_text(types);
                let types_offset = tag_offset + 1 + tag_len + raw_types.len()
                    - raw_types.trim_start_matches([' ', '\t']).len();
                let lexed = crate::lexer::lex_annotation(types);
                let line_breaks = lexed.tokens.iter()
                    .filter(|token| matches!(token.kind, TokenKind::Newline))
                    .map(|token| token.span.start + types_offset)
                    .collect();
                let tokens = lexed
                    .tokens
                    .into_iter()
                    // Lines are whitespace within a tag's text.
                    .filter(|token| !matches!(token.kind, TokenKind::Newline))
                    .flat_map(|mut token| {
                        token.span.start += types_offset;
                        token.span.end += types_offset;
                        // Annotation generic closers are individual source `>`
                        // tokens, while the shared Lua lexer recognizes `>>`.
                        if matches!(&token.kind, TokenKind::Symbol(symbol) if symbol == ">>") {
                            let middle = token.span.start + 1;
                            vec![
                                Token {
                                    kind: TokenKind::Symbol(">".to_owned()),
                                    span: Span::new(token.span.start, middle),
                                },
                                Token {
                                    kind: TokenKind::Symbol(">".to_owned()),
                                    span: Span::new(middle, token.span.end),
                                },
                            ]
                        } else {
                            vec![token]
                        }
                    })
                    .collect();
                let lexer_diagnostics: Vec<Diagnostic> = lexed.diagnostics.into_iter().map(|mut diagnostic| {
                    diagnostic.span.start += types_offset;
                    diagnostic.span.end += types_offset;
                    diagnostic
                }).collect();
                let mut parser = TypeParser {
                    tokens,
                    line_breaks,
                    pos: 0,
                    diagnostics: Vec::new(),
                    depth: 0,
                    comma_ends_type: 0,
                    function_parameter_comma_depth: None,
                    in_conditional_extends: false,
                    emmylua: options.emmylua,
                    decay_optionals: options.decay_optionals,
                    optional_suffixes: Vec::new(),
                };
                let annotation_span = Span::new(tag_offset, types_offset + types.len());
                let mut group = FunctionAnnotations::default();
                // The parameter, field or type the tag names, and where the text that
                // `documentation_tail` consumes as a description starts.
                let mut subject: Option<String> = None;
                let mut tail_start: Option<usize> = None;
                match tag_name {
                    "see" => {
                        if let Some(target) = parser.declaration_name("@see target") {
                            let member = if parser.take("#").is_some() {
                                parser.declaration_name("@see member")
                            } else {
                                None
                            };
                            result.declarations.push(DeclarationItem::Reference(
                                crate::DocumentationReference {
                                    target,
                                    member,
                                    span: annotation_span,
                                },
                            ));
                        }
                    }
                    "overload" => {
                        let ty = parser.primary();
                        if let TypeExprKind::Function(signature) = ty.kind {
                            result.declarations.push(DeclarationItem::Overload {
                                signature: Box::new(signature),
                                span: annotation_span,
                                tag_span: Span::new(tag_offset, tag_offset + 1 + tag_len),
                            });
                        } else {
                            parser.diagnostics.push(Diagnostic {
                                origin: crate::DiagnosticOrigin::Annotation,
                                span: ty.span,
                                message: "@overload requires a function type".into(),
                            });
                        }
                    }
                    "alias" | "shape" | "class" | "module" => {
                        active_function = None;
                        // LuaLS's `---@class (exact) Name` and `(partial)` modifiers.
                        if tag_name == "class" && parser.take("(").is_some() {
                            parser.declaration_name("@class modifier");
                            parser.expect(")");
                        }
                        if let Some(name) = parser.declaration_name("named type") {
                            let generics = parser.declaration_parameters();
                            let kind = if tag_name == "alias" {
                                // An alias without a type lists its members on the `---|` lines
                                // that follow it (LuaLS).
                                let target = if parser.eof() {
                                    alias_continuation(&lines[index + 1..], base + line_offset + line.len(), annotation_span, &mut diagnostics, options)
                                } else {
                                    parser.union()
                                };
                                NamedTypeDeclarationKind::Alias { target }
                            } else {
                                let base = parser.take(":").map(|_| parser.union());
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
                            subject = Some(name.text.clone());
                            result.declarations.push(DeclarationItem::Named {
                                name,
                                generics,
                                kind,
                                span: annotation_span,
                                enumeration: None,
                            });
                            tail_start = parser.documentation_tail();
                        }
                    }
                    "enum" => {
                        active_function = None;
                        // LuaLS's `---@enum (key) Name` names the values (or keys) of the table
                        // after it: `typeof X[keyof typeof X]` (or `keyof typeof X`) once it is
                        // attached to the statement that assigns `X`. Without one, the name is an
                        // alias of any value (of string for keys).
                        let mut keys = false;
                        if parser.take("(").is_some() {
                            keys = parser.declaration_name("@enum modifier").is_some_and(|modifier| modifier.text == "key");
                            parser.expect(")");
                        }
                        if let Some(name) = parser.declaration_name("@enum") {
                            let target = TypeExpr { kind: TypeExprKind::Named(if keys { "string" } else { "any" }.to_owned()), span: name.span };
                            subject = Some(name.text.clone());
                            result.declarations.push(DeclarationItem::Named {
                                name,
                                generics: Vec::new(),
                                kind: NamedTypeDeclarationKind::Alias { target },
                                span: annotation_span,
                                enumeration: Some(keys),
                            });
                            tail_start = parser.documentation_tail();
                        }
                    }
                    "operator" => {
                        active_function = None;
                        let tag_span = Span::new(tag_offset, tag_offset + 1 + tag_len);
                        if let Some(item) = parser.operator_declaration(annotation_span, tag_span) {
                            result.declarations.push(item);
                        }
                    }
                    "field" => {
                        active_function = None;
                        if let Some(field) = parser.field_declaration(annotation_span) {
                            if let TypeFieldKey::Named(name) = &field.key { subject = Some(name.text.clone()); }
                            result.declarations.push(DeclarationItem::Field(field));
                        }
                    }
                    "function" => {
                        if let Some((name, explicit_owner, method_colon, signature)) =
                            parser.function_declaration()
                        {
                            subject = Some(name.text.clone());
                            active_function = Some(result.declarations.len());
                            result.declarations.push(DeclarationItem::Function {
                                name,
                                explicit_owner,
                                method_colon,
                                signature: Box::new(signature),
                                annotations: std::mem::take(&mut result.functions),
                                span: annotation_span,
                                tag_span: Span::new(tag_offset, tag_offset + 1 + tag_len),
                            });
                        }
                    }
                    "generic" => {
                        group.generics = parser.generic_parameters();
                    }
                    "param" => {
                        // `---@param ... T` (LuaLS) types the variadic parameters, as @vararg does.
                        if let Some(ellipsis_span) = parser.take("...") {
                            subject = Some("...".to_owned());
                            // LuaLS writes a type after `...`; a `#` begins a description instead.
                            let ty = if parser.eof() || parser.symbol("#") {
                                TypeExpr { kind: TypeExprKind::Named("any".to_owned()), span: ellipsis_span }
                            } else {
                                parser.parameter_type().0
                            };
                            group.vararg = Some(Box::new(VarargAnnotation {
                                ty,
                                span: annotation_span,
                                ellipsis_span: None,
                            }));
                        } else if let Some(name) = parser.name() {
                            subject = Some(name.text.clone());
                            let optional = parser.take("?").is_some();
                            let (ty, absent) = parser.parameter_type();
                            group.params.push(ParamAnnotation {
                                name,
                                ty,
                                optional: optional || absent,
                                span: annotation_span,
                            });
                        }
                    }
                    "vararg" => {
                        group.vararg = Some(Box::new(VarargAnnotation {
                            ty: parser.parameter_type().0,
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
                    "readonly" => group.readonly = Some(Span::new(tag_offset, tag_offset + 1 + tag_len)),
                    "return_overload" => {
                        let (types, packs) = parser.return_packs();
                        group.return_overloads.push(TypeAnnotation { span: annotation_span, types, packs });
                    }
                    "type" => {
                        let (types, packs) = parser.annotation_type_list();
                        group.type_annotation = Some(TypeAnnotation {
                            span: annotation_span,
                            types,
                            packs,
                        });
                    }
                    "not" => {
                        let (types, packs) = parser.annotation_type_list();
                        result.exclusions.push(TypeAnnotation {
                            span: annotation_span,
                            types,
                            packs,
                        });
                    }
                    "as" => {
                        let (types, packs) = parser.annotation_type_list();
                        result.as_casts.push(TypeAnnotation { span: annotation_span, types, packs });
                    }
                    "cast" => {
                        // `---@cast x -A, -B, +C`: the removed types are excluded together; `+T`
                        // and `+?` widen, which needs the variable's type, so they cast nothing.
                        let tag_span = Span::new(tag_offset, tag_offset + 1 + tag_len);
                        if let Some(name) = parser.name() {
                            let mut removed = Vec::new();
                            let mut replaced = Vec::new();
                            loop {
                                if parser.take("-").is_some() {
                                    removed.push(match parser.take("?") {
                                        Some(span) => TypeExpr { kind: TypeExprKind::Named("nil".to_owned()), span },
                                        None => parser.union(),
                                    });
                                } else if parser.take("+").is_some() {
                                    if parser.take("?").is_none() { parser.union(); }
                                } else if parser.eof() || matches!(&parser.current().kind, TokenKind::Symbol(symbol) if symbol == "#") {
                                    break;
                                } else {
                                    replaced.push(parser.union());
                                }
                                if parser.take(",").is_none() { break; }
                            }
                            let union = |mut types: Vec<TypeExpr>| if types.len() == 1 { types.remove(0) } else {
                                let span = types.iter().map(|ty| ty.span).reduce(Span::join).unwrap_or(name.span);
                                TypeExpr { kind: TypeExprKind::Union(types), span }
                            };
                            let cast = if !replaced.is_empty() {
                                Some((crate::ExpressionAssertionKind::Type, union(replaced)))
                            } else if !removed.is_empty() {
                                Some((crate::ExpressionAssertionKind::Not, union(removed)))
                            } else { None };
                            if let Some((kind, ty)) = cast {
                                result.casts.push(CastAnnotation {
                                    name,
                                    kind,
                                    annotation: TypeAnnotation { span: annotation_span, types: vec![ty], packs: None },
                                    tag_span,
                                });
                            }
                        }
                    }
                    _ => unreachable!(),
                }
                // Text after a complete type is its description, as LuaLS and EmmyLua read it:
                // `---@param path string # the file`, `---@return integer count`.
                // In v1 a `)` after a complete type also ends it, as its lexer leaves the type
                // there (`---@param options A | B): Result`). A Lua comment, `-- text`, does too.
                // Text on a later line than a complete type is its description too.
                let on_later_line = |parser: &TypeParser| parser.pos > 0 && {
                    let previous = parser.tokens[parser.pos - 1].span.end - types_offset;
                    let current = parser.current().span.start - types_offset;
                    types.get(previous..current).is_some_and(|between| between.contains('\n'))
                };
                let description = (!parser.eof()).then(|| parser.description_start().or_else(|| {
                    let lua_comment = matches!(parser.current().kind, TokenKind::Comment(_));
                    (parser.symbol(")") || lua_comment || on_later_line(&parser)).then_some(parser.current().span.start)
                })).flatten();
                if !parser.eof() && description.is_none() {
                    parser.error(format!(
                        "unsupported or unexpected syntax after @{tag_name}"
                    ));
                }
                if let Some(start) = tail_start.or(description)
                    && let Some(text) = types.get(start - types_offset..).map(description_text).filter(|text| !text.is_empty())
                {
                    result.descriptions.push(TagDescription { tag: tag_name.to_owned(), subject, text });
                }
                diagnostics.extend(lexer_diagnostics.into_iter().filter(|diagnostic| description.is_none_or(|start| diagnostic.span.start < start)));
                diagnostics.extend(parser.diagnostics);
                if !options.emmylua {
                    diagnostics.extend(parser.optional_suffixes.iter().map(|&span| optional_suffix_diagnostic(span)));
                }
                if let Some(DeclarationItem::Function { annotations, .. }) =
                    active_function.map(|index| &mut result.declarations[index])
                {
                    merge_annotations(annotations, group, true, &mut diagnostics);
                } else {
                    merge_annotations(&mut result.functions, group, true, &mut diagnostics);
                }
            }
        }
        line_offset += line.len();
    }
    crate::completion_metadata::validate(&result, &mut diagnostics);
    (result, diagnostics)
}

/// EmmyLua Analyzer's `---@return_overload` rows are the function's result contract: a call
/// returns one of them, as `---@return (true, T) | (false, E)` writes it.
fn folded_return_overloads(rows: &[TypeAnnotation]) -> Option<TypeAnnotation> {
    let span = rows.iter().map(|row| row.span).reduce(Span::join)?;
    let mut types = Vec::new();
    let mut alternatives = Vec::new();
    for row in rows.iter().cloned() {
        let offset = types.len();
        match row.packs {
            Some(packs) => alternatives.extend(packs.alternatives.into_iter().map(|mut pack| {
                pack.fixed = pack.fixed.start + offset..pack.fixed.end + offset;
                if let Some(variadic) = pack.variadic.as_mut() { variadic.type_index += offset; }
                pack
            })),
            None => alternatives.push(TypePack { fixed: offset..offset + row.types.len(), variadic: None, span: row.span }),
        }
        types.extend(row.types);
    }
    Some(TypeAnnotation { types, packs: Some(Box::new(TypePacks { alternatives, span })), span })
}

/// A description as written over one or more lines: each line trimmed, without the `#` that
/// may introduce it (`---@param path string # the file`).
fn description_text(text: &str) -> String {
    let text = text.trim_start();
    let text = text.strip_prefix('#').unwrap_or(text);
    text.lines().map(str::trim).collect::<Vec<_>>().join("\n").trim().to_owned()
}

/// Whether a line of documentation begins a tag (`---@name`, or `@name` in a block). A line with
/// more dashes than `---` comments one out.
fn is_tag_line(line: &str) -> bool {
    let trimmed = line.trim_start_matches([' ', '\t', '\r']);
    let dashes = trimmed.len() - trimmed.trim_start_matches('-').len();
    dashes > 3 || trimmed.trim_start_matches('-').trim_start_matches([' ', '\t']).starts_with('@')
}

/// A continuation line as part of a tag's text: its comment prefix blanked, and the description
/// of a `---| member # description` line too, so that its offsets are the source's.
fn continuation_text(line: &str) -> String {
    let trimmed = line.trim_start_matches([' ', '\t', '\r']);
    let content = trimmed.trim_start_matches('-');
    let prefix = line.len() - content.len();
    let mut text = " ".repeat(prefix);
    let body = content.trim_end_matches(['\r', '\n']);
    let end = if body.trim_start().starts_with('|') {
        // `#` outside a quoted literal begins the member's description.
        let mut quote = None;
        body.char_indices().find(|&(_, ch)| match quote {
            Some(open) => { if ch == open { quote = None; } false }
            None if ch == '"' || ch == '\'' => { quote = Some(ch); false }
            None => ch == '#',
        }).map_or(body.len(), |(index, _)| index)
    } else {
        body.len()
    };
    text.push_str(&body[..end]);
    text.push_str(&" ".repeat(body.len() - end));
    text.push_str(&content[body.len()..]);
    text
}

fn type_depth(root: &TypeExpr) -> usize {
    let mut pending = vec![(root, 1_usize)];
    let mut depth = 0;
    while let Some((ty, current)) = pending.pop() {
        depth = depth.max(current);
        match &ty.kind {
            TypeExprKind::Parenthesized(element) | TypeExprKind::Array { element, .. } | TypeExprKind::TypePredicate { ty: Some(element), .. }
            | TypeExprKind::KeyOf(element) | TypeExprKind::Infer { constraint: Some(element), .. } => {
                pending.push((element, current + 1))
            }
            TypeExprKind::IndexedAccess { object, index } => pending.extend([(object.as_ref(), current + 1), (index.as_ref(), current + 1)]),
            TypeExprKind::Conditional { check, extends, yes, no } => {
                pending.extend([check, extends, yes, no].map(|ty| (ty.as_ref(), current + 1)));
            }
            TypeExprKind::Mapped(mapped) => {
                pending.extend([&mapped.constraint].into_iter().chain(&mapped.name).chain([&mapped.ty]).map(|ty| (ty, current + 1)));
            }
            TypeExprKind::Union(types) | TypeExprKind::Intersection(types) | TypeExprKind::Generic { args: types, .. }
            | TypeExprKind::TemplateLiteral { types, .. } => {
                pending.extend(types.iter().map(|ty| (ty, current + 1)));
            }
            TypeExprKind::Record(fields) => {
                for field in fields {
                    if let TypeFieldKey::Indexed(key) = &field.key {
                        pending.push((key, current + 1));
                    }
                    pending.push((&field.ty, current + 1));
                }
            }
            TypeExprKind::Function(function) => {
                pending.extend(function.generics.iter().flat_map(|parameter| parameter.constraint.iter().chain(&parameter.default)).map(|ty| (ty, current + 1)));
                pending.extend(
                    function
                        .params
                        .iter()
                        .filter_map(|param| param.ty.as_ref())
                        .map(|ty| (ty, current + 1)),
                );
                if let Some(vararg) = &function.vararg {
                    pending.push((&vararg.ty, current + 1));
                }
                pending.extend(function.returns.iter().map(|ty| (ty, current + 1)));
            }
            TypeExprKind::Named(_) | TypeExprKind::Literal(_) | TypeExprKind::CompletionHint(_) | TypeExprKind::TypePredicate { ty: None, .. }
            | TypeExprKind::TypeQuery(_) | TypeExprKind::Infer { constraint: None, .. } | TypeExprKind::Error => {}
        }
    }
    depth
}

fn annotation_type_text(text: &str) -> &str {
    if !text.contains('@') {
        return text.trim_end();
    }
    let end = crate::lexer::lex_annotation(text)
        .tokens
        .iter()
        .find_map(|token| {
            matches!(&token.kind, TokenKind::Symbol(symbol) if symbol == "@")
                .then_some(token.span.start)
        })
        .unwrap_or(text.len());
    text[..end].trim_end()
}

/// `contiguous` documentation is one group, where LuaLS writes one @return line per
/// result; @return lines in separate groups still conflict.
pub(crate) fn merge_annotations(
    target: &mut FunctionAnnotations,
    incoming: FunctionAnnotations,
    contiguous: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    target.overloads.extend(incoming.overloads);
    target.return_overloads.extend(incoming.return_overloads);
    if incoming.readonly.is_some() { target.readonly = incoming.readonly; }
    for generic in incoming.generics {
        if target
            .generics
            .iter()
            .any(|existing| existing.name.text == generic.name.text)
        {
            diagnostics.push(Diagnostic {
                origin: crate::DiagnosticOrigin::Annotation,
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
                origin: crate::DiagnosticOrigin::Annotation,
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
                origin: crate::DiagnosticOrigin::Annotation,
                span: vararg.span,
                message: "multiple @vararg annotations for one function".to_owned(),
            });
        } else {
            target.vararg = Some(vararg);
        }
    }
    // `---@return_overload` rows describe what a `---@return` beside them says more generally,
    // so once there are rows they are the results, whatever the order of the lines.
    let returns = incoming.returns.filter(|_| target.return_overloads.is_empty());
    if let Some(returns) = folded_return_overloads(&target.return_overloads) { target.returns = Some(returns); }
    if let Some(returns) = returns {
        // LuaLS writes one @return line per value: they list the function's results in order.
        if let Some(existing) = &target.returns
            && existing.types.iter().chain(&returns.types).any(|ty| matches!(ty.kind, TypeExprKind::TypePredicate { .. }))
        {
            diagnostics.push(Diagnostic {
                origin: crate::DiagnosticOrigin::Annotation,
                span: returns.span,
                message: "a type predicate must be the sole function return type".to_owned(),
            });
        } else if contiguous
            && let Some(existing) = target.returns.as_mut()
            && existing.packs.is_none()
            && returns.packs.as_ref().is_none_or(|packs| packs.alternatives.len() == 1)
        {
            let offset = existing.types.len();
            existing.types.extend(returns.types);
            existing.span = existing.span.join(returns.span);
            // A variadic final line, `---@return ...`, continues the results before it.
            if let Some(mut packs) = returns.packs {
                let pack = &mut packs.alternatives[0];
                pack.fixed = 0..pack.fixed.end + offset;
                if let Some(variadic) = pack.variadic.as_mut() {
                    variadic.type_index += offset;
                }
                pack.span = existing.span;
                packs.span = existing.span;
                existing.packs = Some(packs);
            }
        } else if target.returns.is_some() {
            diagnostics.push(Diagnostic {
                origin: crate::DiagnosticOrigin::Annotation,
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
                origin: crate::DiagnosticOrigin::Annotation,
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
        .chain(group.overloads.iter().map(|overload| overload.span))
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
    /// Where the tag's continuation lines begin, in order: its tokens omit them.
    line_breaks: Vec<usize>,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
    depth: usize,
    // A comma outside a return pack delimits its surrounding record field or
    // function parameter. Parenthesized return packs remove that ambiguity.
    comma_ends_type: usize,
    // At this depth, a nested callback may retain v1's bare result list.
    // Nested records, generic arguments and constraints still own their commas.
    function_parameter_comma_depth: Option<usize>,
    /// Directly in a conditional type's extends type, where `infer P extends C` constrains `P`.
    in_conditional_extends: bool,
    /// EmmyLua Analyzer's `A - B`, `sync fun` and parameters' `T?` ([`crate::ParseOptions::emmylua`]).
    emmylua: bool,
    /// `T?` is `nil | T` everywhere ([`crate::ParseOptions::decay_optionals`]).
    decay_optionals: bool,
    /// The `?` of each `T?` that no position written `T?` has claimed: without EmmyLua Analyzer's
    /// syntax, each is reported where its tag ends.
    optional_suffixes: Vec<Span>,
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
                let TypeExpr { kind: TypeExprKind::Named(text), span } = self.qualified(text, span) else {
                    unreachable!("a qualified name is a named type")
                };
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
            let constraint = if self.take_constraint_keyword() {
                self.comma_ends_type += 1;
                // generic_def and class extension use one `ty` in doc.bnf;
                // a union constraint must be explicitly parenthesized.
                let constraint = self.primary();
                self.comma_ends_type -= 1;
                Some(constraint)
            } else {
                None
            };
            let default = self.generic_default();
            let span = default.as_ref().or(constraint.as_ref())
                .map_or(name.span, |ty| name.span.join(ty.span));
            parameters.push(GenericParameter {
                name,
                constraint,
                default,
                span,
            });
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
            TokenKind::Identifier(word)
                if matches!(word.as_str(), "public" | "protected" | "private" | "package") =>
            {
                let kind = match word.as_str() {
                    "public" => FieldVisibility::Public,
                    "protected" => FieldVisibility::Protected,
                    "private" => FieldVisibility::Private,
                    "package" => FieldVisibility::Package,
                    _ => unreachable!(),
                };
                let span = self.current().span;
                self.pos += 1;
                Some(FieldVisibilityAnnotation { kind, span })
            }
            _ => None,
        };
        let readonly = self.readonly_field_modifier();
        let mut deprecated_owner_span = None;
        let mut explicit_owner = if let Some(open) = self.take("<") {
            let owner = self.declaration_name("field owner");
            let close = self.expect(">");
            deprecated_owner_span = Some(open.join(close));
            owner
        } else {
            None
        };
        let key = if self.take("[").is_some() {
            let ty = self.union();
            self.expect("]");
            TypeFieldKey::Indexed(Box::new(ty))
        } else {
            let (owner, member) = split_owner(self.declaration_name("field")?);
            if let Some(owner) = owner {
                if explicit_owner.is_some() {
                    self.diagnostics.push(Diagnostic {
                        origin: crate::DiagnosticOrigin::Annotation,
                        span: member.span,
                        message: "@field cannot combine qualified and angle-bracket owners".into(),
                    });
                }
                explicit_owner = Some(owner);
            }
            TypeFieldKey::Named(member)
        };
        // `---@field name? T` (LuaLS): the field may be nil. EmmyLua Analyzer writes it
        // `---@field name T?`.
        let optional = self.take("?");
        let mut ty = self.union();
        if self.emmylua { self.claim_optional(&ty); }
        let optional = optional.or_else(|| self.absent_type(&mut ty));
        if let Some(question) = optional {
            // Its own span: the type's is the key of an array, table or record head it may be.
            let span = question.join(ty.span);
            ty = TypeExpr {
                kind: TypeExprKind::Union(vec![ty, TypeExpr { kind: TypeExprKind::Named("nil".to_owned()), span: question }]),
                span,
            };
        }
        Some(FieldDeclaration {
            key,
            ty,
            visibility,
            readonly,
            optional: optional.is_some(),
            explicit_owner,
            deprecated_owner_span,
            span,
        })
    }

    /// `readonly` modifies a field only when a key and a complete type follow it, so that
    /// `---@field readonly readonly boolean` is a read-only field named `readonly`, while
    /// `---@field readonly boolean` and `---@field readonly string[]` still name a field
    /// `readonly`. Nothing is consumed or reported unless it is the modifier.
    fn readonly_field_modifier(&mut self) -> Option<Span> {
        if !self.word_at(self.pos, "readonly") { return None; }
        let (pos, diagnostics) = (self.pos, self.diagnostics.len());
        let span = self.current().span;
        self.pos += 1;
        let key = if self.take("[").is_some() {
            self.union();
            self.take("]").is_some()
        } else {
            // `readonly string[]`, `readonly string|nil` and `readonly table<K, V>` continue a type.
            self.declaration_name("field").is_some() && !(self.symbol("[") && self.symbol_at(self.pos + 1, "]"))
                && !["|", "&", "<"].into_iter().any(|operator| self.symbol(operator))
        };
        self.take("?");
        // A field's description may continue on the next line: its type is on the key's.
        let same_line = !self.eof() && {
            let (key_end, type_start) = (self.tokens[self.pos - 1].span.end, self.current().span.start);
            !self.line_breaks.iter().any(|&line_break| (key_end..type_start).contains(&line_break))
        };
        let typed = key && same_line && { let start = self.pos; self.union(); self.pos > start };
        let modifier = typed && self.diagnostics.len() == diagnostics;
        self.pos = pos;
        self.diagnostics.truncate(diagnostics);
        modifier.then(|| { self.pos += 1; span })
    }

    /// `---@operator name[(Other)][:] Result`, as proposed in LuaLS issue 599 and
    /// implemented by LuaLS. A binary operator without an operand accepts any
    /// right operand; `call` declares a call signature, as a class `@overload`.
    fn operator_declaration(&mut self, span: Span, tag_span: Span) -> Option<DeclarationItem> {
        let name = self.declaration_name("operator")?;
        let operator = match name.text.as_str() {
            "add" => Some(crate::LuaOperator::Add),
            "sub" => Some(crate::LuaOperator::Subtract),
            "mul" => Some(crate::LuaOperator::Multiply),
            "div" => Some(crate::LuaOperator::Divide),
            "idiv" => Some(crate::LuaOperator::FloorDivide),
            "mod" => Some(crate::LuaOperator::Modulo),
            "pow" => Some(crate::LuaOperator::Power),
            "concat" => Some(crate::LuaOperator::Concat),
            "band" => Some(crate::LuaOperator::BitwiseAnd),
            "bor" => Some(crate::LuaOperator::BitwiseOr),
            "bxor" => Some(crate::LuaOperator::BitwiseXor),
            "shl" => Some(crate::LuaOperator::ShiftLeft),
            "shr" => Some(crate::LuaOperator::ShiftRight),
            "unm" => Some(crate::LuaOperator::Negate),
            "bnot" => Some(crate::LuaOperator::BitwiseNot),
            "len" => Some(crate::LuaOperator::Length),
            "lt" => Some(crate::LuaOperator::Less),
            "le" => Some(crate::LuaOperator::LessEqual),
            "call" | "eq" => None,
            _ => {
                self.diagnostics.push(Diagnostic {
                    origin: crate::DiagnosticOrigin::Annotation,
                    span: name.span,
                    message: format!("unknown operator '{}'; expected add, sub, mul, div, idiv, mod, pow, concat, band, bor, bxor, shl, shr, unm, bnot, len, lt, le, eq or call", name.text),
                });
                return None;
            }
        };
        let open = self.take("(");
        let operand = match open {
            Some(_) if self.symbol(")") => None,
            Some(_) => Some(self.union()),
            None => None,
        };
        let parameters_span = match open {
            Some(open) => Some(open.join(self.expect(")"))),
            None => None,
        };
        let colon = self.take(":");
        let result = self.union();
        let unary = operator.is_some_and(crate::LuaOperator::unary);
        if unary && let Some(operand) = &operand {
            self.diagnostics.push(Diagnostic {
                origin: crate::DiagnosticOrigin::Annotation,
                span: operand.span,
                message: format!("unary operator '{}' takes no operand", name.text),
            });
        }
        let any = |span| TypeExpr { kind: TypeExprKind::Named("any".to_owned()), span };
        let parameter = |ty: TypeExpr| FunctionTypeParam { name: Name { text: "other".to_owned(), span: ty.span }, span: ty.span, ty: Some(ty), optional: false };
        let (params, vararg) = match (operator, operand) {
            (Some(operator), _) if operator.unary() => (Vec::new(), None),
            (_, Some(operand)) => (vec![parameter(operand)], None),
            // `---@operator call: R` is called with any arguments.
            (None, None) if name.text == "call" => (Vec::new(), Some(Box::new(VarargAnnotation { ty: any(name.span), span: name.span, ellipsis_span: None }))),
            (_, None) => (vec![parameter(any(name.span))], None),
        };
        let signature = Box::new(FunctionType {
            generics: Vec::new(),
            parameters_span: Some(parameters_span.unwrap_or(name.span)),
            params,
            vararg,
            returns_span: Some(colon.map_or(result.span, |colon| colon.join(result.span))),
            span: name.span.join(result.span),
            returns: vec![result],
            packs: None,
        });
        match operator {
            Some(operator) => Some(DeclarationItem::Operator { operator, signature, span, tag_span }),
            None if name.text == "call" => Some(DeclarationItem::Overload { signature, span, tag_span }),
            // `==` compares identity whatever a type declares, as in TypeScript.
            None => None,
        }
    }

    fn function_declaration(&mut self) -> Option<(Name, Option<Name>, Option<Span>, FunctionType)> {
        let start = self.current().span;
        let leading_colon = self.take(":");
        let first = self.declaration_name("function")?;
        let (name, owner, method_colon) = if leading_colon.is_some() {
            (first, None, leading_colon)
        } else if let Some(colon) = self.take(":") {
            let method_colon = Some(colon);
            (self.declaration_name("function member")?, Some(first), method_colon)
        } else {
            let (owner, name) = split_owner(first);
            (name, owner, None)
        };
        let signature = self.function_type_tail(start);
        if signature.parameters_span.is_none() {
            self.error("@function requires a parenthesized parameter list".into());
        }
        Some((name, owner, method_colon, signature))
    }

    /// Where a description starts: the current token, when it is `#` or a word, string or
    /// number rather than punctuation continuing a malformed type.
    fn description_start(&self) -> Option<usize> {
        let starts = matches!(&self.current().kind, TokenKind::Symbol(symbol) if symbol == "#")
            || matches!(self.current().kind, TokenKind::Identifier(_) | TokenKind::Keyword(_) | TokenKind::String(_) | TokenKind::Number(_) | TokenKind::CompletionHint(_));
        starts.then_some(self.current().span.start)
    }

    /// Consumes a description after a named declaration, returning where it starts.
    fn documentation_tail(&mut self) -> Option<usize> {
        // doc.bnf permits comment_string after named declarations/fields.
        // Punctuation that would continue a malformed type remains an error.
        if matches!(
            self.current().kind,
            TokenKind::Identifier(_) | TokenKind::Keyword(_) | TokenKind::String(_)
        ) {
            let start = self.current().span.start;
            while !self.eof() {
                self.pos += 1;
            }
            return Some(start);
        }
        None
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
    fn word_at(&self, pos: usize, value: &str) -> bool {
        self.tokens.get(pos).is_some_and(
            |token| matches!(&token.kind, TokenKind::Identifier(word) | TokenKind::Keyword(word) if word == value),
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
            origin: crate::DiagnosticOrigin::Annotation,
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
            let constraint = if self.take_constraint_keyword() {
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
            let default = self.generic_default();
            let span = default.as_ref().or(constraint.as_ref())
                .map_or(name.span, |ty| name.span.join(ty.span));
            parameters.push(GenericParameter {
                name,
                constraint,
                default,
                span,
            });
            if self.take(",").is_none() {
                break;
            }
        }
        parameters
    }
    /// A template literal type's text, between its backticks at `span`: literal text, a
    /// backslash escaping the character after it, and a type in each `${…}`.
    fn template_literal(&mut self, text: &str, span: Span) -> TypeExpr {
        // The text starts after the opening backtick.
        let base = span.start + 1;
        let mut texts = Vec::new();
        let mut types = Vec::new();
        let mut literal = String::new();
        let mut chars = text.char_indices().peekable();
        while let Some((at, ch)) = chars.next() {
            if ch == '\\' {
                if let Some((_, escaped)) = chars.next() {
                    literal.push(escaped);
                }
                continue;
            }
            if ch != '$' || chars.peek().is_none_or(|(_, next)| *next != '{') {
                literal.push(ch);
                continue;
            }
            chars.next();
            let body = at + 2;
            let mut depth = 0usize;
            let mut end = None;
            for (at, ch) in chars.by_ref() {
                match ch {
                    '{' => depth += 1,
                    '}' if depth == 0 => {
                        end = Some(at);
                        break;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            let Some(end) = end else {
                self.diagnostics.push(Diagnostic {
                    origin: crate::DiagnosticOrigin::Annotation,
                    span: Span { start: base + at, end: span.end },
                    message: "a template literal type's `${` needs a closing `}`".to_owned(),
                });
                break;
            };
            let tokens = crate::lexer::lex_annotation(&text[body..end]).tokens.into_iter().map(|mut token| {
                token.span.start += base + body;
                token.span.end += base + body;
                token
            }).collect();
            let mut parser = TypeParser { tokens, line_breaks: Vec::new(), pos: 0, diagnostics: Vec::new(), depth: self.depth, comma_ends_type: 0, function_parameter_comma_depth: None, in_conditional_extends: false, emmylua: self.emmylua, decay_optionals: self.decay_optionals, optional_suffixes: Vec::new() };
            let ty = if parser.eof() {
                parser.error("a template literal type's `${…}` needs a type".to_owned());
                TypeExpr { kind: TypeExprKind::Error, span: Span { start: base + at, end: base + end + 1 } }
            } else {
                parser.union()
            };
            if !parser.eof() {
                parser.error("unexpected text in a template literal type's `${…}`".to_owned());
            }
            self.diagnostics.extend(parser.diagnostics);
            self.optional_suffixes.extend(parser.optional_suffixes);
            texts.push(std::mem::take(&mut literal));
            types.push(ty);
        }
        texts.push(literal);
        TypeExpr { kind: TypeExprKind::TemplateLiteral { texts, types }, span }
    }

    /// A generic parameter's `:` or `extends` before its constraint: `T: C` (v1, LuaLS) or
    /// `T extends C` (EmmyLua Analyzer, TypeScript).
    fn take_constraint_keyword(&mut self) -> bool {
        if self.take(":").is_some() {
            return true;
        }
        if self.word_at(self.pos, "extends") && self.pos + 1 < self.tokens.len() {
            self.pos += 1;
            return true;
        }
        false
    }
    /// A generic parameter's `= Default`, as EmmyLua Analyzer and TypeScript write it: one type,
    /// as a constraint is, so a union default is parenthesized.
    fn generic_default(&mut self) -> Option<TypeExpr> {
        self.take("=")?;
        self.comma_ends_type += 1;
        let default = self.primary();
        self.comma_ends_type -= 1;
        Some(default)
    }
        fn return_packs(&mut self) -> (Vec<TypeExpr>, Option<Box<TypePacks>>) {
        // `is` is contextual: preserve it as a named type, and preserve prose
        // after completed types in annotation grammars without predicates.
        if matches!(self.current().kind, TokenKind::Identifier(_))
            && self.word_at(self.pos + 1, "is")
        {
            return self.predicate_return(false);
        }
        // So is `asserts`, before a parameter's name.
        if self.word_at(self.pos, "asserts")
            && matches!(self.tokens.get(self.pos + 1).map(|token| &token.kind), Some(TokenKind::Identifier(_)))
        {
            return self.predicate_return(true);
        }
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
        self.optional_results(&mut types, &alternatives);
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
    /// A result is written `T?`, having no name to write it on: `void | T`, a T or none, whereas
    /// `nil | T` is a value. A required result cannot follow one, as TypeScript's required tuple
    /// element cannot follow an optional one. A tail's values are each a T: one that is absent
    /// only shortens it. Decayed, `T?` is `nil | T`, a value. Out of line, to keep the recursive
    /// result parser's frame small.
    #[inline(never)]
    fn optional_results(&mut self, types: &mut [TypeExpr], alternatives: &[TypePack]) {
        for pack in alternatives {
            for ty in &types[pack.fixed.clone()] {
                self.claim_optional(ty);
            }
            if let Some(tail) = &pack.variadic {
                self.claim_optional(&types[tail.type_index]);
                if !self.decay_optionals { without_suffix_nil(&mut types[tail.type_index]); }
            }
            if self.decay_optionals { continue }
            let optional: Vec<_> = types[pack.fixed.clone()].iter_mut().map(optional_result).collect();
            let required = optional.iter().rposition(Option::is_none).unwrap_or(0);
            for &span in optional[..required].iter().flatten() {
                self.diagnostics.push(Diagnostic {
                    origin: crate::DiagnosticOrigin::Annotation,
                    span,
                    message: "a required result cannot follow a `T?` result, which may be absent".to_owned(),
                });
            }
        }
    }

    #[inline(never)]
    fn predicate_return(&mut self, asserts: bool) -> (Vec<TypeExpr>, Option<Box<TypePacks>>) {
        let asserts = asserts.then(|| {
            self.pos += 1;
            self.tokens[self.pos - 1].span
        });
        let parameter = self.name().expect("predicate parameter");
        self.depth += 1;
        // The asserted type owns unions and intersections, while an enclosing
        // callback parameter or generic argument still owns its comma.
        // `asserts value` alone asserts that the value is truthy.
        let ty = (asserts.is_none() || self.word_at(self.pos, "is")).then(|| {
            self.pos += 1; // Contextual `is`.
            self.union()
        });
        let mut span = asserts.unwrap_or(parameter.span).join(ty.as_ref().map_or(parameter.span, |ty| ty.span));
        let kind = if self.symbol("...") || self.return_comma() {
            self.error("a type predicate must be the sole function return type".to_owned());
            if self.take("...").is_some() {
                span = span.join(self.tokens[self.pos - 1].span);
            }
            if self.take(",").is_some() {
                let (pack, next) = self.return_pack();
                span = span.join(pack.span);
                if let Some(next) = next { span = span.join(next.span); }
            }
            TypeExprKind::Error
        } else {
            TypeExprKind::TypePredicate { parameter, ty: ty.map(Box::new), asserts }
        };
        self.depth -= 1;
        (vec![TypeExpr { kind, span }], None)
    }
    fn return_comma(&self) -> bool {
        if !self.symbol(",") {
            return false;
        }
        if self.comma_ends_type == 0 {
            return true;
        }
        if self.function_parameter_comma_depth != Some(self.comma_ends_type) {
            return false;
        }
        // v1 gives a callback's trailing bare types to its result list, as in
        // fun(callback: fun(): A, B). An explicitly named next parameter keeps
        // the surrounding signature's comma; parenthesize a scalar callback
        // when an untyped outer parameter would otherwise be ambiguous.
        !self.symbol_at(self.pos + 1, ")")
            && !self.symbol_at(self.pos + 1, "...")
            && !self.word_at(self.pos + 1, "vararg")
            && !self.symbol_at(self.pos + 2, ":")
            && !(self.symbol_at(self.pos + 2, "?")
                && self.symbol_at(self.pos + 3, ":"))
    }
    fn return_pack(&mut self) -> (ParsedReturnPack, Option<ParsedReturnPack>) {
        let first = match self.return_unit() {
            ReturnUnit::Pack(pack) => return (pack, None),
            ReturnUnit::Scalar(ty) => ty,
        };
        let start = first.span;
        let (first, mut pending) = self.return_scalar_union(first);
        let mut types = vec![first];
        let mut bare_tail = None;
        if pending.is_none() {
            while self.return_comma() {
                self.pos += 1;
                // `A, ...` (LuaLS): any number of further values of any type.
                if self.symbol("...") {
                    let ReturnUnit::Pack(mut pack) = self.bare_variadic_return() else { unreachable!() };
                    types.append(&mut pack.types);
                    bare_tail = pack.ellipsis_span;
                    break;
                }
                let first = match self.return_unit() {
                    ReturnUnit::Scalar(ty) => ty,
                    ReturnUnit::Pack(pack) => {
                        self.diagnostics.push(Diagnostic {
                            origin: crate::DiagnosticOrigin::Annotation,
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
        let ellipsis_span = if bare_tail.is_some() {
            bare_tail
        } else if pending.is_none() {
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
    /// LuaLS names function-type results: `fun(): (ok: boolean, message: string)`.
    /// The names document the results; they do not change their types.
    #[inline(never)]
    fn skip_result_name(&mut self) {
        if matches!(self.current().kind, TokenKind::Identifier(_))
            && self.symbol_at(self.pos + 1, ":")
        {
            self.pos += 2;
        }
    }

    #[inline(never)]
    fn result_list(&mut self) -> Vec<TypeExpr> {
        self.skip_result_name();
        let mut types = vec![self.union()];
        while self.take(",").is_some() {
            self.skip_result_name();
            types.push(self.union());
        }
        types
    }

    // A bare `...` (LuaLS) returns any number of values of any type. Kept out of
    // the recursive return parsers so their frames stay small.
    #[cold]
    #[inline(never)]
    fn bare_variadic_return(&mut self) -> ReturnUnit {
        let ellipsis_span = self.expect("...");
        ReturnUnit::Pack(ParsedReturnPack {
            types: vec![TypeExpr { kind: TypeExprKind::Named("any".to_owned()), span: ellipsis_span }],
            ellipsis_span: Some(ellipsis_span),
            span: ellipsis_span,
        })
    }
    fn return_scalar_union(&mut self, first: TypeExpr) -> (TypeExpr, Option<ParsedReturnPack>) {
        let first = self.intersected(first);
        let mut types = vec![first];
        let mut pending = None;
        while self.take("|").is_some() {
            match self.return_unit() {
                ReturnUnit::Scalar(ty) => types.push(self.intersected(ty)),
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
        // `T extends U and X or Y` is one result, as in a `---@type`.
        let ty = if pending.is_none() && self.word_at(self.pos, "extends") { self.conditional(ty) } else { ty };
        (ty, pending)
    }
    fn return_unit(&mut self) -> ReturnUnit {
        self.skip_result_name();
        if self.symbol("...") {
            return self.bare_variadic_return();
        }
        if !self.symbol("(") || self.depth >= 128 {
            return ReturnUnit::Scalar(self.primary());
        }
        let start = self.take("(").unwrap();
        let surrounding_comma_mode = self.comma_ends_type;
        let surrounding_parameter_depth = self.function_parameter_comma_depth;
        // Parse the contents before deciding whether parentheses enclose a
        // scalar type or a result pack. Commas in a callback's own return list
        // must remain owned by that callback, even at this group's depth.
        self.comma_ends_type = 0;
        self.function_parameter_comma_depth = None;
        self.depth += 1;
        let mut types = self.result_list();
        let ellipsis_span = self.take("...");
        let end = self.expect(")");
        self.depth -= 1;
        self.comma_ends_type = surrounding_comma_mode;
        self.function_parameter_comma_depth = surrounding_parameter_depth;
        let span = start.join(end);
        if types.len() == 1 && ellipsis_span.is_none() {
            let ty = TypeExpr {
                kind: TypeExprKind::Parenthesized(Box::new(types.pop().unwrap())),
                span,
            };
            ReturnUnit::Scalar(self.type_suffixes(ty))
        } else {
            ReturnUnit::Pack(ParsedReturnPack {
                types,
                ellipsis_span,
                span,
            })
        }
    }
    fn union(&mut self) -> TypeExpr {
        // A whole type, as in parentheses or a type argument, may be conditional again.
        let in_extends = std::mem::replace(&mut self.in_conditional_extends, false);
        let check = self.union_members();
        self.in_conditional_extends = in_extends;
        if self.word_at(self.pos, "extends") { self.conditional(check) } else { check }
    }
    /// EmmyLua Analyzer's `Check extends Constraint and Yes or No`, after its check type: a union,
    /// as TypeScript's check type is. Its constraint is a union too, and each branch a whole type,
    /// so a conditional in the false branch chains. Without `and` after a constraint, `extends`
    /// starts prose, not a conditional.
    /// Out of line: the recursive type parsers keep small frames.
    #[inline(never)]
    fn conditional(&mut self, check: TypeExpr) -> TypeExpr {
        let (pos, diagnostics) = (self.pos, self.diagnostics.len());
        self.pos += 1;
        let in_extends = std::mem::replace(&mut self.in_conditional_extends, true);
        let extends = self.union_members();
        self.in_conditional_extends = in_extends;
        if !self.keyword("and") {
            self.pos = pos;
            self.diagnostics.truncate(diagnostics);
            return check;
        }
        self.pos += 1;
        let yes = self.union();
        if !self.keyword("or") {
            self.error("expected `or` and a conditional type's false branch".to_owned());
            let span = check.span.join(yes.span);
            return TypeExpr { kind: TypeExprKind::Error, span };
        }
        self.pos += 1;
        let no = self.union();
        let span = check.span.join(no.span);
        TypeExpr {
            kind: TypeExprKind::Conditional { check: Box::new(check), extends: Box::new(extends), yes: Box::new(yes), no: Box::new(no) },
            span,
        }
    }
    /// `infer P` or `infer P extends C`, at `infer`.
    #[inline(never)]
    fn infer_type(&mut self, start: Span) -> TypeExpr {
        self.pos += 1;
        let TokenKind::Identifier(parameter) = self.current().kind.clone() else { unreachable!() };
        let parameter = Name { text: parameter, span: self.current().span };
        self.pos += 1;
        let constraint = self.infer_constraint();
        let end = constraint.as_ref().map_or(parameter.span, |constraint| constraint.span);
        TypeExpr { span: start.join(end), kind: TypeExprKind::Infer { name: parameter, constraint: constraint.map(Box::new) } }
    }
    /// `extends C` after `infer P`, as TypeScript's `tryParseConstraintOfInferType` takes it: always
    /// directly in a conditional type's extends type, elsewhere unless `and` follows, when the
    /// `extends` is a conditional type's whose check type is `infer P`.
    fn infer_constraint(&mut self) -> Option<TypeExpr> {
        if !matches!(&self.current().kind, TokenKind::Identifier(word) if word == "extends") {
            return None;
        }
        let (pos, diagnostics) = (self.pos, self.diagnostics.len());
        self.pos += 1;
        let in_extends = std::mem::replace(&mut self.in_conditional_extends, true);
        let constraint = self.union_members();
        self.in_conditional_extends = in_extends;
        if in_extends || !self.keyword("and") {
            return Some(constraint);
        }
        self.pos = pos;
        self.diagnostics.truncate(diagnostics);
        None
    }
    fn keyword(&self, value: &str) -> bool {
        matches!(&self.current().kind, TokenKind::Keyword(word) if word == value)
    }
    fn union_members(&mut self) -> TypeExpr {
        // A leading `|`, as a union split over lines begins (`---| "a"`).
        self.take("|");
        let first = self.intersection();
        if self.take("|").is_none() {
            return first;
        }
        let mut types = vec![first];
        loop {
            types.push(self.intersection());
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
    /// Types joined by `&`, which binds more tightly than `|`, as in TypeScript.
    fn intersection(&mut self) -> TypeExpr {
        let first = self.subtracted();
        self.intersected(first)
    }
    fn intersected(&mut self, first: TypeExpr) -> TypeExpr {
        if !self.symbol("&") {
            return first;
        }
        let mut types = vec![first];
        while self.take("&").is_some() {
            types.push(self.subtracted());
        }
        let span = types.first().unwrap().span.join(types.last().unwrap().span);
        TypeExpr {
            kind: TypeExprKind::Intersection(types),
            span,
        }
    }
    /// EmmyLua Analyzer's `A - B`: `A` without the members of `B`, `Exclude<A, B>`, binding more
    /// tightly than `&`. `T - ?` is `T` without nil. A `-` that begins a description
    /// (`number - the count`) subtracts nothing: a subtrahend other than `?` must end the type.
    fn subtracted(&mut self) -> TypeExpr {
        let mut ty = self.primary();
        while self.emmylua && self.symbol("-") {
            let (pos, diagnostics) = (self.pos, self.diagnostics.len());
            let operator = self.current().span;
            self.pos += 1;
            let removed = match self.take("?") {
                Some(question) => TypeExpr { kind: TypeExprKind::Named("nil".to_owned()), span: question },
                None => {
                    let removed = self.primary();
                    if matches!(removed.kind, TypeExprKind::Error) || !self.type_ends() {
                        self.pos = pos;
                        self.diagnostics.truncate(diagnostics);
                        break;
                    }
                    removed
                }
            };
            let span = ty.span.join(removed.span);
            ty = TypeExpr {
                kind: TypeExprKind::Generic {
                    base: Name { text: "Exclude".to_owned(), span: operator },
                    args: vec![ty, removed],
                    args_span: span,
                },
                span,
            };
        }
        ty
    }
    /// Whether the current token can follow a complete type, rather than continue a description.
    fn type_ends(&self) -> bool {
        match &self.current().kind {
            TokenKind::Eof | TokenKind::Newline => true,
            TokenKind::Symbol(symbol) => matches!(symbol.as_str(), "," | ")" | "]" | "}" | ">" | "|" | "&" | "-" | "?" | "@" | "#" | "..." | ";" | "="),
            TokenKind::Keyword(word) => matches!(word.as_str(), "and" | "or"),
            _ => false,
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
            // A backtick with no `${…}` is v1's completion hint, not a template literal.
            TokenKind::CompletionHint(text) if text.contains("${") => {
                self.pos += 1;
                self.template_literal(&text, start)
            }
            TokenKind::CompletionHint(text) => {
                self.pos += 1;
                TypeExpr { kind: TypeExprKind::CompletionHint(text), span: start }
            }
            TokenKind::Identifier(name) if name == "fun" => self.function_type(),
            // `infer P`, contextual: `infer` is a name unless one follows it.
            TokenKind::Identifier(name) if name == "infer" && matches!(self.tokens.get(self.pos + 1).map(|token| &token.kind), Some(TokenKind::Identifier(_))) => {
                self.infer_type(start)
            }
            // LuaLS's `async fun(...)`, and EmmyLua Analyzer's `sync fun(...)`: either is an
            // ordinary function type.
            TokenKind::Identifier(name) if (name == "async" || (name == "sync" && self.emmylua))
                && matches!(self.tokens.get(self.pos + 1).map(|token| &token.kind), Some(TokenKind::Identifier(next)) if next == "fun") =>
            {
                self.pos += 1;
                self.function_type()
            }
            // `keyof` and `typeof` are contextual, as in TypeScript: before an operand they are
            // operators, and otherwise names.
            TokenKind::Identifier(name) if name == "keyof" && self.type_starts_at(self.pos + 1) => self.key_of(start),
            TokenKind::Identifier(name) if name == "typeof"
                && matches!(self.tokens.get(self.pos + 1).map(|token| &token.kind), Some(TokenKind::Identifier(_))) =>
            {
                self.type_query(start)
            }
            TokenKind::Identifier(name) => {
                self.pos += 1;
                self.qualified(name, start)
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
                // Parentheses own their delimiters. An outer parameter or
                // generic argument comma cannot split a callback's returns
                // inside this original parenthesized type.
                let surrounding_comma_mode = self.comma_ends_type;
                let surrounding_parameter_depth = self.function_parameter_comma_depth;
                self.comma_ends_type = 0;
                self.function_parameter_comma_depth = None;
                let inner = self.union();
                self.comma_ends_type = surrounding_comma_mode;
                self.function_parameter_comma_depth = surrounding_parameter_depth;
                let end = self.expect(")");
                TypeExpr {
                    kind: TypeExprKind::Parenthesized(Box::new(inner)),
                    span: start.join(end),
                }
            }
            TokenKind::Symbol(symbol) if symbol == "{" => self.record(),
            // LuaLS's tuple `[T1, T2]`: the positional record `{T1, T2}`.
            TokenKind::Symbol(symbol) if symbol == "[" => self.tuple(),
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
                let base = Name {
                    text: text.clone(),
                    span: ty.span,
                };
                let args_start = self.expect("<");
                let mut args = Vec::new();
                if self.symbol(">") {
                    self.error("generic type argument list must not be empty".to_owned());
                } else {
                    loop {
                        self.comma_ends_type += 1;
                        args.push(self.union());
                        self.comma_ends_type -= 1;
                        if self.take(",").is_none() {
                            break;
                        }
                        if self.symbol(">") {
                            self.error(
                                "generic type argument list cannot have a trailing comma"
                                    .to_owned(),
                            );
                            break;
                        }
                    }
                }
                let end = self.expect(">");
                let args_span = args_start.join(end);
                let span = base.span.join(end);
                if args.iter().map(type_depth).max().unwrap_or(0) >= 128 {
                    self.diagnostics.push(Diagnostic {
                        origin: crate::DiagnosticOrigin::Annotation,
                        span: args_span,
                        message: "generic type nesting exceeds 128".to_owned(),
                    });
                    ty = TypeExpr {
                        kind: TypeExprKind::Error,
                        span,
                    };
                } else {
                    ty = TypeExpr {
                        kind: TypeExprKind::Generic {
                            base,
                            args,
                            args_span,
                        },
                        span,
                    };
                }
            }
        }
        ty = self.type_suffixes(ty);
        self.depth -= 1;
        ty
    }

    /// Array and index suffixes, then LuaLS's `T?`: T or nil, whose nil has the `?`'s span (a
    /// result's is a T or none instead: [`optional_result`]). Out of line, as one call, to
    /// keep the recursive type parsers' frames small.
    #[inline(never)]
    fn type_suffixes(&mut self, ty: TypeExpr) -> TypeExpr {
        let mut ty = self.postfix_types(ty);
        if let Some(question) = self.take("?") {
            let span = ty.span.join(question);
            ty = TypeExpr {
                kind: TypeExprKind::Union(vec![ty, TypeExpr { kind: TypeExprKind::Named("nil".to_owned()), span: question }]),
                span,
            };
            self.optional_suffixes.push(question);
            ty = self.postfix_types(ty);
        }
        ty
    }

    /// `T[]` and `T[K]`, in any order. An index touches its type, so a bracketed description
    /// after a type (`string [optional]`) is not an index.
    fn postfix_types(&mut self, mut ty: TypeExpr) -> TypeExpr {
        loop {
            ty = self.array_suffix(ty);
            if !self.symbol("[") || self.current().span.start != ty.span.end || matches!(ty.kind, TypeExprKind::Error) {
                return ty;
            }
            self.pos += 1;
            // The brackets own their commas, as parentheses do.
            let surrounding_comma_mode = self.comma_ends_type;
            let surrounding_parameter_depth = self.function_parameter_comma_depth;
            self.comma_ends_type = 0;
            self.function_parameter_comma_depth = None;
            let index = self.union();
            self.comma_ends_type = surrounding_comma_mode;
            self.function_parameter_comma_depth = surrounding_parameter_depth;
            let end = self.expect("]");
            let span = ty.span.join(end);
            ty = TypeExpr { kind: TypeExprKind::IndexedAccess { object: Box::new(ty), index: Box::new(index) }, span };
        }
    }

    /// Whether a type can begin at `pos`, so that a contextual operator before it applies to it.
    fn type_starts_at(&self, pos: usize) -> bool {
        match self.tokens.get(pos).map(|token| &token.kind) {
            Some(TokenKind::Identifier(_) | TokenKind::Number(_) | TokenKind::String(_)) => true,
            Some(TokenKind::Keyword(name)) => matches!(name.as_str(), "nil" | "true" | "false" | "function"),
            Some(TokenKind::Symbol(symbol)) => matches!(symbol.as_str(), "(" | "{" | "[" | "-"),
            _ => false,
        }
    }

    /// `keyof` and its operand, with the operand's array and index suffixes: `keyof T[]` is
    /// `keyof (T[])`. Out of line, as the other recursive type parsers' helpers are.
    #[inline(never)]
    fn key_of(&mut self, start: Span) -> TypeExpr {
        self.pos += 1;
        let operand = self.primary();
        let span = start.join(operand.span);
        TypeExpr { kind: TypeExprKind::KeyOf(Box::new(operand)), span }
    }

    /// `typeof` and the value it names: a name and the members after it, touching
    /// (`typeof M.config`).
    #[inline(never)]
    fn type_query(&mut self, start: Span) -> TypeExpr {
        self.pos += 1;
        let mut path = vec![self.name().expect("type query name")];
        while self.symbol(".")
            && self.current().span.start == path.last().unwrap().span.end
            && let Some(next) = self.tokens.get(self.pos + 1)
            && next.span.start == self.current().span.end
            && let TokenKind::Identifier(text) | TokenKind::Keyword(text) = &next.kind
        {
            path.push(Name { text: text.clone(), span: next.span });
            self.pos += 2;
        }
        let span = start.join(path.last().unwrap().span);
        TypeExpr { kind: TypeExprKind::TypeQuery(path), span }
    }

    /// A dotted or hyphenated name continued from `first` (`parser.object`, `fs.path`,
    /// `require-manager.result`): parts that touch, with nothing between them.
    #[inline(never)]
    fn qualified(&mut self, first: String, start: Span) -> TypeExpr {
        let mut name = first;
        let mut span = start;
        while (self.symbol(".") || self.symbol("-"))
            && self.current().span.start == span.end
            && let Some(next) = self.tokens.get(self.pos + 1)
            && next.span.start == self.current().span.end
            && let TokenKind::Identifier(part) | TokenKind::Keyword(part) = &next.kind
        {
            let separator = if self.symbol(".") { '.' } else { '-' };
            name.push(separator);
            name.push_str(part);
            span = span.join(next.span);
            self.pos += 2;
        }
        // LuaLS spells the io file class `file*`, after C's `FILE*`.
        if name == "file" && self.symbol("*") && self.current().span.start == span.end {
            span = span.join(self.current().span);
            self.pos += 1;
        }
        // LuaLS's `lightuserdata` is a userdata: Lua's `type` reports "userdata".
        if name == "lightuserdata" {
            name = "userdata".to_owned();
        }
        TypeExpr { kind: TypeExprKind::Named(name), span }
    }

    #[inline(never)]
    fn vararg_type(&mut self, untyped: bool, ellipsis: Span) -> TypeExpr {
        if untyped {
            return TypeExpr { kind: TypeExprKind::Named("any".to_owned()), span: ellipsis };
        }
        self.comma_ends_type += 1;
        let (ty, _) = self.parameter_type();
        self.comma_ends_type -= 1;
        ty
    }

    /// A parameter's type, and whether it may be absent. EmmyLua Analyzer's `T?` there is read only
    /// with its syntax, as Luanalysis's optional parameter is `name? T`. It is a value that may be
    /// absent, as a `T?` result is: the parameter is optional, and a variadic parameter's values
    /// are each a `T`. With optionals decayed it is `nil | T`.
    fn parameter_type(&mut self) -> (TypeExpr, bool) {
        let mut ty = self.union();
        if self.emmylua { self.claim_optional(&ty); }
        let absent = self.absent_type(&mut ty).is_some();
        (ty, absent)
    }

    /// Claims the `T?`s that `ty` is, or that members of its union are, for a position that may
    /// write them.
    #[inline(never)]
    fn claim_optional(&mut self, ty: &TypeExpr) {
        let mut spans = Vec::new();
        suffix_spans(ty, &mut spans);
        if !spans.is_empty() {
            self.optional_suffixes.retain(|span| !spans.contains(span));
        }
    }

    /// The `T?` of a parameter, field or table element, which may be absent and is otherwise a
    /// T: its `?`, and `ty` without its nil. Decayed, it is `nil | T`, which is no absence.
    #[inline(never)]
    fn absent_type(&mut self, ty: &mut TypeExpr) -> Option<Span> {
        if self.decay_optionals { return None }
        without_suffix_nil(ty)
    }

    /// A table field's or element's `T?`, claimed for an `element`, which has no name to write it
    /// on, or in EmmyLua Analyzer's syntax: the `?` when it may be absent. A required element
    /// cannot follow one that may be absent, each of the `optional_elements` before it. Out of
    /// line, to keep the recursive table parsers' frames small.
    #[inline(never)]
    fn optional_field(&mut self, ty: &mut TypeExpr, element: bool, optional_elements: &mut Vec<Span>) -> Option<Span> {
        if element || self.emmylua { self.claim_optional(ty); }
        let optional = self.absent_type(ty);
        match optional {
            Some(span) if element => optional_elements.push(span),
            None if element => {
                for span in optional_elements.drain(..) {
                    self.diagnostics.push(Diagnostic {
                        origin: crate::DiagnosticOrigin::Annotation,
                        span,
                        message: "a required element cannot follow a `T?` element, which may be absent".to_owned(),
                    });
                }
            }
            _ => {}
        }
        optional
    }

    #[inline(never)]
    fn tuple(&mut self) -> TypeExpr {
        let start = self.expect("[");
        let mut fields = Vec::new();
        let mut optional_elements = Vec::new();
        while !self.eof() && !self.symbol("]") {
            self.comma_ends_type += 1;
            let mut ty = self.union();
            self.comma_ends_type -= 1;
            // An element has no name to write `?` on: its `T?` may be absent.
            let optional = self.optional_field(&mut ty, true, &mut optional_elements);
            fields.push(TypeField { key: TypeFieldKey::Sequence { index: fields.len() + 1 }, span: ty.span, ty, readonly: None, optional });
            if self.take(",").is_none() { break; }
        }
        let end = self.expect("]");
        TypeExpr { kind: TypeExprKind::Record(fields), span: start.join(end) }
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
                    self.diagnostics.push(Diagnostic {
                        origin: crate::DiagnosticOrigin::Annotation,
                        span: suffix_span,
                        message: "array type nesting exceeds 128".to_owned(),
                    });
                }
                ty = TypeExpr {
                    kind: TypeExprKind::Error,
                    span,
                };
                depth = 1;
                exceeded = true;
            } else {
                ty = TypeExpr {
                    kind: TypeExprKind::Array {
                        element: Box::new(ty),
                        suffix_span,
                    },
                    span,
                };
                depth += 1;
            }
        }
        ty
    }
    fn function_type(&mut self) -> TypeExpr {
        let start = self.current().span;
        self.pos += 1;
        let signature = self.function_type_tail(start);
        TypeExpr {
            span: signature.span,
            kind: TypeExprKind::Function(signature),
        }
    }

    fn function_type_tail(&mut self, start: Span) -> FunctionType {
        let mut params = Vec::new();
        let mut vararg = None;
        let generics = self.declaration_parameters();
        let mut parameters_span = None;
        if let Some(open) = self.take("(") {
            while !self.eof() && !self.symbol(")") {
                let param_start = self.current().span;
                let ellipsis = self.symbol("...");
                let word = matches!(&self.current().kind, TokenKind::Identifier(name) if name == "vararg")
                    && !self.symbol_at(self.pos + 1, ":")
                    && !self.symbol_at(self.pos + 1, "?");
                if ellipsis || word {
                    self.pos += 1;
                    // LuaLS also writes an untyped `...`, which is `any`.
                    let untyped = ellipsis && !self.symbol(":");
                    if ellipsis && !untyped {
                        self.expect(":");
                    }
                    let ty = self.vararg_type(untyped, param_start);
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
                        "unnamed function parameters are not supported".to_owned(),
                    );
                    while !self.eof() && !self.symbol(",") && !self.symbol(")") {
                        self.pos += 1;
                    }
                    if self.take(",").is_some() {
                        continue;
                    }
                    break;
                };
                let mut optional = self.take("?").is_some();
                let ty = if self.take(":").is_some() {
                    self.comma_ends_type += 1;
                    let surrounding_parameter_depth = self.function_parameter_comma_depth;
                    self.function_parameter_comma_depth = Some(self.comma_ends_type);
                    let (ty, absent) = self.parameter_type();
                    optional |= absent;
                    self.function_parameter_comma_depth = surrounding_parameter_depth;
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
            let close = self.expect(")");
            parameters_span = Some(open.join(close));
        }
        let returns_start = self.take(":");
        let (returns, packs) = if returns_start.is_some() {
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
        FunctionType {
            generics,
            parameters_span,
            params,
            vararg,
            returns_span: returns_start.map(|start| start.join(end)),
            returns,
            packs,
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
        if self.at_mapped_type() {
            return self.mapped_type();
        }
        let start = self.expect("{");
        let mut fields = Vec::new();
        let mut sequence_index = 0;
        let mut optional_elements = Vec::new();
        while !self.eof() && !self.symbol("}") {
            let field_start = self.current().span;
            let readonly = self.readonly_record_modifier();
            let mut optional = None;
            let key = if self.take("[").is_some() {
                self.comma_ends_type += 1;
                let mut key = self.union();
                self.comma_ends_type -= 1;
                if type_depth(&key) >= 128 {
                    self.diagnostics.push(Diagnostic {
                        origin: crate::DiagnosticOrigin::Annotation,
                        span: key.span,
                        message: "record key type nesting exceeds 128".to_owned(),
                    });
                    key = TypeExpr {
                        kind: TypeExprKind::Error,
                        span: key.span,
                    };
                }
                self.expect("]");
                optional = self.take("?");
                self.expect(":");
                TypeFieldKey::Indexed(Box::new(key))
            } else if let TokenKind::Identifier(text) | TokenKind::Keyword(text) =
                self.current().kind.clone()
            {
                let name_span = self.current().span;
                if self.symbol_at(self.pos + 1, "?") && self.symbol_at(self.pos + 2, ":") {
                    self.pos += 1;
                    optional = self.take("?");
                    self.expect(":");
                    TypeFieldKey::Named(Name {
                        text,
                        span: name_span,
                    })
                } else if self.symbol_at(self.pos + 1, ":") {
                    self.pos += 1;
                    self.expect(":");
                    TypeFieldKey::Named(Name {
                        text,
                        span: name_span,
                    })
                } else {
                    sequence_index += 1;
                    TypeFieldKey::Sequence {
                        index: sequence_index,
                    }
                }
            } else {
                sequence_index += 1;
                TypeFieldKey::Sequence {
                    index: sequence_index,
                }
            };
            self.comma_ends_type += 1;
            let mut ty = self.union();
            self.comma_ends_type -= 1;
            // A positional element is written `T?`, as a tuple's is; a named or indexed field's
            // `?` is on its key, except in EmmyLua Analyzer's syntax.
            let element = matches!(key, TypeFieldKey::Sequence { .. });
            optional = optional.or(self.optional_field(&mut ty, element, &mut optional_elements));
            if type_depth(&ty) >= 128 {
                self.diagnostics.push(Diagnostic {
                    origin: crate::DiagnosticOrigin::Annotation,
                    span: ty.span,
                    message: "record value type nesting exceeds 128".to_owned(),
                });
                ty = TypeExpr {
                    kind: TypeExprKind::Error,
                    span: ty.span,
                };
            }
            let span = field_start.join(ty.span);
            fields.push(TypeField { key, ty, readonly, optional, span });
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
    /// At `{` of `{ [P in K]: T }`, perhaps after `readonly`, `+readonly` or `-readonly`.
    fn at_mapped_type(&self) -> bool {
        let mut pos = self.pos + 1;
        if self.symbol_at(pos, "+") || self.symbol_at(pos, "-") {
            if !self.word_at(pos + 1, "readonly") { return false; }
            pos += 2;
        } else if self.word_at(pos, "readonly") && self.symbol_at(pos + 1, "[") {
            pos += 1;
        }
        self.symbol_at(pos, "[")
            && matches!(self.tokens.get(pos + 1).map(|token| &token.kind), Some(TokenKind::Identifier(_)))
            && self.word_at(pos + 2, "in")
    }
    /// `{ [P in K as N]?: T }` with its modifiers, as TypeScript writes it. Out of line: the
    /// recursive type parsers keep small frames.
    #[inline(never)]
    fn mapped_type(&mut self) -> TypeExpr {
        let start = self.expect("{");
        let readonly = if self.take("+").is_some() {
            self.expect_word("readonly");
            MappedModifier::Plus
        } else if self.take("-").is_some() {
            self.expect_word("readonly");
            MappedModifier::Remove
        } else if self.word_at(self.pos, "readonly") {
            self.pos += 1;
            MappedModifier::Add
        } else {
            MappedModifier::None
        };
        self.expect("[");
        let TokenKind::Identifier(text) = self.current().kind.clone() else { unreachable!("a mapped type's parameter") };
        let parameter = Name { text, span: self.current().span };
        self.pos += 2;
        let constraint = self.union();
        let name = self.word_at(self.pos, "as").then(|| {
            self.pos += 1;
            self.union()
        });
        self.expect("]");
        let optional = if self.take("+").is_some() {
            self.expect("?");
            MappedModifier::Plus
        } else if self.take("-").is_some() {
            self.expect("?");
            MappedModifier::Remove
        } else if self.take("?").is_some() {
            MappedModifier::Add
        } else {
            MappedModifier::None
        };
        self.expect(":");
        self.comma_ends_type += 1;
        let ty = self.union();
        self.comma_ends_type -= 1;
        if self.take(";").is_none() { self.take(","); }
        if !self.eof() && !self.symbol("}") {
            self.error("a mapped type has one member".to_owned());
            self.recover_record();
        }
        let end = self.expect("}");
        TypeExpr {
            kind: TypeExprKind::Mapped(Box::new(MappedType { readonly, parameter, constraint, name, optional, ty })),
            span: start.join(end),
        }
    }
    fn expect_word(&mut self, word: &str) {
        if self.word_at(self.pos, word) {
            self.pos += 1;
        } else {
            self.error(format!("expected `{word}`"));
        }
    }
    /// `{ readonly name: T }` and `{ readonly [K]: V }`, as in TypeScript; `{ readonly: T }`
    /// is a field named `readonly`.
    fn readonly_record_modifier(&mut self) -> Option<Span> {
        if !self.word_at(self.pos, "readonly") { return None; }
        let named = matches!(self.tokens.get(self.pos + 1).map(|token| &token.kind), Some(TokenKind::Identifier(_) | TokenKind::Keyword(_)))
            && self.symbol_at(self.pos + 2, ":");
        let indexed = self.symbol_at(self.pos + 1, "[") && {
            let (pos, diagnostics) = (self.pos, self.diagnostics.len());
            self.pos += 2;
            self.comma_ends_type += 1;
            self.union();
            self.comma_ends_type -= 1;
            let indexed = self.take("]").is_some() && self.symbol(":") && self.diagnostics.len() == diagnostics;
            self.pos = pos;
            self.diagnostics.truncate(diagnostics);
            indexed
        };
        (named || indexed).then(|| {
            let span = self.current().span;
            self.pos += 1;
            span
        })
    }
    fn recover_record(&mut self) {
        while !self.eof() && !self.symbol(",") && !self.symbol("}") {
            self.pos += 1;
        }
    }
}

/// `Owner.member` as its owner and member; dotted owners (`parser.object.member`)
/// keep every part but the last.
fn split_owner(name: Name) -> (Option<Name>, Name) {
    let Some(dot) = name.text.rfind('.') else { return (None, name) };
    let start = name.span.start;
    let owner = Name { text: name.text[..dot].to_owned(), span: Span::new(start, start + dot) };
    let member = Name { text: name.text[dot + 1..].to_owned(), span: Span::new(start + dot + 1, name.span.end) };
    (Some(owner), member)
}

/// The members of an alias written on the `---| type # description` lines after it, as one union.
/// `offset` is where the first of `lines` starts in the source.
fn alias_continuation(lines: &[&str], mut offset: usize, alias: Span, diagnostics: &mut Vec<Diagnostic>, options: crate::ParseOptions) -> TypeExpr {
    let mut members = Vec::new();
    for line in lines {
        let stripped = line.trim_start_matches([' ', '\t', '\r']).trim_start_matches('-').trim_start_matches([' ', '\t']);
        let Some(member) = stripped.strip_prefix('|') else { break };
        let start = offset + line.len() - member.len();
        // A member's description follows `#`, outside a quoted value.
        let tokens = crate::lexer::lex_annotation(member).tokens;
        let end = tokens.iter()
            .find(|token| matches!(&token.kind, TokenKind::Symbol(symbol) if symbol == "#"))
            .map_or(member.trim_end().len(), |token| token.span.start);
        let text = member[..end].trim_end();
        let tokens = crate::lexer::lex_annotation(text).tokens.into_iter().map(|mut token| {
            token.span.start += start;
            token.span.end += start;
            token
        }).collect();
        let mut parser = TypeParser { tokens, line_breaks: Vec::new(), pos: 0, diagnostics: Vec::new(), depth: 0, comma_ends_type: 0, function_parameter_comma_depth: None, in_conditional_extends: false, emmylua: options.emmylua, decay_optionals: options.decay_optionals, optional_suffixes: Vec::new() };
        members.push(parser.union());
        diagnostics.extend(parser.diagnostics);
        if !options.emmylua {
            diagnostics.extend(parser.optional_suffixes.iter().map(|&span| optional_suffix_diagnostic(span)));
        }
        offset += line.len();
    }
    match members.len() {
        0 => {
            diagnostics.push(Diagnostic {
                origin: crate::DiagnosticOrigin::Annotation,
                span: alias,
                message: "an alias needs a type, or `---|` lines listing its members".to_owned(),
            });
            TypeExpr { kind: TypeExprKind::Error, span: alias }
        }
        1 => members.pop().expect("one member"),
        _ => {
            let span = members[0].span.join(members.last().expect("members").span);
            TypeExpr { kind: TypeExprKind::Union(members), span }
        }
    }
}

#[cfg(test)]
mod predicate_documentation_tests {
    use super::{TagDescription, annotations};

    #[test]
    fn named_declaration_descriptions_preserve_initial_contextual_is() {
        for tag in ["class", "enum", "shape", "module"] {
            let source = format!("---@{tag} Widget is documented here\nlocal Widget = {{}}");
            let file = crate::parse(&source);
            assert!(file.diagnostics.is_empty(), "{tag}: {:#?}", file.diagnostics);
            let (parsed, diagnostics) = annotations(&file.comments[0], file.options);
            assert!(diagnostics.is_empty(), "{tag}: {diagnostics:#?}");
            assert_eq!(parsed.descriptions, vec![TagDescription {
                tag: tag.to_owned(),
                subject: Some("Widget".to_owned()),
                text: "is documented here".to_owned(),
            }]);
        }
    }

    #[test]
    fn type_descriptions_preserve_initial_contextual_is() {
        for (tag, subject, header) in [
            ("param", Some("value"), "value string"),
            ("type", None, "string[]"),
            ("field", Some("value"), "value {count:number}"),
            ("alias", Some("Values"), "Values string[]"),
        ] {
            let text = format!("---@{tag} {header} is documented here");
            let comment = crate::Comment {
                span: crate::Span::new(0, text.len()),
                text,
                kind: crate::CommentKind::Documentation,
            };
            let (parsed, diagnostics) = annotations(&comment, crate::ParseOptions::default());
            assert!(diagnostics.is_empty(), "{tag}: {diagnostics:#?}");
            assert_eq!(parsed.descriptions, vec![TagDescription {
                tag: tag.to_owned(),
                subject: subject.map(str::to_owned),
                text: "is documented here".to_owned(),
            }]);
        }
    }
}

/// A result slot's `T?` (the nil [`TypeParser::type_suffixes`] gives the `?`'s own one-byte span)
/// is `void | T`: a T, or no result, as a bare `return` or reaching the end of the function
/// gives. It is not nil: `return nil` is no `T?`. The `?`'s span, when `ty` is one.
fn optional_result(ty: &mut TypeExpr) -> Option<Span> {
    let TypeExprKind::Union(members) = &mut ty.kind else { return None };
    let mut optional = None;
    for member in members {
        if let Some(span) = suffix_nil(member) {
            member.kind = TypeExprKind::Named("void".to_owned());
            optional = Some(span);
        } else if let Some(span) = optional_result(member) {
            optional = Some(span);
        }
    }
    optional
}

/// The `?`s of the `T?`s that `ty` is, or that members of its union are.
fn suffix_spans(ty: &TypeExpr, spans: &mut Vec<Span>) {
    let TypeExprKind::Union(members) = &ty.kind else { return };
    for member in members {
        match suffix_nil(member) {
            Some(span) => spans.push(span),
            None => suffix_spans(member, spans),
        }
    }
}

/// A `T?` where Luanalysis writes the `?` on a name, or writes `nil | T`.
fn optional_suffix_diagnostic(span: Span) -> Diagnostic {
    Diagnostic {
        origin: crate::DiagnosticOrigin::Annotation,
        span,
        message: "`T?` here is EmmyLua Analyzer syntax (--emmylua-syntax): only a result or a tuple element is written `T?`; an optional parameter or field is `name? T`, and a type that admits nil is `nil | T`".to_owned(),
    }
}

/// `ty` without `T?`'s nil, a value that may be absent: the `?`'s span, when it was one.
fn without_suffix_nil(ty: &mut TypeExpr) -> Option<Span> {
    let TypeExprKind::Union(members) = &mut ty.kind else { return None };
    let mut optional = None;
    members.retain(|member| match suffix_nil(member) {
        Some(span) => {
            optional = Some(span);
            false
        }
        None => true,
    });
    for member in members.iter_mut() {
        if let Some(span) = without_suffix_nil(member) {
            optional = Some(span);
        }
    }
    if members.len() == 1 {
        let member = members.remove(0);
        *ty = TypeExpr { span: ty.span, ..member };
    }
    optional
}

/// The span of `T?`'s nil, the `?` itself: a written `nil` is three bytes.
fn suffix_nil(member: &TypeExpr) -> Option<Span> {
    match &member.kind {
        TypeExprKind::Named(name) if name == "nil" && member.span.end == member.span.start + 1 => Some(member.span),
        _ => None,
    }
}
