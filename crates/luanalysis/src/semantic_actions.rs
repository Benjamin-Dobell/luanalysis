//! Editor-independent intentions over the current source revision. Types are demanded from the
//! native project session; no display type is inserted until the annotation parser accepts it.

use crate::incremental::ProjectWorkspace;
use crate::native_functions::project_provider::ProjectRegistry;
use crate::project_check::ProjectCheckOptions;
use luanalysis_engine::{Type, TypeQueries};
use luanalysis_project::{FileId, Project};
use luanalysis_syntax::{CommentKind, Expr, ExprKind, File, Function, Span, Stmt, StmtKind, TokenKind, TypeExpr, TypeExprKind};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub span: Span,
    pub replacement: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub title: String,
    pub kind: &'static str,
    /// Nonoverlapping original-source UTF-8 byte edits, independent of the editor's encoding.
    pub edits: Vec<Edit>,
}

/// Intentions for `range` of `file` in this exact project revision. The LSP adapter must keep
/// its normal stale-query guard and attach the open document version to these edits.
pub fn actions(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    range: Range<usize>,
    deliver: impl FnOnce(Vec<Action>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: false, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        let result = (|| {
            let text = &project.file(file)?.source;
            let syntax = registry.source(file)?.syntax();
            // Recovery trees are useful for completion but cannot prove a rewrite's boundaries.
            if !syntax.diagnostics.is_empty() || range.start > range.end || range.end > text.len()
                || !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end)
            { return None; }
            let mut builder = Builder { text, syntax, registry, file, range, queries, actions: Vec::new() };
            builder.statements(&syntax.statements);
            crate::module_requests::visit_expressions(&syntax.statements, &mut |expression| builder.expression(expression));
            for declaration in &syntax.type_declarations {
                match &declaration.kind {
                    luanalysis_syntax::NamedTypeDeclarationKind::Alias { target } => builder.annotation_type(target),
                    luanalysis_syntax::NamedTypeDeclarationKind::Record { fields, .. } => {
                        for field in fields { builder.annotation_type(&field.ty); }
                    }
                }
            }
            for declaration in &syntax.function_declarations { builder.annotation_function(&declaration.signature); }
            for overload in &syntax.overload_annotations { builder.annotation_function(&overload.signature); }
            builder.actions.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.edits[0].span.start.cmp(&b.edits[0].span.start)));
            builder.actions.dedup();
            Some(builder.actions)
        })().unwrap_or_default();
        deliver(result);
    });
}

struct Builder<'a> {
    text: &'a str,
    syntax: &'a File,
    registry: &'a ProjectRegistry,
    file: FileId,
    range: Range<usize>,
    queries: &'a mut dyn TypeQueries,
    actions: Vec<Action>,
}

