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
use crate::rename_failure::{RenameFailure, RenameReason};
use crate::{bind, describe_native};
use luanalysis_engine::{NativeCheckMode, Type};
use luanalysis_project::{FileId, Project};
use luanalysis_syntax::{CommentKind, Documentation, Expr, ExprKind, Name, Span, TokenKind};
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
    /// A tag's name after `---@`.
    Tag,
    /// A type in an annotation: `---@param point Po`.
    AnnotationType,
    /// The name of a global function being defined: `function onObjectDr`.
    FunctionName,
    /// The contents of a quoted string passed as an argument.
    StringArgument { quote: char },
    /// A Lua 5.4 local attribute, after `local name <`.
    LocalAttribute { closed: bool },
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
    /// Whether the word alone begins a statement of the main chunk, on a line of its own.
    pub statement: bool,
    /// Whether a parameter list already follows the word.
    pub parameters_follow: bool,
    /// Whether the word is a value that may have an expected type: an argument, an assigned
    /// value, a table constructor's field, a returned value, or an operand of `and` or `or`.
    pub value: bool,
    /// Which completions' types and documentation to describe.
    pub details: Details,
}

/// Which completions' `detail` (their type) and documentation to compute: every one's; none, as
/// an editor resolves them for the item it shows, which spares typing every name in scope; or
/// one's, by its label, when the editor resolves it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Details {
    #[default]
    All,
    Lazy,
    Only(String),
}

impl Details {
    fn shown(&self, label: &str) -> bool {
        match self {
            Self::All => true,
            Self::Lazy => false,
            Self::Only(only) => only == label,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Variable,
    Function,
    Field,
    Method,
    Keyword,
    Class,
    TypeParameter,
    /// A function implementation, of a host's callback or of an expected function value.
    Snippet,
    /// A literal value the expected type allows.
    Value,
}

/// A function's implementation, which a completion inserts with an empty body: what precedes its
/// parameter list (`function onSave`, the name after a written `function`, or `function` for a
/// function value), and its parameters' names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Implementation {
    pub head: String,
    pub parameters: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    /// Plain text to insert when the displayed label differs from the replacement.
    pub insert_text: Option<String>,
    pub kind: CompletionKind,
    /// The item's type, when known.
    pub detail: Option<String>,
    /// What the item's declaration documents, as Markdown.
    pub documentation: Option<String>,
    /// A callable's required parameters, which a call snippet fills in, and whether it takes
    /// more (optional or variadic ones) after them.
    pub parameters: Option<(Vec<String>, bool)>,
    /// A function's implementation, which the completion inserts.
    pub implementation: Option<Implementation>,
    /// Whether its declaration is `---@deprecated`.
    pub deprecated: bool,
}

/// The completion site at a byte offset of `text`, or `None` where nothing is completed: inside
/// a comment, a non-argument string or a number, at a name being declared, or where no repair parses.
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
    // Token spans keep a partially typed Unicode name intact, just as they do
    // for an ASCII name. Byte scanning alone stops in the middle of that name.
    if let Some(token) = tokens.iter().find(|token| token.span.start <= offset && offset <= token.span.end
        && matches!(token.kind, TokenKind::Identifier(_) | TokenKind::Keyword(_))) {
        start = token.span.start;
        end = token.span.end;
    }
    if let Some(context) = annotation_context(text, &tokens, start, offset) {
        return Some(CompletionSite { context, text: text.to_owned(), placeholder: Span { start, end: start }, word: start..end, statement: false, parameters_follow: false, value: false, details: Details::All });
    }
    if let Some(token) = tokens.iter().find(|token| matches!(token.kind, TokenKind::String(_))
        && token.span.start < offset && offset <= token.span.end)
        && let Some(site) = string_argument_site(text, token.span, offset)
    {
        return Some(site);
    }
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
    if matches!(previous.map(|token| &token.kind), Some(TokenKind::Symbol(symbol)) if symbol == "<") {
        let closed = text[end..].trim_start().starts_with('>');
        let candidate = format!("{}{PLACEHOLDER}{}{}", &text[..start], if closed { "" } else { ">" }, &text[end..]);
        if has_local_attribute(&luanalysis_syntax::parse(&candidate).statements, start) {
            return Some(CompletionSite { context: CompletionContext::LocalAttribute { closed }, text: candidate,
                placeholder: Span { start, end: start + PLACEHOLDER.len() }, word: start..end,
                statement: false, parameters_follow: false, value: false, details: Details::All });
        }
    }
    let context = match previous.map(|token| &token.kind) {
        Some(TokenKind::Symbol(symbol)) if symbol == "." => CompletionContext::Member { method: false },
        Some(TokenKind::Symbol(symbol)) if symbol == ":" => CompletionContext::Member { method: true },
        // A global function's name may implement a host's callback; a local's is its own.
        Some(TokenKind::Keyword(keyword)) if keyword == "function" => {
            let before = tokens.iter()
                .filter(|token| previous.is_some_and(|previous| token.span.end <= previous.span.start)
                    && !matches!(token.kind, TokenKind::Newline | TokenKind::Comment(_) | TokenKind::Eof))
                .next_back();
            if matches!(before.map(|token| &token.kind), Some(TokenKind::Keyword(keyword)) if keyword == "local") { return None; }
            CompletionContext::FunctionName
        }
        // Any other name being declared is not completed.
        Some(TokenKind::Keyword(keyword)) if keyword == "local" || keyword == "for" => return None,
        _ => CompletionContext::Name,
    };
    let value = context == CompletionContext::Name && match previous.map(|token| &token.kind) {
        Some(TokenKind::Symbol(symbol)) => matches!(symbol.as_str(), "=" | "(" | "," | "{" | ";"),
        Some(TokenKind::Keyword(keyword)) => matches!(keyword.as_str(), "return" | "and" | "or"),
        _ => false,
    };
    let head = &text[..start];
    let tail = &text[end..];
    let line_head = &head[head.rfind('\n').map_or(0, |newline| newline + 1)..];
    let line_tail = &tail[..tail.find('\n').unwrap_or(tail.len())];
    let statement = context == CompletionContext::Name && line_head.trim().is_empty() && line_tail.trim().is_empty() && closers(head).is_empty();
    let parameters_follow = tail.trim_start().starts_with('(');
    let mut candidates = vec![
        format!("{head}{PLACEHOLDER}{tail}"),
        format!("{head}{PLACEHOLDER}(){tail}"),
    ];
    // A function being named is completed as an empty one, keeping the rest of the text.
    if context == CompletionContext::FunctionName { candidates.insert(1, format!("{head}{PLACEHOLDER}() end{tail}")); }
    for call in ["", "()"] {
        let prefix = format!("{head}{PLACEHOLDER}{call}");
        let closers = closers(&prefix);
        candidates.push(prefix + &closers);
    }
    let text = repaired(candidates, |parsed| placeholder_parsed(parsed, start))?;
    Some(CompletionSite {
        context,
        text,
        placeholder: Span { start, end: start + PLACEHOLDER.len() },
        word: start..end,
        statement,
        parameters_follow,
        value,
        details: Details::All,
    })
}

/// Strings keep their quotes; replacing their full contents also handles escapes and a cursor
/// in the middle of a word. Only argument strings opt into completion, so ordinary strings
/// retain the editor's existing behavior.
fn string_argument_site(text: &str, span: Span, offset: usize) -> Option<CompletionSite> {
    let quote = *text.as_bytes().get(span.start)?;
    if !matches!(quote, b'\'' | b'"') { return None; }
    let closed = span.end - span.start > 1 && text.as_bytes().get(span.end.checked_sub(1)?) == Some(&quote)
        && !luanalysis_syntax::lex(&text[span.start..span.end]).diagnostics.iter().any(|diagnostic| diagnostic.message.starts_with("unterminated string"));
    let end = span.end - usize::from(closed);
    if offset > end { return None; }
    let replacement = format!("{}{quote}{PLACEHOLDER}{quote}{}", &text[..span.start], &text[span.end..], quote = quote as char);
    let completed = replacement.clone() + &closers(&replacement);
    let placeholder = Span { start: span.start, end: span.start + PLACEHOLDER.len() + 2 };
    let repaired = repaired(vec![replacement, completed], |syntax| argument_context(syntax, placeholder).is_some())?;
    if argument_context(&luanalysis_syntax::parse(&repaired), placeholder).is_none() { return None; }
    Some(CompletionSite { context: CompletionContext::StringArgument { quote: quote as char }, text: repaired,
        placeholder, word: span.start + 1..end, statement: false, parameters_follow: false, value: true, details: Details::All })
}

/// The parser owns whether `<name>` is a local attribute or a comparison. An unknown temporary
/// attribute is retained as a node even while its spelling is incomplete.
fn has_local_attribute(body: &[luanalysis_syntax::Stmt], start: usize) -> bool {
    use luanalysis_syntax::StmtKind;
    body.iter().any(|statement| match &statement.kind {
        StmtKind::Local { attributes, .. } => attributes.iter().flatten().any(|attribute| attribute.span.start == start),
        StmtKind::Block { body, .. } | StmtKind::While { body, .. } | StmtKind::Repeat { body, .. }
        | StmtKind::NumericFor { body, .. } | StmtKind::GenericFor { body, .. } => has_local_attribute(body, start),
        StmtKind::LocalFunction { function, .. } | StmtKind::FunctionDeclaration { function, .. } => has_local_attribute(&function.body, start),
        StmtKind::If { branches, else_branch } => branches.iter().any(|branch| has_local_attribute(&branch.body, start))
            || else_branch.as_ref().is_some_and(|branch| has_local_attribute(&branch.body, start)),
        _ => false,
    }) || {
        let mut found = false;
        crate::module_requests::visit_expressions(body, &mut |expression| {
            if let ExprKind::Function(function) = &expression.kind { found |= has_local_attribute(&function.body, start); }
        });
        found
    }
}

/// The first of `candidates` that parses. In a file with syntax errors elsewhere, such as another
/// line being typed, none does: then the one with the fewest that `accept` accepts, as the tree
/// the parser recovers is checked too.
fn repaired(candidates: Vec<String>, accept: impl Fn(&luanalysis_syntax::File) -> bool) -> Option<String> {
    let mut fallback: Option<(usize, String)> = None;
    for candidate in candidates {
        let parsed = luanalysis_syntax::parse(&candidate);
        let errors = parsed.diagnostics.iter().filter(|diagnostic| diagnostic.origin == luanalysis_syntax::DiagnosticOrigin::LuaSyntax).count();
        if errors == 0 { return Some(candidate); }
        if fallback.as_ref().is_none_or(|(fewest, _)| errors < *fewest) && accept(&parsed) {
            fallback = Some((errors, candidate));
        }
    }
    fallback.map(|(_, candidate)| candidate)
}

/// Whether `file` parsed the completion placeholder at `start` as a name: of a value, a member or
/// a function being defined.
fn placeholder_parsed(file: &luanalysis_syntax::File, start: usize) -> bool {
    let mut found = false;
    crate::module_requests::visit_expressions(&file.statements, &mut |expression: &Expr| match &expression.kind {
        ExprKind::Name(name) | ExprKind::Field { name, .. } | ExprKind::MethodCall { name, .. } => found |= name.span.start == start && name.text == PLACEHOLDER,
        _ => {}
    });
    found
}

/// The completion an annotation takes at `offset`, when it is in a documentation comment: a tag
/// name after `@`, or a type where the tag takes one. A tag's own names (`@param name`,
/// `@class Name`) and its description are not completed.
fn annotation_context(text: &str, tokens: &[luanalysis_syntax::Token], start: usize, offset: usize) -> Option<CompletionContext> {
    let comment = tokens.iter().find(|token| matches!(&token.kind, TokenKind::Comment(comment) if comment.kind == CommentKind::Documentation)
        && token.span.start < offset && offset <= token.span.end)?;
    let line_start = text[..start].rfind('\n').map_or(0, |newline| newline + 1).max(comment.span.start);
    let line = &text[line_start..start];
    let content = line.trim_start().trim_start_matches('-').trim_start_matches(['[', '=']).trim_start();
    let Some(tag) = content.strip_prefix('@') else {
        // A line continuing a tag continues its type.
        return (!content.trim().is_empty() || line.trim_start().starts_with("---")).then_some(CompletionContext::AnnotationType);
    };
    let name_end = tag.find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_').unwrap_or(tag.len());
    if name_end == tag.len() { return Some(CompletionContext::Tag); }
    let (name, rest) = tag.split_at(name_end);
    // The words a tag names before its type: `@param name`, `@field name`, `@cast name`.
    let words = rest.split_whitespace().count() + usize::from(rest.ends_with(char::is_whitespace));
    let typed = match name {
        "type" | "return" | "vararg" | "not" | "as" | "overload" => true,
        "param" | "field" | "cast" => words >= 2,
        "class" | "shape" | "module" => rest.contains(':'),
        "alias" => words >= 2,
        "generic" => rest.contains(':'),
        _ => false,
    };
    typed.then_some(CompletionContext::AnnotationType)
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

/// Prepares `project`'s sources for the queries that follow, as a query would, so the first of
/// them need not.
pub fn prepare(project: Project, options: ProjectCheckOptions, workspace: &mut ProjectWorkspace) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: false, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |_, registry, queries| {
        registry.prepare_documentation_members(queries);
    });
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
    if let CompletionContext::LocalAttribute { closed } = site.context {
        use crate::standard_environment::StandardEnvironmentProfile;
        let lua54 = options.environment == StandardEnvironmentProfile::Lua54
            || matches!(options.environment, StandardEnvironmentProfile::LegacyMinimal | StandardEnvironmentProfile::None)
                && options.loader_policy == luanalysis_engine::NativeLoaderPolicy::Lua54StandardFile;
        deliver(if lua54 { ["const", "close"].into_iter().map(|name| CompletionItem { insert_text: (!closed).then(|| format!("{name}>")),
            label: name.to_owned(), kind: CompletionKind::Keyword, detail: None, documentation: None,
            parameters: None, implementation: None, deprecated: false,
        }).collect() } else { Vec::new() });
        return;
    }
    let options = ProjectCheckOptions { jobs: 1, describe_types: false, inspections: false, ..options };
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
        CompletionContext::LocalAttribute { .. } => {}
        CompletionContext::StringArgument { quote } => {
            items.extend(argument_history(project, registry, file, site, quote, queries));
            let mut pending = contextual_types(registry, file, site.placeholder, queries, 0);
            let mut seen = Vec::new();
            while let Some(ty) = pending.pop() {
                if seen.len() > 64 || seen.contains(&ty) { continue; }
                seen.push(ty.clone());
                match queries.inspect_type(&ty) {
                    Type::Union(parts) => pending.extend(parts),
                    Type::StringLiteral(bytes) => {
                        let item = string_completion(&bytes, quote);
                        if !items.iter().any(|other| other.label == item.label) { items.push(item); }
                    }
                    _ => {}
                }
            }
        }
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
                let shown = site.details.shown(&name);
                let documentation = receiver_type.as_ref()
                    .and_then(|receiver| member_documentation(project, registry, receiver, &name, &value, queries));
                let deprecated = documentation.as_ref().is_some_and(|documentation| documentation.deprecated.is_some());
                let documentation = documentation.filter(|_| shown).map(|documentation| documentation_markdown(&documentation, true));
                let parameters = callable.then(|| required_parameters(&value, usize::from(method), queries)).flatten();
                let detail = shown.then(|| describe_native(&value, queries, &mut Vec::new()));
                items.push(CompletionItem { insert_text: None, label: name, kind, detail, documentation, parameters, implementation: None, deprecated });
            }
        }
        CompletionContext::FunctionName => items.extend(callback_implementations(project, registry, file, site, false, queries)),
        CompletionContext::Tag => {
            for tag in ["param", "return", "type", "class", "shape", "field", "alias", "generic", "overload", "vararg", "function", "operator", "not", "see", "module", "enum", "cast", "as"] {
                items.push(CompletionItem { insert_text: None, label: tag.to_owned(), kind: CompletionKind::Keyword, detail: None, documentation: None, parameters: None, implementation: None, deprecated: false });
            }
        }
        CompletionContext::AnnotationType => {
            let block = luanalysis_syntax::type_names(source.syntax(), &project.file(file).map(|file| file.source.clone()).unwrap_or_default())
                .into_iter().find(|name| name.block.start <= site.word.start && site.word.start <= name.block.end).map(|name| name.block);
            for other in &project.files {
                let Some(syntax) = registry.source(other.id).map(|source| source.syntax()) else { continue };
                for name in luanalysis_syntax::type_names(syntax, &other.source) {
                    let kind = match name.kind {
                        luanalysis_syntax::TypeNameKind::Declaration => CompletionKind::Class,
                        luanalysis_syntax::TypeNameKind::Generic if other.id == file && Some(name.block) == block => CompletionKind::TypeParameter,
                        _ => continue,
                    };
                    if name.name == PLACEHOLDER || items.iter().any(|item: &CompletionItem| item.label == name.name) { continue; }
                    let documentation = (kind == CompletionKind::Class).then(|| luanalysis_syntax::documentation_at(syntax, &other.source, name.span.start))
                        .map(|documentation| { let mut summary = documentation; summary.fields.clear(); documentation_markdown(&summary, false) })
                        .filter(|markdown| !markdown.is_empty());
                    items.push(CompletionItem { insert_text: None, label: name.name, kind, detail: None, documentation, parameters: None, implementation: None, deprecated: false });
                }
            }
            for builtin in ["nil", "boolean", "number", "integer", "string", "table", "function", "userdata", "thread", "any", "unknown", "never", "void", "self", "fun"] {
                if !items.iter().any(|item| item.label == builtin) {
                    items.push(CompletionItem { insert_text: None, label: builtin.to_owned(), kind: CompletionKind::Keyword, detail: None, documentation: None, parameters: None, implementation: None, deprecated: false });
                }
            }
        }
        CompletionContext::Name => {
            // Names need no check: a local's detail is its declared type.
            let bound = source.bound();
            for binding in visible_bindings(bound, site.placeholder.start) {
                let declaration = bound.declaration(binding);
                let shown = site.details.shown(&declaration.name);
                let kind = match declaration.kind {
                    bind::DeclarationKind::LocalFunction => CompletionKind::Function,
                    _ => CompletionKind::Variable,
                };
                // A local's type is computed only to describe it, or for a function's call.
                let value = (shown || kind == CompletionKind::Function)
                    .then(|| source.resolve_binding(binding, queries)).flatten()
                    .map(|binding| binding.constraint);
                let detail = value.as_ref().filter(|_| shown).map(|value| describe_native(value, queries, &mut Vec::new()));
                let documentation = binding_documentation(project, registry, file, binding);
                let deprecated = documentation.as_ref().is_some_and(|documentation| documentation.deprecated.is_some());
                let documentation = documentation.filter(|_| shown).map(|documentation| documentation_markdown(&documentation, true));
                let parameters = value.filter(|_| kind == CompletionKind::Function)
                    .and_then(|value| required_parameters(&value, 0, queries));
                items.push(CompletionItem { insert_text: None, label: declaration.name.clone(), kind, detail, documentation, parameters, implementation: None, deprecated });
            }
            // Listing globals for an editor that resolves their details needs no global's value:
            // their kinds and calls come from their declarations. Typing every global would
            // infer each one's value, the modules it requires included.
            if site.details == Details::Lazy {
                for (name, parameters) in registry.global_completions(queries) {
                    if !is_identifier(&name) || items.iter().any(|item| item.label == name) { continue; }
                    let kind = if parameters.is_some() { CompletionKind::Function } else { CompletionKind::Variable };
                    let deprecated = global_documentation(project, registry, &name).is_some_and(|documentation| documentation.deprecated.is_some());
                    items.push(CompletionItem { insert_text: None, label: name, kind, detail: None, documentation: None, parameters, implementation: None, deprecated });
                }
            } else if let Some(environment) = source.global_environment()
                && let Ok(environment) = queries.global_environment_type(environment)
            {
                let members = match &site.details {
                    // Resolving one global's details types that global alone.
                    Details::Only(label) => queries.table_lookup(&environment, &luanalysis_engine::TableKey::Literal(luanalysis_engine::LuaKey::String(label.as_bytes().to_vec())))
                        .ok().filter(|value| !matches!(value, Type::Nil | Type::Undefined)).map(|value| vec![(label.clone(), value)]).unwrap_or_default(),
                    _ => queries.type_members(&environment),
                };
                for (name, value) in members {
                    if !is_identifier(&name) || items.iter().any(|item| item.label == name) { continue; }
                    let shown = site.details.shown(&name);
                    let kind = if queries.call_signature_types(&value).is_empty() { CompletionKind::Variable } else { CompletionKind::Function };
                    let documentation = global_documentation(project, registry, &name)
                        .or_else(|| shown.then(|| value_documentation(project, registry, &value, queries)).flatten());
                    let deprecated = documentation.as_ref().is_some_and(|documentation| documentation.deprecated.is_some());
                    let documentation = documentation.filter(|_| shown).map(|documentation| documentation_markdown(&documentation, true));
                    let parameters = (kind == CompletionKind::Function).then(|| required_parameters(&value, 0, queries)).flatten();
                    let detail = shown.then(|| describe_native(&value, queries, &mut Vec::new()));
                    items.push(CompletionItem { insert_text: None, label: name, kind, detail, documentation, parameters, implementation: None, deprecated });
                }
            }
            // Resolving one name's details needs none of what follows.
            items.extend(implicit_receiver_methods(project, registry, file, site, queries));
            if let Details::Only(_) = site.details { return items; }
            // A statement of its own may begin a callback's implementation.
            if site.statement { items.extend(callback_implementations(project, registry, file, site, true, queries)); }
            // A value may be expected to be a function or one of some literals.
            if site.value { items.splice(0..0, expected_values(registry, file, site, queries)); }
            items.extend(KEYWORDS.iter().map(|keyword| CompletionItem { insert_text: None, label: (*keyword).to_owned(), kind: CompletionKind::Keyword, detail: None, documentation: None, parameters: None, implementation: None, deprecated: false }));
        }
    }
    items
}

