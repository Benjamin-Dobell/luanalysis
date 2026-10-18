//! What the language server asks beyond checking. As tsgo answers a completion from its checker
//! on the current program, a request runs one native session over the project with the editor's
//! text and evaluates only what the answer needs.
//!
//! An editor's text is usually incomplete where it is being typed (`player.`), and only a source
//! without syntax errors is bound and checked. The word at the cursor is therefore replaced with a
//! placeholder name, and if that does not parse, the text is repaired around it: a call is added
//! (`player.placeholder()` is a statement), and failing that, the text after the cursor is dropped
//! and the open brackets and blocks before it are closed.

use crate::incremental::ProjectWorkspace;
use crate::project_check::ProjectCheckOptions;
use crate::{bind, describe_native};
use luanalysis_engine::{NativeCheckMode, Type};
use luanalysis_project::{FileId, Project};
use luanalysis_syntax::{CommentKind, Documentation, Expr, ExprKind, Span, TokenKind};
use std::ops::Range;

/// The name that stands for the word being completed.
pub const PLACEHOLDER: &str = "__luanalysis_completion__";

const KEYWORDS: [&str; 22] = [
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionContext {
    /// A name: locals in scope, globals and keywords.
    Name,
    /// A member after `.`, or after `:` when `method` is set.
    Member { method: bool },
}

/// Where a completion was requested, and the text to check in its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionSite {
    pub context: CompletionContext,
    /// The editor's text with the word at the cursor replaced by `PLACEHOLDER`, repaired to parse.
    pub text: String,
    /// `PLACEHOLDER` in `text`.
    pub placeholder: Span,
    /// The word at the cursor in the editor's text, which a completion replaces.
    pub word: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Variable,
    Function,
    Field,
    Method,
    Keyword,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    /// The item's type, when known.
    pub detail: Option<String>,
    /// What the item's declaration documents, as Markdown.
    pub documentation: Option<String>,
}

/// The completion site at a byte offset of `text`, or `None` where nothing is completed: inside
/// a comment, a string or a number, at a name being declared, or where no repair parses.
pub fn completion_site(text: &str, offset: usize) -> Option<CompletionSite> {
    let offset = floor_char_boundary(text, offset);
    let bytes = text.as_bytes();
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut start = offset;
    while start > 0 && is_word(bytes[start - 1]) { start -= 1; }
    let mut end = offset;
    while end < bytes.len() && is_word(bytes[end]) { end += 1; }
    if bytes.get(start).is_some_and(u8::is_ascii_digit) { return None; }
    let tokens = luanalysis_syntax::lex(text).tokens;
    for token in &tokens {
        let inside = match &token.kind {
            TokenKind::Comment(comment) => token.span.start < offset
                && (offset < token.span.end || offset == token.span.end && comment.kind != CommentKind::Long),
            TokenKind::String(_) | TokenKind::Number(_) | TokenKind::CompletionHint(_) => token.span.start < offset && offset < token.span.end,
            _ => false,
        };
        if inside { return None; }
    }
    // The last significant token before the word decides the context.
    let previous = tokens.iter()
        .filter(|token| token.span.end <= start && !matches!(token.kind, TokenKind::Newline | TokenKind::Comment(_) | TokenKind::Eof))
        .next_back();
    let context = match previous.map(|token| &token.kind) {
        Some(TokenKind::Symbol(symbol)) if symbol == "." => CompletionContext::Member { method: false },
        Some(TokenKind::Symbol(symbol)) if symbol == ":" => CompletionContext::Member { method: true },
        // A name being declared is not completed.
        Some(TokenKind::Keyword(keyword)) if keyword == "local" || keyword == "function" || keyword == "for" => return None,
        _ => CompletionContext::Name,
    };
    let head = &text[..start];
    let tail = &text[end..];
    let mut candidates = vec![
        format!("{head}{PLACEHOLDER}{tail}"),
        format!("{head}{PLACEHOLDER}(){tail}"),
    ];
    for call in ["", "()"] {
        let prefix = format!("{head}{PLACEHOLDER}{call}");
        let closers = closers(&prefix);
        candidates.push(prefix + &closers);
    }
    let text = candidates.into_iter().find(|candidate| !luanalysis_syntax::parse(candidate).has_lua_syntax_errors())?;
    Some(CompletionSite {
        context,
        text,
        placeholder: Span { start, end: start + PLACEHOLDER.len() },
        word: start..end,
    })
}

fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) { offset -= 1; }
    offset
}

/// What closes the brackets and blocks still open at the end of `prefix`.
fn closers(prefix: &str) -> String {
    #[derive(Clone, Copy, PartialEq)]
    enum Open { Paren, Brace, Bracket, Block, Repeat, Condition, LoopHead }
    let mut open = Vec::new();
    for token in luanalysis_syntax::lex(prefix).tokens {
        match &token.kind {
            TokenKind::Keyword(keyword) => match keyword.as_str() {
                "function" => open.push(Open::Block),
                "do" => {
                    if open.last() == Some(&Open::LoopHead) { open.pop(); }
                    open.push(Open::Block);
                }
                "then" => {
                    if open.last() == Some(&Open::Condition) { open.pop(); }
                    open.push(Open::Block);
                }
                "if" => open.push(Open::Condition),
                "elseif" => {
                    if open.last() == Some(&Open::Block) { open.pop(); }
                    open.push(Open::Condition);
                }
                "while" | "for" => open.push(Open::LoopHead),
                "repeat" => open.push(Open::Repeat),
                "until" | "end" => { open.pop(); }
                _ => {}
            },
            TokenKind::Symbol(symbol) => match symbol.as_str() {
                "(" => open.push(Open::Paren),
                "{" => open.push(Open::Brace),
                "[" => open.push(Open::Bracket),
                ")" | "}" | "]" => { open.pop(); }
                _ => {}
            },
            _ => {}
        }
    }
    open.iter().rev().map(|open| match open {
        Open::Paren => ")",
        Open::Brace => "}",
        Open::Bracket => "]",
        Open::Block => "\nend",
        Open::Repeat => "\nuntil true",
        Open::Condition => " then end",
        Open::LoopHead => " do end",
    }).collect()
}

/// The completions at `site` in `file`, whose text in `project` is `site.text`. `deliver`
/// receives them as soon as they are known, before the session's state is dropped.
pub fn complete(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    site: &CompletionSite,
    deliver: impl FnOnce(Vec<CompletionItem>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: true, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        deliver(completions(project, registry, file, site, queries));
    });
}

fn completions(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    site: &CompletionSite,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<CompletionItem> {
    let Some(source) = registry.source(file) else { return Vec::new() };
    registry.prepare_documentation_members(queries);
    let mut items = Vec::new();
    match site.context {
        CompletionContext::Member { method } => {
            let mut receiver = None;
            crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| match &expression.kind {
                ExprKind::Field { base, name } | ExprKind::MethodCall { receiver: base, name, .. } if name.span == site.placeholder => {
                    receiver = Some(base.span);
                }
                _ => {}
            });
            let Some(receiver) = receiver else { return items };
            let (receiver_type, members) = receiver_members(registry, file, receiver, queries);
            for (name, value) in members {
                if !is_identifier(&name) || items.iter().any(|item: &CompletionItem| item.label == name) { continue; }
                let callable = matches!(value, Type::Any) || !queries.call_signature_types(&value).is_empty();
                if method && !callable { continue; }
                let kind = match (callable, method) {
                    (false, _) => CompletionKind::Field,
                    (true, true) => CompletionKind::Method,
                    (true, false) => CompletionKind::Function,
                };
                let documentation = receiver_type.as_ref()
                    .and_then(|receiver| member_documentation(project, registry, receiver, &name, &value, queries))
                    .map(|documentation| documentation_markdown(&documentation, true));
                items.push(CompletionItem { label: name, kind, detail: Some(describe_native(&value, queries, &mut Vec::new())), documentation });
            }
        }
        CompletionContext::Name => {
            // Names need no check: a local's detail is its declared type.
            let bound = source.bound();
            for binding in visible_bindings(bound, site.placeholder.start) {
                let declaration = bound.declaration(binding);
                let detail = source.resolve_binding(binding, queries)
                    .map(|binding| describe_native(&binding.constraint, queries, &mut Vec::new()));
                let kind = match declaration.kind {
                    bind::DeclarationKind::LocalFunction => CompletionKind::Function,
                    _ => CompletionKind::Variable,
                };
                let documentation = binding_documentation(project, registry, file, binding)
                    .map(|documentation| documentation_markdown(&documentation, true));
                items.push(CompletionItem { label: declaration.name.clone(), kind, detail, documentation });
            }
            if let Some(environment) = source.global_environment()
                && let Ok(environment) = queries.global_environment_type(environment)
            {
                for (name, value) in queries.type_members(&environment) {
                    if !is_identifier(&name) || items.iter().any(|item| item.label == name) { continue; }
                    let kind = if queries.call_signature_types(&value).is_empty() { CompletionKind::Variable } else { CompletionKind::Function };
                    let documentation = value_documentation(project, registry, &value, queries)
                        .map(|documentation| documentation_markdown(&documentation, true));
                    items.push(CompletionItem { label: name, kind, detail: Some(describe_native(&value, queries, &mut Vec::new())), documentation });
                }
            }
            items.extend(KEYWORDS.iter().map(|keyword| CompletionItem { label: (*keyword).to_owned(), kind: CompletionKind::Keyword, detail: None, documentation: None }));
        }
    }
    items
}