impl Builder<'_> {
    fn selected(&self, span: Span) -> bool {
        if self.range.is_empty() { span.start <= self.range.start && self.range.start <= span.end }
        else { span.start < self.range.end && self.range.start < span.end }
    }

    fn push(&mut self, title: &str, edits: Vec<Edit>) {
        if edits.is_empty() { return; }
        let action = Action { title: title.to_owned(), kind: "refactor.rewrite", edits };
        // In particular, removing call parentheses can expose a newline-sensitive call. Reject
        // the edit if the complete current buffer would no longer parse.
        let mut rewritten = self.text.to_owned();
        let mut ordered = action.edits.clone();
        ordered.sort_by_key(|edit| std::cmp::Reverse((edit.span.start, edit.span.end)));
        let mut previous = self.text.len();
        for edit in &ordered {
            if edit.span.end > previous || edit.span.start > edit.span.end { return; }
            rewritten.replace_range(edit.span.start..edit.span.end, &edit.replacement);
            previous = edit.span.start;
        }
        if luanalysis_syntax::parse(&rewritten).diagnostics.is_empty() { self.actions.push(action); }
    }

    fn statements(&mut self, statements: &[Stmt]) {
        for statement in statements {
            match &statement.kind {
                StmtKind::Local { names, values, annotation, .. } => {
                    if annotation.is_none() && !values.is_empty() && self.selected(statement.span)
                        && !values.iter().any(|value| matches!(value.kind, ExprKind::Function(_) | ExprKind::Nil))
                    {
                        let source = self.registry.source(self.file).unwrap();
                        let types: Option<Vec<_>> = names.iter().map(|name| {
                            let binding = source.bound().binding_at(name.span.start)?;
                            if source.binding_has_written_contract(binding) { return None; }
                            let resolved = source.resolve_binding(binding, self.queries)?;
                            if resolved.evolves_from_nil { return None; }
                            let ty = resolved.constraint;
                            let text = crate::describe_native(&ty, self.queries, &mut Vec::new());
                            valid_type(&text).then_some(text)
                        }).collect();
                        if let Some(types) = types {
                            self.insert_annotations("Create type annotation", statement.span.start, &[format!("---@type {}", types.join(", "))]);
                        }
                    }
                    if let Some(annotation) = annotation { for ty in &annotation.types { self.annotation_type(ty); } }
                    if let ([name], [value]) = (names.as_slice(), values.as_slice())
                        && let ExprKind::Function(function) = &value.kind
                    { self.function(statement.span.start, &name.text, function, false); }
                }
                StmtKind::LocalFunction { name, function } => self.function(statement.span.start, &name.text, function, false),
                StmtKind::FunctionDeclaration { target, function, method_span } => {
                    let name = self.text.get(target.span.start..target.span.end).unwrap_or_default();
                    self.function(statement.span.start, name, function, method_span.is_some());
                }
                StmtKind::Assign { annotation: Some(annotation), .. } => for ty in &annotation.types { self.annotation_type(ty); },
                _ => {}
            }
            match &statement.kind {
                StmtKind::LocalFunction { function, .. } | StmtKind::FunctionDeclaration { function, .. } => self.statements(&function.body),
                StmtKind::Block { body, .. } | StmtKind::While { body, .. } | StmtKind::Repeat { body, .. }
                    | StmtKind::NumericFor { body, .. } | StmtKind::GenericFor { body, .. } => self.statements(body),
                StmtKind::If { branches, else_branch } => {
                    for branch in branches { self.statements(&branch.body); }
                    if let Some(branch) = else_branch { self.statements(&branch.body); }
                }
                _ => {}
            }
        }
    }

    fn function(&mut self, start: usize, name: &str, function: &Function, method: bool) {
        let header = Span::new(start, function.body_span.start);
        let source = self.registry.source(self.file).unwrap();
        if self.selected(header) && let Some(key) = source.function_key_at(function.span) {
            let ty = Type::NativeFunction(key);
            let signature = self.queries.native_function_signature(key);
            let hidden = usize::from(source.bound().implicit_receiver(function.span).is_some());
            let params: Vec<Option<String>> = function.params.iter().enumerate().map(|(index, name)| {
                let parameter = signature.params.get(index + hidden)?;
                let ty = crate::describe_native(&parameter.ty, self.queries, &mut Vec::new());
                valid_type(&ty).then(|| format!("---@param {}{} {ty}", name.text, if parameter.optional { "?" } else { "" }))
            }).collect();
            let returns = crate::describe_signature(&ty, self.queries).map(|(_, returns)| returns).filter(|ty| valid_return(ty));
            let rest = crate::describe_signature(&ty, self.queries).and_then(|(parameters, _)| {
                parameters.into_iter().find_map(|parameter| parameter.strip_prefix("...: ").map(str::to_owned))
            }).filter(|ty| valid_type(ty));
            let doc = documentation_span_above(self.syntax, self.text, start);
            if doc.is_none() && function.annotations.params.is_empty() && function.annotations.returns.is_none()
                && function.annotations.type_annotation.is_none()
                && let Some(mut lines) = params.iter().cloned().collect::<Option<Vec<_>>>()
            {
                lines.insert(0, format!("--- {name}"));
                if function.vararg.is_some() { lines.push(format!("---@param ... {}", rest.as_deref().unwrap_or("any"))); }
                if let Some(returns) = &returns { lines.push(format!("---@return {returns}")); }
                self.insert_annotations("Create LuaDoc", start, &lines);
            }
            for (index, parameter) in function.params.iter().enumerate() {
                if self.selected(parameter.span) && !function.annotations.params.iter().any(|tag| tag.name.text == parameter.text)
                    && function.annotations.type_annotation.is_none()
                    && let Some(line) = &params[index]
                { self.insert_annotations("Create parameter annotation", start, std::slice::from_ref(line)); }
                if method && self.selected(parameter.span) && parameter.text != "self"
                    && !function.params.iter().any(|parameter| parameter.text == "self")
                { self.field_assignment(start, function, &parameter.text); }
            }
            if let Some(vararg) = function.vararg && self.selected(vararg)
                && function.annotations.vararg.is_none()
                && !function.annotations.params.iter().any(|tag| tag.name.text == "...")
            { self.insert_annotations("Create parameter annotation", start, &[format!("---@param ... {}", rest.as_deref().unwrap_or("any"))]); }
            if function.annotations.returns.is_none() && function.annotations.type_annotation.is_none()
                && let Some(returns) = returns
            { self.insert_annotations("Create return annotation", start, &[format!("---@return {returns}")]); }
        }
        for tag in &function.annotations.params {
            if !tag.optional && self.selected(tag.span) {
                self.push("Make parameter optional", vec![Edit { span: Span::new(tag.name.span.end, tag.name.span.end), replacement: "?".to_owned() }]);
            }
            self.annotation_type(&tag.ty);
        }
        if let Some(annotation) = &function.annotations.returns { for ty in &annotation.types { self.annotation_type(ty); } }
        if let Some(annotation) = &function.annotations.type_annotation { for ty in &annotation.types { self.annotation_type(ty); } }
    }

    fn field_assignment(&mut self, start: usize, function: &Function, name: &str) {
        let mut exists = false;
        crate::module_requests::visit_expressions(&function.body, &mut |expression| {
            if let ExprKind::Field { base, name: field } = &expression.kind
                && matches!(&base.kind, ExprKind::Name(base) if base.text == "self") && field.text == name
            { exists = true; }
        });
        if exists { return; }
        let Some((_, indent)) = declaration_line(self.text, start) else { return };
        let newline = newline(self.text);
        let body_indent = function.body.first().and_then(|statement| declaration_line(self.text, statement.span.start))
            .map(|(_, indent)| indent).filter(|body_indent| body_indent.len() > indent.len())
            .map(str::to_owned).unwrap_or_else(|| format!("{indent}{}", if indent.contains('\t') { "\t" } else { "    " }));
        self.push("Create field for parameter", vec![Edit {
            span: Span::new(function.body_span.start, function.body_span.start),
            replacement: format!("{newline}{body_indent}self.{name} = {name}{newline}{indent}"),
        }]);
    }

    fn insert_annotations(&mut self, title: &str, start: usize, lines: &[String]) {
        let Some((line_start, indent)) = declaration_line(self.text, start) else { return };
        let newline = newline(self.text);
        let replacement = lines.iter().map(|line| format!("{indent}{line}{newline}")).collect();
        self.push(title, vec![Edit { span: Span::new(line_start, line_start), replacement }]);
    }

    fn expression(&mut self, expression: &Expr) {
        if let ExprKind::Function(function) = &expression.kind {
            self.statements(&function.body);
            // Anonymous callbacks cannot own a standalone doc block; their existing annotation
            // types still offer optional-parameter intentions.
            for tag in &function.annotations.params { self.annotation_type(&tag.ty); }
        }
        if !self.selected(expression.span) { return; }
        match &expression.kind {
            ExprKind::Binary { .. } if pure_logical_constant(expression) => self.constant_value(expression),
            ExprKind::Boolean(value) => self.push("Invert boolean value", vec![Edit {
                span: expression.span, replacement: (!value).to_string(),
            }]),
            ExprKind::Call { callee, args } => self.call_parentheses(expression.span, callee.span.end, args),
            ExprKind::MethodCall { name, args, .. } => self.call_parentheses(expression.span, name.span.end, args),
            _ => {}
        }
    }

    fn constant_value(&mut self, expression: &Expr) {
        let lexed = luanalysis_syntax::lex(&self.text[expression.span.start..expression.span.end]);
        if lexed.tokens.iter().any(|token| matches!(token.kind, TokenKind::Comment(_))) { return; }
        let source = self.registry.source(self.file).unwrap();
        let ty = source.source_check_expression(source.expression_key_for_span(expression.span), None, luanalysis_engine::NativeCheckMode::empty(), self.queries);
        let replacement = match self.queries.inspect_type(&ty) {
            Type::Nil => "nil".to_owned(),
            Type::BooleanLiteral(value) => value.to_string(),
            Type::StringLiteral(value) => {
                // Decimal byte escapes work in every supported Lua version, unlike hexadecimal
                // escapes, and exactly three digits cannot consume a following decimal digit.
                let mut quoted = String::from("\"");
                for byte in value {
                    match byte {
                        b'"' => quoted.push_str("\\\""),
                        b'\\' => quoted.push_str("\\\\"),
                        32..=126 => quoted.push(byte as char),
                        _ => quoted.push_str(&format!("\\{byte:03}")),
                    }
                }
                quoted.push('"');
                quoted
            }
            // The native number domain does not retain Lua's integer/float representation.
            // Do not manufacture a potentially different numeral from a displayed f64 type.
            _ => return,
        };
        self.push("Compute constant value", vec![Edit { span: expression.span, replacement }]);
    }

    fn call_parentheses(&mut self, call: Span, callee_end: usize, args: &[Expr]) {
        let [argument] = args else { return };
        if !matches!(argument.kind, ExprKind::String(_) | ExprKind::Table(_)) { return; }
        let lexed = luanalysis_syntax::lex(&self.text[callee_end..call.end]);
        let tokens: Vec<_> = lexed.tokens.iter().filter(|token| !matches!(token.kind, TokenKind::Comment(_) | TokenKind::Newline | TokenKind::Eof)).collect();
        let parenthesized = tokens.first().is_some_and(|token| matches!(&token.kind, TokenKind::Symbol(symbol) if symbol == "("));
        if parenthesized {
            let Some(close) = tokens.last().filter(|token| matches!(&token.kind, TokenKind::Symbol(symbol) if symbol == ")")) else { return };
            let open = tokens[0];
            self.push("Remove call parentheses", vec![
                Edit { span: Span::new(callee_end + open.span.start, callee_end + open.span.end), replacement: " ".to_owned() },
                Edit { span: Span::new(callee_end + close.span.start, callee_end + close.span.end), replacement: String::new() },
            ]);
        } else {
            self.push("Add call parentheses", vec![
                Edit { span: Span::new(argument.span.start, argument.span.start), replacement: "(".to_owned() },
                Edit { span: Span::new(argument.span.end, argument.span.end), replacement: ")".to_owned() },
            ]);
        }
    }

    fn annotation_function(&mut self, function: &luanalysis_syntax::FunctionType) {
        for parameter in &function.params {
            if !parameter.optional && parameter.ty.is_some() && self.selected(parameter.span) {
                self.push("Make parameter optional", vec![Edit { span: Span::new(parameter.name.span.end, parameter.name.span.end), replacement: "?".to_owned() }]);
            }
            if let Some(ty) = &parameter.ty { self.annotation_type(ty); }
        }
        for ty in &function.returns { self.annotation_type(ty); }
    }

    fn annotation_type(&mut self, ty: &TypeExpr) {
        match &ty.kind {
            TypeExprKind::Union(parts) | TypeExprKind::Intersection(parts) => for part in parts { self.annotation_type(part); },
            TypeExprKind::Record(fields) => for field in fields { self.annotation_type(&field.ty); },
            TypeExprKind::Parenthesized(ty) | TypeExprKind::Array { element: ty, .. }
                | TypeExprKind::TypePredicate { ty, .. } => self.annotation_type(ty),
            TypeExprKind::Function(function) => self.annotation_function(function),
            TypeExprKind::Generic { args, .. } => for arg in args { self.annotation_type(arg); },
            _ => {}
        }
    }
}

