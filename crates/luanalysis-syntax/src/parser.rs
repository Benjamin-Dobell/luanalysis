use crate::{
    BinaryOperator, Comment, CommentKind, Diagnostic, Expr, ExprKind, File, Function,
    FunctionAnnotations, Name, NamedTypeDeclaration, NamedTypeDeclarationId,
    NamedTypeDeclarationKind, Span, Stmt, StmtKind, TableField, TableFieldKind, Token, TokenKind,
    TypeAnnotation, TypeExpr, TypeExprKind, TypeExtension, UnaryOperator, lex, types,
    OverloadAnnotation, OverloadDeclarationId, OverloadReference,
};

#[path = "control.rs"]
mod control;

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
        pending_annotations: FunctionAnnotations::default(),
        type_declarations: Vec::new(),
        function_declarations: Vec::new(),
        type_extensions: Vec::new(),
        expression_assertions: Vec::new(),
        overload_annotations: Vec::new(),
        documentation_references: Vec::new(),
        return_annotations: Vec::new(),
        documentation_group: None,
        depth: 0,
        block_depth: 0,
        loop_depth: 0,
        vararg_allowed: true,
    };
    let statements = parser.statements(&[]);
    File {
        statements,
        expression_assertions: parser.expression_assertions,
        type_declarations: parser.type_declarations,
        function_declarations: parser.function_declarations,
        type_extensions: parser.type_extensions,
        overload_annotations: parser.overload_annotations,
        documentation_references: parser.documentation_references,
        return_annotations: parser.return_annotations,
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
    pending_annotations: FunctionAnnotations,
    type_declarations: Vec<NamedTypeDeclaration>,
    function_declarations: Vec<crate::DocFunctionDeclaration>,
    type_extensions: Vec<TypeExtension>,
    expression_assertions: Vec<crate::ExpressionAssertion>,
    overload_annotations: Vec<OverloadAnnotation>,
    documentation_references: Vec<crate::DocumentationReference>,
    return_annotations: Vec<crate::ReturnAnnotation>,
    documentation_group: Option<DocumentationGroup>,
    depth: usize,
    block_depth: usize,
    loop_depth: usize,
    vararg_allowed: bool,
}

struct DocumentationGroup {
    span: Span,
    declarations: Vec<usize>,
    extensions: Vec<usize>,
    record: Option<usize>,
    overloads: Vec<usize>,
    functions: Vec<usize>,
    active_function: Option<usize>,
}

fn documentation_gap_is_contiguous(gap: &str) -> bool {
    if !gap.chars().all(char::is_whitespace) {
        return false;
    }
    let mut lines = 0;
    let mut carriage_return = false;
    for character in gap.chars() {
        if character == '\r' || character == '\n' && !carriage_return {
            lines += 1;
        }
        carriage_return = character == '\r';
    }
    lines <= 1
}