/// The members of the receiver at `span`. Asked alone, the receiver has its type at its flow
/// position, as tsgo asks `getTypeAtLocation`. Where that names no member, such as a callback's
/// parameter outside the call that types it, the file is checked so that the receiver has its
/// type in each frame it is checked in.
fn receiver_members(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    span: Span,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> (Option<Type>, Vec<(String, Type)>) {
    let Some(source) = registry.source(file) else { return (None, Vec::new()) };
    let alone = source.source_check_expression(source.expression_key_for_span(span), None, NativeCheckMode::empty(), queries);
    let alone = queries.exclude_type(&alone, &Type::Nil);
    let found = queries.type_members(&alone);
    if !found.is_empty() { return (Some(alone), found); }
    source.probe_expression(span);
    registry.check_owned_roots(queries, &[file]);
    let mut receiver = None;
    let mut found: Vec<(String, Type)> = Vec::new();
    for ty in source.probed_types() {
        let ty = queries.exclude_type(&ty, &Type::Nil);
        for (name, value) in queries.type_members(&ty) {
            if !found.iter().any(|(other, _)| *other == name) { found.push((name, value)); }
        }
        receiver.get_or_insert(ty);
    }
    (receiver, found)
}

/// The locals in scope at `offset`, innermost first; a shadowed name appears once.
fn visible_bindings(bound: &bind::BoundFile, offset: usize) -> Vec<bind::BindingId> {
    let mut visible: Vec<bind::BindingId> = Vec::new();
    let mut scope = bound.scope_at(offset);
    while let Some(current) = scope {
        let node = bound.scope(current);
        for &binding in node.bindings.iter().rev() {
            let declaration = bound.declaration(binding);
            if declaration.activation > offset || declaration.name == PLACEHOLDER { continue; }
            if visible.iter().any(|&other| bound.declaration(other).name == declaration.name) { continue; }
            visible.push(binding);
        }
        scope = node.parent;
    }
    visible
}

fn is_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && !KEYWORDS.contains(&name)
        && name != PLACEHOLDER
}

/// Where signature help was requested: in the argument list of a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureSite {
    /// The editor's text, repaired to parse if it does not.
    pub text: String,
    /// The call's `(`, at the same offset in the editor's text and in `text`.
    pub open: usize,
    /// The argument the cursor is in, counted from 0.
    pub argument: usize,
}

