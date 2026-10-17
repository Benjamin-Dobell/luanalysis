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
use luanalysis_syntax::{CommentKind, Expr, ExprKind, Span, TokenKind};
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

/// The completions at `site` in `file`, whose text in `project` is `site.text`.
pub fn complete(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    site: &CompletionSite,
) -> Vec<CompletionItem> {
    let options = ProjectCheckOptions { jobs: 1, describe_types: true, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |_, registry, queries| {
        let Some(source) = registry.source(file) else { return Vec::new() };
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
                for (name, value) in receiver_members(registry, file, receiver, queries) {
                    if !is_identifier(&name) || items.iter().any(|item: &CompletionItem| item.label == name) { continue; }
                    let callable = matches!(value, Type::Any) || !queries.call_signature_types(&value).is_empty();
                    if method && !callable { continue; }
                    let kind = match (callable, method) {
                        (false, _) => CompletionKind::Field,
                        (true, true) => CompletionKind::Method,
                        (true, false) => CompletionKind::Function,
                    };
                    items.push(CompletionItem { label: name, kind, detail: Some(describe_native(&value, queries, &mut Vec::new())) });
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
                    items.push(CompletionItem { label: declaration.name.clone(), kind, detail });
                }
                if let Some(environment) = source.global_environment()
                    && let Ok(environment) = queries.global_environment_type(environment)
                {
                    for (name, value) in queries.type_members(&environment) {
                        if !is_identifier(&name) || items.iter().any(|item| item.label == name) { continue; }
                        let kind = if queries.call_signature_types(&value).is_empty() { CompletionKind::Variable } else { CompletionKind::Function };
                        items.push(CompletionItem { label: name, kind, detail: Some(describe_native(&value, queries, &mut Vec::new())) });
                    }
                }
                items.extend(KEYWORDS.iter().map(|keyword| CompletionItem { label: (*keyword).to_owned(), kind: CompletionKind::Keyword, detail: None }));
            }
        }
        items
    })
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
) -> Vec<(String, Type)> {
    let Some(source) = registry.source(file) else { return Vec::new() };
    let members = |ty: &Type, queries: &mut dyn luanalysis_engine::TypeQueries| {
        let ty = queries.exclude_type(ty, &Type::Nil);
        queries.type_members(&ty)
    };
    let alone = source.source_check_expression(source.expression_key_for_span(span), None, NativeCheckMode::empty(), queries);
    let found = members(&alone, queries);
    if !found.is_empty() { return found; }
    source.probe_expression(span);
    registry.check_owned_roots(queries, &[file]);
    let mut found: Vec<(String, Type)> = Vec::new();
    for ty in source.probed_types() {
        for (name, value) in members(&ty, queries) {
            if !found.iter().any(|(other, _)| *other == name) { found.push((name, value)); }
        }
    }
    found
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

    #[test]
    fn concatenation_is_not_a_member_access() {
        let site = site("local text = prefix ..|").unwrap();
        assert_eq!(site.context, CompletionContext::Name);
    }
}