fn expression_assertion_comment(comment: &Comment) -> bool {
    if comment.kind != CommentKind::Documentation {
        return false;
    }
    let (documentation, _) = types::annotations(comment);
    documentation.functions.type_annotation.is_some() || !documentation.exclusions.is_empty()
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
        self.diagnostic_at(crate::DiagnosticOrigin::LuaSyntax, self.current().span, message);
    }
    // Keep diagnostic construction outside recursive expression/table frames.
    // Their stack budget must not depend on diagnostic metadata layout.
    #[cold]
    #[inline(never)]
    fn diagnostic_at(&mut self, origin: crate::DiagnosticOrigin, span: Span, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            origin,
            span,
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
    fn statements(&mut self, terminators: &[&str]) -> Vec<Stmt> {
        let mut statements = Vec::new();
        let mut returned = false;
        let mut return_semicolon_used = false;
        loop {
            self.leading_trivia();
            if self.eof() || terminators.iter().any(|word| self.keyword(word)) {
                break;
            }
            if self.symbol(";") {
                if returned {
                    if return_semicolon_used {
                        self.error("Lua return accepts at most one terminal semicolon");
                    }
                    return_semicolon_used = true;
                }
                self.pos += 1;
                continue;
            }
            if returned {
                self.error("return must be the final statement in its Lua block");
                returned = false;
            }
            let before = self.pos;
            let group = self.documentation_group.take();
            if let Some(group) = &group {
                self.finish_documentation_functions(group);
            }
            if let Some(stmt) = self.statement() {
                if let Some(group) = group {
                    self.attach_type_group(group, stmt.span);
                }
                returned = matches!(stmt.kind, StmtKind::Return(_));
                return_semicolon_used = false;
                statements.push(stmt);
            }
            if self.pos == before {
                self.pos += 1;
            }
        }
        let annotations = std::mem::take(&mut self.pending_annotations);
        self.unattached_annotations(&annotations);
        if let Some(group) = self.documentation_group.take() {
            self.finish_documentation_functions(&group);
        }
        statements
    }

    fn finish_documentation_functions(&mut self, group: &DocumentationGroup) {
        let owners = group
            .declarations
            .iter()
            .filter(|&&index| {
                matches!(self.type_declarations[index].kind,
                    NamedTypeDeclarationKind::Record {
                        flavor: crate::NamedRecordKind::Class | crate::NamedRecordKind::Shape, ..
                    })
            })
            .count();
        for &index in &group.functions {
            let declaration = &mut self.function_declarations[index];
            if declaration.owner.is_some() {
                continue;
            }
            let message = if owners > 1 {
                Some("@function has multiple class or shape owners in its documentation group")
            } else if declaration.method_colon.is_some() {
                Some("@function implicit-self member requires an explicit or documentation-group owner")
            } else {
                None
            };
            declaration.invalid_owner = message.is_some();
            if let Some(message) = message {
                self.diagnostics.push(Diagnostic {
                    origin: crate::DiagnosticOrigin::Annotation,
                    span: declaration.name.span,
                    message: message.into(),
                });
            }
        }
    }

    fn attach_type_group(&mut self, group: DocumentationGroup, statement: Span) {
        let gap = &self.source[group.span.end..statement.start];
        if !documentation_gap_is_contiguous(gap) {
            return;
        }
        for index in group.declarations {
            self.type_declarations[index].attachment = Some(statement);
        }
        for index in group.extensions {
            self.type_extensions[index].attachment = Some(statement);
        }
    }

    fn record_documentation(
        &mut self,
        comment: &Comment,
        mut documentation: types::ParsedDocumentation,
    ) -> FunctionAnnotations {
        for annotation in &documentation.exclusions {
            self.diagnostic_at(crate::DiagnosticOrigin::Annotation, annotation.span, "@not must annotate an expression");
        }
        let contiguous = self.documentation_group.as_ref().is_some_and(|group| {
            let gap = &self.source[group.span.end..comment.span.start];
            documentation_gap_is_contiguous(gap)
        });
        if !contiguous {
            if let Some(group) = self.documentation_group.take() {
                self.finish_documentation_functions(&group);
            }
            self.documentation_group = Some(DocumentationGroup {
                span: comment.span,
                declarations: Vec::new(),
                extensions: Vec::new(),
                record: None,
                overloads: Vec::new(),
                functions: Vec::new(),
                active_function: None,
            });
        }
        let group = self.documentation_group.as_mut().unwrap();
        group.span = group.span.join(comment.span);
        for item in documentation.declarations {
            match item {
                types::DeclarationItem::Reference(reference) => {
                    self.documentation_references.push(reference);
                }
                types::DeclarationItem::Overload { signature, span, tag_span } => {
                    let index = self.overload_annotations.len();
                    let id = OverloadDeclarationId(index.try_into().expect("original overload index fits u32"));
                    self.overload_annotations.push(OverloadAnnotation {
                        id, signature, span, tag_span, group_span: group.span, named_candidates: Vec::new(),
                    });
                    group.overloads.push(index);
                    let reference = OverloadReference { id, span };
                    if let Some(index) = group.active_function {
                        self.function_declarations[index].annotations.overloads.push(reference);
                    } else {
                        documentation.functions.overloads.push(reference);
                    }
                }
                types::DeclarationItem::Function {
                    name,
                    explicit_owner,
                    method_colon,
                    signature,
                    mut annotations,
                    span,
                    tag_span,
                } => {
                    let index = self.function_declarations.len();
                    let id = crate::DocFunctionDeclarationId(
                        index.try_into().expect("documentation function index fits u32"),
                    );
                    if types::annotation_spans(&self.pending_annotations)
                        .iter()
                        .all(|span| span.start >= group.span.start)
                    {
                        types::merge_annotations(
                            &mut annotations,
                            std::mem::take(&mut self.pending_annotations),
                            &mut self.diagnostics,
                        );
                    }
                    self.function_declarations.push(crate::DocFunctionDeclaration {
                        id,
                        name,
                        owner: explicit_owner.map(crate::DocFunctionOwner::Explicit),
                        invalid_owner: false,
                        method_colon,
                        signature: *signature,
                        annotations,
                        span,
                        tag_span,
                        group_span: group.span,
                    });
                    group.functions.push(index);
                    group.active_function = Some(index);
                }
                types::DeclarationItem::Named { name, generics, kind, span } => {
                    group.active_function = None;
                    let index = self.type_declarations.len();
                    let record = matches!(kind, NamedTypeDeclarationKind::Record { .. });
                    self.type_declarations.push(NamedTypeDeclaration {
                        id: NamedTypeDeclarationId(
                            index.try_into().expect("named declaration index fits u32"),
                        ),
                        name,
                        generics,
                        kind,
                        span,
                        group_span: group.span,
                        attachment: None,
                    });
                    group.declarations.push(index);
                    if record {
                        group.record = Some(index);
                    }
                }
                types::DeclarationItem::Field(field) => {
                    group.active_function = None;
                    let record = group.record.filter(|&index| {
                        field.explicit_owner.as_ref().is_none_or(|owner| {
                            owner.text == self.type_declarations[index].name.text
                        })
                    });
                    if let Some(index) = record {
                        let declaration = &mut self.type_declarations[index];
                        let NamedTypeDeclarationKind::Record { fields, .. } = &mut declaration.kind
                        else {
                            unreachable!()
                        };
                        declaration.span = declaration.span.join(field.span);
                        fields.push(field);
                    } else if let Some(owner) = field.explicit_owner.clone() {
                        let index = self.type_extensions.len();
                        self.type_extensions.push(TypeExtension {
                            owner,
                            span: field.span,
                            fields: vec![field],
                            group_span: group.span,
                            attachment: None,
                        });
                        group.extensions.push(index);
                    } else {
                        self.diagnostics.push(Diagnostic {
                            origin: crate::DiagnosticOrigin::Annotation,
                            span: field.span, message: "@field has no preceding class, shape, module or explicit owner in its documentation group".to_owned(),
                        });
                    }
                }
            }
        }
        for &index in &group.declarations {
            self.type_declarations[index].group_span = group.span;
        }
        for &index in &group.extensions {
            self.type_extensions[index].group_span = group.span;
        }
        let named_candidates: Vec<_> = group.declarations.iter().filter_map(|&index| {
            matches!(self.type_declarations[index].kind, NamedTypeDeclarationKind::Record { .. }).then_some(self.type_declarations[index].id)
        }).collect();
        for &index in &group.overloads {
            self.overload_annotations[index].group_span = group.span;
            self.overload_annotations[index].named_candidates.clone_from(&named_candidates);
        }
        let owners: Vec<_> = group.declarations.iter().filter_map(|&index| {
            matches!(self.type_declarations[index].kind,
                NamedTypeDeclarationKind::Record {
                    flavor: crate::NamedRecordKind::Class | crate::NamedRecordKind::Shape, ..
                })
                .then_some(self.type_declarations[index].id)
        }).collect();
        for &index in &group.functions {
            let declaration = &mut self.function_declarations[index];
            declaration.group_span = group.span;
            if !matches!(declaration.owner, Some(crate::DocFunctionOwner::Explicit(_))) {
                declaration.owner = match owners.as_slice() {
                    [owner] => Some(crate::DocFunctionOwner::Contextual(*owner)),
                    _ => None,
                };
            }
        }
        if let Some(index) = group.active_function {
            types::merge_annotations(
                &mut self.function_declarations[index].annotations,
                std::mem::take(&mut documentation.functions),
                &mut self.diagnostics,
            );
        }
        documentation.functions
    }
    fn unattached_annotations(&mut self, annotations: &FunctionAnnotations) {
        for span in types::annotation_spans(annotations) {
            if annotations.overloads.iter().any(|reference| reference.span == span
                && !self.overload_annotations[reference.id.0 as usize].named_candidates.is_empty()) {
                continue;
            }
            self.diagnostics.push(Diagnostic {
                origin: crate::DiagnosticOrigin::Annotation,
                span,
                message:
                    "Luanalysis annotation is not attached to a compatible declaration or function"
                        .to_owned(),
            });
        }
    }
    fn leading_trivia(&mut self) {
        loop {
            match self.current().kind.clone() {
                TokenKind::Newline => self.pos += 1,
                TokenKind::Comment(comment) => {
                    if expression_assertion_comment(&comment) {
                        let next = self.significant_index();
                        let declaration = matches!(&self.tokens[next].kind, TokenKind::Keyword(word) if matches!(word.as_str(), "local" | "function" | "return" | "if" | "while" | "repeat" | "for" | "do"));
                        if !declaration
                            && !self.source[comment.span.end..self.tokens[next].span.start]
                                .contains(['\r', '\n'])
                        {
                            break;
                        }
                    }
                    self.pos += 1;
                    if comment.kind != CommentKind::Documentation {
                        continue;
                    }
                    let (annotations, diagnostics) = types::annotations(&comment);
                    self.diagnostics.extend(diagnostics);
                    let annotations = self.record_documentation(&comment, annotations);
                    if !types::annotation_spans(&annotations).is_empty() {
                        let line_start = self.source[..comment.span.start]
                            .rfind(['\r', '\n'])
                            .map_or(0, |index| index + 1);
                        let standalone = self.source[line_start..comment.span.start]
                            .chars()
                            .all(|ch| matches!(ch, ' ' | '\t'));
                        if !standalone {
                            self.unattached_annotations(&annotations);
                        } else {
                            types::merge_annotations(
                                &mut self.pending_annotations,
                                annotations,
                                &mut self.diagnostics,
                            );
                        }
                    }
                }
                _ => break,
            }
        }
    }
    fn inner_trivia(&mut self) {
        self.trivia(false);
    }
    // Expression and field owners retain their own documentation. In
    // particular this never uses the statement-level pending annotations.
    fn expression_trivia(&mut self) {
        self.trivia(true);
    }
    fn trivia(&mut self, expression_documentation: bool) {
        loop {
            match self.current().kind.clone() {
                TokenKind::Newline => self.pos += 1,
                TokenKind::Comment(comment) => {
                    if expression_assertion_comment(&comment) {
                        break;
                    }
                    if expression_documentation
                        && comment.kind == CommentKind::Documentation
                        && {
                            let documentation=types::annotations(&comment).0;
                            !types::annotation_spans(&documentation.functions).is_empty()
                                || documentation.declarations.iter().any(|item|matches!(item,types::DeclarationItem::Overload {..}))
                        }
                    {
                        break;
                    }
                    self.pos += 1;
                    if comment.kind == CommentKind::Documentation {
                        let (annotations, diagnostics) = types::annotations(&comment);
                        self.diagnostics.extend(diagnostics);
                        let outer = self.documentation_group.take();
                        let annotations = self.record_documentation(&comment, annotations);
                        if let Some(group) = self.documentation_group.take() {
                            self.finish_documentation_functions(&group);
                        }
                        self.documentation_group = outer;
                        for span in types::annotation_spans(&annotations) {
                            self.diagnostics.push(Diagnostic {
                                origin: crate::DiagnosticOrigin::Annotation,
                                span, message: "annotations inside expressions are unsupported unless attached to a function declaration".to_owned(),
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
        let mut annotations = std::mem::take(&mut self.pending_annotations);
        if self.keyword("function") {
            return self.function_declaration(start, annotations);
        }
        if matches!(&self.current().kind, TokenKind::Keyword(word)
            if matches!(word.as_str(), "if" | "while" | "repeat" | "for" | "break"))
        {
            self.unattached_annotations(&annotations);
            return self.control_statement(start);
        }
        if self.keyword("do") {
            self.unattached_annotations(&annotations);
            self.pos += 1;
            let body_start = self.tokens[self.pos - 1].span.end;
            if self.block_depth + self.depth >= 128 {
                self.error("block nesting exceeds 128");
                self.recover_block();
                return None;
            }
            self.block_depth += 1;
            let mut body = self.statements(&["end"]);
            self.block_depth -= 1;
            let body_end = self.current().span.start;
            let end = self.expect_end();
            if statements_depth(&body) >= 128 {
                self.diagnostic_at(crate::DiagnosticOrigin::LuaSyntax, start.join(end), "block nesting exceeds 128");
                body.clear();
            }
            return Some(Stmt {
                kind: StmtKind::Block {
                    body,
                    body_span: Span::new(body_start, body_end),
                },
                span: start.join(end),
            });
        }
        if self.keyword("return") {
            self.pos += 1;
            let next = &self.tokens[self.significant_index()];
            let empty = matches!(&next.kind, TokenKind::Eof)
                || matches!(&next.kind, TokenKind::Keyword(word) if matches!(word.as_str(), "end" | "elseif" | "else" | "until"))
                || matches!(&next.kind, TokenKind::Symbol(symbol) if symbol == ";");
            let mut values = if empty { Vec::new() } else { self.expr_list() };
            let mut annotations = annotations;
            let function_contract = values.len() == 1
                && function_initializer(&mut values[0]).is_some()
                && annotations.type_annotation.as_ref().is_some_and(function_type_annotation);
            let return_annotation = if function_contract { None } else { annotations.type_annotation.take() };
            self.attach_function_annotations(&mut values, annotations);
            let end = values.last().map_or(start, |value| value.span);
            let statement = start.join(end);
            if let Some(annotation) = return_annotation {
                self.return_annotations.push(crate::ReturnAnnotation { statement, annotation });
            }
            return Some(Stmt {
                kind: StmtKind::Return(values),
                span: statement,
            });
        }
        if self.keyword("local") {
            self.pos += 1;
            if self.tokens[self.significant_index()].kind
                == TokenKind::Keyword("function".to_owned())
            {
                self.inner_trivia();
                let function_start = self.current().span;
                self.pos += 1;
                let Some(name) = self.name() else {
                    self.recover_statement();
                    return None;
                };
                let (function, _) = self.function_body(function_start, annotations);
                let span = start.join(function.span);
                return Some(Stmt {
                    kind: StmtKind::LocalFunction {
                        name,
                        function: Box::new(function),
                    },
                    span,
                });
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
            let mut values = if self.significant_symbol("=") {
                self.inner_trivia();
                self.pos += 1;
                self.expr_list()
            } else {
                Vec::new()
            };
            let value_end = values.last().map_or(names.last().unwrap().span, |value| value.span);
            self.trailing_type_annotation(value_end, &mut annotations);
            let annotation = annotations.type_annotation.take();
            if annotation.as_ref().is_some_and(function_type_annotation)
                && values.len() == 1
                && function_initializer(&mut values[0]).is_some()
            {
                annotations.type_annotation = annotation.clone();
            }
            self.attach_function_annotations(&mut values, annotations);
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
            self.unattached_annotations(&annotations);
            self.error("unexpected or unsupported Lua statement");
            self.recover_statement();
            return None;
        }
        let mut targets = self.expr_list();
        if !self.significant_symbol("=") {
            self.unattached_annotations(&annotations);
            if targets.len() == 1
                && matches!(
                    targets[0].kind,
                    ExprKind::Call { .. } | ExprKind::MethodCall { .. }
                )
            {
                let call = targets.pop().unwrap();
                let span = call.span;
                return Some(Stmt {
                    kind: StmtKind::Call(call),
                    span,
                });
            }
            self.error("expected an assignment or function call; other expression statements are invalid Lua");
            self.recover_statement();
            return None;
        }
        self.inner_trivia();
        self.pos += 1;
        for target in &targets {
            if !matches!(
                target.kind,
                ExprKind::Name(_) | ExprKind::Field { .. } | ExprKind::Index { .. }
            ) {
                self.diagnostic_at(crate::DiagnosticOrigin::LuaSyntax, target.span, "invalid assignment target");
            }
        }
        let mut values = self.expr_list();
        let annotation = annotations.type_annotation.take();
        if annotation.as_ref().is_some_and(function_type_annotation)
            && values.len() == 1
            && function_initializer(&mut values[0]).is_some()
        {
            annotations.type_annotation = annotation.clone();
        }
        self.attach_function_annotations(&mut values, annotations);
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
    fn record_expression_overloads(
        &mut self,
        comment: Span,
        declarations: Vec<types::DeclarationItem>,
        annotations: &mut FunctionAnnotations,
        role: &str,
    ) {
        for declaration in declarations {
            match declaration {
                types::DeclarationItem::Reference(reference) => {
                    self.documentation_references.push(reference);
                }
                types::DeclarationItem::Overload { signature, span, tag_span } => {
                    let id = OverloadDeclarationId(self.overload_annotations.len().try_into().expect("original overload index fits u32"));
                    self.overload_annotations.push(OverloadAnnotation {
                        id, signature, span, tag_span, group_span: comment, named_candidates: Vec::new(),
                    });
                    annotations.overloads.push(OverloadReference { id, span });
                }
                _ => {
                    self.diagnostics.push(Diagnostic {
                        origin: crate::DiagnosticOrigin::Annotation,
                        span: comment,
                        message: format!("named declarations cannot annotate {role}"),
                    });
                }
            }
        }
    }

    fn trailing_type_annotation(&mut self, end: Span, annotations: &mut FunctionAnnotations) {
        while let TokenKind::Comment(comment) = self.current().kind.clone() {
            if comment.kind != CommentKind::Documentation
                || comment.span.start < end.end
                || !self.source[end.end..comment.span.start].chars().all(|ch| matches!(ch, ' ' | '\t'))
            {
                break;
            }
            let (documentation, diagnostics) = types::annotations(&comment);
            if documentation.functions.type_annotation.is_none()
                || types::annotation_spans(&documentation.functions).len() != 1
                || documentation.declarations.iter().any(|item| {
                    !matches!(item, types::DeclarationItem::Reference(_))
                })
                || !documentation.exclusions.is_empty()
            {
                break;
            }
            self.pos += 1;
            self.diagnostics.extend(diagnostics);
            self.documentation_references.extend(
                documentation.declarations.into_iter().filter_map(|item| {
                    if let types::DeclarationItem::Reference(reference) = item {
                        Some(reference)
                    } else {
                        None
                    }
                }),
            );
            types::merge_annotations(annotations, documentation.functions, &mut self.diagnostics);
        }
    }

    fn attach_function_annotations(
        &mut self,
        values: &mut [Expr],
        annotations: FunctionAnnotations,
    ) {
        if types::annotation_spans(&annotations).is_empty() {
            return;
        }
        if let [value] = values {
            if let Some(function) = function_initializer(value) {
                // Documentation before a declaration/field precedes any
                // documentation directly on its initializer in source order.
                let mut combined = annotations;
                types::merge_annotations(
                    &mut combined,
                    std::mem::take(&mut function.annotations),
                    &mut self.diagnostics,
                );
                function.annotations = combined;
                // Field/declaration docs and initializer docs describe the same
                // original function. Keep one declaration site and source order.
                let group=function.annotations.overloads.iter()
                    .map(|reference|self.overload_annotations[reference.id.0 as usize].group_span)
                    .reduce(Span::join);
                if let Some(group)=group {
                    for reference in &function.annotations.overloads {
                        self.overload_annotations[reference.id.0 as usize].group_span=group;
                    }
                }
                return;
            }
        }
        self.unattached_annotations(&annotations);
    }
    fn expect_end(&mut self) -> Span {
        self.expect_keyword("end")
    }
    fn expect_keyword(&mut self, word: &str) -> Span {
        self.inner_trivia();
        if self.keyword(word) {
            let span = self.current().span;
            self.pos += 1;
            span
        } else {
            self.error(format!("expected '{word}' in Lua statement"));
            self.current().span
        }
    }
    fn function_body(
        &mut self,
        start: Span,
        annotations: FunctionAnnotations,
    ) -> (Function, usize) {
        if self.block_depth + self.depth >= 128 {
            self.error("function nesting exceeds 128");
            self.recover_block();
            let end = self.tokens[self.pos.saturating_sub(1)].span;
            return (
                Function {
                    params: Vec::new(),
                    vararg: None,
                    body: Vec::new(),
                    annotations,
                    span: start.join(end),
                    body_span: Span::new(end.end, end.end),
                },
                2,
            );
        }
        self.expect("(");
        let mut params = Vec::new();
        let mut vararg = None;
        self.inner_trivia();
        while !self.eof() && !self.symbol(")") {
            if self.symbol("...") {
                vararg = Some(self.current().span);
                self.pos += 1;
                self.inner_trivia();
                if !self.symbol(")") {
                    self.error("Lua vararg parameter '...' must be the final parameter");
                    while !self.eof() && !self.symbol(")") {
                        self.pos += 1;
                    }
                }
                break;
            }
            let Some(name) = self.name() else {
                self.error("malformed Lua function parameter list");
                while !self.eof() && !self.symbol(")") {
                    self.pos += 1;
                }
                break;
            };
            params.push(name);
            self.inner_trivia();
            if self.take_symbol(",").is_some() {
                self.inner_trivia();
                if self.symbol(")") {
                    self.error("trailing comma in Lua function parameter list");
                }
                continue;
            }
            if !self.symbol(")") {
                self.error("expected ',' or ')' after function parameter");
                while !self.eof() && !self.symbol(")") {
                    self.pos += 1;
                }
            }
            break;
        }
        let close_params = self.expect(")");
        self.block_depth += 1;
        let enclosing_loop_depth = self.loop_depth;
        let enclosing_vararg = self.vararg_allowed;
        self.loop_depth = 0;
        self.vararg_allowed = vararg.is_some();
        let mut body = self.statements(&["end"]);
        self.loop_depth = enclosing_loop_depth;
        self.vararg_allowed = enclosing_vararg;
        self.block_depth -= 1;
        let body_end = self.current().span.start;
        let end = self.expect_end();
        let mut built_depth = statements_depth(&body) + 2;
        if built_depth > 128 {
            self.diagnostic_at(crate::DiagnosticOrigin::LuaSyntax, start.join(end), "function nesting exceeds 128");
            body.clear();
            built_depth = 2;
        }
        (
            Function {
                params,
                vararg,
                body,
                annotations,
                span: start.join(end),
                body_span: Span::new(close_params.end, body_end.max(close_params.end)),
            },
            built_depth,
        )
    }
    fn function_declaration(
        &mut self,
        start: Span,
        annotations: FunctionAnnotations,
    ) -> Option<Stmt> {
        self.pos += 1;
        let Some(root) = self.name() else {
            self.recover_statement();
            return None;
        };
        let mut target = Expr {
            span: root.span,
            kind: ExprKind::Name(root),
        };
        let mut depth = 1;
        let mut exceeded = false;
        let mut method_span = None;
        while self.significant_symbol(".") || self.significant_symbol(":") {
            self.inner_trivia();
            let separator = self.current().span;
            let method = self.symbol(":");
            self.pos += 1;
            let Some(name) = self.name() else {
                self.recover_statement();
                return None;
            };
            let span = target.span.join(name.span);
            if exceeded || depth >= 128 {
                if !exceeded {
                    self.diagnostic_at(crate::DiagnosticOrigin::LuaSyntax, separator.join(name.span), "function name nesting exceeds 128");
                }
                target = Expr {
                    kind: ExprKind::Error,
                    span,
                };
                depth = 1;
                exceeded = true;
            } else {
                target = Expr {
                    kind: ExprKind::Field {
                        base: Box::new(target),
                        name,
                    },
                    span,
                };
                depth += 1;
            }
            if method {
                method_span = Some(separator);
                if self.significant_symbol(".") || self.significant_symbol(":") {
                    self.error("Lua function names allow a single ':' member only at the end");
                    while !self.eof()
                        && !self.significant_symbol("(")
                        && !matches!(self.current().kind, TokenKind::Newline)
                    {
                        self.pos += 1;
                    }
                }
                break;
            }
        }
        let (function, _) = self.function_body(start, annotations);
        let span = start.join(function.span);
        Some(Stmt {
            kind: StmtKind::FunctionDeclaration {
                target: Box::new(target),
                function: Box::new(function),
                method_span,
            },
            span,
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
        self.expr_binding_power(0)
    }
    fn expr_binding_power(&mut self, min_precedence: u8) -> (Expr, usize) {
        let mut assertions = Vec::new();
        let mut function_annotations = FunctionAnnotations::default();
        loop {
            self.expression_trivia();
            let TokenKind::Comment(comment) = self.current().kind.clone() else {
                break;
            };
            if comment.kind != CommentKind::Documentation {
                break;
            }
            self.pos += 1;
            let (mut documentation, diagnostics) = types::annotations(&comment);
            self.diagnostics.extend(diagnostics);
            if let Some(annotation) = documentation.functions.type_annotation.take() {
                assertions.push((
                    crate::ExpressionAssertionKind::Type,
                    annotation,
                    comment.span,
                ));
            }
            for annotation in documentation.exclusions {
                assertions.push((
                    crate::ExpressionAssertionKind::Not,
                    annotation,
                    comment.span,
                ));
            }
            self.record_expression_overloads(comment.span,documentation.declarations,&mut documentation.functions,"an inline expression");
            types::merge_annotations(
                &mut function_annotations,
                documentation.functions,
                &mut self.diagnostics,
            );
        }
        let start = self.current().span;
        if self.depth + self.block_depth >= 128 {
            self.error("expression nesting exceeds 128");
            self.recover_expression();
            return (
                Expr {
                    kind: ExprKind::Error,
                    span: start,
                },
                1,
            );
        }
        self.depth += 1;
        let (mut expr, mut built_depth) = self.prefix_with_suffix();
        let mut binary_exceeded = false;
        loop {
            let next = &self.tokens[self.significant_index()];
            let Some((op, precedence, rhs_precedence)) = binary_operator(&next.kind) else {
                break;
            };
            if precedence < min_precedence {
                break;
            }
            self.inner_trivia();
            let op_span = self.current().span;
            self.pos += 1;
            let (right, right_depth) = self.expr_binding_power(rhs_precedence);
            let span = expr.span.join(right.span);
            if binary_exceeded || built_depth.max(right_depth) >= 128 {
                if !binary_exceeded {
                    self.diagnostic_at(crate::DiagnosticOrigin::LuaSyntax, op_span, "binary expression nesting exceeds 128");
                }
                expr = Expr {
                    kind: ExprKind::Error,
                    span,
                };
                built_depth = 1;
                binary_exceeded = true;
            } else {
                expr = Expr {
                    kind: ExprKind::Binary {
                        op,
                        op_span,
                        left: Box::new(expr),
                        right: Box::new(right),
                    },
                    span,
                };
                built_depth = built_depth.max(right_depth) + 1;
            }
        }
        self.depth -= 1;
        self.attach_function_annotations(std::slice::from_mut(&mut expr), function_annotations);
        for (kind, annotation, comment) in assertions {
            self.expression_assertions.push(crate::ExpressionAssertion {
                kind,
                annotation,
                expression: expr.span,
                span: comment.join(expr.span),
            });
        }
        (expr, built_depth)
    }
    fn prefix_with_suffix(&mut self) -> (Expr, usize) {
        let start = self.current().span;
        let mut built_depth = 1;
        let mut suffix_eligible = false;
        let mut expr = if unary_operator(&self.current().kind).is_some() {
            // A prefix chain is right-nested but can be collected without
            // recursive parser frames. This also bounds adversarial `not` runs.
            let mut operators = Vec::new();
            while let Some(op) = unary_operator(&self.current().kind) {
                operators.push((op, self.current().span));
                self.pos += 1;
                self.expression_trivia();
            }
            // Lua exponentiation binds more tightly than unary operators.
            let (mut value, mut value_depth) = self.expr_binding_power(13);
            let mut unary_exceeded = false;
            for (op, op_span) in operators.into_iter().rev() {
                let span = op_span.join(value.span);
                if unary_exceeded || value_depth >= 128 {
                    if !unary_exceeded {
                        self.diagnostic_at(crate::DiagnosticOrigin::LuaSyntax, op_span, "unary expression nesting exceeds 128");
                    }
                    value = Expr {
                        kind: ExprKind::Error,
                        span,
                    };
                    value_depth = 1;
                    unary_exceeded = true;
                } else {
                    value = Expr {
                        kind: ExprKind::Unary {
                            op,
                            op_span,
                            value: Box::new(value),
                        },
                        span,
                    };
                    value_depth += 1;
                }
            }
            built_depth = value_depth;
            value
        } else {
            match self.current().kind.clone() {
                TokenKind::Identifier(text) => {
                    self.pos += 1;
                    suffix_eligible = true;
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
                TokenKind::Keyword(keyword) if keyword == "function" => {
                    self.pos += 1;
                    let (function, function_depth) =
                        self.function_body(start, FunctionAnnotations::default());
                    built_depth = function_depth;
                    let span = function.span;
                    Expr {
                        kind: ExprKind::Function(Box::new(function)),
                        span,
                    }
                }
                TokenKind::Symbol(symbol) if symbol == "..." => {
                    self.pos += 1;
                    if !self.vararg_allowed {
                        self.diagnostic_at(crate::DiagnosticOrigin::LuaSyntax, start, "'...' is outside a variadic Lua function");
                    }
                    Expr {
                        kind: ExprKind::Vararg,
                        span: start,
                    }
                }
                TokenKind::Symbol(symbol) if symbol == "(" => {
                    self.pos += 1;
                    suffix_eligible = true;
                    let (inner, inner_depth) = self.expr_with_depth();
                    let end = self.expect(")");
                    let span = start.join(end);
                    if inner_depth >= 128 {
                        self.diagnostic_at(crate::DiagnosticOrigin::LuaSyntax, span, "expression nesting exceeds 128");
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
                    if !self.expression_boundary() {
                        self.pos += 1;
                    }
                    Expr {
                        kind: ExprKind::Error,
                        span: start,
                    }
                }
            }
        };
        let mut postfix_exceeded = false;
        while suffix_eligible && (self.significant_symbol(".")
            || self.significant_symbol("(")
            || self.significant_symbol("[")
            || self.significant_symbol(":"))
        {
            self.inner_trivia();
            let postfix_start = self.current().span;
            if self.symbol("[") {
                self.pos += 1;
                let (index, index_depth) = self.expr_with_depth();
                let end = self.expect("]");
                let span = expr.span.join(end);
                if postfix_exceeded || built_depth.max(index_depth) >= 128 {
                    if !postfix_exceeded {
                        self.diagnostic_at(crate::DiagnosticOrigin::LuaSyntax, postfix_start.join(end), "indexed expression nesting exceeds 128");
                    }
                    expr = Expr {
                        kind: ExprKind::Error,
                        span,
                    };
                    built_depth = 1;
                    postfix_exceeded = true;
                } else {
                    expr = Expr {
                        kind: ExprKind::Index {
                            base: Box::new(expr),
                            index: Box::new(index),
                        },
                        span,
                    };
                    built_depth = built_depth.max(index_depth) + 1;
                }
                continue;
            }
            if self.symbol(":") {
                self.pos += 1;
                let Some(name) = self.name() else {
                    break;
                };
                if !self.significant_symbol("(") {
                    self.error("Lua ':' access requires a method call argument list");
                    break;
                }
                let (args, args_span, args_depth) = self.call_args();
                let span = expr.span.join(args_span);
                if postfix_exceeded || built_depth.max(args_depth) >= 128 {
                    if !postfix_exceeded {
                        self.diagnostic_at(crate::DiagnosticOrigin::LuaSyntax, postfix_start.join(args_span), "method call nesting exceeds 128");
                    }
                    expr = Expr {
                        kind: ExprKind::Error,
                        span,
                    };
                    built_depth = 1;
                    postfix_exceeded = true;
                } else {
                    expr = Expr {
                        kind: ExprKind::MethodCall {
                            receiver: Box::new(expr),
                            name,
                            args,
                        },
                        span,
                    };
                    built_depth = built_depth.max(args_depth) + 1;
                }
                continue;
            }
            if self.symbol("(") {
                let (args, args_span, args_depth) = self.call_args();
                let span = expr.span.join(args_span);
                if postfix_exceeded || built_depth.max(args_depth) >= 128 {
                    if !postfix_exceeded {
                        self.diagnostic_at(crate::DiagnosticOrigin::LuaSyntax, args_span, "call expression nesting exceeds 128");
                    }
                    expr = Expr {
                        kind: ExprKind::Error,
                        span,
                    };
                    built_depth = 1;
                    postfix_exceeded = true;
                } else {
                    expr = Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(expr),
                            args,
                        },
                        span,
                    };
                    built_depth = built_depth.max(args_depth) + 1;
                }
                continue;
            }
            self.pos += 1;
            let Some(name) = self.name() else {
                break;
            };
            let span = expr.span.join(name.span);
            if postfix_exceeded || built_depth >= 128 {
                if !postfix_exceeded {
                    self.diagnostic_at(crate::DiagnosticOrigin::LuaSyntax, postfix_start.join(name.span), "expression nesting exceeds 128");
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
        (expr, built_depth)
    }
    fn expression_boundary(&self) -> bool {
        self.eof()
            || matches!(&self.current().kind,
            TokenKind::Symbol(symbol) if matches!(symbol.as_str(), "," | ";" | ")" | "]" | "}"))
            || matches!(&self.current().kind, TokenKind::Keyword(keyword)
                if matches!(keyword.as_str(), "local" | "return" | "do" | "end" | "then"
                    | "else" | "elseif" | "until" | "if" | "while" | "for" | "repeat"))
    }
    fn recover_expression(&mut self) {
        let mut nesting = 0_usize;
        while !self.eof() {
            if nesting == 0
                && (self.expression_boundary() || matches!(self.current().kind, TokenKind::Newline))
            {
                break;
            }
            if self.symbol("(") || self.symbol("[") || self.symbol("{") {
                nesting += 1;
            }
            if self.symbol(")") || self.symbol("]") || self.symbol("}") {
                if nesting == 0 {
                    break;
                }
                nesting -= 1;
            }
            self.pos += 1;
        }
    }
    fn call_args(&mut self) -> (Vec<Expr>, Span, usize) {
        let start = self.expect("(");
        let mut args = Vec::new();
        let mut built_depth = 0;
        self.expression_trivia();
        while !self.eof() && !self.symbol(")") {
            let (arg, arg_depth) = self.expr_with_depth();
            args.push(arg);
            built_depth = built_depth.max(arg_depth);
            self.inner_trivia();
            if self.take_symbol(",").is_some() {
                self.expression_trivia();
                if self.symbol(")") {
                    self.error("trailing comma in Lua function call arguments");
                }
                continue;
            }
            if !self.symbol(")") {
                self.error("expected ',' or ')' after function call argument");
                while !self.eof() && !self.symbol(")") {
                    self.pos += 1;
                }
            }
            break;
        }
        let end = self.expect(")");
        (args, start.join(end), built_depth)
    }
    fn table(&mut self) -> (Expr, usize) {
        let start = self.expect("{");
        let mut fields = Vec::new();
        let mut built_depth = 1;
        let mut sequence_index = 0;
        loop {
            self.expression_trivia();
            if self.eof() || self.symbol("}") {
                break;
            }
            let mut function_annotations = FunctionAnnotations::default();
            while let TokenKind::Comment(comment) = self.current().kind.clone() {
                if expression_assertion_comment(&comment)
                    && !self.table_field_declaration_ahead(&comment)
                {
                    break;
                }
                self.pos += 1;
                let (mut documentation, diagnostics) = types::annotations(&comment);
                self.diagnostics.extend(diagnostics);
                self.record_expression_overloads(comment.span,documentation.declarations,&mut documentation.functions,"a table field");
                types::merge_annotations(
                    &mut function_annotations,
                    documentation.functions,
                    &mut self.diagnostics,
                );
                self.expression_trivia();
            }
            let before = self.pos;
            let field_start = self.current().span;
            let kind = if self.take_symbol("[").is_some() {
                let (mut key, key_depth) = self.expr_with_depth();
                if key_depth >= 128 {
                    self.diagnostic_at(crate::DiagnosticOrigin::LuaSyntax, key.span, "table key nesting exceeds 128");
                    key = Expr {
                        kind: ExprKind::Error,
                        span: key.span,
                    };
                } else {
                    built_depth = built_depth.max(key_depth + 1);
                }
                self.expect("]");
                self.expect("=");
                TableFieldKind::Keyed(Box::new(key))
            } else if matches!(self.current().kind, TokenKind::Identifier(_)) {
                let mut lookahead = self.pos + 1;
                while matches!(
                    self.tokens[lookahead].kind,
                    TokenKind::Newline | TokenKind::Comment(_)
                ) {
                    lookahead += 1;
                }
                if matches!(&self.tokens[lookahead].kind, TokenKind::Symbol(symbol) if symbol == "=")
                {
                    let name = self.name().unwrap();
                    self.expect("=");
                    TableFieldKind::Named(name)
                } else {
                    sequence_index += 1;
                    TableFieldKind::Sequence {
                        index: sequence_index,
                    }
                }
            } else {
                sequence_index += 1;
                TableFieldKind::Sequence {
                    index: sequence_index,
                }
            };
            let (mut value, mut value_depth) = self.expr_with_depth();
            let annotation = function_annotations.type_annotation.take();
            // Share the original head's source span with the actual function
            // body, as local declaration annotations already do. The field
            // retains its contract for storage and initializer checking too.
            if annotation.as_ref().is_some_and(function_type_annotation)
                && function_initializer(&mut value).is_some()
            {
                function_annotations.type_annotation = annotation.clone();
            }
            self.attach_function_annotations(
                std::slice::from_mut(&mut value),
                function_annotations,
            );
            if value_depth >= 128 {
                self.diagnostic_at(crate::DiagnosticOrigin::LuaSyntax, value.span, "expression nesting exceeds 128");
                value = Expr {
                    kind: ExprKind::Error,
                    span: value.span,
                };
                value_depth = 1;
            }
            built_depth = built_depth.max(value_depth + 1);
            let span = field_start.join(value.span);
            fields.push(TableField { kind, value, annotation, span });
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
    fn table_field_declaration_ahead(&self, comment: &Comment) -> bool {
        let documentation = types::annotations(comment).0;
        if documentation.functions.type_annotation.is_none() || !documentation.exclusions.is_empty() {
            return false;
        }
        let next = self.significant_index();
        let mut after = next + 1;
        if matches!(self.tokens[next].kind, TokenKind::Identifier(_)) {
            while matches!(self.tokens[after].kind, TokenKind::Newline | TokenKind::Comment(_)) {
                after += 1;
            }
            if matches!(&self.tokens[after].kind, TokenKind::Symbol(symbol) if symbol == "=") {
                return true;
            }
        }
        if matches!(&self.tokens[next].kind, TokenKind::Symbol(symbol) if symbol == "[") {
            let mut depth = 1usize;
            while after < self.tokens.len() && depth != 0 {
                match &self.tokens[after].kind {
                    TokenKind::Symbol(symbol) if symbol == "[" => depth += 1,
                    TokenKind::Symbol(symbol) if symbol == "]" => depth -= 1,
                    TokenKind::Eof => break,
                    _ => {}
                }
                after += 1;
            }
            while after < self.tokens.len() && matches!(self.tokens[after].kind, TokenKind::Newline | TokenKind::Comment(_)) {
                after += 1;
            }
            if depth == 0 && self.tokens.get(after).is_some_and(|token| matches!(&token.kind, TokenKind::Symbol(symbol) if symbol == "=")) {
                return true;
            }
        }
        // A standalone documentation line also owns a sequence field. Inline
        // expression assertions after '{' or ',' keep their existing meaning.
        let line_start = self.source[..comment.span.start].rfind(['\r', '\n']).map_or(0, |index| index + 1);
        self.source[line_start..comment.span.start].chars().all(|ch| matches!(ch, ' ' | '\t'))
            && self.source[comment.span.end..self.tokens[next].span.start].contains(['\r', '\n'])
    }
    fn recover_statement(&mut self) {
        while !self.eof()
            && !matches!(self.current().kind, TokenKind::Newline)
            && !self.symbol(";")
            && !self.keyword("end")
        {
            self.pos += 1;
        }
    }
    fn recover_block(&mut self) {
        self.recover_block_until("end", true);
    }
    fn recover_block_until(&mut self, terminator: &str, consume_terminator: bool) {
        let mut terminators = vec![terminator];
        while !self.eof() {
            if self.keyword("do") || self.keyword("function") || self.keyword("if") {
                terminators.push("end");
            } else if self.keyword("repeat") {
                terminators.push("until");
            } else if terminators.last().is_some_and(|word| self.keyword(word)) {
                terminators.pop();
                if terminators.is_empty() {
                    if consume_terminator {
                        self.pos += 1;
                    }
                    break;
                }
            }
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

fn unary_operator(kind: &TokenKind) -> Option<UnaryOperator> {
    match kind {
        TokenKind::Keyword(word) if word == "not" => Some(UnaryOperator::Not),
        TokenKind::Symbol(symbol) => match symbol.as_str() {
            "#" => Some(UnaryOperator::Length),
            "-" => Some(UnaryOperator::Negate),
            "~" => Some(UnaryOperator::BitwiseNot),
            _ => None,
        },
        _ => None,
    }
}

/// Inclusive Pratt powers adapted from Lua 5.4's lparser.c priorities.
/// Concat and exponentiation associate right; all other binary operators left.
fn binary_operator(kind: &TokenKind) -> Option<(BinaryOperator, u8, u8)> {
    let (op, precedence, right_associative) = match kind {
        TokenKind::Keyword(word) if word == "or" => (BinaryOperator::Or, 1, false),
        TokenKind::Keyword(word) if word == "and" => (BinaryOperator::And, 2, false),
        TokenKind::Symbol(symbol) => match symbol.as_str() {
            "==" => (BinaryOperator::Equal, 3, false),
            "~=" => (BinaryOperator::NotEqual, 3, false),
            "<" => (BinaryOperator::Less, 3, false),
            "<=" => (BinaryOperator::LessEqual, 3, false),
            ">" => (BinaryOperator::Greater, 3, false),
            ">=" => (BinaryOperator::GreaterEqual, 3, false),
            "|" => (BinaryOperator::BitwiseOr, 4, false),
            "~" => (BinaryOperator::BitwiseXor, 5, false),
            "&" => (BinaryOperator::BitwiseAnd, 6, false),
            "<<" => (BinaryOperator::ShiftLeft, 7, false),
            ">>" => (BinaryOperator::ShiftRight, 7, false),
            ".." => (BinaryOperator::Concat, 9, true),
            "+" => (BinaryOperator::Add, 10, false),
            "-" => (BinaryOperator::Subtract, 10, false),
            "*" => (BinaryOperator::Multiply, 11, false),
            "/" => (BinaryOperator::Divide, 11, false),
            "//" => (BinaryOperator::FloorDivide, 11, false),
            "%" => (BinaryOperator::Modulo, 11, false),
            "^" => (BinaryOperator::Power, 14, true),
            _ => return None,
        },
        _ => return None,
    };
    Some((
        op,
        precedence,
        if right_associative {
            precedence
        } else {
            precedence + 1
        },
    ))
}

fn function_initializer(value: &mut Expr) -> Option<&mut Function> {
    match &mut value.kind {
        ExprKind::Function(function) => Some(function),
        ExprKind::Parenthesized(inner) => function_initializer(inner),
        _ => None,
    }
}

fn function_type_annotation(annotation: &TypeAnnotation) -> bool {
    fn function_type(ty: &TypeExpr) -> bool {
        match &ty.value_syntax().kind {
            TypeExprKind::Function(_) => true,
            TypeExprKind::Parenthesized(inner) => function_type(inner),
            _ => false,
        }
    }
    annotation.types.len() == 1 && function_type(&annotation.types[0])
}

/// Count constructed native statement/expression depth without recursive walks.
/// This keeps function bodies from multiplying independently bounded call chains.
fn statements_depth(body: &[Stmt]) -> usize {
    enum Node<'a> {
        Statement(&'a Stmt),
        Expression(&'a Expr),
    }
    let mut pending = body
        .iter()
        .map(|stmt| (Node::Statement(stmt), 1_usize))
        .collect::<Vec<_>>();
    let mut max_depth = 0;
    while let Some((node, depth)) = pending.pop() {
        max_depth = max_depth.max(depth);
        match node {
            Node::Statement(stmt) => match &stmt.kind {
                StmtKind::Local { values, .. } | StmtKind::Return(values) => {
                    pending.extend(
                        values
                            .iter()
                            .map(|expr| (Node::Expression(expr), depth + 1)),
                    );
                }
                StmtKind::Assign {
                    targets, values, ..
                } => {
                    pending.extend(
                        targets
                            .iter()
                            .chain(values)
                            .map(|expr| (Node::Expression(expr), depth + 1)),
                    );
                }
                StmtKind::Block { body, .. } => {
                    pending.extend(body.iter().map(|stmt| (Node::Statement(stmt), depth + 1)));
                }
                StmtKind::LocalFunction { function, .. } => {
                    pending.extend(
                        function
                            .body
                            .iter()
                            .map(|stmt| (Node::Statement(stmt), depth + 2)),
                    );
                }
                StmtKind::FunctionDeclaration {
                    target, function, ..
                } => {
                    pending.push((Node::Expression(target), depth + 1));
                    pending.extend(
                        function
                            .body
                            .iter()
                            .map(|stmt| (Node::Statement(stmt), depth + 2)),
                    );
                }
                StmtKind::Call(expr) => pending.push((Node::Expression(expr), depth + 1)),
                StmtKind::If {
                    branches,
                    else_branch,
                } => {
                    for branch in branches {
                        pending.push((Node::Expression(&branch.condition), depth + 2));
                        pending.extend(
                            branch
                                .body
                                .iter()
                                .map(|stmt| (Node::Statement(stmt), depth + 2)),
                        );
                    }
                    if let Some(branch) = else_branch {
                        pending.extend(
                            branch
                                .body
                                .iter()
                                .map(|stmt| (Node::Statement(stmt), depth + 2)),
                        );
                    }
                }
                StmtKind::While {
                    condition, body, ..
                }
                | StmtKind::Repeat {
                    condition, body, ..
                } => {
                    pending.push((Node::Expression(condition), depth + 1));
                    pending.extend(body.iter().map(|stmt| (Node::Statement(stmt), depth + 1)));
                }
                StmtKind::NumericFor {
                    start,
                    limit,
                    step,
                    body,
                    ..
                } => {
                    pending.push((Node::Expression(start), depth + 1));
                    pending.push((Node::Expression(limit), depth + 1));
                    if let Some(step) = step {
                        pending.push((Node::Expression(step), depth + 1));
                    }
                    pending.extend(body.iter().map(|stmt| (Node::Statement(stmt), depth + 1)));
                }
                StmtKind::GenericFor { values, body, .. } => {
                    pending.extend(
                        values
                            .iter()
                            .map(|expr| (Node::Expression(expr), depth + 1)),
                    );
                    pending.extend(body.iter().map(|stmt| (Node::Statement(stmt), depth + 1)));
                }
                StmtKind::Break => {}
            },
            Node::Expression(expr) => match &expr.kind {
                ExprKind::Parenthesized(base)
                | ExprKind::Field { base, .. }
                | ExprKind::Unary { value: base, .. } => {
                    pending.push((Node::Expression(base), depth + 1));
                }
                ExprKind::Binary { left, right, .. } => {
                    pending.push((Node::Expression(left), depth + 1));
                    pending.push((Node::Expression(right), depth + 1));
                }
                ExprKind::Index { base, index } => {
                    pending.push((Node::Expression(base), depth + 1));
                    pending.push((Node::Expression(index), depth + 1));
                }
                ExprKind::Call { callee, args } => {
                    pending.push((Node::Expression(callee), depth + 1));
                    pending.extend(args.iter().map(|arg| (Node::Expression(arg), depth + 1)));
                }
                ExprKind::MethodCall { receiver, args, .. } => {
                    pending.push((Node::Expression(receiver), depth + 1));
                    pending.extend(args.iter().map(|arg| (Node::Expression(arg), depth + 1)));
                }
                ExprKind::Table(fields) => {
                    for field in fields {
                        if let TableFieldKind::Keyed(key) = &field.kind {
                            pending.push((Node::Expression(key), depth + 1));
                        }
                        pending.push((Node::Expression(&field.value), depth + 1));
                    }
                }
                ExprKind::Function(function) => {
                    pending.extend(
                        function
                            .body
                            .iter()
                            .map(|stmt| (Node::Statement(stmt), depth + 2)),
                    );
                }
                _ => {}
            },
        }
    }
    max_depth
}