/// A signature of the callee, with each parameter's place in its label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureInformation {
    /// `name(a: number, b?: string): boolean`.
    pub label: String,
    /// Each parameter's byte range in `label`.
    pub parameters: Vec<Range<usize>>,
    /// The parameter the cursor's argument is for; a rest parameter takes every argument after
    /// the others.
    pub active_parameter: usize,
    /// The function's description and results, as Markdown.
    pub documentation: Option<String>,
    /// Each parameter's description.
    pub parameter_documentation: Vec<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureHelp {
    pub signatures: Vec<SignatureInformation>,
    /// The overload that the arguments before the cursor select.
    pub active_signature: usize,
    /// The parameter of the active signature that the cursor's argument is for.
    pub active_parameter: usize,
}

/// The call whose arguments surround a byte offset of `text`, or `None` outside one, in a
/// comment or a string, or in a function's parameter list.
pub fn signature_site(text: &str, offset: usize) -> Option<SignatureSite> {
    let offset = floor_char_boundary(text, offset);
    let head = &text[..offset];
    let tokens = luanalysis_syntax::lex(head).tokens;
    // A comment or a string still open at the cursor is a token that ends there, unclosed.
    let inside = tokens.iter().any(|token| token.span.end == offset && match &token.kind {
        TokenKind::Comment(comment) => comment.kind != CommentKind::Long || !head[token.span.start..].ends_with(']'),
        TokenKind::String(_) => !closed_string(&head[token.span.start..]),
        _ => false,
    });
    if inside { return None; }
    let significant: Vec<_> = tokens.iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::Comment(_) | TokenKind::Eof))
        .collect();
    // Open brackets, each with the commas directly inside it.
    let mut open: Vec<(usize, usize)> = Vec::new();
    for (index, token) in significant.iter().enumerate() {
        let TokenKind::Symbol(symbol) = &token.kind else { continue };
        match symbol.as_str() {
            "(" | "{" | "[" => open.push((index, 0)),
            ")" | "}" | "]" => { open.pop(); }
            "," => if let Some((_, commas)) = open.last_mut() { *commas += 1; },
            _ => {}
        }
    }
    let &(index, argument) = open.iter().rev().find(|&&(index, _)| {
        matches!(&significant[index].kind, TokenKind::Symbol(symbol) if symbol == "(") && is_call_paren(&significant, index)
    })?;
    let paren = significant[index].span.start;
    let tail = &text[offset..];
    let mut candidates = vec![text.to_owned(), format!("{head}{PLACEHOLDER}{tail}")];
    for argument in ["", PLACEHOLDER] {
        let prefix = format!("{head}{argument}");
        let closers = closers(&prefix);
        candidates.push(prefix + &closers);
    }
    let text = candidates.into_iter().find(|candidate| !luanalysis_syntax::parse(candidate).has_lua_syntax_errors())?;
    Some(SignatureSite { text, open: paren, argument })
}

/// Whether a string token is closed, as `"text"` is and `"text` is not.
fn closed_string(token: &str) -> bool {
    let bytes = token.as_bytes();
    match bytes.first() {
        Some(&quote @ (b'"' | b'\'')) => {
            bytes.len() >= 2 && bytes[bytes.len() - 1] == quote
                && bytes[1..bytes.len() - 1].iter().rev().take_while(|&&byte| byte == b'\\').count() % 2 == 0
        }
        _ => token.ends_with(']'),
    }
}

/// Whether the `(` at `index` opens a call's arguments, not a parenthesized expression or a
/// function's parameters.
fn is_call_paren(tokens: &[&luanalysis_syntax::Token], index: usize) -> bool {
    let Some(previous) = index.checked_sub(1).map(|index| &tokens[index].kind) else { return false };
    let callee = match previous {
        TokenKind::Identifier(_) | TokenKind::String(_) => true,
        TokenKind::Symbol(symbol) => symbol == ")" || symbol == "]" || symbol == "}",
        _ => false,
    };
    if !callee { return false; }
    // `function name.path:method(` and `function(` declare parameters.
    let mut cursor = index;
    while cursor > 0 && matches!(&tokens[cursor - 1].kind, TokenKind::Identifier(_))
        || cursor > 1 && matches!(&tokens[cursor - 1].kind, TokenKind::Symbol(symbol) if symbol == "." || symbol == ":")
            && matches!(&tokens[cursor - 2].kind, TokenKind::Identifier(_))
    {
        cursor -= 1;
    }
    !(cursor > 0 && matches!(&tokens[cursor - 1].kind, TokenKind::Keyword(keyword) if keyword == "function"))
}