/// Implementations of the callbacks a declaration library declares as values of a function type
/// (`---@type nil | fun(...)` above `name = nil`), for a global function's name or a statement
/// of its own. A callback with several signatures, one the workspace already implements, and one
/// a local of the same name hides are not offered.
fn callback_implementations(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    site: &CompletionSite,
    keyword: bool,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<CompletionItem> {
    let Some(source) = registry.source(file) else { return Vec::new() };
    let bound = source.bound();
    let shadowed: Vec<String> = visible_bindings(bound, site.placeholder.start).into_iter().map(|binding| bound.declaration(binding).name.clone()).collect();
    let implemented = implemented_globals(project, registry, file, site.word.start);
    let mut items = Vec::new();
    for (declared_in, name, key, statement) in registry.ambient_values() {
        if shadowed.contains(&name) || implemented.contains(&name) || items.iter().any(|item: &CompletionItem| item.label == name) { continue; }
        let Ok(declared) = queries.value_declaration_type(key) else { continue };
        let callable = queries.exclude_type(&declared, &Type::Nil);
        let heads = queries.call_signature_types(&callable);
        let [head] = heads.as_slice() else { continue };
        let Some((parameters, _)) = crate::describe_signature(head, queries) else { continue };
        let names: Vec<String> = parameters.iter().map(|parameter| parameter.split(':').next().unwrap_or_default().trim_end_matches('?').trim().to_owned()).collect();
        let documentation = registry.source(declared_in).zip(project.file(declared_in))
            .map(|(declaring, text)| luanalysis_syntax::documentation_above(declaring.syntax(), &text.source, statement.start))
            .filter(|documentation| !documentation.is_empty())
            .map(|documentation| documentation_markdown(&documentation, true));
        // With a parameter list already written, only the name is completed.
        let head = if keyword { format!("function {name}") } else { name.clone() };
        let implementation = (!site.parameters_follow).then(|| Implementation { head, parameters: names.clone() });
        let kind = if implementation.is_some() { CompletionKind::Snippet } else { CompletionKind::Function };
        items.push(CompletionItem { insert_text: None,
            detail: Some(format!("Implement {name}({})", names.join(", "))),
            label: name,
            kind,
            documentation,
            parameters: None,
            implementation,
            deprecated: false,
        });
    }
    items
}

fn value_completion(label: String) -> CompletionItem {
    CompletionItem { insert_text: None, label, kind: CompletionKind::Value, detail: None, documentation: None,
        parameters: None, implementation: None, deprecated: false }
}

fn string_completion(bytes: &[u8], quote: char) -> CompletionItem {
    let escaped = string_contents(bytes, quote);
    let label = String::from_utf8(bytes.to_vec()).unwrap_or_else(|_| escaped.clone());
    CompletionItem { insert_text: Some(escaped), ..value_completion(label) }
}

/// A visible implicit receiver is a real lexical binding. Enumerating its type also respects
/// inheritance and generic receivers; a local or explicit parameter named `self` hides it.
fn implicit_receiver_methods(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    site: &CompletionSite,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<CompletionItem> {
    let Some(source) = registry.source(file) else { return Vec::new() };
    let bound = source.bound();
    let receiver = visible_bindings(bound, site.placeholder.start).into_iter()
        .find(|binding| bound.declaration(*binding).name == "self")
        .filter(|binding| bound.declaration(*binding).kind == bind::DeclarationKind::ImplicitReceiver);
    let Some(receiver) = receiver.and_then(|binding| source.resolve_binding(binding, queries)) else { return Vec::new() };
    let receiver = queries.exclude_type(&receiver.constraint, &Type::Nil);
    let mut items = Vec::new();
    for (name, value) in queries.type_members(&receiver) {
        if !is_identifier(&name) || queries.call_signature_types(&value).is_empty() { continue; }
        let label = format!("self:{name}");
        let shown = site.details.shown(&label);
        let documentation = member_documentation(project, registry, &receiver, &name, &value, queries);
        let deprecated = documentation.as_ref().is_some_and(|documentation| documentation.deprecated.is_some());
        items.push(CompletionItem { insert_text: None, label, kind: CompletionKind::Method,
            detail: shown.then(|| describe_native(&value, queries, &mut Vec::new())),
            documentation: documentation.filter(|_| shown).map(|documentation| documentation_markdown(&documentation, true)),
            parameters: required_parameters(&value, 1, queries), implementation: None, deprecated });
    }
    items
}

/// History belongs to the selected function value and its physical argument slot. Names alone
/// would combine unrelated shadowed functions, while physical slots let dot and colon calls
/// contribute to the same method parameter.
fn argument_history(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    site: &CompletionSite,
    quote: char,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<CompletionItem> {
    let Some(source) = registry.source(file) else { return Vec::new() };
    let Some((callee, index)) = argument_context(source.syntax(), site.placeholder) else { return Vec::new() };
    let Some((callee, index)) = callee_type(registry, file, &callee, index, queries) else { return Vec::new() };
    if untyped(&callee, queries) { return Vec::new(); }
    let key = queries.native_function_key(&callee);
    let heads = queries.call_signature_types(&callee);
    let signature_keys: Option<Vec<_>> = heads.iter().map(|head| queries.native_signature_key(head)).collect();
    if key.is_none() && signature_keys.as_ref().is_none_or(Vec::is_empty) { return Vec::new(); }
    let mut items: Vec<CompletionItem> = Vec::new();
    for other in &project.files {
        let Some(source) = registry.source(other.id) else { continue };
        let mut candidates = Vec::new();
        crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression| {
            let (callee, args) = match &expression.kind {
                ExprKind::Call { callee, args } => (Callee::Function(callee.span), args),
                ExprKind::MethodCall { receiver, name, args } => (Callee::Method { receiver: receiver.span, name: name.text.clone() }, args),
                _ => return,
            };
            let written = index.checked_sub(usize::from(matches!(callee, Callee::Method { .. })));
            if let Some(argument) = written.and_then(|index| args.get(index))
                && !(other.id == file && argument.span == site.placeholder)
                && let ExprKind::String(bytes) = &argument.kind
            {
                candidates.push((callee, bytes.clone()));
            }
        });
        for (candidate, bytes) in candidates {
            let Some((candidate, _)) = callee_type(registry, other.id, &candidate, 0, queries) else { continue };
            let same = match key {
                Some(key) => queries.native_function_key(&candidate) == Some(key),
                None => {
                    let candidate_heads = queries.call_signature_types(&candidate);
                    let candidate_keys: Option<Vec<_>> = candidate_heads.iter().map(|head| queries.native_signature_key(head)).collect();
                    candidate_keys == signature_keys
                }
            };
            if !same { continue; }
            let item = string_completion(&bytes, quote);
            if !items.iter().any(|other| other.label == item.label) { items.push(item); }
        }
    }
    items
}

fn argument_context(syntax: &luanalysis_syntax::File, mut span: Span) -> Option<(Callee, usize)> {
    loop {
        match value_context(syntax, span)? {
            Context::Through(parent) => span = parent,
            Context::Argument { callee, index } => return Some((callee, index)),
            _ => return None,
        }
    }
}

fn callee_type(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    callee: &Callee,
    index: usize,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<(Type, usize)> {
    match callee {
        Callee::Function(span) => expression_type(registry, file, *span, queries).map(|callee| (callee, index)),
        Callee::Method { receiver, name } => member_type(registry, file, *receiver, name, queries).map(|callee| (callee, index + 1)),
    }
}

/// Type lowering deliberately erases editor-only hints. Follow the resolved callable back to
/// its original annotation rather than interpreting the suggested text as a Lua expression.
fn argument_metadata_hints(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    span: Span,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<String> {
    let Some(source) = registry.source(file) else { return Vec::new() };
    let Some((callee, index)) = argument_context(source.syntax(), span) else { return Vec::new() };
    let Some((callee, index)) = callee_type(registry, file, &callee, index, queries) else { return Vec::new() };
    let mut hints = Vec::new();
    for head in queries.call_signature_types(&callee) {
        let signature = queries.native_signature_key(&head);
        if signature.is_none() && let Some(key) = queries.native_function_key(&head) {
            let Some((file, _)) = registry.function_site(key) else { continue };
            let Some(source) = registry.source(file) else { continue };
            function_parameter_hints(source, key, index, &mut hints);
        }
        if let Some(key) = signature {
            let Some((file, _, _)) = registry.signature_site(key) else { continue };
            let Some(source) = registry.source(file) else { continue };
            let recipe = source.signatures.get(key);
            if let Some(function) = recipe.ambient_function {
                function_parameter_hints(source, function, index, &mut hints);
                continue;
            }
            let Some(signature) = &recipe.signature else { continue };
            let documentation = recipe.documentation.and_then(|id| source.documentation_function(id));
            let hidden = usize::from(luanalysis_engine::NativeTypeProvider::signature_is_method(source, key));
            let Some(index) = index.checked_sub(hidden) else { continue };
            if let Some(parameter) = signature.params.get(index) {
                let ty = parameter.ty.as_ref().or_else(|| documentation.and_then(|doc| doc.annotations.params.iter().find(|annotation| annotation.name.text == parameter.name.text)).map(|annotation| &annotation.ty));
                if let Some(ty) = ty { completion_hints(ty, &mut hints); }
            } else if let Some(annotation) = signature.vararg.as_ref().or_else(|| documentation.and_then(|doc| doc.annotations.vararg.as_ref())) {
                completion_hints(&annotation.ty, &mut hints);
            }
        }
    }
    hints
}

fn function_parameter_hints(source: &crate::native_functions::NativeRegistry, key: luanalysis_engine::NativeFunctionKey, index: usize, hints: &mut Vec<String>) {
    let function = source.function(key);
    let Some(index) = index.checked_sub(usize::from(source.implicit_receiver(function).is_some())) else { return };
    if let Some(parameter) = function.params.get(index)
        && let Some(annotation) = function.annotations.params.iter().find(|annotation| annotation.name.text == parameter.text)
    {
        completion_hints(&annotation.ty, hints);
    } else if index >= function.params.len() && let Some(annotation) = &function.annotations.vararg {
        completion_hints(&annotation.ty, hints);
    }
}

fn completion_hints(ty: &luanalysis_syntax::TypeExpr, hints: &mut Vec<String>) {
    use luanalysis_syntax::TypeExprKind;
    match &ty.kind {
        TypeExprKind::CompletionHint(text) => if !hints.contains(text) { hints.push(text.clone()); },
        TypeExprKind::Union(parts) => for part in parts { completion_hints(part, hints); },
        TypeExprKind::Parenthesized(inner) => completion_hints(inner, hints),
        _ => {}
    }
}

/// The values the type expected at `site` allows: a function implementing each signature it has,
/// with an empty body, and each literal it may be. `callback_function = ` given
/// `nil | fun(object: tts__Object)` completes `function(object) end`.
fn expected_values(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    site: &CompletionSite,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<CompletionItem> {
    let mut pending = contextual_types(registry, file, site.placeholder, queries, 0);
    pending.reverse();
    let mut seen = Vec::new();
    let mut items: Vec<CompletionItem> = argument_metadata_hints(registry, file, site.placeholder, queries)
        .into_iter().map(|label| CompletionItem { insert_text: None, kind: CompletionKind::Snippet, ..value_completion(label) }).collect();
    let mut add = |item: CompletionItem| if !items.iter().any(|other| other.label == item.label) { items.push(item) };
    while let Some(ty) = pending.pop() {
        if seen.len() > 64 || seen.contains(&ty) { continue; }
        seen.push(ty.clone());
        let literal = |label: String| CompletionItem { insert_text: None, label, kind: CompletionKind::Value, detail: None, documentation: None, parameters: None, implementation: None, deprecated: false };
        let any_function = match queries.inspect_type(&ty) {
            Type::Union(parts) => { pending.extend(parts.into_iter().rev()); continue; }
            Type::StringLiteral(bytes) => { add(literal(lua_string(&bytes))); continue; }
            Type::NumberLiteral(value) => { add(literal(if value.fract() == 0.0 && value.abs() < 1e15 { format!("{}", value as i64) } else { value.to_string() })); continue; }
            inspected => inspected == Type::RuntimeFunction,
        };
        for head in queries.call_signature_types(&ty) {
            let Some((parameters, _)) = crate::describe_signature(&head, queries) else { continue };
            let names: Vec<String> = parameters.iter().map(|parameter| parameter.split(':').next().unwrap_or_default().trim_end_matches('?').trim().to_owned()).collect();
            // Any function at all (`function`) takes whatever it is given.
            let detail = if any_function { "function".to_owned() } else { describe_native(&head, queries, &mut Vec::new()) };
            add(CompletionItem { insert_text: None,
                label: format!("function({})", names.join(", ")),
                kind: CompletionKind::Snippet,
                detail: Some(detail),
                documentation: None,
                parameters: None,
                implementation: Some(Implementation { head: "function".to_owned(), parameters: names }),
                deprecated: false,
            });
        }
    }
    items
}

/// What a value is written as, which says what it is expected to be.
enum Context {
    /// The operand of parentheses, `or` or `and`, expected to be what the whole is.
    Through(Span),
    /// A table constructor's `name = value` or `["name"] = value`.
    Field { table: Span, key: luanalysis_engine::LuaKey },
    /// A table constructor's positional value, by its one-based index.
    Sequence { table: Span, index: usize },
    /// A call's argument, by its index among the written arguments.
    Argument { callee: Callee, index: usize },
    /// A local's initial value.
    Local { name: Span },
    /// A value assigned to a target.
    Assign(Target),
    /// A returned value, by its index, and the function it is returned from (none for a chunk).
    Return { owner: Option<Owner>, index: usize },
}

#[derive(Clone)]
enum Callee {
    Function(Span),
    Method { receiver: Span, name: String },
}

#[derive(Clone)]
enum Target {
    Field { base: Span, name: String },
    Name(Name),
}

/// A function a value is returned from: its span, whether its results are declared, and what
/// else says what it returns: the expected type of a function value, or a function statement's
/// target's type.
#[derive(Clone)]
struct Owner {
    span: Span,
    declared: bool,
    expression: bool,
    target: Option<Target>,
}

/// The types the value at `span` in `file` is expected to have, as TypeScript's contextual type:
/// from what the value is written as, its parent's expected type when that decides it, without
/// checking the file. `callback_function = ` in a table passed to `spawnObjectData` expects the
/// shape's field's type; an argument, its parameter's in each signature of the call, even one
/// the call cannot take yet.
fn contextual_types(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    span: Span,
    queries: &mut dyn luanalysis_engine::TypeQueries,
    depth: usize,
) -> Vec<Type> {
    let Some(source) = registry.source(file) else { return Vec::new() };
    if depth > 16 { return Vec::new(); }
    let Some(context) = value_context(source.syntax(), span) else { return Vec::new() };
    let parent = |span: Span, queries: &mut dyn luanalysis_engine::TypeQueries| contextual_types(registry, file, span, queries, depth + 1);
    let types = match context {
        Context::Through(whole) => parent(whole, queries),
        Context::Field { table, key } => parent(table, queries).into_iter()
            .filter_map(|table| queries.table_value_context(&table, &luanalysis_engine::TableKey::Literal(key.clone())))
            .collect(),
        Context::Sequence { table, index } => {
            let mut elements = Vec::new();
            for table in parent(table, queries) {
                if let Some(pack) = queries.table_sequence_context(&table, i64::try_from(index).unwrap_or(i64::MAX)) {
                    elements.push(queries.pack_slot(&pack, 0));
                }
            }
            elements
        }
        Context::Argument { callee, index } => {
            // A method call passes its receiver as the first argument.
            let (callee, position) = match callee {
                Callee::Function(callee) => (expression_type(registry, file, callee, queries), index),
                Callee::Method { receiver, name } => (member_type(registry, file, receiver, &name, queries), index + 1),
            };
            let mut expected = Vec::new();
            for head in callee.map(|callee| queries.call_signature_types(&callee)).unwrap_or_default() {
                let Some(signature) = queries.function_signature(&head) else { continue };
                match signature.params.get(position) {
                    Some(parameter) => expected.push(parameter.ty.clone()),
                    None => if let Some(rest) = queries.signature_rest_pack(&head) {
                        expected.push(queries.pack_slot(&rest, position - signature.params.len()));
                    },
                }
            }
            expected
        }
        Context::Local { name } => source.bound().binding_at(name.start)
            .and_then(|binding| source.resolve_binding(binding, queries))
            .map(|binding| binding.constraint)
            .into_iter()
            .collect(),
        Context::Assign(target) => target_type(registry, file, &target, queries).into_iter().collect(),
        Context::Return { owner: None, .. } => Vec::new(),
        Context::Return { owner: Some(owner), index } => {
            let functions: Vec<Type> = if owner.declared {
                source.function_key_at(owner.span).map(Type::NativeFunction).into_iter().collect()
            } else if owner.expression {
                parent(owner.span, queries)
            } else {
                owner.target.and_then(|target| target_type(registry, file, &target, queries)).into_iter().collect()
            };
            let mut results = Vec::new();
            for function in functions {
                let function = queries.exclude_type(&function, &Type::Nil);
                for head in queries.call_signature_types(&function) {
                    if let Some(pack) = queries.function_result_pack(&head) { results.push(queries.pack_slot(&pack, index)); }
                }
            }
            results
        }
    };
    types.into_iter().filter(|ty| !untyped(ty, queries)).collect()
}

/// The type of member `name` of the value at `receiver`.
fn member_type(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    receiver: Span,
    name: &str,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<Type> {
    let receiver = expression_type(registry, file, receiver, queries)?;
    let receiver = queries.exclude_type(&receiver, &Type::Nil);
    queries.type_members(&receiver).into_iter().find(|(member, _)| member == name).map(|(_, value)| value)
}

/// The type of an assignment's target: a field's, a local's declared type, or a global's.
fn target_type(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    target: &Target,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<Type> {
    let source = registry.source(file)?;
    match target {
        Target::Field { base, name } => member_type(registry, file, *base, name, queries),
        Target::Name(name) => match source.bound().binding_at(name.span.start) {
            Some(binding) => source.resolve_binding(binding, queries).map(|binding| binding.constraint),
            None => {
                let environment = queries.global_environment_type(source.global_environment()?).ok()?;
                queries.type_members(&environment).into_iter().find(|(member, _)| *member == name.text).map(|(_, value)| value)
            }
        },
    }
}

/// What the value at `span` is written as, in `syntax`.
fn value_context(syntax: &luanalysis_syntax::File, span: Span) -> Option<Context> {
    use luanalysis_syntax::{BinaryOperator, TableFieldKind};
    let mut found = None;
    crate::module_requests::visit_expressions(&syntax.statements, &mut |expression: &Expr| {
        if found.is_some() { return; }
        found = match &expression.kind {
            ExprKind::Table(fields) => fields.iter().find(|field| field.value.span == span).and_then(|field| match &field.kind {
                TableFieldKind::Named(name) => Some(Context::Field { table: expression.span, key: luanalysis_engine::LuaKey::String(name.text.as_bytes().to_vec()) }),
                TableFieldKind::Keyed(key) => match &key.kind {
                    ExprKind::String(bytes) => Some(Context::Field { table: expression.span, key: luanalysis_engine::LuaKey::String(bytes.clone()) }),
                    _ => None,
                },
                TableFieldKind::Sequence { index } => Some(Context::Sequence { table: expression.span, index: *index }),
            }),
            ExprKind::Call { callee, args } => args.iter().position(|argument| argument.span == span)
                .map(|index| Context::Argument { callee: Callee::Function(callee.span), index }),
            ExprKind::MethodCall { receiver, name, args } => args.iter().position(|argument| argument.span == span)
                .map(|index| Context::Argument { callee: Callee::Method { receiver: receiver.span, name: name.text.clone() }, index }),
            ExprKind::Parenthesized(inner) if inner.span == span => Some(Context::Through(expression.span)),
            ExprKind::Binary { op: BinaryOperator::Or, left, right, .. } if left.span == span || right.span == span => Some(Context::Through(expression.span)),
            ExprKind::Binary { op: BinaryOperator::And, right, .. } if right.span == span => Some(Context::Through(expression.span)),
            ExprKind::Function(function) => {
                let owner = Owner { span: function.span, declared: declares_results(function), expression: true, target: None };
                statement_context(&function.body, Some(&owner), span)
            }
            _ => None,
        };
    });
    found.or_else(|| statement_context(&syntax.statements, None, span))
}

/// What the value at `span` is written as in `body`'s statements, and those of the blocks and
/// function statements in them (a function value's are another body's), returned from `owner`.
fn statement_context(body: &[luanalysis_syntax::Stmt], owner: Option<&Owner>, span: Span) -> Option<Context> {
    use luanalysis_syntax::StmtKind;
    let position = |values: &[Expr]| values.iter().position(|value| value.span == span);
    for statement in body {
        let found = match &statement.kind {
            StmtKind::Local { names, values, .. } => position(values).and_then(|index| names.get(index)).map(|name| Context::Local { name: name.span }),
            StmtKind::Assign { targets, values, .. } => position(values).and_then(|index| targets.get(index)).and_then(target).map(Context::Assign),
            StmtKind::Return(values) => position(values).map(|index| Context::Return { owner: owner.cloned(), index }),
            StmtKind::Block { body, .. } | StmtKind::While { body, .. } | StmtKind::Repeat { body, .. }
            | StmtKind::NumericFor { body, .. } | StmtKind::GenericFor { body, .. } => statement_context(body, owner, span),
            StmtKind::If { branches, else_branch } => branches.iter().find_map(|branch| statement_context(&branch.body, owner, span))
                .or_else(|| else_branch.as_ref().and_then(|branch| statement_context(&branch.body, owner, span))),
            StmtKind::LocalFunction { function, .. } => {
                let owner = Owner { span: function.span, declared: declares_results(function), expression: false, target: None };
                statement_context(&function.body, Some(&owner), span)
            }
            StmtKind::FunctionDeclaration { target: written, function, .. } => {
                let owner = Owner { span: function.span, declared: declares_results(function), expression: false, target: target(written) };
                statement_context(&function.body, Some(&owner), span)
            }
            _ => None,
        };
        if found.is_some() { return found; }
    }
    None
}

/// An assignment's target that has a type of its own: a name or a field.
fn target(expression: &Expr) -> Option<Target> {
    match &expression.kind {
        ExprKind::Name(name) => Some(Target::Name(name.clone())),
        ExprKind::Field { base, name } => Some(Target::Field { base: base.span, name: name.text.clone() }),
        _ => None,
    }
}

/// Whether a function's documentation declares its results (`---@return`, or a `---@type`).
fn declares_results(function: &luanalysis_syntax::Function) -> bool {
    function.annotations.returns.is_some() || function.annotations.type_annotation.is_some()
}

/// `bytes` as a Lua string literal: UTF-8 text as it is, other bytes and control characters
/// escaped.
fn lua_string(bytes: &[u8]) -> String {
    lua_string_delimited(bytes, '"')
}

fn string_contents(bytes: &[u8], quote: char) -> String {
    let literal = lua_string_delimited(bytes, quote);
    literal[1..literal.len() - 1].to_owned()
}

fn lua_string_delimited(bytes: &[u8], quote: char) -> String {
    let mut literal = quote.to_string();
    let escape = |literal: &mut String, character: char| match character {
        character if character == quote => { literal.push('\\'); literal.push(character); }
        '\\' => literal.push_str("\\\\"),
        '\n' => literal.push_str("\\n"),
        '\r' => literal.push_str("\\r"),
        '\t' => literal.push_str("\\t"),
        character if character.is_control() => {
            for byte in character.to_string().bytes() { literal.push_str(&format!("\\{byte:03}")) }
        }
        character => literal.push(character),
    };
    match std::str::from_utf8(bytes) {
        Ok(text) => for character in text.chars() { escape(&mut literal, character) },
        Err(_) => for &byte in bytes {
            if byte.is_ascii() { escape(&mut literal, byte as char) } else { literal.push_str(&format!("\\{byte:03}")) }
        },
    }
    literal.push(quote);
    literal
}

/// The globals the workspace's scripts implement at the top level of their main chunks
/// (`function name()`, `name = function() end`, or through `_G`), except at `except` in `file`.
fn implemented_globals(project: &Project, registry: &crate::native_functions::project_provider::ProjectRegistry, file: FileId, except: usize) -> Vec<String> {
    let mut names = Vec::new();
    for other in project.files.iter().filter(|other| other.role != luanalysis_project::SourceRole::Declaration) {
        let Some(source) = registry.source(other.id) else { continue };
        let bound = source.bound();
        let global = |target: &Expr| -> Option<Name> {
            match &target.kind {
                ExprKind::Name(name) if bound.binding_at(name.span.start).is_none() => Some(name.clone()),
                ExprKind::Field { base, name } if matches!(&base.kind, ExprKind::Name(root) if root.text == "_G" && bound.binding_at(root.span.start).is_none()) => Some(name.clone()),
                _ => None,
            }
        };
        for statement in &source.syntax().statements {
            let implemented = match &statement.kind {
                luanalysis_syntax::StmtKind::FunctionDeclaration { target, .. } => global(target),
                luanalysis_syntax::StmtKind::Assign { targets, values, .. } => targets.iter().zip(values)
                    .find(|(_, value)| matches!(value.kind, ExprKind::Function(_)))
                    .and_then(|(target, _)| global(target)),
                _ => None,
            };
            if let Some(name) = implemented
                && !(other.id == file && name.span.start <= except && except <= name.span.end)
            {
                names.push(name.text);
            }
        }
    }
    names
}

/// The names of the required parameters of a callable's first signature, after the `hidden`
/// ones a method call passes itself, and whether it takes more after them.
fn required_parameters(value: &Type, hidden: usize, queries: &mut dyn luanalysis_engine::TypeQueries) -> Option<(Vec<String>, bool)> {
    let head = queries.call_signature_types(value).into_iter().next()?;
    let (parameters, _) = crate::describe_signature(&head, queries)?;
    let parameters: Vec<&String> = parameters.iter().skip(hidden).collect();
    let required: Vec<String> = parameters.iter()
        .take_while(|parameter| !parameter.starts_with("...") && !parameter.split(':').next().is_some_and(|name| name.ends_with('?')))
        .map(|parameter| parameter.split(':').next().unwrap_or_default().trim().to_owned())
        .collect();
    let more = parameters.len() > required.len();
    Some((required, more))
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
    for ty in source.probed_types(span) {
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
    if name == PLACEHOLDER { return false; }
    // Use the Lua lexer's identifier policy, including its Unicode names. A
    // separate ASCII filter hid valid members and globals from completions.
    let tokens = luanalysis_syntax::lex(name).tokens;
    matches!(tokens.as_slice(), [token, eof]
        if matches!(&token.kind, TokenKind::Identifier(identifier) if identifier == name)
            && token.span == (Span { start: 0, end: name.len() })
            && matches!(eof.kind, TokenKind::Eof))
}

/// Where signature help was requested: in the argument list of a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureSite {
    /// The editor's text, repaired to parse if it does not.
    pub text: String,
    /// The call's `(`, or the argument delimiter of a shorthand string call, at the
    /// same offset in the editor's text and in `text`.
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

/// The call whose arguments surround a byte offset of `text`, including the contents of
/// quoted and long-string arguments, or `None` in a comment or a function's parameter list.
pub fn signature_site(text: &str, offset: usize) -> Option<SignatureSite> {
    let offset = floor_char_boundary(text, offset);
    // Lex the complete buffer: truncating at the caret makes an otherwise complete string
    // look unterminated. Its punctuation still belongs to the literal, never to the call.
    let tokens = luanalysis_syntax::lex(text).tokens;
    if tokens.iter().any(|token| token.span.start < offset && match &token.kind {
        TokenKind::Comment(comment) => offset < token.span.end
            || offset == token.span.end && comment.kind != CommentKind::Long,
        _ => false,
    }) { return None; }
    let significant: Vec<_> = tokens.iter()
        .filter(|token| token.span.start < offset
            && !matches!(token.kind, TokenKind::Newline | TokenKind::Comment(_) | TokenKind::Eof))
        .collect();
    // Open brackets, each with the commas directly inside it. A string is a single token,
    // even when the caret is in it, so nested calls and long strings count correctly.
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
    let mut call = open.iter().rev().find(|&&(index, _)| {
        matches!(&significant[index].kind, TokenKind::Symbol(symbol) if symbol == "(") && is_call_paren(&significant, index)
    }).map(|&(index, argument)| (significant[index].span.start, argument));
    // Lua also permits `require "name"` and `consume [=[text]=]`. Their string
    // delimiter opens the argument list; prefer this inner call to an enclosing `(`.
    if let Some(literal) = tokens.iter().find(|token| matches!(token.kind, TokenKind::String(_))
        && token.span.start <= offset && offset <= token.span.end)
    {
        let parsed = luanalysis_syntax::parse(text);
        crate::module_requests::visit_expressions(&parsed.statements, &mut |expression: &Expr| {
            let (end, args) = match &expression.kind {
                ExprKind::Call { callee, args } => (callee.span.end, args),
                ExprKind::MethodCall { name, args, .. } => (name.span.end, args),
                _ => return,
            };
            if args.first().is_some_and(|argument| argument.span == literal.span)
                && text.get(end..literal.span.start).is_some_and(|between| between.trim().is_empty())
                && call.is_none_or(|(open, _)| open < literal.span.start)
            { call = Some((literal.span.start, 0)); }
        });
    }
    let (paren, argument) = call?;
    let head = &text[..offset];
    let tail = &text[offset..];
    let mut candidates = vec![text.to_owned(), format!("{head}{PLACEHOLDER}{tail}")];
    // A partially typed literal is closed before its call is repaired. A trailing backslash
    // must not escape the synthetic quote. Long brackets must keep their exact '=' level.
    if let Some(token) = significant.last().filter(|token| matches!(token.kind, TokenKind::String(_)) && offset <= token.span.end) {
        let literal = &text[token.span.start..offset];
        if let Some(closer) = string_closer(literal) {
            let prefix = format!("{head}{closer}");
            let closing = closers(&prefix);
            candidates.push(prefix + &closing);
        }
    }
    for argument in ["", PLACEHOLDER] {
        let prefix = format!("{head}{argument}");
        let closing = closers(&prefix);
        candidates.push(prefix + &closing);
    }
    let text = repaired(candidates, |_| true)?;
    Some(SignatureSite { text, open: paren, argument })
}

/// What completes a prefix of a Lua string token. The lexer, rather than character scanning,
/// identifies the token; this only closes its delimiter for incomplete-buffer recovery.
fn string_closer(token: &str) -> Option<String> {
    let bytes = token.as_bytes();
    match bytes.first() {
        Some(&quote @ (b'"' | b'\'')) => {
            let trailing = bytes.iter().rev().take_while(|&&byte| byte == b'\\').count();
            Some(format!("{}{}", if trailing % 2 == 1 { "\\" } else { "" }, quote as char))
        }
        Some(b'[') => {
            let level = bytes.iter().skip(1).take_while(|&&byte| byte == b'=').count();
            (bytes.get(level + 1) == Some(&b'[')).then(|| format!("]{}]", "=".repeat(level)))
        }
        _ => None,
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
    let options = ProjectCheckOptions { jobs: 1, describe_types: false, inspections: false, ..options };
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
        // Keep the callee's description as a fallback for an undocumented head.
        (ExprKind::Call { callee: written, .. }, _) if matches!(written.kind, ExprKind::Name(_) | ExprKind::Field { .. }) => {
            documentation_at_offset(project, registry, file, written.span.end, queries)
                .or_else(|| value_documentation(project, registry, &callee, queries))
        }
        _ => value_documentation(project, registry, &callee, queries),
    }.unwrap_or_default();
    // A method call passes its receiver as the first argument, which the signatures leave out.
    let hidden = usize::from(receiver.is_some());
    let name = if name.len() > 80 { "function" } else { name };
    let mut signatures = Vec::new();
    let mut shapes = Vec::new();
    for head in &heads {
        let Some((parameters, returns)) = crate::describe_signature(head, queries) else { continue };
        let documentation = value_documentation(project, registry, head, queries)
            .unwrap_or_else(|| documentation.clone());
        let described = {
            let mut summary = documentation.clone();
            summary.params.clear();
            Some(documentation_markdown(&summary, true)).filter(|markdown| !markdown.is_empty())
        };
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
        signatures.push(SignatureInformation { label, parameters: ranges, active_parameter, documentation: described, parameter_documentation });
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
    if !untyped(&alone, queries) { return Some(alone); }
    source.probe_expression(span);
    registry.check_owned_roots(queries, &[file]);
    source.probed_types(span).into_iter().next().or(Some(alone))
}

/// Whether `ty` says nothing of a value: `any` or `unknown`, as the value of an expression
/// checked alone, outside the function that gives it a type, often is.
fn untyped(ty: &Type, queries: &mut dyn luanalysis_engine::TypeQueries) -> bool {
    matches!(ty, Type::Any | Type::Unknown) || matches!(queries.inspect_type(ty), Type::Any | Type::Unknown)
}

/// Documentation as Markdown: the description, then each parameter's and result's.
pub fn documentation_markdown(documentation: &Documentation, parameters: bool) -> String {
    let mut sections = Vec::new();
    if let Some(deprecated) = &documentation.deprecated {
        sections.push(if deprecated.is_empty() { "**Deprecated.**".to_owned() } else { format!("**Deprecated:** {deprecated}") });
    }
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
    let (file, span, inside) = if let Some(key) = queries.native_signature_key(value) {
        registry.signature_site(key)?
    } else if let Some(key) = queries.native_function_key(value) {
        let (file, span) = registry.function_site(key)?;
        (file, span, false)
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
            match if let Type::NativeTypeParameter(_) = ty { ty.clone() } else { queries.inspect_type(&ty) } {
                // Each alternative of a union, or part of an intersection, declares its own.
                Type::Union(parts) | Type::Intersection(parts) => pending.extend(parts),
                // A type parameter's members, such as a method's `self`'s, are its constraint's.
                Type::NativeTypeParameter(key) => pending.extend(queries.type_parameter_constraint(key)),
                _ => {}
            }
            continue;
        };
        if seen.contains(&key) || seen.len() > 32 { continue; }
        seen.push(key);
        // A table's constructor declares the fields it is built with; later stores declare the
        // others.
        if let Some(site) = registry.runtime_table_field_site(key, name) {
            if !sites.contains(&site) { sites.push(site); }
            continue;
        }
        // The members checking applies to this object, its own and then inherited ones: a
        // declaration's candidates include every same-spelled augmentation, of any owner.
        let applied = queries.named_object_member_origins(&ty, name);
        if !applied.is_empty() {
            for member in applied {
                if let Some(site) = registry.member_site(member) && !sites.contains(&site) { sites.push(site); }
            }
            continue;
        }
        for member in registry.type_declaration_members(key) {
            let named = matches!(registry.type_member_header(member).key, TypeMemberKeyRecipe::Fixed(LuaKey::String(bytes)) if bytes == name.as_bytes());
            if named && queries.named_member_owner(member).map_or(true, |owner| owner.declaration == key)
                && let Some(site) = registry.member_site(member) && !sites.contains(&site) { sites.push(site); }
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

/// The documentation of the global `name`: what its declaration's doc comments say, whether it
/// is declared by a statement or a `---@function` tag.
fn global_documentation(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    name: &str,
) -> Option<Documentation> {
    registry.global_sites(name).into_iter().find_map(|(file, span, tag)| {
        let (source, text) = (registry.source(file)?, project.file(file)?);
        let documentation = if tag {
            luanalysis_syntax::documentation_at(source.syntax(), &text.source, span.start)
        } else {
            luanalysis_syntax::documentation_above(source.syntax(), &text.source, span.start)
        };
        (!documentation.is_empty()).then_some(documentation)
    })
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
    let options = ProjectCheckOptions { jobs: 1, describe_types: false, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        let markdown = type_name_hover(project, registry, file, offset).or_else(|| {
            overload_hover_documentation(project, registry, file, offset, queries).or_else(|| {
                documentation_at_offset(project, registry, file, offset, queries)
                    .map(|documentation| documentation_markdown(&documentation, true))
            })
        });
        deliver(markdown.filter(|markdown| !markdown.is_empty()));
    });
}

/// Distinct overload documentation belongs beside its own head. A shared documentation block
/// keeps the ordinary concise hover, while an undocumented head uses the callee's description.
fn overload_hover_documentation(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<String> {
    let source = registry.source(file)?;
    let text = &project.file(file)?.source;
    registry.prepare_documentation_members(queries);
    let contains = |span: Span| span.start <= offset && offset <= span.end;
    let mut member = None;
    let mut name = None;
    crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| match &expression.kind {
        ExprKind::Field { base, name: field } | ExprKind::MethodCall { receiver: base, name: field, .. } if contains(field.span) => {
            member = Some((base.span, field.text.clone()));
        }
        ExprKind::Index { base, index } => if let Some(field) = string_key(text, index) && contains(field.span) {
            member = Some((base.span, field.text));
        },
        ExprKind::Name(_) if contains(expression.span) => name = Some(expression.span),
        _ => {}
    });
    let value = if let Some((receiver, field)) = member {
        let receiver = expression_type(registry, file, receiver, queries)?;
        let receiver = queries.exclude_type(&receiver, &Type::Nil);
        queries.type_members(&receiver).into_iter().find(|(member, _)| *member == field)?.1
    } else {
        expression_type(registry, file, name?, queries)?
    };
    let heads = queries.call_signature_types(&value);
    if heads.len() < 2 { return None; }
    let fallback = documentation_at_offset(project, registry, file, offset, queries).unwrap_or_default();
    let documented: Vec<_> = heads.iter().map(|head| {
        value_documentation(project, registry, head, queries).unwrap_or_else(|| fallback.clone())
    }).collect();
    if documented.windows(2).all(|pair| pair[0] == pair[1]) { return None; }
    let sections: Vec<_> = heads.iter().zip(documented).filter_map(|(head, documentation)| {
        let (parameters, returns) = crate::describe_signature(head, queries)?;
        let label = format!("fun({}): {returns}", parameters.join(", "));
        let markdown = documentation_markdown(&documentation, true);
        Some(if markdown.is_empty() { format!("```lua\n{label}\n```") } else { format!("```lua\n{label}\n```\n\n{markdown}") })
    }).collect();
    (!sections.is_empty()).then(|| sections.join("\n\n---\n\n"))
}

/// A hover on a type name in an annotation: the tag that declares the type, and what its
/// documentation says.
fn type_name_hover(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
) -> Option<String> {
    let text = &project.file(file)?.source;
    let names = luanalysis_syntax::type_names(registry.source(file)?.syntax(), text);
    let at = names.iter().find(|name| name.span.start <= offset && offset <= name.span.end)?;
    let navigation = type_navigation(project, registry, file, at, &names, false)?;
    let (path, span, _) = navigation.occurrences.iter().find(|occurrence| occurrence.2)?;
    let declaration = project.files.iter().find(|other| other.path == *path)?;
    let line_start = declaration.source[..span.start].rfind('\n').map_or(0, |newline| newline + 1);
    let line_end = declaration.source[span.end..].find('\n').map_or(declaration.source.len(), |newline| span.end + newline);
    let tag = declaration.source[line_start..line_end].trim();
    let documentation = luanalysis_syntax::documentation_at(registry.source(declaration.id)?.syntax(), &declaration.source, span.start);
    let mut summary = documentation.clone();
    summary.fields.clear();
    let described = documentation_markdown(&summary, false);
    Some(if described.is_empty() { format!("```lua\n{tag}\n```") } else { format!("```lua\n{tag}\n```\n\n{described}") })
}

fn documentation_at_offset(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<Documentation> {
    let source = registry.source(file)?;
    let text = &project.file(file)?.source;
    registry.prepare_documentation_members(queries);
    let contains = |span: Span| span.start <= offset && offset <= span.end;
    let mut member = None;
    let mut name = None;
    crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| match &expression.kind {
        ExprKind::Field { base, name: field } | ExprKind::MethodCall { receiver: base, name: field, .. } if contains(field.span) => {
            member = Some((base.span, field.text.clone()));
        }
        ExprKind::Index { base, index } => if let Some(field) = string_key(text, index) && contains(field.span) {
            member = Some((base.span, field.text));
        },
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
    let name = name?;
    if let Some(text) = registry.source(file).zip(project.file(file)).and_then(|(_, text)| text.source.get(name.start..name.end))
        && let Some(documentation) = global_documentation(project, registry, text)
    {
        return Some(documentation);
    }
    let value = expression_type(registry, file, name, queries)?;
    value_documentation(project, registry, &value, queries)
}

/// A name the editor navigates from, and where it is declared and used.
#[derive(Clone, Debug, Default)]
pub struct Navigation {
    /// The name at the position, and its span there.
    pub name: String,
    pub span: Span,
    /// Each declaration, and when references were asked for, each use: its file, its name's span,
    /// and whether it declares.
    pub occurrences: Vec<(std::path::PathBuf, Span, bool)>,
    /// The text of each file with an occurrence, which the spans index.
    pub texts: std::collections::HashMap<std::path::PathBuf, String>,
    /// Whether the name is a type's (which may contain `.`) rather than a member's.
    pub type_name: bool,
}

/// Where the member or type name at `offset` in `file` is declared, and with `references`, where
/// it is used throughout the project. Locals and globals are the symbol index's.
pub fn navigate(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    offset: usize,
    references: bool,
    deliver: impl FnOnce(Option<Navigation>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: false, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        deliver(navigation(project, registry, file, offset, references, queries));
    });
}

type Site = (FileId, Span, bool);

fn navigation(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
    references: bool,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<Navigation> {
    let source = registry.source(file)?;
    let text = &project.file(file)?.source;
    let contains = |span: Span| span.start <= offset && offset <= span.end;
    let names = luanalysis_syntax::type_names(source.syntax(), text);
    if let Some(at) = names.iter().find(|name| contains(name.span)) {
        return type_navigation(project, registry, file, at, &names, references);
    }
    registry.prepare_documentation_members(queries);
    let (name, span, sites) = member_target(registry, file, text, offset, queries)?;
    let mut navigation = Navigation { name: name.clone(), span, ..Navigation::default() };
    if references {
        let _ = member_occurrences(project, registry, &name, &sites, &mut navigation, None, queries);
    } else {
        for &(site_file, site, _) in &sites {
            let Some(site_text) = project.file(site_file) else { continue };
            let span = word_in(&site_text.source, site, &name).unwrap_or(site);
            add_occurrence(&mut navigation, &site_text.path, &site_text.source, span, true);
        }
    }
    Some(navigation)
}

fn add_occurrence(navigation: &mut Navigation, path: &std::path::Path, text: &str, span: Span, declaration: bool) {
    let occurrence = (path.to_path_buf(), span, declaration);
    if !navigation.occurrences.contains(&occurrence) { navigation.occurrences.push(occurrence); }
    navigation.texts.entry(path.to_path_buf()).or_insert_with(|| text.to_owned());
}

/// The first occurrence of `word` as a whole word in `span` of `text`.
fn word_in(text: &str, span: Span, word: &str) -> Option<Span> {
    let region = text.get(span.start..span.end)?;
    region.match_indices(word).find(|&(index, _)| {
        let before = region[..index].chars().next_back();
        let after = region[index + word.len()..].chars().next();
        !before.is_some_and(luanalysis_syntax::identifier_continue)
            && !after.is_some_and(luanalysis_syntax::identifier_continue)
    }).map(|(index, _)| Span { start: span.start + index, end: span.start + index + word.len() })
}

/// A type name's declarations and uses. A `---@generic` parameter is its documentation block's;
/// any other name is a type's, declared by `---@class`, `---@shape`, `---@alias` or `---@enum`
/// in any file of the project.
fn type_navigation(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    at: &luanalysis_syntax::TypeName,
    names: &[luanalysis_syntax::TypeName],
    references: bool,
) -> Option<Navigation> {
    use luanalysis_syntax::TypeNameKind;
    let source = project.file(file)?;
    let mut navigation = Navigation { name: at.name.clone(), span: at.span, type_name: true, ..Navigation::default() };
    let shadowed = |names: &[luanalysis_syntax::TypeName], block: Span| names.iter()
        .any(|name| name.kind == TypeNameKind::Generic && name.block == block && name.name == at.name);
    if shadowed(names, at.block) {
        for name in names.iter().filter(|name| name.block == at.block && name.name == at.name) {
            let declaration = name.kind == TypeNameKind::Generic;
            if references || declaration { add_occurrence(&mut navigation, &source.path, &source.source, name.span, declaration); }
        }
        return Some(navigation);
    }
    for other in &project.files {
        let Some(syntax) = registry.source(other.id).map(|source| source.syntax()) else { continue };
        let names = luanalysis_syntax::type_names(syntax, &other.source);
        for name in names.iter().filter(|name| name.name == at.name && name.kind != TypeNameKind::Generic) {
            let declaration = name.kind == TypeNameKind::Declaration;
            if (references || declaration) && !shadowed(&names, name.block) {
                add_occurrence(&mut navigation, &other.path, &other.source, name.span, declaration);
            }
        }
    }
    // A built-in type (`number`, `string`) has no declaration to navigate to.
    navigation.occurrences.iter().any(|(_, _, declaration)| *declaration).then_some(navigation)
}

/// The member named at `offset`: in `receiver.name`, `receiver:name(...)`, a table constructor's
/// `name = value`, a `---@field name` or a `---@function Owner.name`; with its span and the
/// sites that declare it.
fn member_target(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    text: &str,
    offset: usize,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<(String, Span, Vec<Site>)> {
    let source = registry.source(file)?;
    let syntax = source.syntax();
    let contains = |span: Span| span.start <= offset && offset <= span.end;
    for declaration in syntax.type_declarations.iter() {
        if let luanalysis_syntax::NamedTypeDeclarationKind::Record { fields, .. } = &declaration.kind {
            for field in fields {
                if let luanalysis_syntax::TypeFieldKey::Named(name) = &field.key && contains(name.span) {
                    return Some((name.text.clone(), name.span, vec![(file, field.span, true)]));
                }
            }
        }
    }
    for extension in &syntax.type_extensions {
        for field in &extension.fields {
            if let luanalysis_syntax::TypeFieldKey::Named(name) = &field.key && contains(name.span) {
                return Some((name.text.clone(), name.span, vec![(file, field.span, true)]));
            }
        }
    }
    for declaration in &syntax.function_declarations {
        if declaration.owner.is_some() && contains(declaration.name.span) {
            return Some((declaration.name.text.clone(), declaration.name.span, vec![(file, declaration.span, true)]));
        }
    }
    let mut found = None;
    member_uses(syntax, text, &mut |receiver, name| if contains(name.span) { found = Some((receiver, name)); });
    let (receiver, name) = found?;
    let receiver = expression_type(registry, file, receiver, queries)?;
    let receiver = queries.exclude_type(&receiver, &Type::Nil);
    let sites = member_sites(registry, &receiver, &name.text, queries);
    (!sites.is_empty()).then_some((name.text, name.span, sites))
}

/// Where the type of the value at `offset` in `file` is declared: each named type (class, shape,
/// alias) it is, of each alternative of a union and a type parameter's constraint.
pub fn type_definition(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    offset: usize,
    deliver: impl FnOnce(Option<Navigation>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: false, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        deliver(type_definitions(project, registry, file, offset, queries));
    });
}

fn type_definitions(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<Navigation> {
    let source = registry.source(file)?;
    let contains = |span: Span| span.start <= offset && offset <= span.end;
    // The innermost expression at the position, or the local declared there.
    let mut innermost: Option<Span> = None;
    crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| {
        let span = match &expression.kind {
            ExprKind::MethodCall { name, .. } if contains(name.span) => return,
            _ => expression.span,
        };
        if contains(span) && innermost.is_none_or(|other| span.end - span.start < other.end - other.start) { innermost = Some(span); }
    });
    let ty = match innermost {
        Some(span) => expression_type(registry, file, span, queries)?,
        None => source.resolve_binding(source.bound().binding_at(offset)?, queries)?.constraint,
    };
    let mut navigation = Navigation::default();
    let mut pending = vec![ty];
    let mut seen = Vec::new();
    while let Some(ty) = pending.pop() {
        if seen.len() > 32 || seen.contains(&ty) { continue; }
        seen.push(ty.clone());
        if let Some(key) = queries.named_object_declaration(&ty) {
            for (site_file, span) in registry.type_declaration_sites(key) {
                if let Some(site) = project.file(site_file) { add_occurrence(&mut navigation, &site.path, &site.source, span, true); }
            }
            continue;
        }
        match if let Type::NativeTypeParameter(_) = ty { ty.clone() } else { queries.inspect_type(&ty) } {
            Type::Union(parts) | Type::Intersection(parts) => pending.extend(parts.into_iter().rev()),
            Type::NativeTypeParameter(key) => pending.extend(queries.type_parameter_constraint(key)),
            _ => {}
        }
    }
    (!navigation.occurrences.is_empty()).then_some(navigation)
}

/// Every use of the member declared at `sites` in the project: member reads and calls, table
/// constructor keys, `---@field` and `---@function` tags. A use's receiver is typed at its flow
/// position; one whose members are declared elsewhere is another member of the same name.
fn member_occurrences(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    name: &str,
    sites: &[Site],
    navigation: &mut Navigation,
    rename: Option<&str>,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Result<(), RenameFailure> {
    let declares = |file: FileId, span: Span| sites.iter().any(|&(site_file, site, _)| site_file == file && site.start <= span.start && span.end <= site.end);
    for other in &project.files {
        let Some(source) = registry.source(other.id) else { continue };
        let syntax = source.syntax();
        for declaration in &syntax.type_declarations {
            if let luanalysis_syntax::NamedTypeDeclarationKind::Record { fields, .. } = &declaration.kind {
                for field in fields {
                    if let luanalysis_syntax::TypeFieldKey::Named(field_name) = &field.key
                        && field_name.text == name && sites.contains(&(other.id, field.span, true))
                    {
                        add_occurrence(navigation, &other.path, &other.source, field_name.span, true);
                    }
                }
            }
        }
        for extension in &syntax.type_extensions {
            for field in &extension.fields {
                if let luanalysis_syntax::TypeFieldKey::Named(field_name) = &field.key
                    && field_name.text == name && sites.contains(&(other.id, field.span, true))
                {
                    add_occurrence(navigation, &other.path, &other.source, field_name.span, true);
                }
            }
        }
        for declaration in &syntax.function_declarations {
            if declaration.name.text == name && sites.contains(&(other.id, declaration.span, true)) {
                add_occurrence(navigation, &other.path, &other.source, declaration.name.span, true);
            }
        }
        let mut candidates = Vec::new();
        member_uses(syntax, &other.source, &mut |receiver, member| if member.text == name { candidates.push((receiver, member.span)); });
        // Receivers without a type of their own, such as a callback's parameters or a method's
        // `self`, have their types in the frames they are checked in: one check of the file
        // probes them all.
        let mut typed = Vec::new();
        let mut probed = false;
        for (receiver, span) in candidates {
            let mut alone = source.source_check_expression(source.expression_key_for_span(receiver), None, NativeCheckMode::empty(), queries);
            // A name's declared type, such as a method's `self`'s or an annotated parameter's,
            // needs no check.
            if untyped(&alone, queries)
                && let Some(occurrence) = source.bound().occurrence_at(receiver.start).filter(|occurrence| occurrence.span == receiver)
                && let Some(binding) = occurrence.binding.and_then(|binding| source.resolve_binding(binding, queries))
                && !untyped(&binding.constraint, queries)
            {
                alone = binding.constraint;
            }
            if untyped(&alone, queries) {
                source.probe_expression(receiver);
                probed = true;
            }
            typed.push((receiver, span, alone));
        }
        if probed { registry.check_owned_roots(queries, &[other.id]); }
        for (receiver, span, alone) in typed {
            let receiver = match untyped(&alone, queries) {
                true => source.probed_types(receiver).into_iter().next().unwrap_or(alone),
                false => alone,
            };
            let receiver = queries.exclude_type(&receiver, &Type::Nil);
            let found = member_sites(registry, &receiver, name, queries);
            if found.iter().any(|site| sites.contains(site)) {
                add_occurrence(navigation, &other.path, &other.source, span, declares(other.id, span));
                let Some(rename) = rename else { continue };
                let (line, column) = crate::symbols::line_column(&other.source, span.start);
                let at = format!("{}:{line}:{column}", other.path.display());
                // A use that may be another member of the same name too would be renamed for one
                // and not the other.
                if found.iter().any(|site| !sites.contains(site)) {
                    return Err(RenameFailure::new(RenameReason::AmbiguousMember,
                        format!("the {name} at {at} may be another table's {name} too, so renaming it is not safe"))
                        .with_related(other, span));
                }
                if !member_sites(registry, &receiver, rename, queries).is_empty() {
                    return Err(RenameFailure::new(RenameReason::MemberConflict,
                        format!("the value at {at} already has a member named {rename}"))
                        .with_related(other, span));
                }
            }
        }
    }
    Ok(())
}

/// Each member name written in `syntax`, with the span of the value it is a member of: in
/// `receiver.name`, `receiver:name(...)`, `receiver["name"]`, and a table constructor's
/// `name = value` and `["name"] = value`.
fn member_uses(syntax: &luanalysis_syntax::File, text: &str, visit: &mut dyn FnMut(Span, Name)) {
    crate::module_requests::visit_expressions(&syntax.statements, &mut |expression: &Expr| match &expression.kind {
        ExprKind::Field { base, name } | ExprKind::MethodCall { receiver: base, name, .. } => visit(base.span, name.clone()),
        ExprKind::Index { base, index } => if let Some(name) = string_key(text, index) { visit(base.span, name) },
        ExprKind::Table(fields) => for field in fields {
            match &field.kind {
                luanalysis_syntax::TableFieldKind::Named(name) => visit(expression.span, name.clone()),
                luanalysis_syntax::TableFieldKind::Keyed(key) => if let Some(name) = string_key(text, key) { visit(expression.span, name) },
                luanalysis_syntax::TableFieldKind::Sequence { .. } => {}
            }
        },
        _ => {}
    });
}

/// A constant string key that is a Lua name, `"name"` or `'name'`, as the name inside its quotes.
fn string_key(text: &str, key: &Expr) -> Option<Name> {
    let ExprKind::String(bytes) = &key.kind else { return None };
    let written = text.get(key.span.start..key.span.end)?;
    let inner = written.strip_prefix('"').and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| written.strip_prefix('\'').and_then(|rest| rest.strip_suffix('\'')))?;
    (inner.as_bytes() == bytes.as_slice() && crate::symbols::is_name(inner))
        .then(|| Name { text: inner.to_owned(), span: Span::new(key.span.start + 1, key.span.end - 1) })
}

/// The name at `offset` in `file`, and with `name`, every occurrence renaming it
/// to `name` edits; or why it cannot be renamed. A library's names, such as a host API's, are
/// not the project's to rename, and a rename must not merge two members or types, nor make a
/// use refer to something else.
pub fn rename(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    offset: usize,
    name: Option<String>,
    deliver: impl FnOnce(Result<Option<Navigation>, String>) + Send,
) {
    rename_detailed(project, options, workspace, file, offset, name,
        move |result| deliver(result.map_err(|failure| failure.message)));
}

/// Rename with the original source locations and machine-readable reasons for rejections.
pub(crate) fn rename_detailed(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    offset: usize,
    name: Option<String>,
    deliver: impl FnOnce(Result<Option<Navigation>, RenameFailure>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: false, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        deliver(renaming(project, registry, file, offset, name.as_deref(), queries));
    });
}

fn renaming(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
    name: Option<&str>,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Result<Option<Navigation>, RenameFailure> {
    let (Some(source), Some(text)) = (registry.source(file), project.file(file)) else { return Ok(None) };
    let text = &text.source;
    let symbols = crate::symbols::SymbolIndex::from_bound_sources(project.files.iter().filter_map(|file| {
        Some((file, registry.source(file.id)?.bound()))
    }));
    if let Some((_, span)) = symbols.symbol_at(file, offset) {
        let occurrences = match name {
            Some(name) => symbols.rename_detailed(file, offset, name),
            None => symbols.prepare_rename_detailed(file, offset).map(|_| Vec::new()),
        }.map_err(RenameFailure::from)?;
        let mut navigation = Navigation { name: text[span.start..span.end].to_owned(), span, ..Navigation::default() };
        for occurrence in occurrences {
            let Some(source) = project.file(occurrence.file) else { continue };
            add_occurrence(&mut navigation, &source.path, &source.source, occurrence.span, occurrence.definition);
        }
        return Ok(Some(navigation));
    }
    let names = luanalysis_syntax::type_names(source.syntax(), text);
    if let Some(at) = names.iter().find(|type_name| type_name.span.start <= offset && offset <= type_name.span.end) {
        let Some(navigation) = type_navigation(project, registry, file, at, &names, name.is_some()) else { return Ok(None) };
        owned_by_library(project, &navigation)?;
        if let Some(name) = name.filter(|name| *name != at.name) { type_rename_conflict(project, registry, file, at, &names, &navigation, name)?; }
        return Ok(Some(navigation));
    }
    registry.prepare_documentation_members(queries);
    let Some((member, span, sites)) = member_target(registry, file, text, offset, queries) else { return Ok(None) };
    let mut navigation = Navigation { name: member.clone(), span, ..Navigation::default() };
    for &(site_file, site, _) in &sites {
        let Some(site_text) = project.file(site_file) else { continue };
        add_occurrence(&mut navigation, &site_text.path, &site_text.source, word_in(&site_text.source, site, &member).unwrap_or(site), true);
    }
    owned_by_library(project, &navigation)?;
    let Some(name) = name else { return Ok(Some(navigation)) };
    if !crate::symbols::is_name(name) { return Err(RenameFailure::new(RenameReason::InvalidName, format!("{name:?} is not a Lua name"))); }
    let rename = (name != member).then_some(name);
    member_occurrences(project, registry, &member, &sites, &mut navigation, rename, queries)?;
    owned_by_library(project, &navigation)?;
    // A declaring tag's type, such as a `---@field`'s class, must not already have the name.
    if let Some(name) = rename {
        for &(site_file, site, tag) in &sites {
            let Some(owner) = tag.then(|| registry.source(site_file)).flatten().and_then(|source| tag_owner(source.syntax(), site)) else { continue };
            let conflict = project.files.iter().find_map(|other| {
                let span = tag_declares(registry.source(other.id)?.syntax(), &owner, name)?;
                Some((other, span))
            });
            if let Some((other, span)) = conflict {
                return Err(RenameFailure::new(RenameReason::MemberConflict, format!("{owner} already has a member named {name}"))
                    .with_related(other, span));
            }
        }
    }
    Ok(Some(navigation))
}

/// Refuses a rename with an occurrence in a library, such as a `--library` declaration file
/// or the standard library, which the project does not own.
fn owned_by_library(project: &Project, navigation: &Navigation) -> Result<(), RenameFailure> {
    let library = navigation.occurrences.iter().filter_map(|(path, span, definition)| {
        let source = project.file_by_path(path)?;
        (source.role == luanalysis_project::SourceRole::Declaration || !matches!(source.origin, luanalysis_project::SourceOrigin::Filesystem))
            .then_some((source, *span, *definition))
    }).min_by(|a, b| b.2.cmp(&a.2).then(a.0.path.cmp(&b.0.path)).then(a.1.start.cmp(&b.1.start)));
    match library {
        Some((source, span, _)) => Err(RenameFailure::new(RenameReason::LibraryOwned,
            format!("{} is declared by a library and cannot be renamed", navigation.name)).with_related(source, span)),
        None => Ok(()),
    }
}

/// The name of the type whose member the `---@field` or `---@function` at `site` declares.
fn tag_owner(syntax: &luanalysis_syntax::File, site: Span) -> Option<String> {
    for declaration in &syntax.type_declarations {
        if let luanalysis_syntax::NamedTypeDeclarationKind::Record { fields, .. } = &declaration.kind
            && fields.iter().any(|field| field.span == site)
        {
            return Some(declaration.name.text.clone());
        }
    }
    if let Some(extension) = syntax.type_extensions.iter().find(|extension| extension.fields.iter().any(|field| field.span == site)) {
        return Some(extension.owner.text.clone());
    }
    let declaration = syntax.function_declarations.iter().find(|declaration| declaration.span == site)?;
    match declaration.owner.as_ref()? {
        luanalysis_syntax::DocFunctionOwner::Explicit(owner) => Some(owner.text.clone()),
        luanalysis_syntax::DocFunctionOwner::Contextual(id) => syntax.type_declarations.iter().find(|declaration| declaration.id == *id).map(|declaration| declaration.name.text.clone()),
    }
}

/// The span of a member name `syntax` declares for `owner` with a `---@field` or `---@function`.
fn tag_declares(syntax: &luanalysis_syntax::File, owner: &str, name: &str) -> Option<Span> {
    let named = |key: &luanalysis_syntax::TypeFieldKey| match key {
        luanalysis_syntax::TypeFieldKey::Named(field) if field.text == name => Some(field.span),
        _ => None,
    };
    for declaration in &syntax.type_declarations {
        if declaration.name.text == owner
            && let luanalysis_syntax::NamedTypeDeclarationKind::Record { fields, .. } = &declaration.kind
            && let Some(span) = fields.iter().find_map(|field| named(&field.key))
        {
            return Some(span);
        }
    }
    for extension in &syntax.type_extensions {
        if extension.owner.text == owner && let Some(span) = extension.fields.iter().find_map(|field| named(&field.key)) {
            return Some(span);
        }
    }
    syntax.function_declarations.iter().find(|declaration| declaration.name.text == name && match &declaration.owner {
        Some(luanalysis_syntax::DocFunctionOwner::Explicit(explicit)) => explicit.text == owner,
        Some(luanalysis_syntax::DocFunctionOwner::Contextual(id)) => syntax.type_declarations.iter().any(|declaration| declaration.id == *id && declaration.name.text == owner),
        None => false,
    }).map(|declaration| declaration.name.span)
}

/// Why renaming type name `at` of `file` (whose type names are `names`) to `name` would change
/// what a type name refers to: the name is built in or already a type's, a `---@generic` of the
/// name would capture a renamed use, or a renamed `---@generic`'s block already uses the name.
fn type_rename_conflict(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    at: &luanalysis_syntax::TypeName,
    names: &[luanalysis_syntax::TypeName],
    navigation: &Navigation,
    name: &str,
) -> Result<(), RenameFailure> {
    use luanalysis_syntax::TypeNameKind;
    const BUILT_IN: [&str; 12] = ["any", "unknown", "never", "nil", "boolean", "number", "integer", "string", "table", "function", "void", "self"];
    if !name.split('.').all(crate::symbols::is_name) { return Err(RenameFailure::new(RenameReason::InvalidName, format!("{name:?} is not a type name"))); }
    if BUILT_IN.contains(&name) { return Err(RenameFailure::new(RenameReason::BuiltInType, format!("{name} is a built-in type"))); }
    // A `---@generic` and its uses are its documentation block's alone.
    let generic = names.iter().any(|other| other.kind == TypeNameKind::Generic && other.block == at.block && other.name == at.name);
    for other in project.files.iter().filter(|other| !generic || other.id == file) {
        let Some(syntax) = registry.source(other.id).map(|source| source.syntax()) else { continue };
        let names = luanalysis_syntax::type_names(syntax, &other.source);
        let generic_in = |block: Span| names.iter().any(|type_name| type_name.kind == TypeNameKind::Generic && type_name.block == block && type_name.name == name);
        let renamed = |type_name: &luanalysis_syntax::TypeName| navigation.occurrences.iter().any(|(path, span, _)| *path == other.path && *span == type_name.span);
        let conflict = names.iter().find(|type_name| match () {
            _ if generic => type_name.block == at.block && type_name.name == name,
            _ if renamed(type_name) => generic_in(type_name.block),
            _ => type_name.name == name && !generic_in(type_name.block),
        });
        if let Some(conflict) = conflict {
            let (line, column) = crate::symbols::line_column(&other.source, conflict.span.start);
            let at = format!("{}:{line}:{column}", other.path.display());
            let message = match conflict.kind {
                TypeNameKind::Declaration if !generic => format!("a type named {name} is already declared at {at}"),
                _ => format!("renaming to {name} would change what the type name at {at} refers to"),
            };
            return Err(RenameFailure::new(RenameReason::TypeConflict, message).with_related(other, conflict.span));
        }
    }
    Ok(())
}


/// What a document symbol is, as the outline shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Class,
    Interface,
    Alias,
    Module,
    Field,
    Function,
    Method,
    Variable,
    Constant,
}

/// A declaration in a document's outline, with the declarations inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: SymbolKind,
    /// The whole declaration, and its name within it.
    pub range: Span,
    pub selection: Span,
    pub children: Vec<DocumentSymbol>,
}

/// The outline of the current source, with runtime members grouped under their lexical
/// owner (including the nominal class attached to a differently named runtime local).
/// A parent's range contains all its children, even when members are declared later.
pub fn document_symbols(text: &str) -> Vec<DocumentSymbol> {
    let file = luanalysis_syntax::parse(text);
    let mut outline = Outline {
        text,
        declarations: &file.type_declarations,
        type_nodes: vec![None; file.type_declarations.len()],
        nodes: Vec::new(),
        roots: Vec::new(),
        scopes: Vec::new(),
    };
    let scope = outline.scope(None, None, file.span, true);
    outline.statements(&file.statements, scope);
    outline.finish(None)
}

struct OutlineNode {
    symbol: DocumentSymbol,
    parent: Option<usize>,
    children: Vec<usize>,
}

struct OutlineScope {
    parent: Option<usize>,
    owner: Option<usize>,
    span: Span,
    direct_locals: bool,
    // A hidden block local still shadows an outer outline owner.
    bindings: Vec<(String, Option<usize>)>,
}

struct Outline<'a> {
    text: &'a str,
    declarations: &'a [luanalysis_syntax::NamedTypeDeclaration],
    type_nodes: Vec<Option<usize>>,
    nodes: Vec<OutlineNode>,
    roots: Vec<usize>,
    scopes: Vec<OutlineScope>,
}

impl Outline<'_> {
    fn slice(&self, span: Span) -> String {
        self.text.get(span.start..span.end).unwrap_or_default().to_owned()
    }

    fn scope(&mut self, parent: Option<usize>, owner: Option<usize>, span: Span, direct_locals: bool) -> usize {
        let id = self.scopes.len();
        self.scopes.push(OutlineScope { parent, owner, span, direct_locals, bindings: Vec::new() });
        id
    }

    fn bind(&mut self, scope: usize, name: String, node: Option<usize>) {
        self.scopes[scope].bindings.push((name, node));
    }

    fn lookup(&self, mut scope: usize, name: &str) -> Option<Option<usize>> {
        loop {
            if let Some((_, node)) = self.scopes[scope].bindings.iter().rev().find(|(binding, _)| binding == name) {
                return Some(*node);
            }
            scope = self.scopes[scope].parent?;
        }
    }

    fn add(&mut self, parent: Option<usize>, symbol: DocumentSymbol) -> usize {
        let id = self.nodes.len();
        if let Some(parent) = parent { self.nodes[parent].children.push(id); } else { self.roots.push(id); }
        self.nodes.push(OutlineNode { symbol, parent, children: Vec::new() });
        id
    }

    fn child(&self, parent: usize, name: &str) -> Option<usize> {
        self.nodes[parent].children.iter().copied().find(|&child| self.nodes[child].symbol.name == name)
    }

    fn symbol(name: String, kind: SymbolKind, range: Span, selection: Span) -> DocumentSymbol {
        DocumentSymbol { name, detail: None, kind, range, selection, children: Vec::new() }
    }

    /// Install documentation in source order before its following runtime statement. Nested
    /// scopes consume their own declarations before their parent reaches the next statement.
    fn types_before(&mut self, end: usize, scope: usize) {
        let declarations = self.declarations;
        for (index, declaration) in declarations.iter().enumerate() {
            if self.type_nodes[index].is_some() || declaration.span.start < self.scopes[scope].span.start || declaration.group_span.end > end {
                continue;
            }
            let kind = match &declaration.kind {
                luanalysis_syntax::NamedTypeDeclarationKind::Alias { .. } => SymbolKind::Alias,
                luanalysis_syntax::NamedTypeDeclarationKind::Record { flavor, .. } => match flavor {
                    luanalysis_syntax::NamedRecordKind::Class => SymbolKind::Class,
                    luanalysis_syntax::NamedRecordKind::Shape => SymbolKind::Interface,
                    luanalysis_syntax::NamedRecordKind::Module => SymbolKind::Module,
                },
            };
            let mut symbol = Self::symbol(declaration.name.text.clone(), kind, declaration.group_span, declaration.name.span);
            if let luanalysis_syntax::NamedTypeDeclarationKind::Alias { target } = &declaration.kind { symbol.detail = Some(self.slice(target.span)); }
            let node = self.add(self.scopes[scope].owner, symbol);
            self.type_nodes[index] = Some(node);
            if kind != SymbolKind::Alias { self.bind(scope, declaration.name.text.clone(), Some(node)); }
            if let luanalysis_syntax::NamedTypeDeclarationKind::Record { fields, .. } = &declaration.kind {
                for field in fields {
                    if let luanalysis_syntax::TypeFieldKey::Named(name) = &field.key {
                        let mut symbol = Self::symbol(name.text.clone(), SymbolKind::Field, field.span, name.span);
                        symbol.detail = Some(self.slice(field.ty.span));
                        self.add(Some(node), symbol);
                    }
                }
            }
        }
    }

    fn attached_records(&self, span: Span) -> Vec<usize> {
        self.declarations.iter().enumerate().filter_map(|(index, declaration)| {
            (declaration.attachment == Some(span) && matches!(declaration.kind, luanalysis_syntax::NamedTypeDeclarationKind::Record { .. }))
                .then_some(self.type_nodes[index]).flatten()
        }).collect()
    }

    fn statements(&mut self, statements: &[luanalysis_syntax::Stmt], scope: usize) {
        use luanalysis_syntax::StmtKind;
        for statement in statements {
            self.types_before(statement.span.start, scope);
            let records = self.attached_records(statement.span);
            match &statement.kind {
                StmtKind::LocalFunction { name, function } => {
                    let node = self.add(self.scopes[scope].owner, Self::symbol(name.text.clone(), SymbolKind::Function, statement.span, name.span));
                    self.bind(scope, name.text.clone(), Some(node));
                    self.function(node, function, scope, None);
                }
                StmtKind::FunctionDeclaration { target, function, method_span } => {
                    let Some(path) = outline_path(target) else { continue };
                    let Some((node, receiver)) = self.target(&path, scope, SymbolKind::Function, statement.span) else { continue };
                    if path.len() > 1 || method_span.is_some() { self.nodes[node].symbol.kind = SymbolKind::Method; }
                    self.function(node, function, scope, method_span.and(receiver));
                }
                StmtKind::Local { names, attributes, values, .. } => {
                    let mut bindings = Vec::with_capacity(names.len());
                    for (index, name) in names.iter().enumerate() {
                        let value = values.get(index);
                        let function = value.is_some_and(|value| matches!(value.kind, ExprKind::Function(_)));
                        if !self.scopes[scope].direct_locals && !function && records.get(index).is_none() {
                            bindings.push((name.text.clone(), None));
                            if let Some(value) = value { self.nested(value, scope); }
                            continue;
                        }
                        let node = if let Some(&record) = records.get(index) {
                            self.nodes[record].symbol.range = self.nodes[record].symbol.range.join(statement.span);
                            record
                        } else {
                            let constant = attributes.get(index).and_then(Option::as_ref).is_some_and(|attribute| attribute.text == "const");
                            self.add(self.scopes[scope].owner, Self::symbol(name.text.clone(), if constant { SymbolKind::Constant } else { SymbolKind::Variable }, statement.span, name.span))
                        };
                        self.value(node, value, scope);
                        bindings.push((name.text.clone(), Some(node)));
                    }
                    // Every initializer sees the old scope, before any names in this local
                    // declaration enter it (including functions capturing a sibling name).
                    for value in values.iter().skip(names.len()) { self.nested(value, scope); }
                    for (name, node) in bindings { self.bind(scope, name, node); }
                }
                StmtKind::Assign { targets, values, .. } => {
                    for (index, target) in targets.iter().enumerate() {
                        let Some(path) = outline_path(target) else {
                            if let Some(value) = values.get(index) { self.nested(value, scope); }
                            continue;
                        };
                        let node = if let Some(&record) = records.get(index) {
                            self.nodes[record].symbol.range = self.nodes[record].symbol.range.join(statement.span);
                            if path.len() == 1 { self.bind(scope, path[0].0.clone(), Some(record)); }
                            record
                        } else {
                            let Some((node, _)) = self.target(&path, scope, SymbolKind::Variable, statement.span) else { continue };
                            node
                        };
                        self.value(node, values.get(index), scope);
                    }
                    for value in values.iter().skip(targets.len()) { self.nested(value, scope); }
                }
                StmtKind::Block { body, body_span } => {
                    let nested = self.scope(Some(scope), self.scopes[scope].owner, *body_span, false);
                    self.statements(body, nested);
                }
                StmtKind::While { condition, body, body_span } => {
                    self.nested(condition, scope);
                    let nested = self.scope(Some(scope), self.scopes[scope].owner, *body_span, false);
                    self.statements(body, nested);
                }
                StmtKind::Repeat { body, body_span, condition } => {
                    let nested = self.scope(Some(scope), self.scopes[scope].owner, *body_span, false);
                    self.statements(body, nested);
                    self.nested(condition, nested);
                }
                StmtKind::NumericFor { name, start, limit, step, body, body_span, .. } => {
                    self.nested(start, scope);
                    self.nested(limit, scope);
                    if let Some(step) = step { self.nested(step, scope); }
                    let nested = self.scope(Some(scope), self.scopes[scope].owner, *body_span, false);
                    self.bind(nested, name.text.clone(), None);
                    self.statements(body, nested);
                }
                StmtKind::GenericFor { names, values, body, body_span, .. } => {
                    for value in values { self.nested(value, scope); }
                    let nested = self.scope(Some(scope), self.scopes[scope].owner, *body_span, false);
                    for name in names { self.bind(nested, name.text.clone(), None); }
                    self.statements(body, nested);
                }
                StmtKind::If { branches, else_branch } => {
                    for branch in branches {
                        self.nested(&branch.condition, scope);
                        let nested = self.scope(Some(scope), self.scopes[scope].owner, branch.body_span, false);
                        self.statements(&branch.body, nested);
                    }
                    if let Some(branch) = else_branch {
                        let nested = self.scope(Some(scope), self.scopes[scope].owner, branch.body_span, false);
                        self.statements(&branch.body, nested);
                    }
                }
                StmtKind::Call(value) => self.nested(value, scope),
                StmtKind::Return(values) => for value in values { self.nested(value, scope); },
                _ => {}
            }
        }
        self.types_before(self.scopes[scope].span.end, scope);
    }

    /// Resolve each written path through the nearest lexical binding. Unknown namespace
    /// prefixes get real source selections; a shadowing hidden local never picks an outer class.
    fn target(&mut self, path: &[(String, Span)], scope: usize, kind: SymbolKind, range: Span) -> Option<(usize, Option<usize>)> {
        let (name, selection) = path.first()?;
        let root = match self.lookup(scope, name) {
            Some(Some(node)) => node,
            Some(None) => return None,
            None => {
                let node = self.add(self.scopes[scope].owner, Self::symbol(name.clone(), if path.len() > 1 { SymbolKind::Variable } else { kind }, range, *selection));
                self.bind(scope, name.clone(), Some(node));
                node
            }
        };
        let mut node = root;
        let mut receiver = None;
        for (name, selection) in &path[1..] {
            receiver = Some(node);
            node = self.child(node, name).unwrap_or_else(|| self.add(Some(node), Self::symbol(name.clone(), SymbolKind::Field, range, *selection)));
        }
        self.nodes[node].symbol.range = self.nodes[node].symbol.range.join(range);
        if kind == SymbolKind::Function { self.nodes[node].symbol.kind = kind; }
        Some((node, receiver))
    }

    fn function(&mut self, node: usize, function: &luanalysis_syntax::Function, parent: usize, receiver: Option<usize>) {
        self.nodes[node].symbol.detail = self.text.get(function.span.start..function.body_span.start).and_then(|header| {
            let open = header.find('(')?;
            Some(header[open..].trim_end().to_owned())
        });
        let scope = self.scope(Some(parent), Some(node), function.body_span, true);
        if let Some(receiver) = receiver { self.bind(scope, "self".into(), Some(receiver)); }
        for parameter in &function.params { self.bind(scope, parameter.text.clone(), None); }
        self.statements(&function.body, scope);
    }

    fn value(&mut self, node: usize, value: Option<&Expr>, scope: usize) {
        let Some(value) = value else { return };
        match &value.kind {
            ExprKind::Parenthesized(inner) => self.value(node, Some(inner), scope),
            ExprKind::Function(function) => {
                if !matches!(self.nodes[node].symbol.kind, SymbolKind::Class | SymbolKind::Interface | SymbolKind::Module) {
                    self.nodes[node].symbol.kind = if self.nodes[node].parent.is_some_and(|parent| !matches!(self.nodes[parent].symbol.kind, SymbolKind::Function | SymbolKind::Method)) { SymbolKind::Method } else { SymbolKind::Function };
                }
                self.function(node, function, scope, None);
            }
            ExprKind::Table(fields) => {
                let table_scope = self.scope(Some(scope), Some(node), value.span, false);
                for field in fields {
                    let key = match &field.kind {
                        luanalysis_syntax::TableFieldKind::Named(name) => Some((name.text.clone(), name.span)),
                        luanalysis_syntax::TableFieldKind::Keyed(key) => outline_literal_key(key),
                        _ => None,
                    };
                    if let Some((name, selection)) = key {
                        let child = self.child(node, &name).unwrap_or_else(|| self.add(Some(node), Self::symbol(name.clone(), SymbolKind::Field, field.span, selection)));
                        self.nodes[child].symbol.range = self.nodes[child].symbol.range.join(field.span);
                        self.value(child, Some(&field.value), table_scope);
                    } else {
                        self.nested(&field.value, table_scope);
                    }
                }
            }
            _ => self.nested(value, scope),
        }
    }

    fn nested(&mut self, value: &Expr, scope: usize) {
        match &value.kind {
            ExprKind::Function(function) => {
                let selection = Span { start: value.span.start, end: (value.span.start + "function".len()).min(value.span.end) };
                let node = self.add(self.scopes[scope].owner, Self::symbol("<anonymous>".into(), SymbolKind::Function, value.span, selection));
                self.function(node, function, scope, None);
            }
            ExprKind::Call { callee, args } => {
                self.nested(callee, scope);
                for arg in args { self.nested(arg, scope); }
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                self.nested(receiver, scope);
                for arg in args { self.nested(arg, scope); }
            }
            ExprKind::Table(fields) => for field in fields { self.nested(&field.value, scope); },
            ExprKind::Parenthesized(inner) | ExprKind::Field { base: inner, .. } | ExprKind::Unary { value: inner, .. } => self.nested(inner, scope),
            ExprKind::Index { base, index } => { self.nested(base, scope); self.nested(index, scope); }
            ExprKind::Binary { left, right, .. } => { self.nested(left, scope); self.nested(right, scope); }
            _ => {}
        }
    }

    fn finish(&self, parent: Option<usize>) -> Vec<DocumentSymbol> {
        let children = parent.map(|parent| &self.nodes[parent].children).unwrap_or(&self.roots);
        let mut symbols: Vec<_> = children.iter().map(|&id| {
            let mut symbol = self.nodes[id].symbol.clone();
            symbol.children = self.finish(Some(id));
            symbol.range = symbol.children.iter().fold(symbol.range, |range, child| range.join(child.range));
            symbol
        }).collect();
        symbols.sort_by_key(|symbol| (symbol.range.start, symbol.selection.start));
        symbols
    }
}

fn outline_path(expression: &Expr) -> Option<Vec<(String, Span)>> {
    match &expression.kind {
        ExprKind::Name(name) => Some(vec![(name.text.clone(), name.span)]),
        ExprKind::Field { base, name } => {
            let mut path = outline_path(base)?;
            path.push((name.text.clone(), name.span));
            Some(path)
        }
        ExprKind::Index { base, index } => {
            let mut path = outline_path(base)?;
            path.push(outline_literal_key(index)?);
            Some(path)
        }
        _ => None,
    }
}

fn outline_literal_key(expression: &Expr) -> Option<(String, Span)> {
    match &expression.kind {
        ExprKind::String(value) => Some((String::from_utf8(value.clone()).ok()?, expression.span)),
        ExprKind::Number(value) => Some((format!("[{value}]"), expression.span)),
        _ => None,
    }
}

/// A range of a document that an editor can fold: from `start` to `end` (byte offsets), with
/// whether it is a comment or a `--region`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    pub start: usize,
    pub end: usize,
    pub kind: Option<FoldKind>,
    /// Code blocks keep their final `end`, `else` or `}` line visible. Literal strings,
    /// comments and regions fold through their final line, even without an LSP fold kind.
    pub keep_last_line_visible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind {
    Comment,
    Region,
}

/// The foldable ranges of the current Lua source: functions, blocks, multiline tables and
/// strings, comment runs, and matched regions. Region markers accept whitespace after `--`,
/// optional `#`, and either `region`/`endregion` or `{{{`/`}}}`.
pub fn folding_ranges(text: &str) -> Vec<Fold> {
    let file = luanalysis_syntax::parse(text);
    let mut folds = Vec::new();
    statement_folds(&file.statements, &mut folds);
    crate::module_requests::visit_expressions(&file.statements, &mut |expression: &Expr| {
        if matches!(expression.kind, ExprKind::Table(_) | ExprKind::Function(_)) {
            folds.push(Fold { start: expression.span.start, end: expression.span.end, kind: None, keep_last_line_visible: true });
        }
        if let ExprKind::Function(function) = &expression.kind { statement_folds(&function.body, &mut folds); }
    });
    // Use lexical spans so long strings still fold in a partially typed statement, without
    // treating region-looking content inside a string as a real comment marker.
    for token in luanalysis_syntax::lex(text).tokens {
        if matches!(token.kind, TokenKind::String(_)) {
            folds.push(Fold { start: token.span.start, end: token.span.end, kind: None, keep_last_line_visible: false });
        }
    }
    let mut regions = Vec::new();
    let mut run: Option<Span> = None;
    let flush = |run: &mut Option<Span>, folds: &mut Vec<Fold>| {
        if let Some(span) = run.take() { folds.push(Fold { start: span.start, end: span.end, kind: Some(FoldKind::Comment), keep_last_line_visible: false }); }
    };
    for comment in &file.comments {
        let line_comment = matches!(comment.kind, CommentKind::Line | CommentKind::Documentation) && comment.text.starts_with("--");
        let body = comment.text.strip_prefix("--").unwrap_or_default().trim_start_matches('-').trim_start();
        let body = body.trim_start_matches('#').trim_start();
        let marker = |word: &str| body.strip_prefix(word).is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace));
        if line_comment && (marker("region") || body.starts_with("{{{")) {
            flush(&mut run, &mut folds);
            regions.push(comment.span.start);
            continue;
        }
        if line_comment && (marker("endregion") || body.starts_with("}}}")) {
            flush(&mut run, &mut folds);
            if let Some(start) = regions.pop() { folds.push(Fold { start, end: comment.span.end, kind: Some(FoldKind::Region), keep_last_line_visible: false }); }
            continue;
        }
        if !line_comment {
            flush(&mut run, &mut folds);
            folds.push(Fold { start: comment.span.start, end: comment.span.end, kind: Some(FoldKind::Comment), keep_last_line_visible: false });
            continue;
        }
        // Consecutive line comments fold together, accepting Lua's LF, CRLF and CR newlines.
        run = Some(match run {
            Some(span) if text.get(span.end..comment.span.start).is_some_and(|gap| gap.trim().is_empty() && gap.replace("\r\n", "\n").chars().filter(|ch| matches!(*ch, '\r' | '\n')).count() <= 1) => span.join(comment.span),
            Some(span) => {
                folds.push(Fold { start: span.start, end: span.end, kind: Some(FoldKind::Comment), keep_last_line_visible: false });
                comment.span
            }
            None => comment.span,
        });
    }
    flush(&mut run, &mut folds);
    // A fold must hide at least part of another line.
    folds.retain(|fold| text.get(fold.start..fold.end).is_some_and(|folded| folded.trim_end().contains(['\r', '\n'])));
    folds.sort_by_key(|fold| (fold.start, fold.end));
    folds.dedup_by_key(|fold| (fold.start, fold.end));
    folds
}

fn statement_folds(statements: &[luanalysis_syntax::Stmt], folds: &mut Vec<Fold>) {
    use luanalysis_syntax::StmtKind;
    for statement in statements {
        match &statement.kind {
            StmtKind::LocalFunction { .. } | StmtKind::FunctionDeclaration { .. } | StmtKind::Block { .. }
                | StmtKind::While { .. } | StmtKind::Repeat { .. } | StmtKind::NumericFor { .. } | StmtKind::GenericFor { .. } => {
                folds.push(Fold { start: statement.span.start, end: statement.span.end, kind: None, keep_last_line_visible: true });
            }
            StmtKind::If { branches, else_branch } => {
                // Each branch folds to the next, so `else` stays visible.
                for branch in branches { folds.push(Fold { start: branch.span.start, end: branch.body_span.end, kind: None, keep_last_line_visible: true }); }
                if let Some(branch) = else_branch { folds.push(Fold { start: branch.span.start, end: branch.body_span.end, kind: None, keep_last_line_visible: true }); }
            }
            _ => {}
        }
        match &statement.kind {
            StmtKind::LocalFunction { function, .. } | StmtKind::FunctionDeclaration { function, .. } => statement_folds(&function.body, folds),
            StmtKind::Block { body, .. } | StmtKind::While { body, .. } | StmtKind::Repeat { body, .. }
                | StmtKind::NumericFor { body, .. } | StmtKind::GenericFor { body, .. } => statement_folds(body, folds),
            StmtKind::If { branches, else_branch } => {
                for branch in branches { statement_folds(&branch.body, folds); }
                if let Some(branch) = else_branch { statement_folds(&branch.body, folds); }
            }
            _ => {}
        }
    }
}

/// For each offset, the spans that an editor's selection grows through: the name or expression
/// there, each expression and statement around it, and the whole source.
pub fn selection_ranges(text: &str, offsets: &[usize]) -> Vec<Vec<Span>> {
    let file = luanalysis_syntax::parse(text);
    let mut spans = Vec::new();
    statement_spans(&file.statements, &mut spans);
    crate::module_requests::visit_expressions(&file.statements, &mut |expression: &Expr| {
        spans.push(expression.span);
        if let ExprKind::Field { name, .. } | ExprKind::MethodCall { name, .. } = &expression.kind { spans.push(name.span); }
    });
    spans.push(Span { start: 0, end: text.len() });
    offsets.iter().map(|&offset| {
        let mut around: Vec<Span> = spans.iter().copied().filter(|span| span.start <= offset && offset <= span.end && span.start < span.end).collect();
        around.sort_by_key(|span| (span.end - span.start, std::cmp::Reverse(span.start)));
        around.dedup();
        // Each range must contain the one before it.
        let mut chain: Vec<Span> = Vec::new();
        for span in around {
            if chain.last().is_none_or(|inner| span.start <= inner.start && inner.end <= span.end && span != *inner) { chain.push(span); }
        }
        chain
    }).collect()
}

fn statement_spans(statements: &[luanalysis_syntax::Stmt], spans: &mut Vec<Span>) {
    use luanalysis_syntax::StmtKind;
    for statement in statements {
        spans.push(statement.span);
        match &statement.kind {
            StmtKind::LocalFunction { function, .. } | StmtKind::FunctionDeclaration { function, .. } => {
                spans.push(function.body_span);
                statement_spans(&function.body, spans);
            }
            StmtKind::Block { body, body_span } | StmtKind::While { body, body_span, .. } | StmtKind::Repeat { body, body_span, .. }
                | StmtKind::NumericFor { body, body_span, .. } | StmtKind::GenericFor { body, body_span, .. } => {
                spans.push(*body_span);
                statement_spans(body, spans);
            }
            StmtKind::If { branches, else_branch } => {
                for branch in branches { spans.push(branch.span); spans.push(branch.body_span); statement_spans(&branch.body, spans); }
                if let Some(branch) = else_branch { spans.push(branch.span); spans.push(branch.body_span); statement_spans(&branch.body, spans); }
            }
            _ => {}
        }
    }
}

/// The names a source declares without a written type whose type is not plain from their
/// initializer: locals without `---@type` holding something other than a literal, a table or a
/// function, and generic `for` variables. An inlay hint shows each one's type.
pub fn untyped_declarations(text: &str) -> Vec<Span> {
    fn collect(statements: &[luanalysis_syntax::Stmt], names: &mut Vec<Span>) {
        use luanalysis_syntax::StmtKind;
        for statement in statements {
            match &statement.kind {
                StmtKind::Local { names: declared, values, annotation: None, .. } => {
                    for (index, name) in declared.iter().enumerate() {
                        // Extra names take the last value's further results.
                        let value = values.get(index).or(if index >= values.len() { values.last() } else { None });
                        let plain = value.is_some_and(|value| matches!(value.kind,
                            ExprKind::Nil | ExprKind::Boolean(_) | ExprKind::Number(_) | ExprKind::String(_) | ExprKind::Table(_) | ExprKind::Function(_)));
                        if value.is_some() && !plain { names.push(name.span); }
                    }
                }
                StmtKind::GenericFor { names: declared, .. } => names.extend(declared.iter().map(|name| name.span)),
                _ => {}
            }
            match &statement.kind {
                StmtKind::LocalFunction { function, .. } | StmtKind::FunctionDeclaration { function, .. } => collect(&function.body, names),
                StmtKind::Block { body, .. } | StmtKind::While { body, .. } | StmtKind::Repeat { body, .. }
                    | StmtKind::NumericFor { body, .. } | StmtKind::GenericFor { body, .. } => collect(body, names),
                StmtKind::If { branches, else_branch } => {
                    for branch in branches { collect(&branch.body, names); }
                    if let Some(branch) = else_branch { collect(&branch.body, names); }
                }
                _ => {}
            }
        }
    }
    let file = luanalysis_syntax::parse(text);
    let mut names = Vec::new();
    collect(&file.statements, &mut names);
    // Functions written as values hold statements too.
    crate::module_requests::visit_expressions(&file.statements, &mut |expression: &Expr| {
        if let ExprKind::Function(function) = &expression.kind { collect(&function.body, &mut names); }
    });
    names.sort_by_key(|span| span.start);
    names.dedup();
    names
}

/// Controls the independently useful inlay categories. Existing clients keep the quiet
/// literal-argument and non-obvious-local defaults; contextual parameters and results opt in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InlayOptions {
    pub variable_types: bool,
    pub parameter_names: bool,
    /// Include binary/unary expressions, tables and function values, as the legacy editor did.
    pub expression_parameters: bool,
    pub parameter_types: bool,
    pub return_types: bool,
}

impl Default for InlayOptions {
    fn default() -> Self {
        Self { variable_types: true, parameter_names: true, expression_parameters: false, parameter_types: false, return_types: false }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlayHint {
    pub offset: usize,
    pub label: String,
    /// A parameter name before an argument (LSP kind 2); otherwise a type (kind 1).
    pub parameter: bool,
}

/// Inlays from one native session over the current project, including its unsaved overlays.
/// Types come from the same canonical body checks that publish diagnostics, so callback
/// parameters keep their contextual types and a stale diagnostics snapshot is unnecessary.
pub fn inlay_hints(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    range: Range<usize>,
    preferences: InlayOptions,
    deliver: impl FnOnce(Vec<InlayHint>) + Send,
) {
    let needs_types = preferences.variable_types || preferences.parameter_types || preferences.return_types;
    let options = ProjectCheckOptions { jobs: 1, describe_types: needs_types, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        let mut hints = Vec::new();
        let Some(source) = registry.source(file) else { return deliver(hints) };
        let text = &project.file(file).expect("a registered source belongs to the project").source;
        let functions = inlay_functions(source.syntax());
        // Probe function expressions before checking, when the call or assignment supplies
        // their expected type. A raw NativeFunction alone cannot describe that context.
        if preferences.return_types {
            for function in &functions { source.probe_expression(function.span); }
        }
        if needs_types {
            let reports = registry.check_owned_roots(queries, &[file]);
            if let Some(report) = reports.iter().find(|report| report.file == file) {
                let variables = if preferences.variable_types { untyped_declarations(text) } else { Vec::new() };
                for binding in &report.report.bindings {
                    let local = variables.contains(&binding.span);
                    let parameter = preferences.parameter_types && functions.iter().any(|function| {
                        function.annotations.type_annotation.is_none()
                            && function.params.iter().any(|name| name.span == binding.span)
                            && !function.annotations.params.iter().any(|annotation| annotation.name.text == binding.name)
                    });
                    if (local || parameter) && range.contains(&binding.span.end)
                        && let Some(label) = inlay_type_label(&binding.ty)
                    {
                        hints.push(InlayHint { offset: binding.span.end, label, parameter: false });
                    }
                }
            }
        }
        if preferences.return_types {
            for function in &functions {
                let probed = source.probed_types(function.span);
                if !range.contains(&function.body_span.start) || function.annotations.returns.is_some()
                    || function.annotations.type_annotation.is_some() || !function.annotations.overloads.is_empty()
                { continue; }
                let value = probed.into_iter().next().or_else(|| source.function_key_at(function.span).map(Type::NativeFunction));
                if let Some((_, returns)) = value.and_then(|value| crate::describe_signature(&value, queries))
                    && returns != "void"
                    && let Some(label) = inlay_type_label(&returns)
                {
                    hints.push(InlayHint { offset: function.body_span.start, label, parameter: false });
                }
            }
        }
        if preferences.parameter_names {
            hints.extend(argument_parameter_names(registry, file, range, preferences.expression_parameters, queries)
                .into_iter().map(|(offset, name)| InlayHint { offset, label: format!("{name}:"), parameter: true }));
        }
        hints.sort_by(|left, right| (left.offset, left.parameter, &left.label).cmp(&(right.offset, right.parameter, &right.label)));
        hints.dedup();
        deliver(hints);
    });
}

fn inlay_type_label(ty: &str) -> Option<String> {
    if matches!(ty, "any" | "unknown" | "never") { return None; }
    let ty = if ty.chars().count() > 60 { format!("{}…", ty.chars().take(59).collect::<String>()) } else { ty.to_owned() };
    Some(format!(": {ty}"))
}

fn inlay_functions(syntax: &luanalysis_syntax::File) -> Vec<Box<luanalysis_syntax::Function>> {
    fn collect(statements: &[luanalysis_syntax::Stmt], functions: &mut Vec<Box<luanalysis_syntax::Function>>) {
        use luanalysis_syntax::StmtKind;
        for statement in statements {
            match &statement.kind {
                StmtKind::LocalFunction { function, .. } | StmtKind::FunctionDeclaration { function, .. } => {
                    functions.push(function.clone());
                    collect(&function.body, functions);
                }
                StmtKind::Block { body, .. } | StmtKind::While { body, .. } | StmtKind::Repeat { body, .. }
                    | StmtKind::NumericFor { body, .. } | StmtKind::GenericFor { body, .. } => collect(body, functions),
                StmtKind::If { branches, else_branch } => {
                    for branch in branches { collect(&branch.body, functions); }
                    if let Some(branch) = else_branch { collect(&branch.body, functions); }
                }
                _ => {}
            }
        }
    }
    let mut functions = Vec::new();
    collect(&syntax.statements, &mut functions);
    crate::module_requests::visit_expressions(&syntax.statements, &mut |expression: &Expr| {
        if let ExprKind::Function(function) = &expression.kind {
            functions.push(function.clone());
            collect(&function.body, &mut functions);
        }
    });
    functions.sort_by_key(|function| (function.span.start, function.span.end));
    functions.dedup_by_key(|function| function.span);
    functions
}

/// Compatibility entry point for callers asking only for literal argument parameter names.
pub fn parameter_hints(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    range: Range<usize>,
    deliver: impl FnOnce(Vec<(usize, String)>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: false, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |_, registry, queries| {
        deliver(argument_parameter_names(registry, file, range, false, queries));
    });
}

fn argument_parameter_names(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    range: Range<usize>,
    expressions: bool,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<(usize, String)> {
    let Some(source) = registry.source(file) else { return Vec::new() };
    let show = |argument: &Expr| matches!(argument.kind, ExprKind::Nil | ExprKind::Boolean(_) | ExprKind::Number(_) | ExprKind::String(_))
        || matches!(&argument.kind, ExprKind::Unary { value, .. } if expressions || matches!(value.kind, ExprKind::Number(_)))
        || expressions && matches!(argument.kind, ExprKind::Binary { .. } | ExprKind::Table(_) | ExprKind::Function(_));
    let mut calls = Vec::new();
    crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| {
        if expression.span.end < range.start || expression.span.start >= range.end { return; }
        match &expression.kind {
            ExprKind::Call { callee, args } if args.iter().any(show) => calls.push((callee.span, None, args.clone())),
            ExprKind::MethodCall { receiver, name, args } if args.iter().any(show) => calls.push((receiver.span, Some(name.text.clone()), args.clone())),
            _ => {}
        }
    });
    let mut hints = Vec::new();
    for (callee, method, args) in calls {
        let Some(callee_type) = expression_type(registry, file, callee, queries) else { continue };
        let (value, receiver) = match &method {
            None => (callee_type, None),
            Some(name) => {
                let receiver = queries.exclude_type(&callee_type, &Type::Nil);
                let Some((_, member)) = queries.type_members(&receiver).into_iter().find(|(member, _)| member == name) else { continue };
                (member, Some(receiver))
            }
        };
        let hidden = usize::from(receiver.is_some());
        let mut given: Vec<Type> = receiver.into_iter().collect();
        given.extend(args.iter().map(|argument| expression_type(registry, file, argument.span, queries).unwrap_or(Type::Any)));
        let mut candidates = Vec::new();
        for head in queries.call_signature_types(&value) {
            let Some((parameters, _)) = crate::describe_signature(&head, queries) else { continue };
            let shown: Vec<String> = parameters.iter().skip(hidden).map(|parameter| parameter.split(':').next().unwrap_or_default().trim_end_matches('?').trim().to_owned()).collect();
            let rest = shown.last().is_some_and(|name| name == "...");
            if !rest && shown.len() < args.len() { continue; }
            let applicable = queries.call(&head, &given).errors.is_empty();
            candidates.push((applicable, shown));
        }
        let Some((_, names)) = candidates.iter().find(|(applicable, _)| *applicable).or_else(|| candidates.first()) else { continue };
        for (argument, name) in args.iter().zip(names) {
            if show(argument) && name != "..." && !name.is_empty() && !name.starts_with('_') && range.contains(&argument.span.start) {
                hints.push((argument.span.start, name.clone()));
            }
        }
    }
    hints
}

/// What a semantic token is, in the order of the language server's legend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenType {
    Class,
    Interface,
    Type,
    TypeParameter,
    Parameter,
    Variable,
    Property,
    Function,
    Method,
}

/// Semantic token modifiers, as bits in the order of the legend.
pub const MODIFIER_DECLARATION: u32 = 1;
pub const MODIFIER_READONLY: u32 = 2;
pub const MODIFIER_DEFAULT_LIBRARY: u32 = 4;
pub const MODIFIER_GLOBAL: u32 = 8;

const STANDARD_GLOBALS: [&str; 40] = [
    "_G", "_VERSION", "_ENV", "assert", "collectgarbage", "dofile", "error", "getmetatable", "ipairs", "load",
    "loadfile", "next", "pairs", "pcall", "print", "rawequal", "rawget", "rawlen", "rawset", "require",
    "select", "setmetatable", "tonumber", "tostring", "type", "xpcall", "unpack", "loadstring", "setfenv", "getfenv",
    "coroutine", "debug", "io", "math", "os", "package", "string", "table", "utf8", "bit32",
];
const BUILTIN_TYPES: [&str; 14] = ["nil", "boolean", "number", "integer", "string", "table", "function", "userdata", "thread", "any", "unknown", "never", "void", "self"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemanticToken {
    pub span: Span,
    pub kind: TokenType,
    pub modifiers: u32,
}

/// The semantic tokens of a Lua source: what each name is, from its binding (a parameter, a
/// local, a function, a global, the standard library's), its place (a field, a method, a table
/// key) and in annotations, a type's declaring tag or a generic parameter.
pub fn semantic_tokens(text: &str) -> Vec<SemanticToken> {
    use std::collections::HashSet;
    let file = luanalysis_syntax::parse(text);
    let bound = bind::bind(&file);
    let mut tokens = Vec::new();
    // Names called, and locals holding functions.
    let mut called: HashSet<(usize, usize)> = HashSet::new();
    let mut functions: HashSet<(usize, usize)> = HashSet::new();
    let mut members: Vec<(Span, TokenType, u32)> = Vec::new();
    // Declarations first: a `function M:f()` target is also a field expression.
    declarations(&file.statements, &mut functions, &mut members);
    crate::module_requests::visit_expressions(&file.statements, &mut |expression: &Expr| match &expression.kind {
        ExprKind::Call { callee, .. } => {
            let span = match &callee.kind { ExprKind::Field { name, .. } => name.span, _ => callee.span };
            called.insert((span.start, span.end));
        }
        ExprKind::Field { name, .. } => members.push((name.span, TokenType::Property, 0)),
        ExprKind::MethodCall { name, .. } => members.push((name.span, TokenType::Method, 0)),
        ExprKind::Table(fields) => for field in fields {
            if let luanalysis_syntax::TableFieldKind::Named(key) = &field.kind {
                let kind = if matches!(field.value.kind, ExprKind::Function(_)) { TokenType::Method } else { TokenType::Property };
                members.push((key.span, kind, MODIFIER_DECLARATION));
            }
        },
        _ => {}
    });
    fn declarations(statements: &[luanalysis_syntax::Stmt], functions: &mut std::collections::HashSet<(usize, usize)>, members: &mut Vec<(Span, TokenType, u32)>) {
        use luanalysis_syntax::StmtKind;
        for statement in statements {
            match &statement.kind {
                StmtKind::Local { names, values, .. } => for (name, value) in names.iter().zip(values) {
                    if matches!(value.kind, ExprKind::Function(_)) { functions.insert((name.span.start, name.span.end)); }
                },
                StmtKind::FunctionDeclaration { target, function, .. } => {
                    if let ExprKind::Field { name, .. } = &target.kind { members.push((name.span, TokenType::Method, MODIFIER_DECLARATION)); }
                    declarations(&function.body, functions, members);
                }
                StmtKind::LocalFunction { function, .. } => declarations(&function.body, functions, members),
                StmtKind::Block { body, .. } | StmtKind::While { body, .. } | StmtKind::Repeat { body, .. }
                    | StmtKind::NumericFor { body, .. } | StmtKind::GenericFor { body, .. } => declarations(body, functions, members),
                StmtKind::If { branches, else_branch } => {
                    for branch in branches { declarations(&branch.body, functions, members); }
                    if let Some(branch) = else_branch { declarations(&branch.body, functions, members); }
                }
                _ => {}
            }
        }
    }
    for occurrence in &bound.occurrences {
        let key = (occurrence.span.start, occurrence.span.end);
        let declaration_modifier = if occurrence.kind == bind::OccurrenceKind::Declaration { MODIFIER_DECLARATION } else { 0 };
        let (kind, modifiers) = match occurrence.binding.map(|binding| bound.declaration(binding)) {
            Some(declaration) => {
                let declared = (declaration.span.start, declaration.span.end);
                let kind = match declaration.kind {
                    bind::DeclarationKind::Parameter | bind::DeclarationKind::ImplicitReceiver => TokenType::Parameter,
                    bind::DeclarationKind::LocalFunction => TokenType::Function,
                    _ if functions.contains(&declared) || called.contains(&key) => TokenType::Function,
                    _ => TokenType::Variable,
                };
                (kind, declaration_modifier | if declaration.constant { MODIFIER_READONLY } else { 0 })
            }
            None => {
                let library = if STANDARD_GLOBALS.contains(&occurrence.name.as_str()) { MODIFIER_DEFAULT_LIBRARY } else { 0 };
                let kind = if called.contains(&key) { TokenType::Function } else { TokenType::Variable };
                (kind, MODIFIER_GLOBAL | library | declaration_modifier)
            }
        };
        tokens.push(SemanticToken { span: occurrence.span, kind, modifiers });
    }
    for (span, kind, modifiers) in members {
        let kind = if kind == TokenType::Property && called.contains(&(span.start, span.end)) { TokenType::Method } else { kind };
        tokens.push(SemanticToken { span, kind, modifiers });
    }
    for declaration in &file.type_declarations {
        if let luanalysis_syntax::NamedTypeDeclarationKind::Record { fields, .. } = &declaration.kind {
            for field in fields {
                if let luanalysis_syntax::TypeFieldKey::Named(name) = &field.key { tokens.push(SemanticToken { span: name.span, kind: TokenType::Property, modifiers: MODIFIER_DECLARATION }); }
            }
        }
    }
    let names = luanalysis_syntax::type_names(&file, text);
    for name in &names {
        let generic = names.iter().any(|other| other.kind == luanalysis_syntax::TypeNameKind::Generic && other.block == name.block && other.name == name.name);
        let (kind, modifiers) = match name.kind {
            luanalysis_syntax::TypeNameKind::Generic => (TokenType::TypeParameter, MODIFIER_DECLARATION),
            _ if generic => (TokenType::TypeParameter, 0),
            luanalysis_syntax::TypeNameKind::Declaration => {
                let declared = file.type_declarations.iter().find(|declaration| declaration.name.span == name.span).map(|declaration| &declaration.kind);
                let kind = match declared {
                    Some(luanalysis_syntax::NamedTypeDeclarationKind::Record { flavor: luanalysis_syntax::NamedRecordKind::Shape, .. }) => TokenType::Interface,
                    Some(luanalysis_syntax::NamedTypeDeclarationKind::Record { .. }) => TokenType::Class,
                    _ => TokenType::Type,
                };
                (kind, MODIFIER_DECLARATION)
            }
            _ if BUILTIN_TYPES.contains(&name.name.as_str()) => (TokenType::Type, MODIFIER_DEFAULT_LIBRARY),
            _ => {
                let declared = file.type_declarations.iter().find(|declaration| declaration.name.text == name.name).map(|declaration| &declaration.kind);
                match declared {
                    Some(luanalysis_syntax::NamedTypeDeclarationKind::Record { flavor: luanalysis_syntax::NamedRecordKind::Shape, .. }) => (TokenType::Interface, 0),
                    Some(luanalysis_syntax::NamedTypeDeclarationKind::Record { .. }) => (TokenType::Class, 0),
                    _ => (TokenType::Type, 0),
                }
            }
        };
        tokens.push(SemanticToken { span: name.span, kind, modifiers });
    }
    // Only names are tokens; an implicit `self` is declared by its method's `:`.
    tokens.retain(|token| text.get(token.span.start..token.span.end)
        .is_some_and(|name| name.starts_with(|ch: char| ch.is_alphabetic() || ch == '_') && !name.contains('\n')));
    tokens.sort_by_key(|token| (token.span.start, token.span.end));
    // Overlapping tokens keep the first.
    let mut kept: Vec<SemanticToken> = Vec::with_capacity(tokens.len());
    for token in tokens {
        if kept.last().is_none_or(|last| last.span.end <= token.span.start) { kept.push(token); }
    }
    kept
}

/// A quick fix: replace `span` with `replacement` to resolve diagnostic number `diagnostic`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    pub title: String,
    pub diagnostic: usize,
    pub span: Span,
    pub replacement: String,
}

/// Spelling fixes for the diagnostics given as (code, span): a missing property (TS2339) is
/// changed to its receiver's nearest member, and an unknown name (TS2304) to the nearest local
/// in scope or global.
pub fn spelling_fixes(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    diagnostics: Vec<(String, Span)>,
    deliver: impl FnOnce(Vec<Fix>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: false, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |_, registry, queries| {
        deliver(spellings(registry, file, &diagnostics, queries));
    });
}

fn spellings(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    diagnostics: &[(String, Span)],
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<Fix> {
    let Some(source) = registry.source(file) else { return Vec::new() };
    let mut fixes = Vec::new();
    for (index, (code, span)) in diagnostics.iter().enumerate() {
        let overlaps = |other: Span| other.start < span.end && span.start < other.end;
        let (word, candidates): (Name, Vec<String>) = match code.as_str() {
            "TS2339" => {
                let mut found = None;
                crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| match &expression.kind {
                    ExprKind::Field { base, name } | ExprKind::MethodCall { receiver: base, name, .. } if overlaps(name.span) => found = Some((base.span, name.clone())),
                    _ => {}
                });
                let Some((receiver, name)) = found else { continue };
                let Some(receiver) = expression_type(registry, file, receiver, queries) else { continue };
                let receiver = queries.exclude_type(&receiver, &Type::Nil);
                (name, queries.type_members(&receiver).into_iter().map(|(member, _)| member).collect())
            }
            "TS2304" => {
                let mut found = None;
                crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| {
                    if let ExprKind::Name(name) = &expression.kind && overlaps(expression.span) { found = Some(name.clone()); }
                });
                let Some(name) = found else { continue };
                let bound = source.bound();
                let mut candidates: Vec<String> = visible_bindings(bound, name.span.start).into_iter().map(|binding| bound.declaration(binding).name.clone()).collect();
                if let Some(environment) = source.global_environment()
                    && let Ok(environment) = queries.global_environment_type(environment)
                {
                    candidates.extend(queries.type_members(&environment).into_iter().map(|(member, _)| member));
                }
                (name, candidates)
            }
            _ => continue,
        };
        if let Some(best) = nearest(&word.text, &candidates) {
            fixes.push(Fix { title: format!("Change to '{best}'"), diagnostic: index, span: word.span, replacement: best });
        }
    }
    fixes
}

/// The candidate TypeScript would suggest for a misspelled `word`.
fn nearest(word: &str, candidates: &[String]) -> Option<String> {
    luanalysis_engine::spelling_suggestion(word, candidates.iter().map(String::as_str).filter(|candidate| is_identifier(candidate)))
        .map(str::to_owned)
}

/// A function in a call hierarchy, or a file's main chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallItem {
    pub name: String,
    pub kind: SymbolKind,
    pub chunk: bool,
    pub path: std::path::PathBuf,
    pub span: Span,
    pub selection: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallDirection {
    Prepare,
    Incoming,
    Outgoing,
}

/// A call hierarchy: the function at a position, and in a direction, the functions that call
/// it or that it calls, each with the spans of the calls in the caller's file.
#[derive(Clone, Debug, Default)]
pub struct CallHierarchy {
    pub item: Option<CallItem>,
    pub calls: Vec<(CallItem, Vec<Span>)>,
    pub texts: std::collections::HashMap<std::path::PathBuf, String>,
}

/// The call hierarchy of the function declared or called at `offset` in `file`. Functions are
/// identified by their value, not their names, so a local, a global and a method alike are
/// followed through their calls.
pub fn call_hierarchy(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    offset: usize,
    direction: CallDirection,
    deliver: impl FnOnce(Option<CallHierarchy>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: false, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        deliver(calls(project, registry, file, offset, direction, queries));
    });
}

/// Each function in a source with its key, and where it is declared: a statement's name (local,
/// global, field or method), a local's or a table key's.
fn source_functions(source: &crate::native_functions::NativeRegistry) -> Vec<(Span, luanalysis_engine::NativeFunctionKey, Option<(String, Span, Span, SymbolKind)>)> {
    fn statements(statements: &[luanalysis_syntax::Stmt], source: &crate::native_functions::NativeRegistry, text: &mut Vec<(Span, luanalysis_engine::NativeFunctionKey, Option<(String, Span, Span, SymbolKind)>)>) {
        use luanalysis_syntax::StmtKind;
        for statement in statements {
            match &statement.kind {
                StmtKind::LocalFunction { name, function } => {
                    text.push((function.span, source.key_for(function), Some((name.text.clone(), statement.span, name.span, SymbolKind::Function))));
                }
                StmtKind::FunctionDeclaration { target, function, method_span } => {
                    let (name, selection, kind) = match &target.kind {
                        ExprKind::Field { name, .. } => (name.text.clone(), name.span, if method_span.is_some() { SymbolKind::Method } else { SymbolKind::Function }),
                        ExprKind::Name(name) => (name.text.clone(), name.span, SymbolKind::Function),
                        _ => continue,
                    };
                    text.push((function.span, source.key_for(function), Some((name, statement.span, selection, kind))));
                }
                StmtKind::Local { names, values, .. } => for (name, value) in names.iter().zip(values) {
                    if let ExprKind::Function(function) = &value.kind {
                        text.push((function.span, source.key_for(function), Some((name.text.clone(), statement.span, name.span, SymbolKind::Function))));
                    }
                },
                _ => {}
            }
        }
    }
    let mut functions = Vec::new();
    let syntax = source.syntax();
    statements(&syntax.statements, source, &mut functions);
    crate::module_requests::visit_expressions(&syntax.statements, &mut |expression: &Expr| match &expression.kind {
        ExprKind::Function(function) => {
            let key = source.key_for(function);
            if !functions.iter().any(|(_, other, _)| *other == key) { functions.push((function.span, key, None)); }
        }
        ExprKind::Table(fields) => for field in fields {
            if let (luanalysis_syntax::TableFieldKind::Named(name), ExprKind::Function(function)) = (&field.kind, &field.value.kind) {
                functions.push((function.span, source.key_for(function), Some((name.text.clone(), field.span, name.span, SymbolKind::Method))));
            }
        },
        _ => {}
    });
    // Visiting statements also visits the functions they declare; each function once.
    let mut seen = std::collections::HashSet::new();
    functions.retain(|(_, key, _)| seen.insert(*key));
    functions
}

fn call_item(project: &Project, registry: &crate::native_functions::project_provider::ProjectRegistry, key: luanalysis_engine::NativeFunctionKey, texts: &mut std::collections::HashMap<std::path::PathBuf, String>) -> Option<CallItem> {
    let (file, span) = registry.function_site(key)?;
    let source = project.file(file)?;
    texts.entry(source.path.clone()).or_insert_with(|| source.source.clone());
    let functions = source_functions(registry.source(file)?);
    let (name, range, selection, kind) = functions.into_iter().find(|(function, _, _)| *function == span)
        .and_then(|(_, _, declared)| declared)
        .unwrap_or_else(|| ("function".to_owned(), span, Span { start: span.start, end: span.start + "function".len().min(span.end - span.start) }, SymbolKind::Function));
    Some(CallItem { name, kind, chunk: false, path: source.path.clone(), span: range, selection })
}

fn calls(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
    direction: CallDirection,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<CallHierarchy> {
    let source = registry.source(file)?;
    let contains = |span: Span| span.start <= offset && offset <= span.end;
    registry.prepare_documentation_members(queries);
    // The function declared at the position, or failing that, called there.
    let callee_key = |source: &crate::native_functions::NativeRegistry, callee: Span, method: Option<&str>, queries: &mut dyn luanalysis_engine::TypeQueries| {
        let value = source.source_check_expression(source.expression_key_for_span(callee), None, NativeCheckMode::empty(), queries);
        let value = match method {
            None => value,
            Some(name) => {
                let receiver = queries.exclude_type(&value, &Type::Nil);
                queries.type_members(&receiver).into_iter().find(|(member, _)| member == name)?.1
            }
        };
        queries.native_function_key(&value)
    };
    let declared = source_functions(source).into_iter()
        .find(|(_, _, declared)| declared.as_ref().is_some_and(|(_, _, selection, _)| contains(*selection)))
        .map(|(_, key, _)| key);
    let key = match declared {
        Some(key) => key,
        None => {
            let mut called = None;
            crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| match &expression.kind {
                ExprKind::Call { callee, .. } if contains(callee.span) => called = Some((callee.span, None)),
                ExprKind::MethodCall { receiver, name, .. } if contains(name.span) => called = Some((receiver.span, Some(name.text.clone()))),
                _ => {}
            });
            let (callee, method) = called?;
            callee_key(source, callee, method.as_deref(), queries)?
        }
    };
    let mut hierarchy = CallHierarchy::default();
    let item = call_item(project, registry, key, &mut hierarchy.texts)?;
    let target_name = item.name.clone();
    hierarchy.item = Some(item);
    match direction {
        CallDirection::Prepare => {}
        CallDirection::Incoming => {
            for other in &project.files {
                let Some(other_source) = registry.source(other.id) else { continue };
                let functions = source_functions(other_source);
                let mut candidates = Vec::new();
                crate::module_requests::visit_expressions(&other_source.syntax().statements, &mut |expression: &Expr| match &expression.kind {
                    ExprKind::Call { callee, .. } => {
                        let name = match &callee.kind { ExprKind::Field { name, .. } => name.clone(), ExprKind::Name(name) => name.clone(), _ => return };
                        if name.text == target_name { candidates.push((callee.span, None, name.span)); }
                    }
                    ExprKind::MethodCall { receiver, name, .. } if name.text == target_name => candidates.push((receiver.span, Some(name.text.clone()), name.span)),
                    _ => {}
                });
                for (callee, method, span) in candidates {
                    if callee_key(other_source, callee, method.as_deref(), queries) != Some(key) { continue; }
                    // The innermost function around the call, or the file's main chunk.
                    let caller = functions.iter().filter(|(function, _, _)| function.start <= span.start && span.end <= function.end)
                        .min_by_key(|(function, _, _)| function.end - function.start);
                    let item = match caller {
                        Some((_, caller, _)) => call_item(project, registry, *caller, &mut hierarchy.texts),
                        None => {
                            hierarchy.texts.entry(other.path.clone()).or_insert_with(|| other.source.clone());
                            let name = other.path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                            Some(CallItem { name, kind: SymbolKind::Module, chunk: true, path: other.path.clone(), span: Span { start: 0, end: other.source.len() }, selection: Span { start: 0, end: 0 } })
                        }
                    };
                    let Some(item) = item else { continue };
                    match hierarchy.calls.iter_mut().find(|(existing, _)| *existing == item) {
                        Some((_, spans)) => spans.push(span),
                        None => hierarchy.calls.push((item, vec![span])),
                    }
                }
            }
        }
        CallDirection::Outgoing => {
            let (target_file, body) = registry.function_site(key)?;
            let Some(target_source) = registry.source(target_file) else { return Some(hierarchy) };
            let functions = source_functions(target_source);
            // Calls in the function's own body, not in the functions inside it.
            let nested: Vec<Span> = functions.iter().map(|(span, _, _)| *span)
                .filter(|span| body.start < span.start && span.end <= body.end).collect();
            let mut candidates = Vec::new();
            crate::module_requests::visit_expressions(&target_source.syntax().statements, &mut |expression: &Expr| {
                let inside = body.start < expression.span.start && expression.span.end <= body.end
                    && !nested.iter().any(|span| span.start <= expression.span.start && expression.span.end <= span.end);
                if !inside { return; }
                match &expression.kind {
                    ExprKind::Call { callee, .. } => {
                        let span = match &callee.kind { ExprKind::Field { name, .. } => name.span, _ => callee.span };
                        candidates.push((callee.span, None, span));
                    }
                    ExprKind::MethodCall { receiver, name, .. } => candidates.push((receiver.span, Some(name.text.clone()), name.span)),
                    _ => {}
                }
            });
            for (callee, method, span) in candidates {
                let Some(called) = callee_key(target_source, callee, method.as_deref(), queries) else { continue };
                let Some(item) = call_item(project, registry, called, &mut hierarchy.texts) else { continue };
                match hierarchy.calls.iter_mut().find(|(existing, _)| *existing == item) {
                    Some((_, spans)) => spans.push(span),
                    None => hierarchy.calls.push((item, vec![span])),
                }
            }
        }
    }
    Some(hierarchy)
}

/// A named type in a type hierarchy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeItem {
    pub name: String,
    pub kind: SymbolKind,
    pub path: std::path::PathBuf,
    pub span: Span,
    pub selection: Span,
}

/// A type hierarchy: the class or shape named at `offset`, and in a direction, the types it
/// extends (supertypes: `true`) or that extend it.
pub fn type_hierarchy(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    offset: usize,
    supertypes: Option<bool>,
    deliver: impl FnOnce(Option<(Vec<TypeItem>, std::collections::HashMap<std::path::PathBuf, String>)>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: false, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, _| {
        deliver(types_related(project, registry, file, offset, supertypes));
    });
}

fn types_related(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
    supertypes: Option<bool>,
) -> Option<(Vec<TypeItem>, std::collections::HashMap<std::path::PathBuf, String>)> {
    let source = project.file(file)?;
    let names = luanalysis_syntax::type_names(registry.source(file)?.syntax(), &source.source);
    let at = names.iter().find(|name| name.span.start <= offset && offset <= name.span.end)?;
    let mut texts = std::collections::HashMap::new();
    // Every record declaration in the project: its file, name, kind, span and base's names.
    let mut declarations = Vec::new();
    for other in &project.files {
        let Some(syntax) = registry.source(other.id).map(|source| source.syntax()) else { continue };
        for declaration in &syntax.type_declarations {
            let luanalysis_syntax::NamedTypeDeclarationKind::Record { flavor, bases: written, .. } = &declaration.kind else { continue };
            let kind = if *flavor == luanalysis_syntax::NamedRecordKind::Shape { SymbolKind::Interface } else { SymbolKind::Class };
            let mut bases = Vec::new();
            let mut pending: Vec<&luanalysis_syntax::TypeExpr> = written.iter().collect();
            while let Some(ty) = pending.pop() {
                match &ty.kind {
                    luanalysis_syntax::TypeExprKind::Named(name) => bases.push(name.clone()),
                    luanalysis_syntax::TypeExprKind::Generic { base, .. } => bases.push(base.text.clone()),
                    luanalysis_syntax::TypeExprKind::Union(types) | luanalysis_syntax::TypeExprKind::Intersection(types) => pending.extend(types),
                    luanalysis_syntax::TypeExprKind::Parenthesized(inner) => pending.push(inner),
                    _ => {}
                }
            }
            declarations.push((other, TypeItem { name: declaration.name.text.clone(), kind, path: other.path.clone(), span: declaration.span.join(declaration.name.span), selection: declaration.name.span }, bases));
        }
    }
    let item = |(other, item, _): &(&luanalysis_project::SourceFile, TypeItem, Vec<String>), texts: &mut std::collections::HashMap<std::path::PathBuf, String>| {
        texts.entry(other.path.clone()).or_insert_with(|| other.source.clone());
        item.clone()
    };
    let items = match supertypes {
        None => declarations.iter().filter(|(_, item, _)| item.name == at.name).map(|declaration| item(declaration, &mut texts)).collect(),
        Some(true) => {
            let bases: Vec<String> = declarations.iter().filter(|(_, item, _)| item.name == at.name).flat_map(|(_, _, bases)| bases.clone()).collect();
            declarations.iter().filter(|(_, item, _)| bases.contains(&item.name)).map(|declaration| item(declaration, &mut texts)).collect()
        }
        Some(false) => declarations.iter().filter(|(_, _, bases)| bases.contains(&at.name)).map(|declaration| item(declaration, &mut texts)).collect(),
    };
    Some((items, texts))
}

#[cfg(test)]
mod tests {
    #[test]
    fn rename_member_words_respect_unicode_identifier_boundaries() {
        for (text, word) in [("名name name名 name", "name"), ("a变量 变量b 变量", "变量")] {
            let at = text.rfind(word).unwrap();
            assert_eq!(super::word_in(text, luanalysis_syntax::Span::new(0, text.len()), word),
                Some(luanalysis_syntax::Span::new(at, at + word.len())));
        }
    }

    use super::*;

    /// Calls the same current-buffer query as the LSP worker, with no background check that
    /// could replace the inputs before the query starts.
    fn rename_buffers(files: &[(&str, &str)], focus: &str, needle: &str, name: Option<&str>) -> Result<Option<Navigation>, String> {
        use luanalysis_project::{ProjectConfig, ProjectInput, ProjectLoadPlan, Resolver, SourceRole};
        let root = std::env::temp_dir().join(format!("luanalysis rename buffers {}", std::process::id()));
        let overlays: std::collections::HashMap<_, _> = files.iter()
            .map(|(path, text)| (root.join(path), (*text).to_owned())).collect();
        let plan = ProjectLoadPlan {
            entries: files.iter().filter(|(path, _)| !path.ends_with(".def.lua")).map(|(path, _)| root.join(path)).collect(),
            libraries: files.iter().filter(|(path, _)| path.ends_with(".def.lua"))
                .map(|(path, _)| ProjectInput::new(root.join(path), SourceRole::Declaration)).collect(),
        };
        let resolver = Resolver::new(ProjectConfig::new([root.clone()])).unwrap();
        let mut workspace = ProjectWorkspace::new();
        let project = Project::load_plan_with_overlays(&resolver, plan, &overlays, |source| workspace.static_require_requests(source));
        let file = project.file_by_path(&root.join(focus)).unwrap();
        let (file, offset) = (file.id, file.source.find(needle).unwrap());
        let mut result = None;
        rename(project, ProjectCheckOptions {
            environment: crate::standard_environment::StandardEnvironmentProfile::Lua54,
            ..ProjectCheckOptions::default()
        }, &mut workspace, file, offset, name.map(str::to_owned), |navigation| result = Some(navigation));
        result.expect("a rename result")
    }

    #[test]
    fn current_buffer_renames_include_locals_and_globals() {
        let text = "local function greet(name) return name end\nlocal message = greet('world')\nreturn greet(message)\n";
        let renamed = rename_buffers(&[("main.lua", text)], "main.lua", "greet('world')", Some("welcome")).unwrap().unwrap();
        assert_eq!(renamed.occurrences.len(), 3);
        assert_eq!(renamed.span.start, text.find("greet('world')").unwrap());
        assert!(renamed.occurrences.iter().all(|(_, span, _)| &text[span.start..span.end] == "greet"));

        let other = "-- unsaved line\nCounter = 0\n";
        let main = "return Counter\n";
        let renamed = rename_buffers(&[("main.lua", main), ("other.lua", other)], "main.lua", "Counter", Some("Total")).unwrap().unwrap();
        assert_eq!(renamed.occurrences.len(), 2);
        let (path, span, _) = renamed.occurrences.iter().find(|(path, _, _)| path.ends_with("other.lua")).unwrap();
        assert_eq!(span.start, other.find("Counter").unwrap());
        assert_eq!(renamed.texts[path], other);
    }

    #[test]
    fn current_buffer_renames_preserve_lexical_and_library_safety() {
        let text = "local first = 1\nlocal second = 2\nprint(first, second)\nGlobal = 1\nlocal Point = {}\nfunction Point:move() return self end\n";
        let files = [("main.lua", text)];
        for name in ["end", "nested.name"] {
            let error = rename_buffers(&files, "main.lua", "first", Some(name)).unwrap_err();
            assert!(error.contains("is not a Lua name"), "{error}");
        }
        let captured = rename_buffers(&files, "main.lua", "first", Some("second")).unwrap_err();
        assert!(captured.contains("would change what the name at"), "{captured}");
        let collision = rename_buffers(&files, "main.lua", "Global", Some("print")).unwrap_err();
        assert!(collision.contains("a global named print already exists"), "{collision}");
        for name in [None, Some("receiver")] {
            let error = rename_buffers(&files, "main.lua", "self", name).unwrap_err();
            assert!(error.contains("implicit receiver"), "{error}");
            let error = rename_buffers(&files, "main.lua", "print", name).unwrap_err();
            assert!(error.contains("declared by a library"), "{error}");
        }
        let library = [("main.lua", "return HostValue\n"), ("host.def.lua", "HostValue = 1\n")];
        let error = rename_buffers(&library, "main.lua", "HostValue", Some("Other")).unwrap_err();
        assert!(error.contains("declared by a library"), "{error}");
    }

    fn site(text: &str) -> Option<CompletionSite> {
        let offset = text.find('|').expect("a cursor");
        let text = text.replacen('|', "", 1);
        completion_site(&text, offset)
    }

    fn completion_buffer(text: &str, environment: crate::standard_environment::StandardEnvironmentProfile) -> (String, CompletionSite, Vec<CompletionItem>) {
        use luanalysis_project::{ProjectConfig, ProjectLoadPlan, Resolver};
        let marker = "--[[caret]]";
        let offset = text.find(marker).expect("a completion cursor");
        let original = text.replacen(marker, "", 1);
        let site = completion_site(&original, offset).expect("a completion site");
        let root = std::env::temp_dir().join(format!("luanalysis completion buffers {}", std::process::id()));
        let path = root.join("main.lua");
        let overlays = std::collections::HashMap::from([(path.clone(), site.text.clone())]);
        let resolver = Resolver::new(ProjectConfig::new([root])).unwrap();
        let mut workspace = ProjectWorkspace::new();
        let project = Project::load_plan_with_overlays(&resolver, ProjectLoadPlan { entries: vec![path.clone()], libraries: Vec::new() },
            &overlays, |source| workspace.static_require_requests(source));
        let file = project.file_by_path(&path).unwrap().id;
        let mut items = Vec::new();
        complete(project, ProjectCheckOptions { environment, ..ProjectCheckOptions::default() }, &mut workspace, file, &site, |found| items = found);
        (original, site, items)
    }

    #[test]
    fn string_argument_history_tracks_function_identity_slot_and_original_quote() {
        use crate::standard_environment::StandardEnvironmentProfile::None;
        let text = "local function take(first, second) end\ntake('his1', 'other-slot')\nlocal alias = take\nalias('his2')\ntake('his1')\ndo local function take(first) end take('shadowed') end\ntake('h--[[caret]]istory', 'preserved')\n";
        let (original, site, items) = completion_buffer(text, None);
        assert_eq!(site.context, CompletionContext::StringArgument { quote: '\'' });
        assert_eq!(&original[site.word.clone()], "history");
        let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels, ["his1", "his2"]);
        let completed = format!("{}{}{}", &original[..site.word.start], items[1].insert_text.as_deref().unwrap(), &original[site.word.end..]);
        assert!(completed.ends_with("take('his2', 'preserved')\n"), "{completed}");
        assert!(!luanalysis_syntax::parse(&completed).has_lua_syntax_errors());

        let (_, _, items) = completion_buffer("local object={}\nfunction object:take(value) end\nobject.take(object, 'dot')\nobject:take('colon')\nobject:take('--[[caret]]')", None);
        let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels, ["dot", "colon"]);
    }

    #[test]
    fn string_argument_insertions_escape_contents_without_replacing_quotes() {
        use crate::standard_environment::StandardEnvironmentProfile::None;
        let (original, site, items) = completion_buffer("local function take(value) end\ntake(\"a'b\\\\c\")\ntake('--[[caret]]')\n", None);
        let item = items.iter().find(|item| item.label == "a'b\\c").unwrap();
        assert_eq!(item.insert_text.as_deref(), Some("a\\'b\\\\c"));
        let completed = format!("{}{}{}", &original[..site.word.start], item.insert_text.as_deref().unwrap(), &original[site.word.end..]);
        assert!(!luanalysis_syntax::parse(&completed).has_lua_syntax_errors(), "{completed}");
        let site = self::site("take('unclosed|").unwrap();
        assert_eq!(site.context, CompletionContext::StringArgument { quote: '\'' });
        assert!(!luanalysis_syntax::parse(&site.text).has_lua_syntax_errors());
    }

    #[test]
    fn implicit_self_methods_are_composite_calls_and_respect_shadowing() {
        use crate::standard_environment::StandardEnvironmentProfile::None;
        let text = "local s = {}\n---@param value number\nfunction s:aaa(value) end\nfunction s:abb() end\ns.count = 1\nfunction s:ccc()\n sa--[[caret]]\nend";
        let (_, _, items) = completion_buffer(text, None);
        let aaa = items.iter().find(|item| item.label == "self:aaa").expect("the receiver's method");
        assert_eq!(aaa.kind, CompletionKind::Method);
        assert_eq!(aaa.parameters.as_ref().map(|(parameters, _)| parameters.as_slice()), Some(["value".to_owned()].as_slice()));
        assert!(items.iter().any(|item| item.label == "self:abb"));
        assert!(!items.iter().any(|item| item.label == "self:count"));
        let untyped = text.replace("---@param value number\n", "");
        let (_, _, items) = completion_buffer(&untyped, None);
        let aaa = items.iter().find(|item| item.label == "self:aaa").unwrap();
        assert_eq!(aaa.parameters, Some((Vec::new(), true)));
        let shadowed = text.replace(" sa", " local self = {}\n sa");
        let (_, _, items) = completion_buffer(&shadowed, None);
        assert!(!items.iter().any(|item| item.label.starts_with("self:")));
    }

    #[test]
    fn local_attributes_follow_the_ast_version_and_existing_closing_delimiter() {
        use crate::standard_environment::StandardEnvironmentProfile::{Lua53, Lua54};
        let (original, site, items) = completion_buffer("local first <const>, second <cl--[[caret]]ose> = 1, nil", Lua54);
        assert_eq!(site.context, CompletionContext::LocalAttribute { closed: true });
        assert_eq!(&original[site.word.clone()], "close");
        assert_eq!(items.iter().map(|item| item.label.as_str()).collect::<Vec<_>>(), ["const", "close"]);
        assert!(items.iter().all(|item| item.insert_text.is_none()));
        let (original, site, items) = completion_buffer("local value <--[[caret]]", Lua54);
        let item = items.iter().find(|item| item.label == "const").unwrap();
        assert_eq!(item.insert_text.as_deref(), Some("const>"));
        let completed = format!("{}{}{}", &original[..site.word.start], item.insert_text.as_deref().unwrap(), &original[site.word.end..]);
        assert!(!luanalysis_syntax::parse(&completed).has_lua_syntax_errors());
        assert!(completion_buffer("local value <--[[caret]]", Lua53).2.is_empty());
        assert_ne!(self::site("local result = left <co|").unwrap().context, CompletionContext::LocalAttribute { closed: false });
    }

    #[test]
    fn argument_completion_publishes_backtick_hints_from_the_selected_parameter() {
        use crate::standard_environment::StandardEnvironmentProfile::None;
        let text = "---@param event string\n---@param callback function | `function(event) end`\nlocal function listen(event, callback) end\nlisten('someEvent', --[[caret]]";
        let (original, site, items) = completion_buffer(text, None);
        let item = items.iter().find(|item| item.label == "function(event) end").expect("the original snippet text");
        assert_eq!(item.kind, CompletionKind::Snippet);
        assert!(item.implementation.is_none());
        let completed = format!("{}{})", &original[..site.word.start], item.label);
        assert!(!luanalysis_syntax::parse(&completed).has_lua_syntax_errors());
        let (_, _, items) = completion_buffer(&text.replace("listen('someEvent', --[[caret]]", "listen(--[[caret]]"), None);
        assert!(!items.iter().any(|item| item.label == "function(event) end"));

        let text = "---@overload fun(value:number | `Suggested.NUMBER`):number\n---@param value any\nlocal function choose(value) return value end\nchoose(--[[caret]]";
        let (_, _, items) = completion_buffer(text, None);
        assert!(items.iter().any(|item| item.label == "Suggested.NUMBER"));

        let text = "---@param value any | `Private.IMPLEMENTATION`\n---@overload fun(value:number | `Suggested.PUBLIC`):number\nlocal function choose(value) return value end\nchoose(--[[caret]]";
        let (_, _, items) = completion_buffer(text, None);
        assert!(items.iter().any(|item| item.label == "Suggested.PUBLIC"));
        assert!(!items.iter().any(|item| item.label == "Private.IMPLEMENTATION"));

        let text = "---@param callback function | `function(event) end`\n---@function listen(callback)\nlisten(--[[caret]]";
        let (_, _, items) = completion_buffer(text, None);
        assert!(items.iter().any(|item| item.label == "function(event) end"));
    }

    #[test]
    fn unicode_member_completion_replaces_the_entire_identifier() {
        let site = site("local 对象 = {}\nlocal x = 对象.名|字\n").unwrap();
        assert_eq!(site.context, CompletionContext::Member { method: false });
        assert_eq!(&"local 对象 = {}\nlocal x = 对象.名字\n"[site.word.clone()], "名字");
        assert_eq!(site.text, format!("local 对象 = {{}}\nlocal x = 对象.{PLACEHOLDER}\n"));
        assert!(is_identifier("名字"));
        assert!(!is_identifier("名字 with spaces"));
        assert!(!is_identifier("end"));
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
    fn parameter_lists_parentheses_and_comments_are_not_calls() {
        for text in ["local function f(|", "function M.f(a, |", "function M:f(|", "local g = function(|", "local x = (|", "f(1) -- f(|", "f(1) --[[ g(| ]]"] {
            assert!(call(text).is_none(), "{text}");
        }
        assert!(call("f('a', |").is_some());
    }

    #[test]
    fn annotations_complete_tags_and_types_but_not_the_names_they_declare() {
        let context = |text: &str| site(text).map(|site| site.context);
        assert_eq!(context("---@par|"), Some(CompletionContext::Tag));
        assert_eq!(context("---@param point Po|"), Some(CompletionContext::AnnotationType));
        assert_eq!(context("---@type |"), Some(CompletionContext::AnnotationType));
        assert_eq!(context("---@return number, str|"), Some(CompletionContext::AnnotationType));
        assert_eq!(context("---@class Point : Ba|"), Some(CompletionContext::AnnotationType));
        assert_eq!(context("---@type\n---| str|"), Some(CompletionContext::AnnotationType));
        assert_eq!(context("---@param po|"), None);
        assert_eq!(context("---@class Po|"), None);
        assert_eq!(context("-- not documentation @param x Po|"), None);
    }

    #[test]
    fn the_outline_nests_fields_and_functions_in_their_declarations() {
        let text = "---@class Point\n---@field x number\nlocal Point = {}\n\nfunction Point:length() local function square(v) return v * v end return 0 end\n\nlocal config = { name = 'a', run = function() end }\nVERSION = 1\nlocal function helper() end\nfor i = 1, 2 do local function inner() end end\n";
        let outline = |symbols: &[DocumentSymbol]| symbols.iter().map(|symbol| {
            let children: Vec<_> = symbol.children.iter().map(|child| format!("{}:{:?}", child.name, child.kind)).collect();
            format!("{}:{:?}[{}]", symbol.name, symbol.kind, children.join(","))
        }).collect::<Vec<_>>();
        assert_eq!(outline(&document_symbols(text)), vec![
            "Point:Class[x:Field,length:Method]",
            "config:Variable[name:Field,run:Method]",
            "VERSION:Variable[]",
            "helper:Function[]",
            "inner:Function[]",
        ]);
    }

    #[test]
    fn functions_blocks_tables_comments_and_regions_fold() {
        let text = "--region setup\n--- One.\n--- Two.\nlocal function f()\n    if x then\n        y()\n    else\n        z()\n    end\nend\nlocal t = {\n    a = 1,\n}\n--endregion\nlocal flat = { 1 }\n";
        let folds: Vec<_> = folding_ranges(text).into_iter().map(|fold| (text[fold.start..fold.end].lines().next().unwrap().to_owned(), fold.kind)).collect();
        assert_eq!(folds, vec![
            ("--region setup".to_owned(), Some(FoldKind::Region)),
            ("--- One.".to_owned(), Some(FoldKind::Comment)),
            ("local function f()".to_owned(), None),
            ("if x then".to_owned(), None),
            ("else".to_owned(), None),
            ("{".to_owned(), None),
        ]);
    }

    #[test]
    fn a_selection_grows_from_a_name_through_its_expressions_and_statements() {
        let text = "local function f()\n    return point.x + 1\nend\n";
        let offset = text.find(".x").unwrap() + 1;
        let chain: Vec<_> = selection_ranges(text, &[offset])[0].iter().map(|span| &text[span.start..span.end]).collect();
        assert_eq!(chain[..4], ["x", "point.x", "point.x + 1", "return point.x + 1"]);
        assert_eq!(*chain.last().unwrap(), text);
    }

    #[test]
    fn semantic_tokens_classify_names_members_and_annotation_types() {
        let text = "---@class Point\n---@field x number\nlocal Point = {}\n\n---@generic T\n---@param value T\nfunction Point:move(value) local limit <const> = 1 print(self.x, limit) return { y = value } end\n";
        let tokens: Vec<_> = semantic_tokens(text).into_iter().map(|token| (&text[token.span.start..token.span.end], token.kind, token.modifiers)).collect();
        for expected in [
            ("Point", TokenType::Class, MODIFIER_DECLARATION),
            ("x", TokenType::Property, MODIFIER_DECLARATION),
            ("number", TokenType::Type, MODIFIER_DEFAULT_LIBRARY),
            ("T", TokenType::TypeParameter, MODIFIER_DECLARATION),
            ("value", TokenType::Parameter, 0),
            ("T", TokenType::TypeParameter, 0),
            ("move", TokenType::Method, MODIFIER_DECLARATION),
            ("limit", TokenType::Variable, MODIFIER_DECLARATION | MODIFIER_READONLY),
            ("print", TokenType::Function, MODIFIER_GLOBAL | MODIFIER_DEFAULT_LIBRARY),
            ("self", TokenType::Parameter, 0),
            ("y", TokenType::Property, MODIFIER_DECLARATION),
        ] {
            assert!(tokens.contains(&expected), "{expected:?} in {tokens:?}");
        }
    }

    #[test]
    fn concatenation_is_not_a_member_access() {
        let site = site("local text = prefix ..|").unwrap();
        assert_eq!(site.context, CompletionContext::Name);
    }
}