/// The existing native checker proves the value; this grammar only excludes runtime effects,
/// metamethods and operations that may raise even when their inferred result is a literal.
fn pure_logical_constant(expression: &Expr) -> bool {
    match &expression.kind {
        ExprKind::Nil | ExprKind::Boolean(_) | ExprKind::Number(_) | ExprKind::String(_) => true,
        ExprKind::Parenthesized(inner) => pure_logical_constant(inner),
        ExprKind::Unary { op: luanalysis_syntax::UnaryOperator::Not, value, .. } => pure_logical_constant(value),
        ExprKind::Binary { op: luanalysis_syntax::BinaryOperator::And | luanalysis_syntax::BinaryOperator::Or, left, right, .. } => {
            pure_logical_constant(left) && pure_logical_constant(right)
        }
        _ => false,
    }
}

fn valid_type(ty: &str) -> bool {
    if ty.is_empty() || ty.contains(['\r', '\n']) || ty.len() > 4096 { return false; }
    let file = luanalysis_syntax::parse(&format!("---@type {ty}\nlocal __generated\n"));
    file.diagnostics.is_empty() && matches!(file.statements.as_slice(), [Stmt { kind: StmtKind::Local { annotation: Some(annotation), .. }, .. }] if annotation.types.len() == 1 && annotation.packs.is_none() && annotation.types[0].span == Span::new("---@type ".len(), "---@type ".len() + ty.len()))
}