/// Signature help for the call at `site` in `file`, whose text in `project` is `site.text`.
pub fn signature_help(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    site: &SignatureSite,
    deliver: impl FnOnce(Option<SignatureHelp>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: true, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        deliver(signatures(project, registry, file, site, queries));
    });
}

fn signatures(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    site: &SignatureSite,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<SignatureHelp> {
    let source = registry.source(file)?;
    registry.prepare_documentation_members(queries);
    let text = &site.text;
    let opens_at = |end: usize| text.get(end..site.open).is_some_and(|between| between.trim().is_empty());
    // The innermost call whose arguments open at the site's `(`.
    let mut call: Option<(Span, Expr)> = None;
    crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| {
        let found = match &expression.kind {
            ExprKind::Call { callee, .. } => opens_at(callee.span.end),
            ExprKind::MethodCall { name, .. } => opens_at(name.span.end),
            _ => false,
        };
        if found && call.as_ref().is_none_or(|(span, _)| expression.span.end - expression.span.start < span.end - span.start) {
            call = Some((expression.span, expression.clone()));
        }
    });
    let (_, call) = call?;
    let (callee, name, receiver, arguments) = match &call.kind {
        ExprKind::Call { callee, args } => (expression_type(registry, file, callee.span, queries)?, &text[callee.span.start..callee.span.end], None, args),
        ExprKind::MethodCall { receiver, name, args } => {
            let receiver_type = expression_type(registry, file, receiver.span, queries)?;
            let receiver_type = queries.exclude_type(&receiver_type, &Type::Nil);
            let member = queries.type_members(&receiver_type).into_iter().find(|(member, _)| *member == name.text)?.1;
            (member, &text[receiver.span.start..name.span.end], Some(receiver_type), args)
        }
        _ => return None,
    };
    let heads = queries.call_signature_types(&callee);
    if heads.is_empty() { return None; }
    let documentation = match (&call.kind, &receiver) {
        (ExprKind::MethodCall { name, .. }, Some(receiver)) => member_documentation(project, registry, receiver, &name.text, &callee, queries),
        _ => value_documentation(project, registry, &callee, queries),
    }.unwrap_or_default();
    let described = {
        let mut summary = documentation.clone();
        summary.params.clear();
        Some(documentation_markdown(&summary, true)).filter(|markdown| !markdown.is_empty())
    };
    // A method call passes its receiver as the first argument, which the signatures leave out.
    let hidden = usize::from(receiver.is_some());
    let name = if name.len() > 80 { "function" } else { name };
    let mut signatures = Vec::new();
    let mut shapes = Vec::new();
    for head in &heads {
        let Some((parameters, returns)) = crate::describe_signature(head, queries) else { continue };
        let shown = parameters.get(hidden..).unwrap_or_default();
        let mut label = format!("{name}(");
        let mut ranges = Vec::new();
        for (index, parameter) in shown.iter().enumerate() {
            if index > 0 { label.push_str(", "); }
            ranges.push(label.len()..label.len() + parameter.len());
            label.push_str(parameter);
        }
        label.push_str("): ");
        label.push_str(&returns);
        let rest = shown.last().is_some_and(|parameter| parameter.starts_with("..."));
        let active_parameter = if rest { site.argument.min(shown.len() - 1) } else { site.argument };
        let parameter_documentation = shown.iter().map(|parameter| {
            let name = parameter.split(':').next().unwrap_or_default().trim_end_matches('?');
            documentation.param(name).map(str::to_owned)
        }).collect();
        shapes.push((head.clone(), shown.len(), rest));
        signatures.push(SignatureInformation { label, parameters: ranges, active_parameter, documentation: described.clone(), parameter_documentation });
    }
    if signatures.is_empty() { return None; }
    // The overload that the complete arguments before the cursor's select: the first whose
    // call with them fails only for want of more arguments, as TypeScript resolves a call
    // while it is being typed. Failing that, the first that takes the cursor's argument.
    let mut given: Vec<Type> = receiver.into_iter().collect();
    for argument in arguments.iter().take(site.argument) {
        given.push(expression_type(registry, file, argument.span, queries).unwrap_or(Type::Any));
    }
    let takes = |&(_, count, rest): &(Type, usize, bool)| rest || site.argument < count;
    let applicable = |queries: &mut dyn luanalysis_engine::TypeQueries, head: &Type| {
        queries.call(head, &given).errors.iter().all(|error| match error {
            luanalysis_engine::CallError::Arity { actual, minimum, .. } => actual < minimum,
            luanalysis_engine::CallError::Argument { .. } | luanalysis_engine::CallError::NotCallable => false,
            luanalysis_engine::CallError::PackArity { .. } | luanalysis_engine::CallError::TypeArgument { .. }
                | luanalysis_engine::CallError::TypeArgumentArity { .. } => false,
            _ => true,
        })
    };
    let active_signature = (0..shapes.len()).find(|&index| takes(&shapes[index]) && applicable(queries, &shapes[index].0))
        .or_else(|| shapes.iter().position(takes))
        .unwrap_or(0);
    let active_parameter = signatures[active_signature].active_parameter;
    Some(SignatureHelp { signatures, active_signature, active_parameter })
}