fn valid_return(ty: &str) -> bool {
    if ty.is_empty() || ty.contains(['\r', '\n']) || ty.len() > 4096 || !valid_type(&format!("fun(): {ty}")) { return false; }
    let file = luanalysis_syntax::parse(&format!("---@return {ty}\nlocal function __generated() end\n"));
    file.diagnostics.is_empty() && matches!(file.statements.as_slice(), [Stmt { kind: StmtKind::LocalFunction { function, .. }, .. }] if function.annotations.returns.is_some())
}

fn declaration_line(text: &str, start: usize) -> Option<(usize, &str)> {
    let line = text.get(..start)?.rfind(['\r', '\n']).map_or(0, |newline| newline + 1);
    let indent = &text[line..start];
    indent.chars().all(|ch| ch == ' ' || ch == '\t').then_some((line, indent))
}

fn newline(text: &str) -> &'static str {
    match text.find(['\r', '\n']).map(|start| &text[start..]) {
        Some(tail) if tail.starts_with("\r\n") => "\r\n",
        Some(tail) if tail.starts_with('\r') => "\r",
        _ => "\n",
    }
}

fn documentation_span_above(file: &File, text: &str, start: usize) -> Option<Span> {
    let (line, _) = declaration_line(text, start)?;
    file.comments.iter().rev().find(|comment| {
        comment.kind == CommentKind::Documentation && comment.span.end <= line
            && text.get(comment.span.end..line).is_some_and(|gap| gap.trim().is_empty() && gap.replace("\r\n", "\n").replace('\r', "\n").matches('\n').count() <= 1)
    }).map(|comment| comment.span)
}

#[cfg(test)]
#[test]
fn generated_annotations_must_consume_the_entire_native_description() {
    assert!(valid_type("fun(value: number): (number, string)"));
    assert!(valid_return("(number, string)"));
    for text in ["contextual type", "type parameter", "any result pack", "<recursive type>"] {
        assert!(!valid_type(text), "{text}");
        assert!(!valid_return(text), "{text}");
    }
}