/// The type of the expression at `span`, at its flow position, or as it is checked in the frames
/// that check it when it has no type alone (a callback's parameter).
fn expression_type(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    span: Span,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<Type> {
    let source = registry.source(file)?;
    let alone = source.source_check_expression(source.expression_key_for_span(span), None, NativeCheckMode::empty(), queries);
    if !matches!(alone, Type::Any | Type::Unknown) { return Some(alone); }
    source.probe_expression(span);
    registry.check_owned_roots(queries, &[file]);
    source.probed_types().into_iter().next().or(Some(alone))
}

/// Documentation as Markdown: the description, then each parameter's and result's.
pub fn documentation_markdown(documentation: &Documentation, parameters: bool) -> String {
    let mut sections = Vec::new();
    if !documentation.description.is_empty() { sections.push(documentation.description.clone()); }
    if parameters {
        let lines: Vec<String> = documentation.params.iter()
            .map(|(name, text)| format!("*@param* `{name}` — {}", text.replace('\n', " ")))
            .chain(documentation.returns.iter().map(|text| format!("*@return* — {}", text.replace('\n', " "))))
            .collect();
        if !lines.is_empty() { sections.push(lines.join("  \n")); }
    }
    sections.join("\n\n")
}

/// The documentation of a value's declaration: a native function's doc comments, or those of a
/// written signature (`---@function`, `---@overload`).
fn value_documentation(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    value: &Type,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<Documentation> {
    let (file, span, inside) = if let Some(key) = queries.native_function_key(value) {
        let (file, span) = registry.function_site(key)?;
        (file, span, false)
    } else if let Type::NativeSignature(key) = value {
        let (file, span) = registry.signature_site(*key)?;
        (file, span, true)
    } else {
        return None;
    };
    let syntax = registry.source(file)?.syntax();
    let text = &project.file(file)?.source;
    let documentation = if inside {
        luanalysis_syntax::documentation_at(syntax, text, span.start)
    } else {
        luanalysis_syntax::documentation_above(syntax, text, span.start)
    };
    (!documentation.is_empty()).then_some(documentation)
}

/// Where member `name` of `receiver` is declared: in the receiver's class (or runtime table)
/// or one it extends, nearest first. Each site is a file, a span, and whether the span is a
/// documentation tag (`---@field`, `---@function`) rather than a statement.
pub(crate) fn member_sites(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    receiver: &Type,
    name: &str,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<(FileId, Span, bool)> {
    use luanalysis_engine::{LuaKey, NativeTypeProvider, TypeMemberKeyRecipe};
    let mut sites = Vec::new();
    let mut pending: Vec<Type> = vec![receiver.clone()];
    let mut seen = Vec::new();
    while let Some(ty) = pending.pop() {
        let Some(key) = queries.named_object_declaration(&ty).or_else(|| queries.runtime_table_declaration(&ty)) else {
            // Each alternative of a union, or part of an intersection, declares its own.
            if let Type::Union(parts) | Type::Intersection(parts) = queries.inspect_type(&ty) { pending.extend(parts); }
            continue;
        };
        if seen.contains(&key) || seen.len() > 32 { continue; }
        seen.push(key);
        for member in registry.type_declaration_members(key) {
            let named = matches!(registry.type_member_header(member).key, TypeMemberKeyRecipe::Fixed(LuaKey::String(bytes)) if bytes == name.as_bytes());
            if named && let Some(site) = registry.member_site(member) && !sites.contains(&site) { sites.push(site); }
        }
        if sites.is_empty() { pending.extend(registry.type_declaration_bases(key, queries)); }
    }
    sites
}

/// The documentation of member `name` of `receiver`: what its declaration's documentation says
/// (a `---@field`'s description, a `---@function`'s or a function statement's doc comments), or
/// failing that, its value's.
fn member_documentation(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    receiver: &Type,
    name: &str,
    value: &Type,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<Documentation> {
    for (file, span, tag) in member_sites(registry, receiver, name, queries) {
        let (Some(source), Some(text)) = (registry.source(file), project.file(file)) else { continue };
        let documentation = if tag {
            let block = luanalysis_syntax::documentation_at(source.syntax(), &text.source, span.start);
            match block.field(name) {
                Some(field) => Documentation { description: field.to_owned(), ..Documentation::default() },
                None if block.fields.is_empty() => block,
                None => continue,
            }
        } else {
            luanalysis_syntax::documentation_above(source.syntax(), &text.source, span.start)
        };
        if !documentation.is_empty() { return Some(documentation); }
    }
    value_documentation(project, registry, value, queries)
}

/// The documentation of a local or parameter: the doc comments above its declaration, or for a
/// parameter, its function's `---@param`.
fn binding_documentation(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    binding: bind::BindingId,
) -> Option<Documentation> {
    let source = registry.source(file)?;
    let declaration = source.bound().declaration(binding);
    let documentation = luanalysis_syntax::documentation_above(source.syntax(), &project.file(file)?.source, declaration.span.start);
    match declaration.kind {
        bind::DeclarationKind::Parameter => documentation.param(&declaration.name)
            .map(|text| Documentation { description: text.to_owned(), ..Documentation::default() }),
        bind::DeclarationKind::Local | bind::DeclarationKind::LocalFunction => (!documentation.is_empty()).then_some(documentation),
        _ => None,
    }
}

/// The documentation for the name at `offset` in `file`, as Markdown: a member's, a local's or a
/// parameter's, or the declaration of the value a name holds.
pub fn hover_documentation(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    offset: usize,
    deliver: impl FnOnce(Option<String>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: true, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        deliver(documentation_at_offset(project, registry, file, offset, queries)
            .map(|documentation| documentation_markdown(&documentation, true))
            .filter(|markdown| !markdown.is_empty()));
    });
}

fn documentation_at_offset(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<Documentation> {
    let source = registry.source(file)?;
    registry.prepare_documentation_members(queries);
    let contains = |span: Span| span.start <= offset && offset <= span.end;
    let mut member = None;
    let mut name = None;
    crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| match &expression.kind {
        ExprKind::Field { base, name: field } | ExprKind::MethodCall { receiver: base, name: field, .. } if contains(field.span) => {
            member = Some((base.span, field.text.clone()));
        }
        ExprKind::Name(_) if contains(expression.span) => name = Some(expression.span),
        _ => {}
    });
    if let Some((receiver, field)) = member {
        let receiver = expression_type(registry, file, receiver, queries)?;
        let receiver = queries.exclude_type(&receiver, &Type::Nil);
        let value = queries.type_members(&receiver).into_iter().find(|(member, _)| *member == field)?.1;
        return member_documentation(project, registry, &receiver, &field, &value, queries);
    }
    if let Some(binding) = source.bound().binding_at(offset) {
        if let Some(documentation) = binding_documentation(project, registry, file, binding) { return Some(documentation); }
        if source.bound().declaration(binding).kind == bind::DeclarationKind::Parameter { return None; }
    }
    let value = expression_type(registry, file, name?, queries)?;
    value_documentation(project, registry, &value, queries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(text: &str) -> Option<CompletionSite> {
        let offset = text.find('|').expect("a cursor");
        let text = text.replacen('|', "", 1);
        completion_site(&text, offset)
    }

    #[test]
    fn a_member_at_statement_position_is_repaired_into_a_call() {
        let site = site("local player = {}\nplayer.na|\nlocal after = 1\n").unwrap();
        assert_eq!(site.context, CompletionContext::Member { method: false });
        assert_eq!(site.text, format!("local player = {{}}\nplayer.{PLACEHOLDER}()\nlocal after = 1\n"));
        assert_eq!(site.word, 25..27);
    }

    #[test]
    fn an_expression_member_keeps_the_rest_of_the_text() {
        let site = site("local value = player.|\n").unwrap();
        assert_eq!(site.text, format!("local value = player.{PLACEHOLDER}\n"));
    }

    #[test]
    fn open_blocks_and_brackets_are_closed_when_the_rest_does_not_parse() {
        let method = site("local function f(player)\n  if player:|").unwrap();
        assert_eq!(method.context, CompletionContext::Member { method: true });
        assert_eq!(method.text, format!("local function f(player)\n  if player:{PLACEHOLDER}() then end\nend"));
        let index = site("print(items[pla|").unwrap();
        assert_eq!(index.context, CompletionContext::Name);
        assert_eq!(index.text, format!("print(items[{PLACEHOLDER}])"));
    }

    #[test]
    fn comments_strings_numbers_and_declared_names_are_not_completed() {
        for text in ["-- player.|", "local text = 'pla|'", "local n = 1.|5", "local pla|", "local function na|", "for i|"] {
            assert!(site(text).is_none(), "{text}");
        }
    }

    fn call(text: &str) -> Option<(usize, usize, String)> {
        let offset = text.find('|').expect("a cursor");
        let text = text.replacen('|', "", 1);
        signature_site(&text, offset).map(|site| (site.open, site.argument, site.text))
    }

    #[test]
    fn the_argument_is_counted_by_the_commas_of_the_innermost_call() {
        assert_eq!(call("print(|)").map(|(open, argument, _)| (open, argument)), Some((5, 0)));
        assert_eq!(call("print(a, |b)").map(|(open, argument, _)| (open, argument)), Some((5, 1)));
        // Commas inside a table constructor or a nested call belong to them.
        assert_eq!(call("f(a, { x = 1, y = 2 }, |").map(|(open, argument, _)| (open, argument)), Some((1, 2)));
        assert_eq!(call("f(g(1, 2), |").map(|(open, argument, _)| (open, argument)), Some((1, 1)));
        assert_eq!(call("f(1, g(|").map(|(open, argument, _)| (open, argument)), Some((6, 0)));
        assert_eq!(call("object:method(1, |").map(|(open, argument, _)| (open, argument)), Some((13, 1)));
        assert_eq!(call("f({ a = |").map(|(open, argument, _)| (open, argument)), Some((1, 0)));
    }

    #[test]
    fn an_incomplete_argument_list_is_repaired_to_parse() {
        assert_eq!(call("local x = f(1, |").unwrap().2, format!("local x = f(1, {PLACEHOLDER})"));
        assert_eq!(call("f(1, |)\nlocal y = 2\n").unwrap().2, format!("f(1, {PLACEHOLDER})\nlocal y = 2\n"));
        assert_eq!(call("if f(|").unwrap().2, "if f() then end");
    }

    #[test]
    fn parameter_lists_parentheses_strings_and_comments_are_not_calls() {
        for text in ["local function f(|", "function M.f(a, |", "function M:f(|", "local g = function(|", "local x = (|", "f('a, |", "f(1) -- f(|", "f(1) --[[ g(| ]]"] {
            assert!(call(text).is_none(), "{text}");
        }
        assert!(call("f('a', |").is_some());
    }

    #[test]
    fn concatenation_is_not_a_member_access() {
        let site = site("local text = prefix ..|").unwrap();
        assert_eq!(site.context, CompletionContext::Name);
    }
}
